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

use std::rc::Rc;

use crate::error::{LuaError, LuaResult, RuntimeError};

use super::callinfo::{CallInfo, LUA_MULTRET};
use super::closure::{Closure, LuaClosure, Upvalue};
use super::gc::arena::GcRef;
use super::instructions::{Instruction, LFIELDS_PER_FLUSH, OpCode, index_k, is_k};
use super::metatable::{MAXTAGLOOP, TMS, get_comp_tm, gettmbyobj, val_raw_equal};
use super::proto::{Proto, VARARG_ISVARARG};
use super::state::{Gc, LUA_MINSTACK, LuaState, MAXCALLS, MAXCCALLS};
use super::table::Table;
use super::value::{Userdata, Val};

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

/// Parse a byte slice as a Lua number (matching PUC-Rio's `luaO_str2d`).
///
/// Supports decimal, hex (`0x`/`0X` prefix), leading/trailing whitespace.
pub(crate) fn str_to_number(data: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(data).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Try hex first
    if trimmed.len() > 2 && trimmed.starts_with("0x")
        || trimmed.starts_with("0X")
        || trimmed.starts_with("-0x")
        || trimmed.starts_with("-0X")
        || trimmed.starts_with("+0x")
        || trimmed.starts_with("+0X")
    {
        // Parse hex integer
        let (sign, hex_str) = if trimmed.starts_with('-') {
            (-1.0_f64, &trimmed[3..])
        } else if trimmed.starts_with('+') {
            (1.0, &trimmed[3..])
        } else {
            (1.0, &trimmed[2..])
        };
        let val = u64::from_str_radix(hex_str, 16).ok()?;
        return Some(sign * val as f64);
    }
    // Try decimal
    trimmed.parse::<f64>().ok()
}

