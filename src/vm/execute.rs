//! Instruction dispatch loop.
//!
//! The `execute` function is the main bytecode interpreter loop. It fetches
//! instructions from the current Proto's code array, decodes them, and
//! dispatches to the appropriate handler.
//!
//! Phase 3: no metamethods. Type mismatches produce runtime errors.
//! Metamethod dispatch is added in Phase 4.
//!
//! Reference: `lvm.c` `luaV_execute` in PUC-Rio Lua 5.1.1.

mod runtime_ops;

use crate::error::{LuaError, LuaResult, RuntimeError};

use self::runtime_ops::{
    call_bin_tm, call_tm_res, ensure_table_not_frozen, get_tm_for_val, propagate_slot_read_taint,
    val_equal, val_less_equal, val_less_than, vm_concat, vm_gettable, vm_settable,
};
pub(crate) use self::runtime_ops::{get_where, l_strcmp};
use super::callinfo::{CallInfo, LUA_MULTRET};
use super::closure::{Closure, LuaClosure, Upvalue};
use super::debug_info;
use super::gc::arena::GcRef;
use super::instructions::{Instruction, LFIELDS_PER_FLUSH, OpCode, index_k, is_k};
use super::metatable::{MAXTAGLOOP, TMS, get_comp_tm, gettmbyobj, val_raw_equal};
use super::proto::ProtoRef;
use super::proto::{Proto, VARARG_ISVARARG, VARARG_NEEDSARG};
use super::state::{
    Gc, HookEvent, LUA_MINSTACK, LuaState, MASK_CALL, MASK_COUNT, MASK_LINE, MASK_RET, MAXCALLS,
    MAXCCALLS,
};
use super::string::LuaString;
use super::table::Table;
use super::value::{Userdata, Val, append_lua_number_bytes, lua_number_string_len};
use crate::check_interrupted;

use crate::platform::{localeconv, strcoll, strtod};

/// Calls libc `strtod` on a NUL-terminated buffer.
///
/// Returns `Some((value, bytes_consumed))` on success, `None` if no
/// conversion was performed (endptr == nptr).
#[allow(unsafe_code)]
pub(crate) fn libc_strtod(s: &[u8]) -> Option<(f64, usize)> {
    // strtod requires NUL-terminated input.
    let mut buf = Vec::with_capacity(s.len() + 1);
    buf.extend_from_slice(s);
    buf.push(0);

    let mut endptr: *mut u8 = std::ptr::null_mut();
    // SAFETY: buf is a valid NUL-terminated C string, endptr is a valid pointer.
    let result = unsafe { strtod(buf.as_ptr(), &raw mut endptr) };
    let consumed = endptr as usize - buf.as_ptr() as usize;
    if consumed == 0 {
        return None;
    }
    Some((result, consumed))
}

/// Returns the locale's decimal point character (from `localeconv()`).
/// Falls back to `'.'` if unavailable.
#[allow(unsafe_code)]
pub(crate) fn locale_decimal_point() -> u8 {
    // SAFETY: localeconv() returns a pointer to a static struct.
    let lc = unsafe { localeconv() };
    if lc.is_null() {
        return b'.';
    }
    let dp = unsafe { (*lc).decimal_point };
    if dp.is_null() {
        return b'.';
    }
    let ch = unsafe { *dp };
    if ch == 0 { b'.' } else { ch }
}

// ---------------------------------------------------------------------------
// Helper: RK resolution
// ---------------------------------------------------------------------------

/// Resolves an RK field: if bit 256 is set, returns the constant at the
/// masked index; otherwise returns the register value.
#[inline]
fn rk(stack: &[Val], base: usize, constants: &[Val], field: u32) -> Val {
    if is_k(field) {
        constants[index_k(field) as usize]
    } else {
        stack[base + field as usize]
    }
}

// ---------------------------------------------------------------------------
// Helper: numeric coercion
// ---------------------------------------------------------------------------

/// Attempts to coerce a value to a number (for arithmetic).
///
/// Numbers pass through. Strings are parsed via Lua 5.1 rules
/// (decimal, hex with `0x` prefix, leading/trailing whitespace OK).
/// Returns `None` if the value is not coercible.
pub(crate) fn coerce_to_number(val: Val, gc: &Gc) -> Option<f64> {
    match val {
        Val::Num(n) => Some(n),
        Val::Str(r) => {
            let s = gc.string_arena.get(r)?;
            str_to_number(s.data())
        }
        _ => None,
    }
}

#[inline]
fn exact_integer_number(n: f64) -> Option<i64> {
    const POSITIVE_ZERO_BITS: u64 = 0.0f64.to_bits();
    const NEGATIVE_ZERO_BITS: u64 = (-0.0f64).to_bits();

    if !n.is_finite() {
        return None;
    }
    let int = n as i64;
    let round_trip = (int as f64).to_bits();
    let input = n.to_bits();
    if round_trip == input
        || (int == 0 && (input == POSITIVE_ZERO_BITS || input == NEGATIVE_ZERO_BITS))
    {
        Some(int)
    } else {
        None
    }
}

fn coerce_integer_for_loop(init: Val, limit: Val, step: Val, gc: &Gc) -> Option<(i64, i64, i64)> {
    let init = exact_integer_number(coerce_to_number(init, gc)?)?;
    let limit = exact_integer_number(coerce_to_number(limit, gc)?)?;
    let step = exact_integer_number(coerce_to_number(step, gc)?)?;
    Some((init, limit, step))
}

fn integer_for_loop_state(state: &LuaState, ra: usize) -> Option<(i64, i64, i64)> {
    let Val::Num(idx) = state.stack_get(ra) else {
        return None;
    };
    let Val::Num(limit) = state.stack_get(ra + 1) else {
        return None;
    };
    let Val::Num(step) = state.stack_get(ra + 2) else {
        return None;
    };
    Some((
        exact_integer_number(idx)?,
        exact_integer_number(limit)?,
        exact_integer_number(step)?,
    ))
}

#[derive(Clone, Copy)]
enum PlainTableKey {
    Str { key: GcRef<LuaString>, hash: u32 },
    Int(i64),
    Other(Val),
}

#[inline]
fn resolve_plain_table_key(key: Val, gc: &Gc) -> PlainTableKey {
    match key {
        Val::Str(string_ref) => {
            gc.string_arena
                .get(string_ref)
                .map_or(PlainTableKey::Other(key), |string| PlainTableKey::Str {
                    key: string_ref,
                    hash: string.hash(),
                })
        }
        Val::Num(number) => {
            exact_integer_number(number).map_or(PlainTableKey::Other(key), PlainTableKey::Int)
        }
        _ => PlainTableKey::Other(key),
    }
}

#[inline]
fn try_plain_table_get_ref(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: Val,
    result_reg: usize,
) -> bool {
    let resolved_key = resolve_plain_table_key(key, &state.gc);
    let Some(table) = state.gc.tables.get(table_ref) else {
        return false;
    };
    let result = match resolved_key {
        PlainTableKey::Str { key, hash } => table.get_str_hashed(key, hash),
        PlainTableKey::Int(integer_key) => table.get_int(integer_key),
        PlainTableKey::Other(raw_key) => table.get(raw_key, &state.gc.string_arena),
    };
    if result.is_nil() && table.metatable().is_some() {
        return false;
    }
    if !result.is_nil() {
        propagate_slot_read_taint(state, table_ref, key);
    }
    state.stack_set(result_reg, result);
    true
}

#[inline]
fn try_plain_table_get(state: &mut LuaState, table_val: Val, key: Val, result_reg: usize) -> bool {
    let Val::Table(table_ref) = table_val else {
        return false;
    };
    try_plain_table_get_ref(state, table_ref, key, result_reg)
}

fn lookup_slot_shadow_table(
    state: &LuaState,
    runtime: &super::state::GlobalSlotRuntime,
) -> Option<GcRef<Table>> {
    let key = runtime.shadow_registry_key?;
    let registry = state.gc.tables.get(state.registry)?;
    match registry.get_str(key, &state.gc.string_arena) {
        Val::Table(table_ref) => Some(table_ref),
        _ => None,
    }
}

fn get_global_slot_key(state: &LuaState, slot_idx: usize) -> Option<Val> {
    let runtime = state.global_slots.as_ref()?;
    runtime.name_keys.get(slot_idx).copied().map(Val::Str)
}

/// Parse a byte slice as a Lua number (matching PUC-Rio's `luaO_str2d`).
///
/// Uses libc `strtod` for locale-aware decimal parsing. Supports hex
/// (`0x`/`0X` prefix) via `strtoul`, and leading/trailing whitespace.
pub(crate) fn str_to_number(data: &[u8]) -> Option<f64> {
    // Skip leading whitespace.
    let start = data.iter().position(|&b| !b.is_ascii_whitespace())?;
    let trimmed = &data[start..];
    if trimmed.is_empty() {
        return None;
    }

    // Use strtod (locale-aware).
    let (result, consumed) = libc_strtod(trimmed)?;

    // Check for hex constant: strtod may have stopped at 'x'/'X'.
    let rest = &trimmed[consumed..];
    if !rest.is_empty() && (rest[0] == b'x' || rest[0] == b'X') {
        // Hex: parse with strtoul (PUC-Rio fallback).
        let hex_str = std::str::from_utf8(trimmed).ok()?;
        let hex_trimmed = hex_str.trim();
        let (sign, hex_part) = if let Some(rest) = hex_trimmed.strip_prefix('-') {
            (-1.0_f64, rest)
        } else if let Some(rest) = hex_trimmed.strip_prefix('+') {
            (1.0, rest)
        } else {
            (1.0, hex_trimmed)
        };
        let hex_part = hex_part
            .strip_prefix("0x")
            .or_else(|| hex_part.strip_prefix("0X"))?;
        let val = u64::from_str_radix(hex_part, 16).ok()?;
        return Some(sign * val as f64);
    }

    // Check that remaining characters are only whitespace.
    if rest.iter().all(u8::is_ascii_whitespace) {
        Some(result)
    } else {
        None
    }
}

/// Coerces a value to a string for concatenation.
///
/// Numbers are formatted using Lua's %.14g rules. Strings pass through.
/// Returns `None` for other types.
#[allow(dead_code)] // Used in Phase 4b (tostring metamethod support)
fn coerce_to_string(val: Val, gc: &mut Gc) -> Option<Val> {
    match val {
        Val::Str(_) => Some(val),
        Val::Num(n) => {
            let mut bytes = Vec::with_capacity(lua_number_string_len(n));
            append_lua_number_bytes(&mut bytes, n);
            let r = gc.intern_string(&bytes);
            Some(Val::Str(r))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helper: runtime error construction
// ---------------------------------------------------------------------------

/// Creates a runtime error with source location from the current proto.
fn runtime_error(proto: &Proto, pc: usize, message: &str) -> LuaError {
    let line = if pc > 0 && pc <= proto.line_info.len() {
        proto.line_info[pc - 1]
    } else if !proto.line_info.is_empty() {
        proto.line_info[0]
    } else {
        0
    };
    LuaError::Runtime(RuntimeError {
        message: format!("{}:{line}: {message}", proto.short_source),
        level: 0,
        traceback: vec![],
    })
}

/// Type error with variable name resolution.
///
/// Matches PUC-Rio's `luaG_typeerror`. Uses `getobjname` to find the
/// variable name and kind (local/global/field/upvalue/method) for the
/// value at register `reg` (relative to base, so the stack position
/// is `base + reg`). Produces messages like:
/// - `"attempt to call local 'x' (a number value)"`
/// - `"attempt to index a nil value"`
fn type_error(
    state: &LuaState,
    proto: &Proto,
    pc: usize,
    base: usize,
    reg: usize,
    opname: &str,
) -> LuaError {
    let val = state.stack_get(base + reg);
    let type_name = val.type_name();
    // pc is already incremented past the current instruction, so use pc-1.
    let current_pc = pc.saturating_sub(1);
    let name_info = debug_info::getobjname(
        proto,
        current_pc,
        #[allow(clippy::cast_possible_truncation)]
        (reg as u32),
        &state.gc.string_arena,
    );
    let message = if let Some((kind, name)) = name_info {
        format!("attempt to {opname} {kind} '{name}' (a {type_name} value)")
    } else {
        format!("attempt to {opname} a {type_name} value")
    };
    runtime_error(proto, pc, &message)
}

/// Arithmetic type error with RK-aware operand resolution.
///
/// Matches PUC-Rio's `luaG_aritherror`: identifies which operand
/// caused the error (first non-numeric), then calls `type_error`
/// if the operand is in a register (not a constant).
#[allow(clippy::too_many_arguments)]
fn arith_error(
    state: &LuaState,
    proto: &Proto,
    pc: usize,
    base: usize,
    lhs: Val,
    rhs: Val,
    b_rk: u32,
    c_rk: u32,
) -> LuaError {
    // PUC-Rio: if first can't convert, error on first operand.
    let rk = if coerce_to_number(lhs, &state.gc).is_none() {
        b_rk
    } else {
        c_rk
    };
    // If the problematic operand is a constant, we can't resolve a variable
    // name. Otherwise use type_error for register-based name resolution.
    if is_k(rk) {
        let val = if coerce_to_number(lhs, &state.gc).is_none() {
            lhs
        } else {
            rhs
        };
        runtime_error(
            proto,
            pc,
            &format!(
                "attempt to perform arithmetic on a {} value",
                val.type_name()
            ),
        )
    } else {
        type_error(state, proto, pc, base, rk as usize, "perform arithmetic on")
    }
}

/// Type error for comparison (no variable names — PUC-Rio doesn't use them).
fn compare_error(proto: &Proto, pc: usize, left: Val, right: Val) -> LuaError {
    let message = if left.type_name() == right.type_name() {
        format!("attempt to compare two {} values", left.type_name())
    } else {
        format!(
            "attempt to compare {} with {}",
            left.type_name(),
            right.type_name()
        )
    };
    runtime_error(proto, pc, &message)
}

/// luaO_fb2int: convert a "floating point byte" to an integer.
///
/// Used by NEWTABLE to decode size hints. The format encodes
/// `(x & 7 + 8) << ((x >> 3) - 1)` for non-zero exponent,
/// or `x` directly when the exponent is zero.
pub(crate) fn fb2int(x: u32) -> u32 {
    let e = (x >> 3) & 31;
    if e == 0 { x } else { ((x & 7) + 8) << (e - 1) }
}

// ---------------------------------------------------------------------------
// Call machinery
// ---------------------------------------------------------------------------

/// Result of `precall`: what should the caller do next?
pub enum CallResult {
    /// The called function is a Lua function. The caller should enter
    /// or re-enter the execute loop.
    Lua,
    /// The called function was a Rust function and has already returned.
    /// Results are in place on the stack.
    Rust,
}

impl LuaState {
    #[inline]
    fn ensure_stack_len(&mut self, needed: usize) {
        if needed > self.stack.len() {
            self.stack.resize(needed, Val::Nil);
        }
    }

    /// Checks `call_depth` against the two-threshold stack overflow model.
    ///
    /// PUC-Rio `luaD_call` uses two thresholds:
    /// - `LUAI_MAXCCALLS` (200): throw recoverable "stack overflow"
    /// - `LUAI_MAXCCALLS + LUAI_MAXCCALLS/8` (225): unrecoverable error
    ///   Calls between 201-224 are allowed as headroom for error handlers.
    fn check_stack_overflow(&self) -> LuaResult<()> {
        if self.call_depth >= MAXCCALLS {
            let hard_limit = MAXCCALLS + (MAXCCALLS >> 3);
            if self.call_depth >= hard_limit {
                return Err(runtime_error_simple("stack overflow"));
            }
            if self.call_depth == MAXCCALLS {
                let where_prefix = get_where(self, 0);
                return Err(LuaError::Runtime(RuntimeError {
                    message: format!("{where_prefix}stack overflow"),
                    level: 0,
                    traceback: vec![],
                }));
            }
        }
        Ok(())
    }

    /// Calls a function at `func_idx` with call depth and yield boundary tracking.
    ///
    /// This is the rilua equivalent of PUC-Rio's `luaD_call()`. It wraps
    /// `precall` + `execute` with depth increment/decrement. Used by
    /// stdlib functions (pcall, table.sort, etc.) and metamethods to call
    /// Lua code.
    ///
    /// Increments both counters:
    /// - `call_depth`: stack overflow detection (checked by `check_stack_overflow`)
    /// - `n_ccalls`: yield boundary (`coroutine.yield()` blocked when > 0)
    pub fn call_function(&mut self, func_idx: usize, num_results: i32) -> LuaResult<()> {
        self.n_ccalls += 1;
        self.call_depth += 1;
        let result = (|| {
            self.check_stack_overflow()?;
            match self.precall(func_idx, num_results)? {
                CallResult::Lua => execute(self),
                CallResult::Rust => Ok(()),
            }
        })();
        self.call_depth -= 1;
        self.n_ccalls -= 1;
        result
    }

    /// Invokes the active hook function with (event_name, line_or_nil).
    ///
    /// Matches PUC-Rio's `luaD_callhook()` in `ldo.c`:
    /// - Saves and restores `top` and `ci.top`
    /// - Sets `allow_hook = false` to prevent recursive hook calls
    /// - Pushes the hook function, event string, and line number (or nil)
    /// - Calls the hook with 2 arguments and 0 results
    pub(crate) fn callhook(&mut self, event: HookEvent, line: i32) -> LuaResult<()> {
        if !self.hook.allow_hook {
            return Ok(());
        }
        let hook_func = self.hook.hook_func;
        if hook_func.is_nil() {
            return Ok(());
        }

        // Save top and ci.top (hook may modify the stack).
        let saved_top = self.top;
        let saved_ci_top = self.call_stack[self.ci].top;

        // Ensure minimum stack for the hook call.
        self.ensure_stack_len(self.top + LUA_MINSTACK);
        self.call_stack[self.ci].top = self.top + LUA_MINSTACK;

        // Disable hooks during the callback (PUC-Rio: L->allowhook = 0).
        self.hook.allow_hook = false;

        let call_base = self.top;
        self.stack_set(call_base, hook_func);
        self.stack_set(call_base + 1, self.hook_event_name(event));
        if line >= 0 {
            #[allow(clippy::cast_precision_loss)]
            self.stack_set(call_base + 2, Val::Num(f64::from(line)));
        } else {
            self.stack_set(call_base + 2, Val::Nil);
        }
        self.top = call_base + 3;

        let result = self.call_function(call_base, 0);

        // Restore state (PUC-Rio: L->allowhook = 1, restore top/ci.top).
        self.hook.allow_hook = true;
        self.call_stack[self.ci].top = saved_ci_top;
        self.top = saved_top;

        result
    }

    /// Sets up a call frame for the function at `func_idx`.
    ///
    /// For Lua functions: pushes a new CallInfo and returns `CallResult::Lua`.
    /// For Rust functions: executes the function, calls `poscall`, and
    /// returns `CallResult::Rust`.
    ///
    /// `num_results` is the number of results the caller expects, or
    /// `LUA_MULTRET` (-1) for all results.
    pub fn precall(&mut self, func_idx: usize, num_results: i32) -> LuaResult<CallResult> {
        // Check total call depth (Lua + Rust). Matches PUC-Rio's `growCI`:
        // - At MAXCALLS (20,000): throw recoverable "stack overflow"
        //   (error handlers like debug.traceback can still run)
        // - Above MAXCALLS: allow calls (headroom for error handling)
        // - At 2*MAXCALLS (40,000): unrecoverable overflow
        //
        // PUC-Rio achieves this by doubling the CI array: first overflow
        // at 20k doubles to 40k capacity. Second overflow at 40k is
        // unrecoverable. We track with `ci_overflow`: false below limit,
        // true once past MAXCALLS. Cleared when pcall/xpcall restores ci.
        let next_ci = self.ci + 1;
        if self.ci_overflow {
            // Already in overflow. Allow headroom but cap at 2*MAXCALLS.
            if next_ci >= MAXCALLS * 2 {
                return Err(runtime_error_simple("stack overflow"));
            }
        } else if next_ci >= MAXCALLS {
            // First overflow. Allow the CI to grow past MAXCALLS for
            // error handlers, but throw a recoverable error.
            self.ci_overflow = true;
            let where_prefix = get_where(self, 0);
            return Err(LuaError::Runtime(RuntimeError {
                message: format!("{where_prefix}stack overflow"),
                level: 0,
                traceback: vec![],
            }));
        }

        let func_val = self.stack_get(func_idx);
        let closure_ref = if let Val::Function(r) = func_val {
            r
        } else {
            // Try __call metamethod.
            let tm = get_tm_for_val(&self.gc, func_val, TMS::Call);
            match tm {
                Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                    // Shift stack up to insert __call at func position.
                    // The original value becomes the first argument.
                    let top = self.top;
                    self.ensure_stack_len(top + 1);
                    let mut p = top;
                    while p > func_idx {
                        let v = self.stack_get(p - 1);
                        self.stack_set(p, v);
                        p -= 1;
                    }
                    self.top = top + 1;
                    self.stack_set(func_idx, tm_val);
                    // Now func_idx points to __call, original value is at func_idx+1.
                    match tm_val {
                        Val::Function(r) => r,
                        _ => unreachable!(),
                    }
                }
                _ => {
                    // Try to resolve the variable name from the caller frame.
                    let ci = &self.call_stack[self.ci];
                    let caller_func = self.stack_get(ci.func);
                    let err = if let Val::Function(r) = caller_func {
                        if let Some(Closure::Lua(lcl)) = self.gc.closures.get(r) {
                            let reg = func_idx - ci.base;
                            type_error(self, &lcl.proto, ci.saved_pc, ci.base, reg, "call")
                        } else {
                            LuaError::Runtime(RuntimeError {
                                message: format!(
                                    "attempt to call a {} value",
                                    func_val.type_name()
                                ),
                                level: 0,
                                traceback: vec![],
                            })
                        }
                    } else {
                        LuaError::Runtime(RuntimeError {
                            message: format!("attempt to call a {} value", func_val.type_name()),
                            level: 0,
                            traceback: vec![],
                        })
                    };
                    return Err(err);
                }
            }
        };

        // Save caller's PC.
        let saved_pc = self.call_stack[self.ci].saved_pc;
        let _ = saved_pc; // used for restoration in poscall

        let closure = self
            .gc
            .closures
            .get(closure_ref)
            .ok_or_else(|| runtime_error_simple("invalid function reference"))?;

        match closure {
            Closure::Lua(lua_cl) => {
                let proto = ProtoRef::clone(&lua_cl.proto);
                let num_params = proto.num_params as usize;
                let max_stack = proto.max_stack_size as usize;
                let is_vararg = proto.is_vararg & VARARG_ISVARARG != 0;

                let nargs = self.get_nargs(func_idx);

                let new_base;
                if is_vararg {
                    new_base = self.adjust_varargs(&proto, nargs, func_idx);
                } else {
                    new_base = func_idx + 1;
                    let fixed_top = new_base + num_params;
                    self.ensure_stack_len(new_base + max_stack);

                    if nargs > num_params {
                        self.top = fixed_top;
                    } else if self.top < fixed_top {
                        self.stack[self.top..fixed_top].fill(Val::Nil);
                        self.top = fixed_top;
                    }
                }

                let ci_top = new_base + max_stack;
                self.ensure_stack_len(ci_top);
                if self.top < ci_top {
                    self.stack[self.top..ci_top].fill(Val::Nil);
                }
                self.top = ci_top;

                // Push new CallInfo.
                let mut ci = CallInfo::new(func_idx, new_base, ci_top, num_results);
                ci.is_lua = true;
                self.push_ci(ci);
                self.base = new_base;

                // Fire call hook (PUC-Rio: luaD_precall lines 299-303).
                // Hooks expect savedpc to point past the first instruction.
                if self.hook.hook_mask & MASK_CALL != 0 {
                    self.call_stack[self.ci].saved_pc += 1;
                    self.callhook(HookEvent::Call, -1)?;
                    self.call_stack[self.ci].saved_pc =
                        self.call_stack[self.ci].saved_pc.saturating_sub(1);
                }

                Ok(CallResult::Lua)
            }
            Closure::Rust(rust_cl) => {
                let func = rust_cl.func;

                // Ensure minimum stack for Rust functions.
                self.ensure_stack_len(self.top + LUA_MINSTACK);

                let ci_top = self.top + LUA_MINSTACK;
                let ci = CallInfo::new(func_idx, func_idx + 1, ci_top, num_results);
                self.push_ci(ci);
                self.base = func_idx + 1;

                // Note: n_ccalls is NOT incremented here. The C-call boundary
                // counter is managed by call_function() (the luaD_call equivalent).
                // This matches PUC-Rio where luaD_precall does not touch nCcalls.

                // Fire call hook (PUC-Rio: luaD_precall lines 316-317).
                if self.hook.hook_mask & MASK_CALL != 0 {
                    self.callhook(HookEvent::Call, -1)?;
                }

                // Execute the Rust function.
                let n_results = func(self)?;

                // Move results into place.
                let first_result = self.top - n_results as usize;
                self.poscall(first_result);

                Ok(CallResult::Rust)
            }
        }
    }

    /// Unwinds a call frame and moves results to the caller's frame.
    ///
    /// `first_result` is the stack index of the first return value.
    /// Returns `true` if the caller is a Lua function (execution should
    /// continue in the caller's frame).
    #[inline]
    pub fn poscall(&mut self, mut first_result: usize) -> bool {
        // Fire return hook before unwinding (PUC-Rio: luaD_poscall line 346).
        // callrethooks fires LUA_HOOKRET, then LUA_HOOKTAILRET for each
        // elided tail call. The hook may reallocate the stack, so
        // first_result is saved/restored as an offset.
        if self.hook.hook_mask & MASK_RET != 0 {
            let fr_offset = first_result;
            let _ = self.callhook(HookEvent::Return, -1);
            // Handle tail return hooks (PUC-Rio: callrethooks lines 335-336).
            let tail_calls = self.call_stack[self.ci].tail_calls;
            for _ in 0..tail_calls {
                let _ = self.callhook(HookEvent::TailReturn, -1);
            }
            first_result = fr_offset;
        }

        // Pop the current CallInfo.
        let ci_func = self.call_stack[self.ci].func;
        let wanted = self.call_stack[self.ci].num_results;
        self.pop_ci();

        // Restore caller's base.
        self.base = self.call_stack[self.ci].base;

        // Move results to where the function was.
        if wanted == LUA_MULTRET {
            let count = self.top.saturating_sub(first_result);
            if count > 0 && first_result != ci_func {
                self.stack.copy_within(first_result..self.top, ci_func);
            }
            self.top = ci_func + count;
        } else if wanted == 0 {
            self.top = ci_func;
        } else if wanted == 1 {
            let result = if first_result < self.top {
                self.stack[first_result]
            } else {
                Val::Nil
            };
            self.stack[ci_func] = result;
            self.top = ci_func + 1;
        } else {
            let wanted = wanted as usize;
            let available = self.top.saturating_sub(first_result).min(wanted);
            if available > 0 && first_result != ci_func {
                self.stack
                    .copy_within(first_result..first_result + available, ci_func);
            }
            let new_top = ci_func + wanted;
            self.ensure_stack_len(new_top);
            if available < wanted {
                self.stack[ci_func + available..new_top].fill(Val::Nil);
            }
            self.top = new_top;
        }

        // Return true if the caller requested fixed results (not MULTRET).
        // Matches PUC-Rio: `return (L->nresults - LUA_MULTRET)` which is
        // non-zero when nresults != LUA_MULTRET. The caller uses this to
        // decide whether to reset top to the frame's max (fixed results)
        // or leave it as-is (MULTRET, so the next operation can read the
        // actual result count from top).
        wanted != LUA_MULTRET
    }

    /// Adjusts the stack for a vararg function call.
    ///
    /// Matches PUC-Rio's `adjust_varargs` in `ldo.c`:
    /// 1. Pads actual args with nil if fewer than num_params
    /// 2. Copies fixed params from their original positions to above the vararg area
    /// 3. Nils out the original fixed param positions
    ///
    /// Returns the new base (pointing to the first fixed param copy).
    fn adjust_varargs(&mut self, proto: &Proto, nargs: usize, func_idx: usize) -> usize {
        let num_params = proto.num_params as usize;

        // Pad with nil if actual < num_params.
        let mut actual = nargs;
        while actual < num_params {
            self.push(Val::Nil);
            actual += 1;
        }

        // LUA_COMPAT_VARARG: create 'arg' table if NEEDSARG is set.
        // The table contains the extra (vararg) arguments with an 'n' field.
        let arg_table = if proto.is_vararg & VARARG_NEEDSARG != 0 {
            let nvar = actual - num_params; // Number of extra arguments.
            let fixed = self.top - actual;
            let mut tbl = Table::new();
            for i in 0..nvar {
                let val = self.stack_get(fixed + num_params + i);
                #[allow(clippy::cast_precision_loss)]
                let _ = tbl.raw_set(Val::Num((i + 1) as f64), val, &self.gc.string_arena);
            }
            let n_key = self.gc.intern_string(b"n");
            #[allow(clippy::cast_precision_loss)]
            let _ = tbl.raw_set(
                Val::Str(n_key),
                Val::Num(nvar as f64),
                &self.gc.string_arena,
            );
            Some(self.gc.alloc_table(tbl))
        } else {
            None
        };

        // `fixed` = first original argument position.
        let fixed = self.top - actual;
        // `base` = where the fixed param copies will start (above all args).
        let new_base = self.top;

        // Ensure enough stack space for the copies + optional 'arg' table.
        self.ensure_stack_len(new_base + num_params + 1);

        // Copy fixed params to above varargs, nil out originals.
        for i in 0..num_params {
            let val = self.stack_get(fixed + i);
            self.stack_set(self.top, val);
            self.top += 1;
            self.stack_set(fixed + i, Val::Nil);
        }

        // Push 'arg' table as an extra parameter if needed.
        if let Some(tbl_ref) = arg_table {
            self.stack_set(self.top, Val::Table(tbl_ref));
            self.top += 1;
        }

        // Stack layout now:
        // [func] [nil...] [vararg1] [vararg2] ... [param1] [param2] [arg?]
        //         ^ fixed positions nilled out       ^ base (new_base)
        let _ = func_idx; // func_idx not needed; `fixed` is derived from top

        new_base
    }

    /// Finds or creates an open upvalue for the given stack index.
    ///
    /// The open_upvalues list is maintained sorted by stack index
    /// (descending). If an upvalue for this index already exists,
    /// it is reused.
    pub fn find_upvalue(&mut self, stack_index: usize) -> super::gc::arena::GcRef<Upvalue> {
        // Search for existing open upvalue at this stack index.
        for &uv_ref in &self.open_upvalues {
            if let Some(uv) = self.gc.upvalues.get(uv_ref)
                && let Some(idx) = uv.stack_index()
            {
                if idx == stack_index {
                    return uv_ref;
                }
                if idx < stack_index {
                    break; // List is sorted descending; won't find it.
                }
            }
        }

        // Create new upvalue and insert in sorted position.
        let uv = Upvalue::new_open(stack_index);
        let uv_ref = self.gc.alloc_upvalue(uv);

        // Find insertion point (maintain descending order by stack_index).
        let pos = self
            .open_upvalues
            .iter()
            .position(|&r| {
                self.gc
                    .upvalues
                    .get(r)
                    .and_then(super::closure::Upvalue::stack_index)
                    .is_none_or(|idx| idx < stack_index)
            })
            .unwrap_or(self.open_upvalues.len());

        self.open_upvalues.insert(pos, uv_ref);
        uv_ref
    }

    /// Closes all open upvalues at or above the given stack level.
    ///
    /// Copies the value from the stack into the upvalue's own storage,
    /// transitioning it from Open to Closed state.
    pub fn close_upvalues(&mut self, level: usize) {
        while let Some(&uv_ref) = self.open_upvalues.first() {
            let should_close = self
                .gc
                .upvalues
                .get(uv_ref)
                .and_then(super::closure::Upvalue::stack_index)
                .is_some_and(|idx| idx >= level);

            if !should_close {
                break;
            }

            // Close the upvalue: captures current stack value.
            if let Some(uv) = self.gc.upvalues.get_mut(uv_ref) {
                uv.close(&self.stack);
            }

            // Write barrier: upvalue now holds the captured value.
            // Read back the closed value for the barrier check.
            let captured_val = self
                .gc
                .upvalues
                .get(uv_ref)
                .map_or(Val::Nil, |uv| uv.get(&self.stack));
            let uv_color = self
                .gc
                .upvalues
                .color(uv_ref)
                .unwrap_or(crate::vm::gc::Color::White0);
            self.gc.barrier_forward_val(uv_color, captured_val);

            self.open_upvalues.remove(0);
        }
    }
}

/// Simple runtime error without source location.
fn runtime_error_simple(message: &str) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: message.to_string(),
        level: 0,
        traceback: vec![],
    })
}