/// Coerces a value to a string for concatenation.
///
/// Numbers are formatted using Lua's %.14g rules. Strings pass through.
/// Returns `None` for other types.
#[allow(dead_code)] // Used in Phase 4b (tostring metamethod support)
fn coerce_to_string(val: Val, gc: &mut Gc) -> Option<Val> {
    match val {
        Val::Str(_) => Some(val),
        Val::Num(_) => {
            // Format the number as a string.
            let formatted = format!("{val}");
            let r = gc.intern_string(formatted.as_bytes());
            Some(Val::Str(r))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helper: runtime error construction
// ---------------------------------------------------------------------------

/// Creates a runtime error with source location from the current proto.
fn runtime_error(proto: &Proto, pc: usize, message: String) -> LuaError {
    let line = if pc > 0 && pc <= proto.line_info.len() {
        proto.line_info[pc - 1]
    } else if !proto.line_info.is_empty() {
        proto.line_info[0]
    } else {
        0
    };
    let source = chunkid(&proto.source);
    LuaError::Runtime(RuntimeError {
        message: format!("{source}:{line}: {message}"),
        level: 0,
        traceback: vec![],
    })
}

/// Type error for arithmetic.
fn arith_error(proto: &Proto, pc: usize, val: Val) -> LuaError {
    runtime_error(
        proto,
        pc,
        format!(
            "attempt to perform arithmetic on a {} value",
            val.type_name()
        ),
    )
}

/// Type error for comparison.
fn compare_error(proto: &Proto, pc: usize, left: Val, right: Val) -> LuaError {
    runtime_error(
        proto,
        pc,
        format!(
            "attempt to compare two {} values",
            if left.type_name() == right.type_name() {
                left.type_name().to_string()
            } else {
                format!("{} with {}", left.type_name(), right.type_name())
            }
        ),
    )
}

/// Type error for concatenation.
fn concat_error(proto: &Proto, pc: usize, val: Val) -> LuaError {
    runtime_error(
        proto,
        pc,
        format!("attempt to concatenate a {} value", val.type_name()),
    )
}

/// Type error for length.
fn len_error(proto: &Proto, pc: usize, val: Val) -> LuaError {
    runtime_error(
        proto,
        pc,
        format!("attempt to get length of a {} value", val.type_name()),
    )
}

/// Type error for indexing.
fn index_error(proto: &Proto, pc: usize, val: Val) -> LuaError {
    runtime_error(
        proto,
        pc,
        format!("attempt to index a {} value", val.type_name()),
    )
}

/// Type error for calling.
#[allow(dead_code)] // Used in Phase 4b (__call metamethod)
fn call_error(proto: &Proto, pc: usize, val: Val) -> LuaError {
    runtime_error(
        proto,
        pc,
        format!("attempt to call a {} value", val.type_name()),
    )
}

/// luaO_fb2int: convert a "floating point byte" to an integer.
///
/// Used by NEWTABLE to decode size hints. The format encodes
/// `(x & 7 + 8) << ((x >> 3) - 1)` for non-zero exponent,
/// or `x` directly when the exponent is zero.
fn fb2int(x: u32) -> u32 {
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
    /// Sets up a call frame for the function at `func_idx`.
    ///
    /// For Lua functions: pushes a new CallInfo and returns `CallResult::Lua`.
    /// For Rust functions: executes the function, calls `poscall`, and
    /// returns `CallResult::Rust`.
    ///
    /// `num_results` is the number of results the caller expects, or
    /// `LUA_MULTRET` (-1) for all results.
    pub fn precall(&mut self, func_idx: usize, num_results: i32) -> LuaResult<CallResult> {
        let func_val = self.stack_get(func_idx);
        let closure_ref = match func_val {
            Val::Function(r) => r,
            _ => {
                // Try __call metamethod.
                let tm = get_tm_for_val(&self.gc, func_val, TMS::Call);
                match tm {
                    Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                        // Shift stack up to insert __call at func position.
                        // The original value becomes the first argument.
                        let top = self.top;
                        self.ensure_stack(top + 1);
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
                        return Err(LuaError::Runtime(RuntimeError {
                            message: format!("attempt to call a {} value", func_val.type_name()),
                            level: 0,
                            traceback: vec![],
                        }));
                    }
                }
            }
        };

        // Save caller's PC.
        let saved_pc = self.call_stack[self.ci].saved_pc;
        let _ = saved_pc; // used for restoration in poscall

        // Check call depth limits.
        if self.call_stack.len() >= MAXCALLS {
            return Err(runtime_error_simple("stack overflow"));
        }

        let closure = self
            .gc
            .closures
            .get(closure_ref)
            .ok_or_else(|| runtime_error_simple("invalid function reference"))?;

        match closure {
            Closure::Lua(lua_cl) => {
                let proto = Rc::clone(&lua_cl.proto);
                let _env = lua_cl.env;
                let num_params = proto.num_params as usize;
                let max_stack = proto.max_stack_size as usize;
                let is_vararg = proto.is_vararg & VARARG_ISVARARG != 0;

                // Ensure enough stack space.
                self.ensure_stack(func_idx + max_stack + 1);

                let nargs = self.get_nargs(func_idx);

                let new_base;
                if !is_vararg {
                    new_base = func_idx + 1;
                    // Trim excess args or pad with nil.
                    if nargs > num_params {
                        self.top = new_base + num_params;
                    } else {
                        // Pad with nil for missing params.
                        while self.top < new_base + num_params {
                            self.push(Val::Nil);
                        }
                    }
                } else {
                    new_base = self.adjust_varargs(&proto, nargs, func_idx);
                }

                // Initialize remaining locals to nil.
                let ci_top = new_base + max_stack;
                for i in self.top..ci_top {
                    self.stack_set(i, Val::Nil);
                }
                self.top = ci_top;

                // Push new CallInfo.
                let ci = CallInfo::new(func_idx, new_base, ci_top, num_results);
                self.push_ci(ci);
                self.base = new_base;

                Ok(CallResult::Lua)
            }
            Closure::Rust(rust_cl) => {
                let func = rust_cl.func;

                // Ensure minimum stack for Rust functions.
                self.ensure_stack(self.top + LUA_MINSTACK);

                let ci_top = self.top + LUA_MINSTACK;
                let ci = CallInfo::new(func_idx, func_idx + 1, ci_top, num_results);
                self.push_ci(ci);
                self.base = func_idx + 1;

                self.n_ccalls += 1;
                if self.n_ccalls >= MAXCCALLS {
                    self.n_ccalls -= 1;
                    self.pop_ci();
                    return Err(runtime_error_simple("C stack overflow"));
                }

                // Execute the Rust function.
                let n_results = func(self)?;

                self.n_ccalls -= 1;

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
    pub fn poscall(&mut self, first_result: usize) -> bool {
        // Pop the current CallInfo.
        let ci_func = self.call_stack[self.ci].func;
        let wanted = self.call_stack[self.ci].num_results;
        self.pop_ci();

        // Restore caller's base.
        self.base = self.call_stack[self.ci].base;

        // Move results to where the function was.
        let mut res = ci_func;
        if wanted == LUA_MULTRET {
            // Move all available results.
            let mut src = first_result;
            while src < self.top {
                self.stack_set(res, self.stack_get(src));
                res += 1;
                src += 1;
            }
            self.top = res;
        } else {
            let mut moved = 0i32;
            let mut src = first_result;
            while moved < wanted && src < self.top {
                self.stack_set(res, self.stack_get(src));
                res += 1;
                src += 1;
                moved += 1;
            }
            // Pad with nil if not enough results.
            while moved < wanted {
                self.stack_set(res, Val::Nil);
                res += 1;
                moved += 1;
            }
            self.top = res;
        }

        // Return true if the caller is a Lua function.
        wanted != LUA_MULTRET || self.ci > 0
    }

    /// Adjusts the stack for a vararg function call.
    ///
    /// Moves fixed parameters to new positions above the vararg area.
    /// Returns the new base (first local slot after fixed params).
    fn adjust_varargs(&mut self, proto: &Proto, nargs: usize, func_idx: usize) -> usize {
        let num_params = proto.num_params as usize;

        // Move fixed params to new positions.
        let new_base = self.top + num_params;

        // Ensure enough stack space.
        self.ensure_stack(new_base + 1);

        // Copy fixed params from original positions to above varargs.
        for i in 0..num_params {
            let src_idx = func_idx + 1 + i;
            let val = if i < nargs {
                self.stack_get(src_idx)
            } else {
                Val::Nil
            };
            self.stack_set(self.top + i, val);
        }

        // Set nil in the vararg area (original param positions become varargs).
        self.top = new_base;

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
            if let Some(uv) = self.gc.upvalues.get(uv_ref) {
                if let Some(idx) = uv.stack_index() {
                    if idx == stack_index {
                        return uv_ref;
                    }
                    if idx < stack_index {
                        break; // List is sorted descending; won't find it.
                    }
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
                    .and_then(|uv| uv.stack_index())
                    .map_or(true, |idx| idx < stack_index)
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
                .and_then(|uv| uv.stack_index())
                .map_or(false, |idx| idx >= level);

            if !should_close {
                break;
            }

            // Close the upvalue.
            if let Some(uv) = self.gc.upvalues.get_mut(uv_ref) {
                uv.close(&self.stack);
            }

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

/// Maximum size for short source names (matches PUC-Rio's `LUA_IDSIZE`).
const LUA_IDSIZE: usize = 60;

/// Converts a source name to a short display form.
///
/// - `=name` -> `name` (literal)
/// - `@filename` -> `filename` or `...filename` (truncated)
/// - other -> `[string "..."]`
///
/// Matches PUC-Rio's `luaO_chunkid`.
fn chunkid(source: &str) -> String {
    if let Some(rest) = source.strip_prefix('=') {
        // Literal name -- strip the '=' prefix.
        if rest.len() < LUA_IDSIZE {
            rest.to_string()
        } else {
            rest[..LUA_IDSIZE - 1].to_string()
        }
    } else if let Some(rest) = source.strip_prefix('@') {
        // File name.
        if rest.len() < LUA_IDSIZE {
            rest.to_string()
        } else {
            let skip = rest.len() - (LUA_IDSIZE - 4);
            format!("...{}", &rest[skip..])
        }
    } else {
        // String source.
        let first_line = source.split('\n').next().unwrap_or(source);
        let max_len = LUA_IDSIZE - "[string \"...\"]".len();
        if first_line.len() <= max_len && !source.contains('\n') {
            format!("[string \"{first_line}\"]")
        } else {
            let truncated = &first_line[..first_line.len().min(max_len)];
            format!("[string \"{truncated}...\"]")
        }
    }
}

/// Gets "source:line: " for a given call stack level.
///
/// Level 0 is the currently running function, level 1 is the caller, etc.
/// Returns an empty string if the level doesn't exist or has no source info.
///
/// Matches PUC-Rio's `luaL_where`.
pub(crate) fn get_where(state: &LuaState, level: u32) -> String {
    let level = level as usize;
    if state.ci < level {
        return String::new();
    }

    let target_ci = state.ci - level;
    let ci = &state.call_stack[target_ci];
    let func_val = state.stack_get(ci.func);

    if let Val::Function(r) = func_val {
        if let Some(Closure::Lua(lcl)) = state.gc.closures.get(r) {
            let proto = &lcl.proto;
            let pc = ci.saved_pc;
            let line = if pc > 0 && pc <= proto.line_info.len() {
                proto.line_info[pc - 1]
            } else if !proto.line_info.is_empty() {
                proto.line_info[0]
            } else {
                return String::new();
            };
            let short_src = chunkid(&proto.source);
            return format!("{short_src}:{line}: ");
        }
    }

    String::new()
}

// ---------------------------------------------------------------------------
// Metamethod helpers
// ---------------------------------------------------------------------------

/// Looks up a metamethod for the given value.
///
/// Returns the metamethod value if found, or `None`.
fn get_tm_for_val(gc: &Gc, val: Val, event: TMS) -> Option<Val> {
    gettmbyobj(
        val,
        event,
        &gc.tables,
        &gc.string_arena,
        &gc.type_metatables,
        &gc.tm_names,
        &gc.userdata,
    )
}

/// Call a metamethod with 2 arguments, storing 1 result.
///
/// Used by arithmetic and comparison metamethods.
/// Stack layout: push tm, push arg1, push arg2, call, read result.
fn call_tm_res(
    state: &mut LuaState,
    tm: Val,
    arg1: Val,
    arg2: Val,
    result_reg: usize,
) -> LuaResult<()> {
    let call_base = state.top;
    state.ensure_stack(call_base + 4);
    state.stack_set(call_base, tm);
    state.stack_set(call_base + 1, arg1);
    state.stack_set(call_base + 2, arg2);
    state.top = call_base + 3;

    match state.precall(call_base, 1)? {
        CallResult::Lua => execute(state)?,
        CallResult::Rust => {}
    }

    // Read the result (poscall placed it at call_base).
    let result = state.stack_get(call_base);
    state.stack_set(result_reg, result);
    Ok(())
}

/// Call a metamethod with 3 arguments, no result stored.
///
/// Used by `__newindex` metamethod.
fn call_tm_void(state: &mut LuaState, tm: Val, arg1: Val, arg2: Val, arg3: Val) -> LuaResult<()> {
    let call_base = state.top;
    state.ensure_stack(call_base + 5);
    state.stack_set(call_base, tm);
    state.stack_set(call_base + 1, arg1);
    state.stack_set(call_base + 2, arg2);
    state.stack_set(call_base + 3, arg3);
    state.top = call_base + 4;

    match state.precall(call_base, 0)? {
        CallResult::Lua => execute(state)?,
        CallResult::Rust => {}
    }

    Ok(())
}

/// Try a binary metamethod on left operand, then right.
///
/// Looks up `event` in lhs's metatable. If not found, tries rhs's.
/// Calls the found metamethod with (lhs, rhs) and stores the result.
/// Returns an error if neither side has the metamethod.
fn call_bin_tm(
    state: &mut LuaState,
    lhs: Val,
    rhs: Val,
    result_reg: usize,
    event: TMS,
    proto: &Proto,
    pc: usize,
) -> LuaResult<()> {
    let tm =
        get_tm_for_val(&state.gc, lhs, event).or_else(|| get_tm_for_val(&state.gc, rhs, event));

    match tm {
        Some(tm_val) => call_tm_res(state, tm_val, lhs, rhs, result_reg),
        None => Err(arith_error(proto, pc, lhs)),
    }
}

/// Try an order (comparison) metamethod.
///
/// Matches PUC-Rio's `call_orderTM`: looks up the event on the left
/// operand first, then checks that the right operand has the SAME
/// metamethod (raw equality). Returns `None` if no matching metamethod
/// exists, `Some(bool)` with the truthiness of the call result.
fn call_order_tm(state: &mut LuaState, lhs: Val, rhs: Val, event: TMS) -> LuaResult<Option<bool>> {
    let tm1 = get_tm_for_val(&state.gc, lhs, event);
    let Some(tm1_val) = tm1 else {
        return Ok(None);
    };

    let tm2 = get_tm_for_val(&state.gc, rhs, event);
    let tm2_val = tm2.unwrap_or(Val::Nil);

    // Both operands must have the same metamethod (raw equality).
    if !val_raw_equal(tm1_val, tm2_val, &state.gc.tables, &state.gc.string_arena) {
        return Ok(None);
    }

    let call_base = state.top;
    state.ensure_stack(call_base + 4);
    state.stack_set(call_base, tm1_val);
    state.stack_set(call_base + 1, lhs);
    state.stack_set(call_base + 2, rhs);
    state.top = call_base + 3;

    match state.precall(call_base, 1)? {
        CallResult::Lua => execute(state)?,
        CallResult::Rust => {}
    }

    let result = state.stack_get(call_base);
    Ok(Some(result.is_truthy()))
}

// ---------------------------------------------------------------------------
// String concatenation with metamethod support
// ---------------------------------------------------------------------------

/// Concatenate registers `base+b` through `base+c`, storing the result
/// in `base+b`. Matches PUC-Rio's `luaV_concat`.
///
/// Processes pairs right-to-left. For each pair, tries to coerce both to
/// strings. If coercion fails, tries the `__concat` metamethod. Coalesces
/// consecutive string/number values into a single buffer for efficiency.
fn vm_concat(
    state: &mut LuaState,
    base: usize,
    b: usize,
    c: usize,
    proto: &Proto,
    pc: usize,
) -> LuaResult<()> {
    let mut total = c - b + 1; // number of values remaining
    let mut last = c; // rightmost index (relative to base)

    while total > 1 {
        let top = base + last + 1;
        let lhs = state.stack_get(top - 2);
        let rhs = state.stack_get(top - 1);

        if !is_string_or_number(lhs, &state.gc) || !is_string_or_number(rhs, &state.gc) {
            // Try __concat metamethod on the pair.
            let tm = get_tm_for_val(&state.gc, lhs, TMS::Concat)
                .or_else(|| get_tm_for_val(&state.gc, rhs, TMS::Concat));
            if let Some(tm_val) = tm {
                call_tm_res(state, tm_val, lhs, rhs, top - 2)?;
            } else {
                // No metamethod -- find which operand is the problem.
                if !is_string_or_number(lhs, &state.gc) {
                    return Err(concat_error(proto, pc, lhs));
                }
                return Err(concat_error(proto, pc, rhs));
            }
            total -= 1;
            last -= 1;
        } else {
            // Both are string/number. Coalesce as many as possible.
            let mut n = 2;
            while n < total && is_string_or_number(state.stack_get(top - n - 1), &state.gc) {
                n += 1;
            }
            // Collect all n values into a single buffer.
            let mut buffer = Vec::new();
            for i in (0..n).rev() {
                let val = state.stack_get(top - 1 - i);
                val_to_string_bytes(val, &state.gc, &mut buffer);
            }
            let r = state.gc.intern_string(&buffer);
            state.stack_set(top - n, Val::Str(r));
            total -= n - 1;
            last -= n - 1;
        }
    }
    Ok(())
}

/// Check if a value is a string or number (coercible for concatenation).
fn is_string_or_number(val: Val, gc: &Gc) -> bool {
    matches!(val, Val::Num(_)) || {
        if let Val::Str(r) = val {
            gc.string_arena.get(r).is_some()
        } else {
            false
        }
    }
}

/// Append the string representation of a value to a buffer.
fn val_to_string_bytes(val: Val, gc: &Gc, buffer: &mut Vec<u8>) {
    match val {
        Val::Str(r) => {
            if let Some(s) = gc.string_arena.get(r) {
                buffer.extend_from_slice(s.data());
            }
        }
        Val::Num(_) => {
            let formatted = format!("{val}");
            buffer.extend_from_slice(formatted.as_bytes());
        }
        _ => {}
    }
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
    loop {
        // Cache values from current frame.
        let ci_func = state.call_stack[state.ci].func;
        let base = state.base;

        // Get the current closure and proto.
        let func_val = state.stack_get(ci_func);
        let closure_ref = match func_val {
            Val::Function(r) => r,
            _ => return Err(runtime_error_simple("not a function")),
        };

        let (proto, env) = {
            let cl = state
                .gc
                .closures
                .get(closure_ref)
                .ok_or_else(|| runtime_error_simple("invalid closure"))?;
            match cl {
                Closure::Lua(lcl) => (Rc::clone(&lcl.proto), lcl.env),
                Closure::Rust(_) => {
                    return Err(runtime_error_simple("expected Lua closure in execute"));
                }
            }
        };

        let mut pc = state.call_stack[state.ci].saved_pc;

        // Inner dispatch loop for this frame.
        loop {
            if pc >= proto.code.len() {
                return Err(runtime_error(&proto, pc, "bytecode overrun".to_string()));
            }

            let instr = Instruction::from_raw(proto.code[pc]);
            pc += 1;

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
                    state.call_stack[state.ci].saved_pc = pc;
                    vm_gettable(state, Val::Table(env), key, ra, &proto, pc)?;
                }

                OpCode::SetGlobal => {
                    let bx = instr.bx() as usize;
                    let key = proto.constants[bx];
                    let val = state.stack_get(ra);
                    state.call_stack[state.ci].saved_pc = pc;
                    vm_settable(state, Val::Table(env), key, val, &proto, pc)?;
                }

                // ----- Table access -----
                OpCode::GetTable => {
                    let b = instr.b() as usize;
                    let table_val = state.stack_get(base + b);
                    let key = rk(&state.stack, base, &proto.constants, instr.c());
                    state.call_stack[state.ci].saved_pc = pc;
                    vm_gettable(state, table_val, key, ra, &proto, pc)?;
                }

                OpCode::SetTable => {
                    let table_val = state.stack_get(ra);
                    let key = rk(&state.stack, base, &proto.constants, instr.b());
                    let val = rk(&state.stack, base, &proto.constants, instr.c());
                    state.call_stack[state.ci].saved_pc = pc;
                    vm_settable(state, table_val, key, val, &proto, pc)?;
                }

                OpCode::NewTable => {
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
                    state.call_stack[state.ci].saved_pc = pc;
                    vm_gettable(state, table_val, key, ra, &proto, pc)?;
                }

                // ----- Arithmetic -----
                OpCode::Add => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    match (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        (Some(nb), Some(nc)) => state.stack_set(ra, Val::Num(nb + nc)),
                        _ => {
                            state.call_stack[state.ci].saved_pc = pc;
                            call_bin_tm(state, b_val, c_val, ra, TMS::Add, &proto, pc)?;
                        }
                    }
                }

                OpCode::Sub => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    match (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        (Some(nb), Some(nc)) => state.stack_set(ra, Val::Num(nb - nc)),
                        _ => {
                            state.call_stack[state.ci].saved_pc = pc;
                            call_bin_tm(state, b_val, c_val, ra, TMS::Sub, &proto, pc)?;
                        }
                    }
                }

                OpCode::Mul => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    match (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        (Some(nb), Some(nc)) => state.stack_set(ra, Val::Num(nb * nc)),
                        _ => {
                            state.call_stack[state.ci].saved_pc = pc;
                            call_bin_tm(state, b_val, c_val, ra, TMS::Mul, &proto, pc)?;
                        }
                    }
                }

                OpCode::Div => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    match (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        (Some(nb), Some(nc)) => state.stack_set(ra, Val::Num(nb / nc)),
                        _ => {
                            state.call_stack[state.ci].saved_pc = pc;
                            call_bin_tm(state, b_val, c_val, ra, TMS::Div, &proto, pc)?;
                        }
                    }
                }

                OpCode::Mod => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    match (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        (Some(nb), Some(nc)) => {
                            // Lua mod: a - floor(a/b)*b
                            state.stack_set(ra, Val::Num(nb - (nb / nc).floor() * nc));
                        }
                        _ => {
                            state.call_stack[state.ci].saved_pc = pc;
                            call_bin_tm(state, b_val, c_val, ra, TMS::Mod, &proto, pc)?;
                        }
                    }
                }

                OpCode::Pow => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    match (
                        coerce_to_number(b_val, &state.gc),
                        coerce_to_number(c_val, &state.gc),
                    ) {
                        (Some(nb), Some(nc)) => state.stack_set(ra, Val::Num(nb.powf(nc))),
                        _ => {
                            state.call_stack[state.ci].saved_pc = pc;
                            call_bin_tm(state, b_val, c_val, ra, TMS::Pow, &proto, pc)?;
                        }
                    }
                }

                OpCode::Unm => {
                    let b = instr.b() as usize;
                    let b_val = state.stack_get(base + b);
                    match coerce_to_number(b_val, &state.gc) {
                        Some(nb) => state.stack_set(ra, Val::Num(-nb)),
                        None => {
                            // __unm: try on the single operand only
                            let tm = get_tm_for_val(&state.gc, b_val, TMS::Unm);
                            match tm {
                                Some(tm_val) => {
                                    state.call_stack[state.ci].saved_pc = pc;
                                    call_tm_res(state, tm_val, b_val, b_val, ra)?;
                                }
                                None => return Err(arith_error(&proto, pc, b_val)),
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
                            let s = state
                                .gc
                                .string_arena
                                .get(r)
                                .ok_or_else(|| len_error(&proto, pc, b_val))?;
                            #[allow(clippy::cast_precision_loss)]
                            state.stack_set(ra, Val::Num(s.len() as f64));
                        }
                        Val::Table(r) => {
                            // Lua 5.1.1: tables always use raw length (no __len).
                            let t = state
                                .gc
                                .tables
                                .get(r)
                                .ok_or_else(|| len_error(&proto, pc, b_val))?;
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
                                return Err(len_error(&proto, pc, b_val));
                            }
                        }
                    }
                }

                OpCode::Concat => {
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
                            call_tm_res(state, tm_val, b_val, c_val, state.top)?;
                            state.stack_get(state.top).is_truthy()
                        } else {
                            false
                        }
                    };
                    // PUC-Rio: if (result == A) then dojump; else skip JMP
                    let expected = a != 0;
                    if equal == expected {
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + (jump_instr.sbx() as i64)) as usize;
                    }
                    pc += 1; // skip the JMP instruction
                }

                OpCode::Lt => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    state.call_stack[state.ci].saved_pc = pc;
                    let result = val_less_than(b_val, c_val, state, &proto, pc)?;
                    // PUC-Rio: if (result == A) then dojump; else skip JMP
                    let expected = a != 0;
                    if result == expected {
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + (jump_instr.sbx() as i64)) as usize;
                    }
                    pc += 1;
                }

                OpCode::Le => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    state.call_stack[state.ci].saved_pc = pc;
                    let result = val_less_equal(b_val, c_val, state, &proto, pc)?;
                    // PUC-Rio: if (result == A) then dojump; else skip JMP
                    let expected = a != 0;
                    if result == expected {
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + (jump_instr.sbx() as i64)) as usize;
                    }
                    pc += 1;
                }

                // ----- Logic / test -----
                OpCode::Test => {
                    let val = state.stack_get(ra);
                    let c = instr.c() != 0;
                    // if (val is falsy) != C then jump
                    if !val.is_truthy() != c {
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + (jump_instr.sbx() as i64)) as usize;
                    }
                    pc += 1;
                }

                OpCode::TestSet => {
                    let b = instr.b() as usize;
                    let rb = state.stack_get(base + b);
                    let c = instr.c() != 0;
                    if !rb.is_truthy() != c {
                        state.stack_set(ra, rb);
                        let jump_instr = Instruction::from_raw(proto.code[pc]);
                        pc = ((pc as i64) + (jump_instr.sbx() as i64)) as usize;
                    }
                    pc += 1;
                }

                // ----- Control flow -----
                OpCode::Jmp => {
                    let sbx = instr.sbx();
                    pc = ((pc as i64) + (sbx as i64)) as usize;
                }

                OpCode::ForPrep => {
                    let init = state.stack_get(ra);
                    let limit = state.stack_get(ra + 1);
                    let step = state.stack_get(ra + 2);

                    let n_init = coerce_to_number(init, &state.gc).ok_or_else(|| {
                        runtime_error(
                            &proto,
                            pc,
                            "'for' initial value must be a number".to_string(),
                        )
                    })?;
                    let n_limit = coerce_to_number(limit, &state.gc).ok_or_else(|| {
                        runtime_error(&proto, pc, "'for' limit must be a number".to_string())
                    })?;
                    let n_step = coerce_to_number(step, &state.gc).ok_or_else(|| {
                        runtime_error(&proto, pc, "'for' step must be a number".to_string())
                    })?;

                    // R(A) -= step (so the first FORLOOP adds it back).
                    state.stack_set(ra, Val::Num(n_init - n_step));
                    state.stack_set(ra + 1, Val::Num(n_limit));
                    state.stack_set(ra + 2, Val::Num(n_step));

                    let sbx = instr.sbx();
                    pc = ((pc as i64) + (sbx as i64)) as usize;
                }

                OpCode::ForLoop => {
                    let step = match state.stack_get(ra + 2) {
                        Val::Num(n) => n,
                        _ => {
                            return Err(runtime_error(
                                &proto,
                                pc,
                                "'for' step is not a number".to_string(),
                            ));
                        }
                    };
                    let idx = match state.stack_get(ra) {
                        Val::Num(n) => n + step,
                        _ => {
                            return Err(runtime_error(
                                &proto,
                                pc,
                                "'for' index is not a number".to_string(),
                            ));
                        }
                    };
                    let limit = match state.stack_get(ra + 1) {
                        Val::Num(n) => n,
                        _ => {
                            return Err(runtime_error(
                                &proto,
                                pc,
                                "'for' limit is not a number".to_string(),
                            ));
                        }
                    };

                    let continue_loop = if step > 0.0 {
                        idx <= limit
                    } else {
                        limit <= idx
                    };

                    if continue_loop {
                        let sbx = instr.sbx();
                        pc = ((pc as i64) + (sbx as i64)) as usize;
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
                    state.precall(cb, c)?;

                    // If precall returned Lua, we need to execute it.
                    if matches!(state.stack_get(cb), Val::Function(r) if {
                        state.gc.closures.get(r).map_or(false, |cl| cl.is_lua())
                    }) {
                        // This shouldn't happen in the normal TFORLOOP flow
                        // because iterators are typically Rust functions.
                        // But handle it: execute the Lua iterator.
                        execute(state)?;
                    }

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
                        pc = ((pc as i64) + (jump_instr.sbx() as i64)) as usize;
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
                            // Re-enter execute for the callee.
                            execute(state)?;
                        }
                        CallResult::Rust => {
                            // Rust function already completed.
                        }
                    }
                    // For fixed results (C != 0), restore top to the frame's
                    // max stack size. For MULTRET (C == 0), leave top as set
                    // by poscall so the caller knows how many results exist.
                    // Matches PUC-Rio: `if (nresults >= 0) L->top = L->ci->top;`
                    if c != 0 {
                        state.top = state.call_stack[state.ci].top;
                    }
                    // Break to outer loop to re-read base and proto.
                    break;
                }

                OpCode::TailCall => {
                    let b = instr.b();

                    if b != 0 {
                        state.top = ra + b as usize;
                    }

                    // Close upvalues at current base.
                    state.close_upvalues(base);

                    // Save pc.
                    state.call_stack[state.ci].saved_pc = pc;

                    // Move function + args down to current frame's func position.
                    let ci_func = state.call_stack[state.ci].func;
                    let nargs = state.top - ra;
                    for i in 0..nargs {
                        state.stack_set(ci_func + i, state.stack_get(ra + i));
                    }
                    state.top = ci_func + nargs;

                    // Pop current frame and set up the tail call.
                    let num_results = state.call_stack[state.ci].num_results;
                    state.pop_ci();
                    state.base = state.call_stack[state.ci].base;

                    match state.precall(ci_func, num_results)? {
                        CallResult::Lua => {
                            // Continue in the outer loop -- we've set up a new
                            // Lua frame at the same call depth.
                        }
                        CallResult::Rust => {
                            // Rust tail-call completed. poscall already
                            // unwound the frame and placed results. Done.
                            return Ok(());
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

                    state.poscall(first_result);

                    // In the recursive call model, each execute() instance
                    // handles exactly one logical function (including tail-call
                    // replacements). Nested Call opcodes invoke execute()
                    // recursively, so the only Return this instance ever sees
                    // is from the function it was called to run. Always exit.
                    return Ok(());
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
                    if let Closure::Lua(lcl) = cl {
                        if b < lcl.upvalues.len() {
                            let uv_ref = lcl.upvalues[b];
                            if let Some(uv) = state.gc.upvalues.get_mut(uv_ref) {
                                uv.set(&mut state.stack, val);
                            }
                        }
                    }
                }

                // ----- Closure creation -----
                OpCode::Closure => {
                    let bx = instr.bx() as usize;
                    let child_proto = Rc::clone(&proto.protos[bx]);
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
                                if let Closure::Lua(lcl) = cl {
                                    if parent_uv_idx < lcl.upvalues.len() {
                                        new_cl.upvalues.push(lcl.upvalues[parent_uv_idx]);
                                    }
                                }
                            }
                            _ => {
                                return Err(runtime_error(
                                    &proto,
                                    pc,
                                    "invalid pseudo-instruction after CLOSURE".to_string(),
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
                    let num_varargs = if base > vararg_start {
                        base - vararg_start
                    } else {
                        0
                    };

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

                    let table_val = state.stack_get(ra);
                    let table_ref = match table_val {
                        Val::Table(r) => r,
                        _ => return Err(index_error(&proto, pc, table_val)),
                    };

                    let offset = (c - 1) * LFIELDS_PER_FLUSH as usize;
                    for i in 1..=n {
                        let val = state.stack_get(ra + i);
                        let key = Val::Num((offset + i) as f64);
                        table_set(state, table_ref, key, val)?;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Table access helpers (raw, no metamethods)
// ---------------------------------------------------------------------------

/// Raw table get.
#[allow(dead_code)] // Used by rawget stdlib function (Phase 4c)
fn table_get(state: &LuaState, table_ref: GcRef<Table>, key: Val) -> LuaResult<Val> {
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    Ok(table.get(key, &state.gc.string_arena))
}

/// Raw table set.
fn table_set(state: &mut LuaState, table_ref: GcRef<Table>, key: Val, value: Val) -> LuaResult<()> {
    let table = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    table.raw_set(key, value, &state.gc.string_arena)
}

// ---------------------------------------------------------------------------
// Table access with metamethods
// ---------------------------------------------------------------------------

/// Lua table get with `__index` metamethod support.
///
/// Matches PUC-Rio's `luaV_gettable`:
/// 1. If `t` is a table, rawget `key`. If not nil, return it.
/// 2. Look up `__index` in `t`'s metatable.
/// 3. If `__index` is a function, call it with (t, key) and return result.
/// 4. If `__index` is a table, repeat the lookup on that table.
/// 5. Loop up to `MAXTAGLOOP` times to prevent infinite chains.
fn vm_gettable(
    state: &mut LuaState,
    t: Val,
    key: Val,
    result_reg: usize,
    proto: &Proto,
    pc: usize,
) -> LuaResult<()> {
    let mut current = t;
    for _ in 0..MAXTAGLOOP {
        if let Val::Table(table_ref) = current {
            // Try raw table get.
            let table = state
                .gc
                .tables
                .get(table_ref)
                .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
            let result = table.get(key, &state.gc.string_arena);

            if !result.is_nil() {
                // Key found -- return it.
                state.stack_set(result_reg, result);
                return Ok(());
            }

            // Key not found. Check for __index metamethod.
            let tm = {
                let mt = table.metatable();
                match mt {
                    Some(mt_ref) => {
                        let tm_val = get_tm_for_table(&state.gc, mt_ref, TMS::Index);
                        tm_val
                    }
                    None => None,
                }
            };

            match tm {
                None => {
                    // No metamethod -- return nil.
                    state.stack_set(result_reg, Val::Nil);
                    return Ok(());
                }
                Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                    // __index is a function: call it with (table, key).
                    call_tm_res(state, tm_val, current, key, result_reg)?;
                    return Ok(());
                }
                Some(tm_val) => {
                    // __index is a table (or other value): loop.
                    current = tm_val;
                }
            }
        } else {
            // Non-table value: check type metatable for __index.
            let tm = get_tm_for_val(&state.gc, current, TMS::Index);
            match tm {
                None => {
                    return Err(index_error(proto, pc, current));
                }
                Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                    call_tm_res(state, tm_val, current, key, result_reg)?;
                    return Ok(());
                }
                Some(tm_val) => {
                    current = tm_val;
                }
            }
        }
    }
    Err(runtime_error_simple("loop in gettable"))
}

/// Lua table set with `__newindex` metamethod support.
///
/// Matches PUC-Rio's `luaV_settable`:
/// 1. If `t` is a table, rawget `key`. If exists (not nil), rawset and return.
/// 2. Look up `__newindex` in `t`'s metatable.
/// 3. If `__newindex` is a function, call it with (t, key, value). Return.
/// 4. If `__newindex` is a table, repeat the set on that table.
/// 5. If no `__newindex`, rawset on original table.
fn vm_settable(
    state: &mut LuaState,
    t: Val,
    key: Val,
    value: Val,
    proto: &Proto,
    pc: usize,
) -> LuaResult<()> {
    let mut current = t;
    for _ in 0..MAXTAGLOOP {
        if let Val::Table(table_ref) = current {
            // Check if key already exists (rawget).
            let existing = {
                let table = state
                    .gc
                    .tables
                    .get(table_ref)
                    .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
                table.get(key, &state.gc.string_arena)
            };

            if !existing.is_nil() {
                // Key exists -- rawset directly.
                let table = state
                    .gc
                    .tables
                    .get_mut(table_ref)
                    .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
                return table.raw_set(key, value, &state.gc.string_arena);
            }

            // Key not found. Check for __newindex metamethod.
            let tm = {
                let table = state
                    .gc
                    .tables
                    .get(table_ref)
                    .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
                let mt = table.metatable();
                match mt {
                    Some(mt_ref) => get_tm_for_table(&state.gc, mt_ref, TMS::NewIndex),
                    None => None,
                }
            };

            match tm {
                None => {
                    // No metamethod -- rawset on this table.
                    let table = state
                        .gc
                        .tables
                        .get_mut(table_ref)
                        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
                    return table.raw_set(key, value, &state.gc.string_arena);
                }
                Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                    // __newindex is a function: call with (table, key, value).
                    call_tm_void(state, tm_val, current, key, value)?;
                    return Ok(());
                }
                Some(tm_val) => {
                    // __newindex is a table: loop.
                    current = tm_val;
                }
            }
        } else {
            // Non-table value: check type metatable for __newindex.
            let tm = get_tm_for_val(&state.gc, current, TMS::NewIndex);
            match tm {
                None => {
                    return Err(index_error(proto, pc, current));
                }
                Some(tm_val) if matches!(tm_val, Val::Function(_)) => {
                    call_tm_void(state, tm_val, current, key, value)?;
                    return Ok(());
                }
                Some(tm_val) => {
                    current = tm_val;
                }
            }
        }
    }
    Err(runtime_error_simple("loop in settable"))
}

/// Look up a metamethod in a specific metatable (fast path for tables).
fn get_tm_for_table(gc: &Gc, mt_ref: GcRef<Table>, event: TMS) -> Option<Val> {
    use super::metatable::fasttm;
    fasttm(&gc.tables, &gc.string_arena, mt_ref, event, &gc.tm_names)
}

// ---------------------------------------------------------------------------
// Comparison helpers
// ---------------------------------------------------------------------------

/// Lua raw equality comparison (no metamethods).
///
/// Delegates to `val_raw_equal` in metatable.rs.
fn val_equal(a: Val, b: Val, gc: &Gc) -> bool {
    val_raw_equal(a, b, &gc.tables, &gc.string_arena)
}

/// Lua less-than comparison with metamethod support.
///
/// Matches PUC-Rio's `luaV_lessthan`: numbers and strings use raw
/// comparison; same-type non-comparable values try `__lt` metamethod
/// (both operands must share the same TM); different types error.
fn val_less_than(
    a: Val,
    b: Val,
    state: &mut LuaState,
    proto: &Proto,
    pc: usize,
) -> LuaResult<bool> {
    match (&a, &b) {
        (Val::Num(x), Val::Num(y)) => Ok(x < y),
        (Val::Str(x), Val::Str(y)) => {
            let sx = state
                .gc
                .string_arena
                .get(*x)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            let sy = state
                .gc
                .string_arena
                .get(*y)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            Ok(sx.data() < sy.data())
        }
        _ => {
            if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
                return Err(compare_error(proto, pc, a, b));
            }
            match call_order_tm(state, a, b, TMS::Lt)? {
                Some(result) => Ok(result),
                None => Err(compare_error(proto, pc, a, b)),
            }
        }
    }
}

/// Lua less-or-equal comparison with metamethod support.
///
/// Matches PUC-Rio's `lessequal`: numbers and strings use raw
/// comparison; same-type non-comparable values try `__le` first,
/// then fall back to `NOT __lt(rhs, lhs)` with reversed arguments.
fn val_less_equal(
    a: Val,
    b: Val,
    state: &mut LuaState,
    proto: &Proto,
    pc: usize,
) -> LuaResult<bool> {
    match (&a, &b) {
        (Val::Num(x), Val::Num(y)) => Ok(x <= y),
        (Val::Str(x), Val::Str(y)) => {
            let sx = state
                .gc
                .string_arena
                .get(*x)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            let sy = state
                .gc
                .string_arena
                .get(*y)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            Ok(sx.data() <= sy.data())
        }
        _ => {
            if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
                return Err(compare_error(proto, pc, a, b));
            }
            // Try __le first.
            if let Some(result) = call_order_tm(state, a, b, TMS::Le)? {
                return Ok(result);
            }
            // Fallback: try NOT __lt(rhs, lhs) (reversed arguments).
            if let Some(result) = call_order_tm(state, b, a, TMS::Lt)? {
                return Ok(!result);
            }
            Err(compare_error(proto, pc, a, b))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::instructions::{Instruction, OpCode, rk_as_k};

    /// Helper: create a LuaState with a proto loaded as the current function.
    fn setup_state(proto: Proto) -> LuaState {
        let mut state = LuaState::new();
        let proto_rc = Rc::new(proto);
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

    // ----- Table tests -----

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

        let proto_rc = Rc::new(make_proto(code, constants));
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

        let proto_rc = Rc::new(make_proto(code, constants));
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

        let proto_rc = Rc::new(make_proto(code, constants));
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