// ---------------------------------------------------------------------------
// Main execute loop
// ---------------------------------------------------------------------------

/// Executes the current Lua function in the given state.
///
/// Assumes a Lua closure has been set up via `precall` and the current
/// CallInfo points to its frame. Runs until `OP_RETURN` drops back to
/// a non-Lua caller (ci == 0 or a Rust frame).
///
/// Returns when the outermost Lua call returns.
pub fn execute(state: &mut LuaState) -> LuaResult<()> {
    // PUC-Rio's `nexeccalls` pattern: tracks how many Lua functions were
    // entered via OP_CALL within this execute() invocation. Starts at 1
    // (the function we were called to run). OP_CALL increments it for
    // Lua callees; OP_RETURN decrements it and returns when it hits 0.
    // This avoids recursive execute() calls for Lua-to-Lua calls,
    // matching PUC-Rio's `goto reentry` model.
    let mut nexeccalls: u32 = 1;

    loop {
        // Cache values from current frame.
        let ci_func = state.call_stack[state.ci].func;
        let mut base = state.base;

        // Get the current closure and proto.
        let Val::Function(closure_ref) = state.stack_get(ci_func) else {
            return Err(runtime_error_simple("not a function"));
        };

        let (proto, env) = {
            let cl = state
                .gc
                .closures
                .get(closure_ref)
                .ok_or_else(|| runtime_error_simple("invalid closure"))?;
            match cl {
                Closure::Lua(lcl) => (ProtoRef::clone(&lcl.proto), lcl.env),
                Closure::Rust(_) => {
                    return Err(runtime_error_simple("expected Lua closure in execute"));
                }
            }
        };

        let mut pc = state.call_stack[state.ci].saved_pc;

        // Inner dispatch loop for this frame.
        loop {
            if pc >= proto.code.len() {
                return Err(runtime_error(&proto, pc, "bytecode overrun"));
            }

            // Read instruction and advance pc BEFORE hook check.
            // PUC-Rio: `const Instruction i = *pc++;` then traceexec(L, pc).
            // After this, pc points one past the instruction being executed,
            // matching PUC-Rio's convention where pcRel(pc, p) = (pc - code) - 1
            // gives the current instruction's index.
            let instr = Instruction::from_raw(proto.code[pc]);
            pc += 1;

            // Interrupt check: abort if the embedder's signal handler set the flag.
            if check_interrupted() {
                return Err(runtime_error(&proto, pc, "interrupted!"));
            }

            // Hook check: line and count hooks (PUC-Rio: lvm.c lines 388-396).
            // The decrement runs every instruction when line or count hooks
            // are set. traceexec fires when counter reaches zero OR line
            // hooks are active.
            let trace_mask = state.hook.hook_mask & (MASK_LINE | MASK_COUNT);
            if trace_mask != 0 {
                let has_count_hook = trace_mask & MASK_COUNT != 0;
                let has_line_hook = trace_mask & MASK_LINE != 0;
                let count_hook_fired = if has_count_hook {
                    state.hook.hook_count -= 1;
                    state.hook.hook_count == 0
                } else {
                    false
                };
                if count_hook_fired || has_line_hook {
                    // npc = index of the current instruction (pc was advanced).
                    // Equivalent to PUC-Rio's pcRel(pc, p) = (pc - code) - 1.
                    let npc = pc - 1;

                    // Save pc for the hook callback (getinfo reads saved_pc).
                    let old_pc = state.call_stack[state.ci].saved_pc;
                    state.call_stack[state.ci].saved_pc = pc;

                    // Count hook: fire when hookcount reaches zero
                    // (PUC-Rio: traceexec lines 64-68).
                    if count_hook_fired {
                        state.hook.hook_count = state.hook.base_hook_count;
                        if state.hook.yield_on_hook {
                            state.call_stack[state.ci].saved_pc = npc;
                            state.yielded_in_hook = true;
                            return Err(LuaError::Yield(0));
                        }
                        state.callhook(HookEvent::Count, -1)?;
                    }

                    // Line hook: fire on new function entry, jump back,
                    // or new source line (PUC-Rio: traceexec lines 70-78).
                    if has_line_hook {
                        #[allow(clippy::cast_possible_wrap)]
                        let newline = proto.line_info.get(npc).copied().unwrap_or(0) as i32;
                        let should_fire = npc == 0 || pc <= old_pc || {
                            let old_npc = old_pc.wrapping_sub(1);
                            #[allow(clippy::cast_possible_wrap)]
                            let oldline = proto.line_info.get(old_npc).copied().unwrap_or(0) as i32;
                            newline != oldline
                        };
                        if should_fire {
                            if state.hook.yield_on_hook {
                                state.call_stack[state.ci].saved_pc = npc;
                                state.yielded_in_hook = true;
                                return Err(LuaError::Yield(0));
                            }
                            state.callhook(HookEvent::Line, newline)?;
                        }
                    }

                    // Hook may have changed base via coroutine ops.
                    base = state.base;
                }
            }

            let op = instr.opcode();
            let a = instr.a() as usize;
            let ra = base + a;

            // Debug tracing (compile-time enabled).
            if option_env!("LUA_DEBUG_VM").is_some() {
                eprintln!(
                    "  [{pc:>4}] {op:<12} A={a} B={} C={} (base={base}, top={})",
                    instr.b(),
                    instr.c(),
                    state.top
                );
            }

            match op {
                // ----- Data movement -----
                OpCode::Move => {
                    let b = instr.b() as usize;
                    let val = state.stack_get(base + b);
                    state.stack_set(ra, val);
                }

                OpCode::LoadK => {
                    let bx = instr.bx() as usize;
                    let val = proto.constants[bx];
                    state.stack_set(ra, val);
                }

                OpCode::LoadBool => {
                    let b = instr.b();
                    state.stack_set(ra, Val::Bool(b != 0));
                    if instr.c() != 0 {
                        pc += 1; // skip next instruction
                    }
                }

                OpCode::LoadNil => {
                    let b = instr.b() as usize;
                    // Set registers A through B to nil.
                    for i in a..=b {
                        state.stack_set(base + i, Val::Nil);
                    }
                }

                // ----- Globals -----
                OpCode::GetGlobal => {
                    let bx = instr.bx() as usize;
                    let key = proto.constants[bx];
                    if !try_plain_table_get_ref(state, env, key, ra) {
                        state.call_stack[state.ci].saved_pc = pc;
                        vm_gettable(state, Val::Table(env), key, ra, &proto, pc, base, None)?;
                    }
                }

                OpCode::SetGlobal => {
                    let bx = instr.bx() as usize;
                    let key = proto.constants[bx];
                    let val = state.stack_get(ra);
                    state.call_stack[state.ci].saved_pc = pc;
                    vm_settable(state, Val::Table(env), key, val, &proto, pc, base, None)?;
                }

                OpCode::GetGlobalSlot => {
                    let slot_idx = instr.bx() as usize;
                    let Some(runtime) = state.global_slots.as_ref() else {
                        return Err(runtime_error(&proto, pc, "global slot runtime missing"));
                    };
                    let Some(key) = get_global_slot_key(state, slot_idx) else {
                        return Err(runtime_error(&proto, pc, "global slot runtime missing"));
                    };

                    if env != runtime.root_global {
                        if !try_plain_table_get_ref(state, env, key, ra) {
                            state.call_stack[state.ci].saved_pc = pc;
                            vm_gettable(state, Val::Table(env), key, ra, &proto, pc, base, None)?;
                        }
                        continue;
                    }

                    if slot_idx == 0 {
                        state.stack_set(ra, runtime.values[slot_idx]);
                        continue;
                    }

                    if let Some(live_ref) = lookup_slot_shadow_table(state, runtime)
                        && let Some(live_table) = state.gc.tables.get(live_ref)
                        && !(live_table.array_len() == 0 && live_table.hash_size() == 0)
                    {
                        let live_val =
                            live_table.get_str(runtime.name_keys[slot_idx], &state.gc.string_arena);
                        if live_val != Val::Nil {
                            propagate_slot_read_taint(state, live_ref, key);
                            state.stack_set(ra, live_val);
                            continue;
                        }
                    }

                    state.stack_set(ra, runtime.values[slot_idx]);
                }

                OpCode::SetGlobalSlot => {
                    let slot_idx = instr.bx() as usize;
                    let Some(key) = get_global_slot_key(state, slot_idx) else {
                        return Err(runtime_error(&proto, pc, "global slot runtime missing"));
                    };
                    let val = state.stack_get(ra);
                    state.call_stack[state.ci].saved_pc = pc;
                    vm_settable(state, Val::Table(env), key, val, &proto, pc, base, None)?;
                }

                // ----- Table access -----
                OpCode::GetTable => {
                    let b = instr.b() as usize;
                    let table_val = state.stack_get(base + b);
                    let key = rk(&state.stack, base, &proto.constants, instr.c());
                    if !try_plain_table_get(state, table_val, key, ra) {
                        state.call_stack[state.ci].saved_pc = pc;
                        vm_gettable(state, table_val, key, ra, &proto, pc, base, Some(b))?;
                    }
                }

                OpCode::SetTable => {
                    let table_val = state.stack_get(ra);
                    let key = rk(&state.stack, base, &proto.constants, instr.b());
                    let val = rk(&state.stack, base, &proto.constants, instr.c());
                    state.call_stack[state.ci].saved_pc = pc;
                    #[cfg(feature = "rehash-stats")]
                    let rehash_count_before = crate::vm::rehash_stats::total_count();
                    vm_settable(state, table_val, key, val, &proto, pc, base, Some(a))?;
                    #[cfg(feature = "rehash-stats")]
                    {
                        let rehash_count_after = crate::vm::rehash_stats::total_count();
                        let current_pc = pc.saturating_sub(1);
                        let line = proto.line_info.get(current_pc).copied().unwrap_or(0);
                        crate::vm::rehash_stats::record_lua_site(
                            &proto.source,
                            line,
                            rehash_count_after.saturating_sub(rehash_count_before),
                        );
                    }
                }

                OpCode::NewTable => {
                    state.gc_check()?;
                    let narray = fb2int(instr.b()) as usize;
                    let nhash = fb2int(instr.c()) as usize;
                    let t = state.gc.alloc_table(Table::with_sizes(narray, nhash));
                    state.stack_set(ra, Val::Table(t));
                }

                OpCode::OpSelf => {
                    let b = instr.b() as usize;
                    let table_val = state.stack_get(base + b);
                    // R(A+1) := R(B) -- save table for method call
                    state.stack_set(ra + 1, table_val);
                    let key = rk(&state.stack, base, &proto.constants, instr.c());
                    if !try_plain_table_get(state, table_val, key, ra) {
                        state.call_stack[state.ci].saved_pc = pc;
                        vm_gettable(state, table_val, key, ra, &proto, pc, base, Some(b))?;
                    }
                }

                // ----- Arithmetic -----
                OpCode::Add => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    if let (Some(nb), Some(nc)) = (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        state.stack_set(ra, Val::Num(nb + nc));
                    } else {
                        state.call_stack[state.ci].saved_pc = pc;
                        call_bin_tm(
                            state,
                            b_val,
                            c_val,
                            ra,
                            TMS::Add,
                            &proto,
                            pc,
                            base,
                            instr.b(),
                            instr.c(),
                        )?;
                    }
                }

                OpCode::Sub => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    if let (Some(nb), Some(nc)) = (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        state.stack_set(ra, Val::Num(nb - nc));
                    } else {
                        state.call_stack[state.ci].saved_pc = pc;
                        call_bin_tm(
                            state,
                            b_val,
                            c_val,
                            ra,
                            TMS::Sub,
                            &proto,
                            pc,
                            base,
                            instr.b(),
                            instr.c(),
                        )?;
                    }
                }

                OpCode::Mul => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    if let (Some(nb), Some(nc)) = (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        state.stack_set(ra, Val::Num(nb * nc));
                    } else {
                        state.call_stack[state.ci].saved_pc = pc;
                        call_bin_tm(
                            state,
                            b_val,
                            c_val,
                            ra,
                            TMS::Mul,
                            &proto,
                            pc,
                            base,
                            instr.b(),
                            instr.c(),
                        )?;
                    }
                }

                OpCode::Div => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    if let (Some(nb), Some(nc)) = (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        state.stack_set(ra, Val::Num(nb / nc));
                    } else {
                        state.call_stack[state.ci].saved_pc = pc;
                        call_bin_tm(
                            state,
                            b_val,
                            c_val,
                            ra,
                            TMS::Div,
                            &proto,
                            pc,
                            base,
                            instr.b(),
                            instr.c(),
                        )?;
                    }
                }

                OpCode::Mod => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    if let (Some(nb), Some(nc)) = (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        // Lua mod: a - floor(a/b)*b
                        state.stack_set(ra, Val::Num((nb / nc).floor().mul_add(-nc, nb)));
                    } else {
                        state.call_stack[state.ci].saved_pc = pc;
                        call_bin_tm(
                            state,
                            b_val,
                            c_val,
                            ra,
                            TMS::Mod,
                            &proto,
                            pc,
                            base,
                            instr.b(),
                            instr.c(),
                        )?;
                    }
                }

                OpCode::Pow => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    if let (Some(nb), Some(nc)) = (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        state.stack_set(ra, Val::Num(nb.powf(nc)));
                    } else {
                        state.call_stack[state.ci].saved_pc = pc;
                        call_bin_tm(
                            state,
                            b_val,
                            c_val,
                            ra,
                            TMS::Pow,
                            &proto,
                            pc,
                            base,
                            instr.b(),
                            instr.c(),
                        )?;
                    }
                }

                OpCode::Unm => {
                    let b = instr.b() as usize;
                    let b_val = state.stack_get(base + b);
                    if let Some(nb) = coerce_to_number(b_val, &state.gc) {
                        state.stack_set(ra, Val::Num(-nb));
                    } else {
                        // __unm: try on the single operand only
                        let tm = get_tm_for_val(&state.gc, b_val, TMS::Unm);
                        match tm {
                            Some(tm_val) => {
                                state.call_stack[state.ci].saved_pc = pc;
                                call_tm_res(state, tm_val, b_val, b_val, ra)?;
                            }
                            None => {
                                return Err(type_error(
                                    state,
                                    &proto,
                                    pc,
                                    base,
                                    b,
                                    "perform arithmetic on",
                                ));
                            }
                        }
                    }
                }

                OpCode::Not => {
                    let b = instr.b() as usize;
                    let b_val = state.stack_get(base + b);
                    state.stack_set(ra, Val::Bool(!b_val.is_truthy()));
                }

                // ----- String operations -----
                OpCode::Len => {
                    let b = instr.b() as usize;
                    let b_val = state.stack_get(base + b);
                    match b_val {
                        Val::Str(r) => {
                            let s =
                                state.gc.string_arena.get(r).ok_or_else(|| {
                                    runtime_error_simple("invalid string reference")
                                })?;
                            #[allow(clippy::cast_precision_loss)]
                            state.stack_set(ra, Val::Num(s.len() as f64));
                        }
                        Val::Table(r) => {
                            // Lua 5.1.1: tables always use raw length (no __len).
                            let t =
                                state.gc.tables.get(r).ok_or_else(|| {
                                    runtime_error_simple("invalid table reference")
                                })?;
                            #[allow(clippy::cast_precision_loss)]
                            let len = t.len(&state.gc.string_arena) as f64;
                            state.stack_set(ra, Val::Num(len));
                        }
                        _ => {
                            // Try __len metamethod for other types.
                            let tm = get_tm_for_val(&state.gc, b_val, TMS::Len);
                            if let Some(tm_val) = tm {
                                state.call_stack[state.ci].saved_pc = pc;
                                call_tm_res(state, tm_val, b_val, Val::Nil, ra)?;
                            } else {
                                return Err(type_error(
                                    state,
                                    &proto,
                                    pc,
                                    base,
                                    b,
                                    "get length of",
                                ));
                            }
                        }
                    }
                }

                OpCode::Concat => {
                    state.gc_check()?;
                    let b = instr.b() as usize;
                    let c = instr.c() as usize;
                    state.call_stack[state.ci].saved_pc = pc;
                    vm_concat(state, base, b, c, &proto, pc)?;
                    // The result is in R(base+b). Copy to R(A) if needed.
                    if ra != base + b {
                        let val = state.stack_get(base + b);
                        state.stack_set(ra, val);
                    }
                }

                // ----- Comparison -----
                OpCode::Eq => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    let equal = if val_equal(b_val, c_val, &state.gc) {
                        // Raw-equal succeeded.
                        true
                    } else if std::mem::discriminant(&b_val) != std::mem::discriminant(&c_val) {
                        // Different types are never equal (no metamethod).
                        false
                    } else {
                        // Same type, not raw-equal. Try __eq metamethod.
                        // Only tables and userdata support __eq.
                        let tm = match (b_val, c_val) {
                            (Val::Table(r1), Val::Table(r2)) => {
                                let mt1 = state.gc.tables.get(r1).and_then(Table::metatable);
                                let mt2 = state.gc.tables.get(r2).and_then(Table::metatable);
                                get_comp_tm(
                                    &state.gc.tables,
                                    &state.gc.string_arena,
                                    mt1,
                                    mt2,
                                    TMS::Eq,
                                    &state.gc.tm_names,
                                )
                            }
                            (Val::Userdata(r1), Val::Userdata(r2)) => {
                                let mt1 = state.gc.userdata.get(r1).and_then(Userdata::metatable);
                                let mt2 = state.gc.userdata.get(r2).and_then(Userdata::metatable);
                                get_comp_tm(
                                    &state.gc.tables,
                                    &state.gc.string_arena,
                                    mt1,
                                    mt2,
                                    TMS::Eq,
                                    &state.gc.tm_names,
                                )
                            }
                            _ => None,
                        };
                        if let Some(tm_val) = tm {
                            state.call_stack[state.ci].saved_pc = pc;
                            let res = state.top;
                            call_tm_res(state, tm_val, b_val, c_val, res)?;
                            // PUC-Rio reads from L->top after callTMres
                            // decrements it. We saved the result position.
                            state.stack_get(res).is_truthy()
                        } else {
                            false
                        }
                    };
                    // PUC-Rio: if (result == A) then dojump; pc++.
                    // dojump adds sbx to pc (can be negative). We combine
                    // with the +1 to avoid usize underflow on the intermediate.
                    let expected = a != 0;
                    if equal == expected {
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + i64::from(jump_instr.sbx()) + 1) as usize;
                    } else {
                        pc += 1;
                    }
                }

                OpCode::Lt => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    state.call_stack[state.ci].saved_pc = pc;
                    let result = val_less_than(b_val, c_val, state, &proto, pc)?;
                    let expected = a != 0;
                    if result == expected {
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + i64::from(jump_instr.sbx()) + 1) as usize;
                    } else {
                        pc += 1;
                    }
                }

                OpCode::Le => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    state.call_stack[state.ci].saved_pc = pc;
                    let result = val_less_equal(b_val, c_val, state, &proto, pc)?;
                    let expected = a != 0;
                    if result == expected {
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + i64::from(jump_instr.sbx()) + 1) as usize;
                    } else {
                        pc += 1;
                    }
                }

                // ----- Logic / test -----
                OpCode::Test => {
                    let val = state.stack_get(ra);
                    let c = instr.c() != 0;
                    if val.is_truthy() == c {
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + i64::from(jump_instr.sbx()) + 1) as usize;
                    } else {
                        pc += 1;
                    }
                }

                OpCode::TestSet => {
                    let b = instr.b() as usize;
                    let rb = state.stack_get(base + b);
                    let c = instr.c() != 0;
                    if rb.is_truthy() == c {
                        state.stack_set(ra, rb);
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + i64::from(jump_instr.sbx()) + 1) as usize;
                    } else {
                        pc += 1;
                    }
                }

                // ----- Control flow -----
                OpCode::Jmp => {
                    let sbx = instr.sbx();
                    pc = ((pc as i64) + i64::from(sbx)) as usize;
                }

                OpCode::ForPrep => {
                    let init = state.stack_get(ra);
                    let limit = state.stack_get(ra + 1);
                    let step = state.stack_get(ra + 2);

                    if let Some((n_init, n_limit, n_step)) =
                        coerce_integer_for_loop(init, limit, step, &state.gc)
                    {
                        state.stack_set(ra, Val::Num((n_init - n_step) as f64));
                        state.stack_set(ra + 1, Val::Num(n_limit as f64));
                        state.stack_set(ra + 2, Val::Num(n_step as f64));

                        let sbx = instr.sbx();
                        pc = ((pc as i64) + i64::from(sbx)) as usize;
                        continue;
                    }

                    let n_init = coerce_to_number(init, &state.gc).ok_or_else(|| {
                        runtime_error(&proto, pc, "'for' initial value must be a number")
                    })?;
                    let n_limit = coerce_to_number(limit, &state.gc)
                        .ok_or_else(|| runtime_error(&proto, pc, "'for' limit must be a number"))?;
                    let n_step = coerce_to_number(step, &state.gc)
                        .ok_or_else(|| runtime_error(&proto, pc, "'for' step must be a number"))?;

                    // R(A) -= step (so the first FORLOOP adds it back).
                    state.stack_set(ra, Val::Num(n_init - n_step));
                    state.stack_set(ra + 1, Val::Num(n_limit));
                    state.stack_set(ra + 2, Val::Num(n_step));

                    let sbx = instr.sbx();
                    pc = ((pc as i64) + i64::from(sbx)) as usize;
                }

                OpCode::ForLoop => {
                    if let Some((idx, limit, step)) = integer_for_loop_state(state, ra)
                        && let Some(next_idx) = idx.checked_add(step)
                    {
                        let continue_loop = if step > 0 {
                            next_idx <= limit
                        } else {
                            limit <= next_idx
                        };

                        if continue_loop {
                            let sbx = instr.sbx();
                            pc = ((pc as i64) + i64::from(sbx)) as usize;
                            let next_idx = Val::Num(next_idx as f64);
                            state.stack_set(ra, next_idx);
                            state.stack_set(ra + 3, next_idx);
                        }
                        continue;
                    }

                    let Val::Num(step) = state.stack_get(ra + 2) else {
                        return Err(runtime_error(&proto, pc, "'for' step is not a number"));
                    };
                    let Val::Num(idx) = state.stack_get(ra) else {
                        return Err(runtime_error(&proto, pc, "'for' index is not a number"));
                    };
                    let idx = idx + step;
                    let Val::Num(limit) = state.stack_get(ra + 1) else {
                        return Err(runtime_error(&proto, pc, "'for' limit is not a number"));
                    };

                    let continue_loop = if step > 0.0 {
                        idx <= limit
                    } else {
                        limit <= idx
                    };

                    if continue_loop {
                        let sbx = instr.sbx();
                        pc = ((pc as i64) + i64::from(sbx)) as usize;
                        state.stack_set(ra, Val::Num(idx)); // internal index
                        state.stack_set(ra + 3, Val::Num(idx)); // external index
                    }
                }

                OpCode::TForLoop => {
                    // Generic for: call iterator function.
                    let cb = ra + 3;
                    // Set up: R(A+3) = R(A), R(A+4) = R(A+1), R(A+5) = R(A+2)
                    state.stack_set(cb + 2, state.stack_get(ra + 2));
                    state.stack_set(cb + 1, state.stack_get(ra + 1));
                    state.stack_set(cb, state.stack_get(ra));
                    state.top = cb + 3;

                    // Save pc before call.
                    state.call_stack[state.ci].saved_pc = pc;

                    let c = instr.c() as i32;
                    state.call_depth += 1;
                    let tfor_result = (|| {
                        state.check_stack_overflow()?;
                        match state.precall(cb, c)? {
                            CallResult::Lua => execute(state),
                            CallResult::Rust => Ok(()),
                        }
                    })();
                    state.call_depth -= 1;
                    tfor_result?;

                    // Restore top to frame top.
                    state.top = state.call_stack[state.ci].top;

                    // Check: if R(A+3) is not nil, continue loop.
                    let result_base = ra + 3;
                    let control = state.stack_get(result_base);
                    if !control.is_nil() {
                        // Save control variable: R(A+2) = R(A+3)
                        state.stack_set(ra + 2, control);
                        // Jump back.
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + i64::from(jump_instr.sbx())) as usize;
                    }
                    pc += 1;
                }

                // ----- Calls and returns -----
                OpCode::Call => {
                    let b = instr.b();
                    let c = instr.c();

                    if b != 0 {
                        state.top = ra + b as usize;
                    }
                    // else: B==0 means top is already set (MULTRET args)

                    let num_results = if c == 0 { LUA_MULTRET } else { c as i32 - 1 };

                    // Save our pc before the call.
                    state.call_stack[state.ci].saved_pc = pc;

                    match state.precall(ra, num_results)? {
                        CallResult::Lua => {
                            // Lua-to-Lua call: no recursive execute(), no
                            // call_depth increment. Just break to the outer
                            // loop to re-read the new frame's proto/base.
                            // This matches PUC-Rio's `goto reentry` pattern.
                            nexeccalls += 1;
                        }
                        CallResult::Rust => {
                            // Rust function already completed in precall.
                            // Restore top for fixed results.
                            if c != 0 {
                                state.top = state.call_stack[state.ci].top;
                            }
                            // Break to outer loop to re-read env -- Rust
                            // functions like module() can change the running
                            // closure's environment via setfenv.
                        }
                    }
                    break;
                }

                OpCode::TailCall => {
                    let b = instr.b();

                    if b != 0 {
                        state.top = ra + b as usize;
                    }

                    // Save pc before the call (matches PUC-Rio).
                    state.call_stack[state.ci].saved_pc = pc;

                    // PUC-Rio calls precall FIRST, then only does the
                    // tail call optimization for Lua-to-Lua calls. For
                    // Lua-to-C calls, the C function has already completed
                    // and the caller's frame stays on the stack.
                    match state.precall(ra, LUA_MULTRET)? {
                        CallResult::Lua => {
                            // Tail call optimization: put the new Lua frame
                            // in place of the previous one.
                            let prev_ci = state.ci - 1;
                            let old_func = state.call_stack[prev_ci].func;
                            let new_func = state.call_stack[state.ci].func;

                            // Close upvalues at the old base.
                            state.close_upvalues(state.call_stack[prev_ci].base);

                            // Adjust previous frame's base.
                            let base_offset = state.call_stack[state.ci].base - new_func;
                            state.call_stack[prev_ci].base = old_func + base_offset;
                            state.base = state.call_stack[prev_ci].base;

                            // Move the new frame's stack down over the old
                            // frame.
                            let mut aux = 0;
                            while new_func + aux < state.top {
                                state.stack_set(old_func + aux, state.stack_get(new_func + aux));
                                aux += 1;
                            }
                            state.top = old_func + aux;
                            state.call_stack[prev_ci].top = state.top;

                            // Update saved_pc from the new frame.
                            state.call_stack[prev_ci].saved_pc =
                                state.call_stack[state.ci].saved_pc;
                            state.call_stack[prev_ci].tail_calls += 1;

                            // Remove the new frame -- the previous frame
                            // now holds the callee's data.
                            state.pop_ci();
                        }
                        CallResult::Rust => {
                            // Rust tail-call completed. poscall already
                            // unwound the Rust frame and placed results.
                            // Do NOT restore top to ci.top here -- poscall
                            // set top to reflect the actual result count,
                            // and the subsequent RETURN (B=0) uses top for
                            // MULTRET. Matches PUC-Rio: case PCRC just
                            // refreshes base and continues.
                            base = state.call_stack[state.ci].base;
                            // Continue executing in the current frame.
                            continue;
                        }
                    }
                    break; // Re-enter outer loop for Lua tail call.
                }

                OpCode::Return => {
                    let b = instr.b();
                    let first_result = ra;

                    // Close upvalues at current base.
                    state.close_upvalues(base);

                    if b != 0 {
                        state.top = first_result + (b as usize) - 1;
                    }
                    // else: B==0, use everything up to current top.

                    let fixed_results = state.poscall(first_result);

                    nexeccalls -= 1;
                    if nexeccalls == 0 {
                        // The function that execute() was called to run has
                        // returned. Exit the execute loop.
                        return Ok(());
                    }
                    // The callee returned but there are still Lua callers
                    // within this execute() invocation. Restore top if
                    // fixed results, then re-enter the outer loop.
                    if fixed_results {
                        state.top = state.call_stack[state.ci].top;
                    }
                    break; // goto reentry
                }

                // ----- Upvalues -----
                OpCode::GetUpval => {
                    let b = instr.b() as usize;
                    let cl = state
                        .gc
                        .closures
                        .get(closure_ref)
                        .ok_or_else(|| runtime_error_simple("invalid closure"))?;
                    if let Closure::Lua(lcl) = cl {
                        if b < lcl.upvalues.len() {
                            let uv_ref = lcl.upvalues[b];
                            let val = state
                                .gc
                                .upvalues
                                .get(uv_ref)
                                .map_or(Val::Nil, |uv| uv.get(&state.stack));
                            state.stack_set(ra, val);
                        } else {
                            state.stack_set(ra, Val::Nil);
                        }
                    }
                }

                OpCode::SetUpval => {
                    let b = instr.b() as usize;
                    let val = state.stack_get(ra);
                    let cl = state
                        .gc
                        .closures
                        .get(closure_ref)
                        .ok_or_else(|| runtime_error_simple("invalid closure"))?;
                    if let Closure::Lua(lcl) = cl
                        && b < lcl.upvalues.len()
                    {
                        let uv_ref = lcl.upvalues[b];
                        let uv_color = state.gc.upvalues.color(uv_ref);
                        if let Some(uv) = state.gc.upvalues.get_mut(uv_ref) {
                            uv.set(&mut state.stack, val);
                        }
                        // Forward barrier: mark child if upvalue is black.
                        if let Some(color) = uv_color {
                            state.gc.barrier_forward_val(color, val);
                        }
                    }
                }

                // ----- Closure creation -----
                OpCode::Closure => {
                    state.gc_check()?;
                    let bx = instr.bx() as usize;
                    let child_proto = ProtoRef::clone(&proto.protos[bx]);
                    let nups = child_proto.num_upvalues as usize;

                    let mut new_cl = LuaClosure::new(child_proto, env);

                    // Process pseudo-instructions for upvalue capture.
                    for _ in 0..nups {
                        let pseudo = Instruction::from_raw(proto.code[pc]);
                        pc += 1;

                        match pseudo.opcode() {
                            OpCode::Move => {
                                // Capture local from current frame.
                                let local_reg = pseudo.b() as usize;
                                let stack_slot = base + local_reg;
                                let uv_ref = state.find_upvalue(stack_slot);
                                new_cl.upvalues.push(uv_ref);
                            }
                            OpCode::GetUpval => {
                                // Share parent's upvalue.
                                let parent_uv_idx = pseudo.b() as usize;
                                let cl = state
                                    .gc
                                    .closures
                                    .get(closure_ref)
                                    .ok_or_else(|| runtime_error_simple("invalid closure"))?;
                                if let Closure::Lua(lcl) = cl
                                    && parent_uv_idx < lcl.upvalues.len()
                                {
                                    new_cl.upvalues.push(lcl.upvalues[parent_uv_idx]);
                                }
                            }
                            _ => {
                                return Err(runtime_error(
                                    &proto,
                                    pc,
                                    "invalid pseudo-instruction after CLOSURE",
                                ));
                            }
                        }
                    }

                    let cl_ref = state.gc.alloc_closure(Closure::Lua(new_cl));
                    state.stack_set(ra, Val::Function(cl_ref));
                }

                OpCode::Close => {
                    state.close_upvalues(ra);
                }

                // ----- Varargs -----
                OpCode::VarArg => {
                    let b = instr.b() as i32;
                    let ci_func = state.call_stack[state.ci].func;
                    // Varargs are stored between ci.func+1 and base.
                    let vararg_start = ci_func + 1 + proto.num_params as usize;
                    let num_varargs = base.saturating_sub(vararg_start);

                    let wanted = if b == 0 {
                        // B==0: copy all varargs, adjust top.
                        num_varargs
                    } else {
                        (b - 1) as usize
                    };

                    // Ensure stack space.
                    state.ensure_stack(ra + wanted);

                    for i in 0..wanted {
                        if i < num_varargs {
                            let val = state.stack_get(vararg_start + i);
                            state.stack_set(ra + i, val);
                        } else {
                            state.stack_set(ra + i, Val::Nil);
                        }
                    }

                    if b == 0 {
                        state.top = ra + wanted;
                    }
                }

                // ----- SETLIST -----
                OpCode::SetList => {
                    let mut n = instr.b() as usize;
                    let mut c = instr.c() as usize;

                    if n == 0 {
                        n = state.top - ra - 1;
                        state.top = state.call_stack[state.ci].top;
                    }
                    if c == 0 {
                        // Next instruction contains the real C value.
                        c = proto.code[pc] as usize;
                        pc += 1;
                    }

                    let Val::Table(table_ref) = state.stack_get(ra) else {
                        return Err(type_error(state, &proto, pc, base, a, "index"));
                    };

                    let offset = (c - 1) * LFIELDS_PER_FLUSH as usize;
                    let last = offset + n;

                    // Pre-allocate array to exact size needed (PUC-Rio
                    // luaH_resizearray). Without this, each insert beyond
                    // the current array size triggers rehash, which rounds
                    // up to a power of 2.
                    if let Some(table) = state.gc.tables.get_mut(table_ref) {
                        let mem_before = table.estimated_memory();
                        table.ensure_array_capacity(last);
                        let mem_after = table.estimated_memory();
                        if mem_after > mem_before {
                            state.gc.gc_state.total_bytes += mem_after - mem_before;
                        }
                    }
                    if state.gc.gc_state.total_bytes > state.gc.gc_state.alloc_limit {
                        return Err(crate::LuaError::Memory);
                    }

                    write_setlist_array_values(state, table_ref, ra, offset, n)?;
                }
            }
        }
    }
}

fn write_setlist_array_values(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    ra: usize,
    offset: usize,
    count: usize,
) -> LuaResult<()> {
    ensure_table_not_frozen(state, table_ref)?;
    let stack = &state.stack;
    let table = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| RuntimeError::new("invalid table reference"))?;

    for i in 1..=count {
        let value = stack.get(ra + i).copied().unwrap_or(Val::Nil);
        table.set_array_slot(offset + i - 1, value);
    }
    state.gc.barrier_back(table_ref);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::approx_constant
)]
mod tests {
    use super::*;
    use crate::vm::gc::arena::GcRef;
    use crate::vm::instructions::{Instruction, OpCode, rk_as_k};
    use crate::vm::string::LuaString;
    use crate::vm::table::Table;

    /// Helper: create a LuaState with a proto loaded as the current function.
    fn setup_state(proto: Proto) -> LuaState {
        let mut state = LuaState::new();
        let proto_rc = ProtoRef::new(proto);
        let env = state.global;

        let cl = LuaClosure::new(proto_rc, env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));

        // Place closure at stack[0], set up frame.
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        state
    }

    /// Helper: create a minimal proto with given instructions.
    fn make_proto(code: Vec<u32>, constants: Vec<Val>) -> Proto {
        let mut p = Proto::new("test");
        let n = code.len();
        p.code = code;
        p.constants = constants;
        p.line_info = vec![1; n];
        p.max_stack_size = 20;
        p.is_vararg = VARARG_ISVARARG;
        p
    }

    fn install_slots(
        state: &mut LuaState,
        values: Vec<Val>,
        names: &[GcRef<LuaString>],
        shadow_key: Option<GcRef<LuaString>>,
    ) {
        let mut aligned_names = if values.len() == names.len() + 1 {
            let mut aligned = Vec::with_capacity(values.len());
            aligned.push(state.gc.intern_string_static(b"_G"));
            aligned.extend_from_slice(names);
            aligned
        } else {
            names.to_vec()
        };
        assert_eq!(
            values.len(),
            aligned_names.len(),
            "test slot helper requires keys aligned with slot indexes",
        );
        state.install_global_slots(
            values.into_boxed_slice(),
            std::mem::take(&mut aligned_names).into_boxed_slice(),
            shadow_key,
        );
    }

    #[test]
    fn poscall_zero_results_clears_function_slot() {
        let mut state = LuaState::new();
        state.stack_set(0, Val::Num(99.0));
        state.top = 3;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);
        state.push_ci(CallInfo::new(0, 1, 5, 0));

        let fixed = state.poscall(3);

        assert!(fixed);
        assert_eq!(state.ci, 0);
        assert_eq!(state.top, 0);
    }

    #[test]
    fn poscall_one_result_moves_single_value() {
        let mut state = LuaState::new();
        state.stack_set(0, Val::Num(11.0));
        state.stack_set(3, Val::Num(42.0));
        state.top = 4;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);
        state.push_ci(CallInfo::new(0, 1, 5, 1));

        let fixed = state.poscall(3);

        assert!(fixed);
        assert_eq!(state.ci, 0);
        assert_eq!(state.stack_get(0), Val::Num(42.0));
        assert_eq!(state.top, 1);
    }

    // ----- Data movement tests -----

    #[test]
    fn op_move() {
        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(), // R(0) = 42.0
            Instruction::abc(OpCode::Move, 1, 0, 0).raw(), // R(1) = R(0)
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(42.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 1), Val::Num(42.0));
    }

    #[test]
    fn op_loadk() {
        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(3.14)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(3.14));
    }

    #[test]
    fn op_loadbool() {
        let code = vec![
            Instruction::abc(OpCode::LoadBool, 0, 1, 0).raw(), // R(0) = true
            Instruction::abc(OpCode::LoadBool, 1, 0, 0).raw(), // R(1) = false
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let mut state = setup_state(make_proto(code, vec![]));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Bool(true));
        assert_eq!(state.stack_get(state.base + 1), Val::Bool(false));
    }

    #[test]
    fn op_loadbool_skip() {
        let code = vec![
            Instruction::abc(OpCode::LoadBool, 0, 1, 1).raw(), // R(0) = true; skip
            Instruction::abc(OpCode::LoadBool, 0, 0, 0).raw(), // skipped
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let mut state = setup_state(make_proto(code, vec![]));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Bool(true));
    }

    #[test]
    fn op_loadnil() {
        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(), // R(0) = 1
            Instruction::a_bx(OpCode::LoadK, 1, 0).raw(), // R(1) = 1
            Instruction::a_bx(OpCode::LoadK, 2, 0).raw(), // R(2) = 1
            Instruction::abc(OpCode::LoadNil, 0, 2, 0).raw(), // R(0..2) = nil
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(1.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert!(state.stack_get(state.base).is_nil());
        assert!(state.stack_get(state.base + 1).is_nil());
        assert!(state.stack_get(state.base + 2).is_nil());
    }

    // ----- Arithmetic tests -----

    #[test]
    fn op_add() {
        let code = vec![
            Instruction::abc(OpCode::Add, 0, rk_as_k(0), rk_as_k(1)).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(10.0), Val::Num(20.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(30.0));
    }

    #[test]
    fn op_sub() {
        let code = vec![
            Instruction::abc(OpCode::Sub, 0, rk_as_k(0), rk_as_k(1)).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(50.0), Val::Num(30.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(20.0));
    }

    #[test]
    fn op_mul() {
        let code = vec![
            Instruction::abc(OpCode::Mul, 0, rk_as_k(0), rk_as_k(1)).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(6.0), Val::Num(7.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(42.0));
    }

    #[test]
    fn op_div() {
        let code = vec![
            Instruction::abc(OpCode::Div, 0, rk_as_k(0), rk_as_k(1)).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(10.0), Val::Num(4.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(2.5));
    }

    #[test]
    fn op_mod() {
        let code = vec![
            Instruction::abc(OpCode::Mod, 0, rk_as_k(0), rk_as_k(1)).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(10.0), Val::Num(3.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(1.0));
    }

    #[test]
    fn op_pow() {
        let code = vec![
            Instruction::abc(OpCode::Pow, 0, rk_as_k(0), rk_as_k(1)).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(2.0), Val::Num(10.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(1024.0));
    }

    #[test]
    fn op_unm() {
        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(),
            Instruction::abc(OpCode::Unm, 1, 0, 0).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(42.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 1), Val::Num(-42.0));
    }

    #[test]
    fn op_not() {
        let code = vec![
            Instruction::abc(OpCode::LoadBool, 0, 1, 0).raw(), // R(0) = true
            Instruction::abc(OpCode::Not, 1, 0, 0).raw(),      // R(1) = not R(0)
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let mut state = setup_state(make_proto(code, vec![]));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 1), Val::Bool(false));
    }

    // ----- Comparison tests -----

    #[test]
    fn op_eq_numbers_equal() {
        // if 1 == 1 then R(0) = true else R(0) = false
        let code = vec![
            Instruction::abc(OpCode::Eq, 1, rk_as_k(0), rk_as_k(0)).raw(), // if (K0 == K0) == 1
            Instruction::a_sbx(OpCode::Jmp, 0, 1).raw(),                   // jump over next
            Instruction::a_sbx(OpCode::Jmp, 0, 1).raw(),                   // skip to false
            Instruction::abc(OpCode::LoadBool, 0, 1, 1).raw(),             // R(0) = true; skip
            Instruction::abc(OpCode::LoadBool, 0, 0, 0).raw(),             // R(0) = false
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(1.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Bool(true));
    }

    // ----- Control flow tests -----

    #[test]
    fn op_jmp() {
        let code = vec![
            Instruction::a_sbx(OpCode::Jmp, 0, 1).raw(), // skip next
            Instruction::abc(OpCode::LoadBool, 0, 0, 0).raw(), // skipped
            Instruction::abc(OpCode::LoadBool, 0, 1, 0).raw(), // R(0) = true
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let mut state = setup_state(make_proto(code, vec![]));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Bool(true));
    }

    #[test]
    fn op_forloop() {
        // for i=1,3 do end -- R(0)=init, R(1)=limit, R(2)=step, R(3)=i
        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(), // R(0) = 1 (init)
            Instruction::a_bx(OpCode::LoadK, 1, 1).raw(), // R(1) = 3 (limit)
            Instruction::a_bx(OpCode::LoadK, 2, 0).raw(), // R(2) = 1 (step)
            Instruction::a_sbx(OpCode::ForPrep, 0, 0).raw(), // R(0) -= step; jmp +0
            Instruction::a_sbx(OpCode::ForLoop, 0, -1).raw(), // loop back
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(1.0), Val::Num(3.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        // After loop, R(0) should be 3 (last successful index)
        // and R(3) should be 3.
        assert_eq!(state.stack_get(state.base + 3), Val::Num(3.0));
    }

    #[test]
    fn op_forloop_descending_integer_step() {
        // for i=3,1,-1 do end
        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(), // R(0) = 3 (init)
            Instruction::a_bx(OpCode::LoadK, 1, 1).raw(), // R(1) = 1 (limit)
            Instruction::a_bx(OpCode::LoadK, 2, 2).raw(), // R(2) = -1 (step)
            Instruction::a_sbx(OpCode::ForPrep, 0, 0).raw(),
            Instruction::a_sbx(OpCode::ForLoop, 0, -1).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(3.0), Val::Num(1.0), Val::Num(-1.0)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 3), Val::Num(1.0));
    }

    #[test]
    fn op_forloop_fractional_step_still_works() {
        // for i=1,2,0.5 do end
        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(), // R(0) = 1 (init)
            Instruction::a_bx(OpCode::LoadK, 1, 1).raw(), // R(1) = 2 (limit)
            Instruction::a_bx(OpCode::LoadK, 2, 2).raw(), // R(2) = 0.5 (step)
            Instruction::a_sbx(OpCode::ForPrep, 0, 0).raw(),
            Instruction::a_sbx(OpCode::ForLoop, 0, -1).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(1.0), Val::Num(2.0), Val::Num(0.5)];
        let mut state = setup_state(make_proto(code, constants));
        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 3), Val::Num(2.0));
    }

    // ----- Table tests -----

    #[test]
    fn op_newtable_respects_compiler_size_hints() {
        // NewTable(0, 0) is emitted for a bare `{}` literal. The executor
        // must keep that table truly empty so integer appends can grow the
        // array part instead of being stranded in preallocated hash slots.
        // Non-zero hints are still preserved.
        let mut state = LuaState::new();

        // fb2int(5) == 5 (values < 8 pass through), so C=5 means nhash=5,
        // which Table::with_sizes rounds up to 8 slots.
        let code = vec![
            Instruction::abc(OpCode::NewTable, 0, 0, 0).raw(), // R(0) = {} (hint 0,0)
            Instruction::abc(OpCode::NewTable, 1, 0, 5).raw(), // R(1) = hash-hint 5 → 8 slots
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let proto_rc = ProtoRef::new(make_proto(code, vec![]));
        let env = state.global;
        let cl = LuaClosure::new(proto_rc, env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();

        let Val::Table(empty) = state.stack_get(state.base) else {
            panic!("R(0) should be a table");
        };
        let Val::Table(hinted) = state.stack_get(state.base + 1) else {
            panic!("R(1) should be a table");
        };
        assert_eq!(
            state.gc.tables.get(empty).unwrap().hash_size(),
            0,
            "empty `{{}}` should not pre-allocate hash slots",
        );
        assert_eq!(
            state.gc.tables.get(hinted).unwrap().hash_size(),
            8,
            "hinted table should round its hint (5) up to next power of 2",
        );
    }

    #[test]
    fn op_newtable_and_settable_gettable() {
        let mut state = LuaState::new();
        let key_ref = state.gc.intern_string(b"x");

        let code = vec![
            Instruction::abc(OpCode::NewTable, 0, 0, 0).raw(), // R(0) = {}
            Instruction::abc(OpCode::SetTable, 0, rk_as_k(0), rk_as_k(1)).raw(), // R(0)["x"] = 42
            Instruction::abc(OpCode::GetTable, 1, 0, rk_as_k(0)).raw(), // R(1) = R(0)["x"]
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Str(key_ref), Val::Num(42.0)];

        let proto_rc = ProtoRef::new(make_proto(code, constants));
        let env = state.global;
        let cl = LuaClosure::new(proto_rc, env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 1), Val::Num(42.0));
    }

    #[test]
    fn op_setlist_populates_array_part() {
        let mut state = LuaState::new();
        let code = vec![
            Instruction::abc(OpCode::NewTable, 0, 3, 0).raw(),
            Instruction::a_bx(OpCode::LoadK, 1, 0).raw(),
            Instruction::a_bx(OpCode::LoadK, 2, 1).raw(),
            Instruction::a_bx(OpCode::LoadK, 3, 2).raw(),
            Instruction::abc(OpCode::SetList, 0, 3, 1).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(10.0), Val::Num(20.0), Val::Num(30.0)];

        let proto_rc = ProtoRef::new(make_proto(code, constants));
        let env = state.global;
        let cl = LuaClosure::new(proto_rc, env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        let Val::Table(table_ref) = state.stack_get(state.base) else {
            panic!("R(0) should be a table");
        };
        let table = state.gc.tables.get(table_ref).expect("table");
        assert_eq!(table.get_int(1), Val::Num(10.0));
        assert_eq!(table.get_int(2), Val::Num(20.0));
        assert_eq!(table.get_int(3), Val::Num(30.0));
    }

    // ----- Globals tests -----

    #[test]
    fn op_setglobal_getglobal() {
        let mut state = LuaState::new();
        let key_ref = state.gc.intern_string(b"myvar");

        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 1).raw(), // R(0) = 99
            Instruction::a_bx(OpCode::SetGlobal, 0, 0).raw(), // _G["myvar"] = R(0)
            Instruction::a_bx(OpCode::GetGlobal, 1, 0).raw(), // R(1) = _G["myvar"]
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Str(key_ref), Val::Num(99.0)];

        let proto_rc = ProtoRef::new(make_proto(code, constants));
        let env = state.global;
        let cl = LuaClosure::new(proto_rc, env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 1), Val::Num(99.0));
    }

    #[test]
    fn op_getglobalslot_falls_back_to_custom_env() {
        let mut state = LuaState::new();
        let custom_env = state.gc.alloc_table(Table::new());
        let g_key = state.gc.intern_string_static(b"_G");
        let key_ref = state.gc.intern_string(b"myvar");
        state.install_global_slots(
            vec![Val::Table(state.global), Val::Num(10.0)].into_boxed_slice(),
            vec![g_key, key_ref].into_boxed_slice(),
            None,
        );
        state
            .gc
            .tables
            .get_mut(custom_env)
            .expect("missing custom env")
            .raw_set(Val::Str(key_ref), Val::Num(77.0), &state.gc.string_arena)
            .expect("custom env raw_set should succeed");

        let code = vec![
            Instruction::a_bx(OpCode::GetGlobalSlot, 0, 1).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let proto_rc = ProtoRef::new(make_proto(code, vec![]));
        let cl = LuaClosure::new(proto_rc, custom_env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(77.0));
    }

    #[test]
    fn op_getglobalslot_reads_shadow_override_on_root_env() {
        let code = vec![
            Instruction::a_bx(OpCode::GetGlobalSlot, 0, 1).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let mut state = setup_state(make_proto(code, vec![]));
        let shadow_key = state.gc.intern_string_static(b"__slot_shadow");
        let g_key = state.gc.intern_string_static(b"_G");
        let key_ref = state.gc.intern_string(b"myvar");
        let shadow_ref = state.gc.alloc_table(Table::new());
        state.install_global_slots(
            vec![Val::Table(state.global), Val::Num(10.0)].into_boxed_slice(),
            vec![g_key, key_ref].into_boxed_slice(),
            Some(shadow_key),
        );
        state
            .gc
            .tables
            .get_mut(shadow_ref)
            .expect("missing shadow table")
            .raw_set(Val::Str(key_ref), Val::Num(77.0), &state.gc.string_arena)
            .expect("shadow raw_set should succeed");
        state
            .gc
            .tables
            .get_mut(state.registry)
            .expect("missing registry")
            .raw_set(
                Val::Str(shadow_key),
                Val::Table(shadow_ref),
                &state.gc.string_arena,
            )
            .expect("registry raw_set should succeed");

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(77.0));
    }

    #[test]
    fn op_setglobalslot_writes_through_current_env() {
        let mut state = LuaState::new();
        let custom_env = state.gc.alloc_table(Table::new());
        let g_key = state.gc.intern_string_static(b"_G");
        let key_ref = state.gc.intern_string(b"myvar");
        state.install_global_slots(
            vec![Val::Table(state.global), Val::Nil].into_boxed_slice(),
            vec![g_key, key_ref].into_boxed_slice(),
            None,
        );

        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(),
            Instruction::a_bx(OpCode::SetGlobalSlot, 0, 1).raw(),
            Instruction::a_bx(OpCode::GetGlobalSlot, 1, 1).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Num(99.0)];
        let proto_rc = ProtoRef::new(make_proto(code, constants));
        let cl = LuaClosure::new(proto_rc, custom_env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 1), Val::Num(99.0));
    }

    #[test]
    fn op_getglobal_slot_reads_frozen_value_for_root_env() {
        let mut state = LuaState::new();
        let mixin_key = state.gc.intern_string(b"Mixin");
        let code = vec![
            Instruction::a_bx(OpCode::GetGlobalSlot, 0, 1).raw(),
            Instruction::abc(OpCode::Return, 0, 2, 0).raw(),
        ];
        let mut proto = make_proto(code, vec![]);
        proto.global_slot_names.push(Some(b"_G".to_vec()));
        proto.global_slot_names.push(Some(b"Mixin".to_vec()));
        let proto_rc = ProtoRef::new(proto);
        let cl = LuaClosure::new(proto_rc, state.global);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);
        let root_global = state.global;
        install_slots(
            &mut state,
            vec![Val::Table(root_global), Val::Num(42.0)],
            &[mixin_key],
            None,
        );

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(42.0));
    }

    #[test]
    fn op_getglobal_slot_falls_back_to_closure_env_lookup() {
        let mut state = LuaState::new();
        let mixin_key = state.gc.intern_string(b"Mixin");
        let custom_env = state.gc.alloc_table(Table::new());
        state
            .gc
            .tables
            .get_mut(custom_env)
            .expect("custom env")
            .raw_set(Val::Str(mixin_key), Val::Num(77.0), &state.gc.string_arena)
            .expect("seed env");
        let root_global = state.global;
        install_slots(
            &mut state,
            vec![Val::Table(root_global), Val::Num(42.0)],
            &[mixin_key],
            None,
        );

        let code = vec![
            Instruction::a_bx(OpCode::GetGlobalSlot, 0, 1).raw(),
            Instruction::abc(OpCode::Return, 0, 2, 0).raw(),
        ];
        let mut proto = make_proto(code, vec![]);
        proto.global_slot_names.push(Some(b"_G".to_vec()));
        proto.global_slot_names.push(Some(b"Mixin".to_vec()));
        let proto_rc = ProtoRef::new(proto);
        let cl = LuaClosure::new(proto_rc, custom_env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(77.0));
    }

    #[test]
    fn op_getglobal_respects_env_index_metamethod() {
        let mut state = LuaState::new();
        let key_ref = state.gc.intern_string(b"myvar");
        let index_key = state.gc.intern_string(b"__index");

        let fallback = state.gc.alloc_table(Table::new());
        let metatable = state.gc.alloc_table(Table::new());

        state
            .gc
            .tables
            .get_mut(fallback)
            .expect("missing fallback table")
            .raw_set(Val::Str(key_ref), Val::Num(77.0), &state.gc.string_arena)
            .expect("fallback raw_set should succeed");
        state
            .gc
            .tables
            .get_mut(metatable)
            .expect("missing metatable")
            .raw_set(
                Val::Str(index_key),
                Val::Table(fallback),
                &state.gc.string_arena,
            )
            .expect("metatable raw_set should succeed");
        state
            .gc
            .tables
            .get_mut(state.global)
            .expect("missing globals table")
            .set_metatable(Some(metatable));

        let code = vec![
            Instruction::a_bx(OpCode::GetGlobal, 0, 0).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Str(key_ref)];

        let proto_rc = ProtoRef::new(make_proto(code, constants));
        let env = state.global;
        let cl = LuaClosure::new(proto_rc, env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(77.0));
    }

    #[test]
    fn op_getglobal_prefers_direct_value_on_metatable_env() {
        let mut state = LuaState::new();
        let key_ref = state.gc.intern_string(b"myvar");
        let index_key = state.gc.intern_string(b"__index");

        let fallback = state.gc.alloc_table(Table::new());
        let metatable = state.gc.alloc_table(Table::new());

        state
            .gc
            .tables
            .get_mut(state.global)
            .expect("missing globals table")
            .raw_set(Val::Str(key_ref), Val::Num(42.0), &state.gc.string_arena)
            .expect("global raw_set should succeed");
        state
            .gc
            .tables
            .get_mut(fallback)
            .expect("missing fallback table")
            .raw_set(Val::Str(key_ref), Val::Num(77.0), &state.gc.string_arena)
            .expect("fallback raw_set should succeed");
        state
            .gc
            .tables
            .get_mut(metatable)
            .expect("missing metatable")
            .raw_set(
                Val::Str(index_key),
                Val::Table(fallback),
                &state.gc.string_arena,
            )
            .expect("metatable raw_set should succeed");
        state
            .gc
            .tables
            .get_mut(state.global)
            .expect("missing globals table")
            .set_metatable(Some(metatable));

        let code = vec![
            Instruction::a_bx(OpCode::GetGlobal, 0, 0).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Str(key_ref)];

        let proto_rc = ProtoRef::new(make_proto(code, constants));
        let env = state.global;
        let cl = LuaClosure::new(proto_rc, env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base), Val::Num(42.0));
    }

    // ----- Len tests -----

    #[test]
    fn op_len_string() {
        let mut state = LuaState::new();
        let s_ref = state.gc.intern_string(b"hello");

        let code = vec![
            Instruction::a_bx(OpCode::LoadK, 0, 0).raw(),
            Instruction::abc(OpCode::Len, 1, 0, 0).raw(),
            Instruction::abc(OpCode::Return, 0, 1, 0).raw(),
        ];
        let constants = vec![Val::Str(s_ref)];

        let proto_rc = ProtoRef::new(make_proto(code, constants));
        let env = state.global;
        let cl = LuaClosure::new(proto_rc, env);
        let cl_ref = state.gc.alloc_closure(Closure::Lua(cl));
        state.stack_set(0, Val::Function(cl_ref));
        state.base = 1;
        state.top = 1;
        state.call_stack[0] = CallInfo::new(0, 1, 41, LUA_MULTRET);

        execute(&mut state).ok();
        assert_eq!(state.stack_get(state.base + 1), Val::Num(5.0));
    }

    // ----- str_to_number tests -----

    #[test]
    fn str_to_number_basic() {
        assert_eq!(str_to_number(b"42"), Some(42.0));
        assert_eq!(str_to_number(b"3.14"), Some(3.14));
        assert_eq!(str_to_number(b"  42  "), Some(42.0));
        assert_eq!(str_to_number(b""), None);
        assert_eq!(str_to_number(b"abc"), None);
    }

    #[test]
    fn str_to_number_hex() {
        assert_eq!(str_to_number(b"0xff"), Some(255.0));
        assert_eq!(str_to_number(b"0xFF"), Some(255.0));
        assert_eq!(str_to_number(b"0x10"), Some(16.0));
    }

    // ----- fb2int tests -----

    #[test]
    fn fb2int_values() {
        assert_eq!(fb2int(0), 0);
        assert_eq!(fb2int(1), 1);
        assert_eq!(fb2int(7), 7); // exponent 0
        assert_eq!(fb2int(8), 8); // exponent 1: (0+8) << 0 = 8
    }

    // ----- Helper tests -----

    #[test]
    fn val_equal_basics() {
        let gc = Gc::default();
        assert!(val_equal(Val::Nil, Val::Nil, &gc));
        assert!(!val_equal(Val::Nil, Val::Bool(false), &gc));
        assert!(val_equal(Val::Num(1.0), Val::Num(1.0), &gc));
        assert!(!val_equal(Val::Num(1.0), Val::Num(2.0), &gc));
        assert!(val_equal(Val::Bool(true), Val::Bool(true), &gc));
    }
}
