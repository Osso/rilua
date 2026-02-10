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
use super::instructions::{Instruction, LFIELDS_PER_FLUSH, OpCode, index_k, is_k};
use super::proto::{Proto, VARARG_ISVARARG};
use super::state::{Gc, LUA_MINSTACK, LuaState, MAXCALLS, MAXCCALLS};
use super::table::Table;
use super::value::Val;

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
fn coerce_to_number(val: Val, gc: &Gc) -> Option<f64> {
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
fn str_to_number(data: &[u8]) -> Option<f64> {
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
    let source = proto.source.clone();
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
                // No metamethods in Phase 3.
                return Err(LuaError::Runtime(RuntimeError {
                    message: format!("attempt to call a {} value", func_val.type_name()),
                    level: 0,
                    traceback: vec![],
                }));
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
                    let val = table_get(state, env, key)?;
                    state.stack_set(ra, val);
                }

                OpCode::SetGlobal => {
                    let bx = instr.bx() as usize;
                    let key = proto.constants[bx];
                    let val = state.stack_get(ra);
                    table_set(state, env, key, val)?;
                }

                // ----- Table access -----
                OpCode::GetTable => {
                    let b = instr.b() as usize;
                    let table_val = state.stack_get(base + b);
                    let key = rk(&state.stack, base, &proto.constants, instr.c());

                    let table_ref = match table_val {
                        Val::Table(r) => r,
                        _ => return Err(index_error(&proto, pc, table_val)),
                    };
                    let val = table_get(state, table_ref, key)?;
                    state.stack_set(ra, val);
                }

                OpCode::SetTable => {
                    let table_val = state.stack_get(ra);
                    let key = rk(&state.stack, base, &proto.constants, instr.b());
                    let val = rk(&state.stack, base, &proto.constants, instr.c());

                    let table_ref = match table_val {
                        Val::Table(r) => r,
                        _ => return Err(index_error(&proto, pc, table_val)),
                    };
                    table_set(state, table_ref, key, val)?;
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

                    let table_ref = match table_val {
                        Val::Table(r) => r,
                        _ => return Err(index_error(&proto, pc, table_val)),
                    };
                    let val = table_get(state, table_ref, key)?;
                    state.stack_set(ra, val);
                }

                // ----- Arithmetic -----
                OpCode::Add => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    let nb = coerce_to_number(b_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, b_val))?;
                    let nc = coerce_to_number(c_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, c_val))?;
                    state.stack_set(ra, Val::Num(nb + nc));
                }

                OpCode::Sub => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    let nb = coerce_to_number(b_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, b_val))?;
                    let nc = coerce_to_number(c_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, c_val))?;
                    state.stack_set(ra, Val::Num(nb - nc));
                }

                OpCode::Mul => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    let nb = coerce_to_number(b_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, b_val))?;
                    let nc = coerce_to_number(c_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, c_val))?;
                    state.stack_set(ra, Val::Num(nb * nc));
                }

                OpCode::Div => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    let nb = coerce_to_number(b_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, b_val))?;
                    let nc = coerce_to_number(c_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, c_val))?;
                    state.stack_set(ra, Val::Num(nb / nc));
                }

                OpCode::Mod => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    let nb = coerce_to_number(b_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, b_val))?;
                    let nc = coerce_to_number(c_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, c_val))?;
                    // Lua mod: a - floor(a/b)*b
                    state.stack_set(ra, Val::Num(nb - (nb / nc).floor() * nc));
                }

                OpCode::Pow => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    let nb = coerce_to_number(b_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, b_val))?;
                    let nc = coerce_to_number(c_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, c_val))?;
                    state.stack_set(ra, Val::Num(nb.powf(nc)));
                }

                OpCode::Unm => {
                    let b = instr.b() as usize;
                    let b_val = state.stack_get(base + b);
                    let nb = coerce_to_number(b_val, &state.gc)
                        .ok_or_else(|| arith_error(&proto, pc, b_val))?;
                    state.stack_set(ra, Val::Num(-nb));
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
                            let t = state
                                .gc
                                .tables
                                .get(r)
                                .ok_or_else(|| len_error(&proto, pc, b_val))?;
                            #[allow(clippy::cast_precision_loss)]
                            let len = t.len(&state.gc.string_arena) as f64;
                            state.stack_set(ra, Val::Num(len));
                        }
                        _ => return Err(len_error(&proto, pc, b_val)),
                    }
                }

                OpCode::Concat => {
                    let b = instr.b() as usize;
                    let c = instr.c() as usize;
                    // Concatenate registers B through C.
                    // First, coerce all to strings.
                    let mut parts: Vec<Vec<u8>> = Vec::with_capacity(c - b + 1);
                    for i in b..=c {
                        let val = state.stack_get(base + i);
                        match val {
                            Val::Str(r) => {
                                let s = state
                                    .gc
                                    .string_arena
                                    .get(r)
                                    .ok_or_else(|| concat_error(&proto, pc, val))?;
                                parts.push(s.data().to_vec());
                            }
                            Val::Num(_) => {
                                let formatted = format!("{val}");
                                parts.push(formatted.into_bytes());
                            }
                            _ => return Err(concat_error(&proto, pc, val)),
                        }
                    }
                    let total_len: usize = parts.iter().map(Vec::len).sum();
                    let mut buffer = Vec::with_capacity(total_len);
                    for part in &parts {
                        buffer.extend_from_slice(part);
                    }
                    let r = state.gc.intern_string(&buffer);
                    state.stack_set(base + b, Val::Str(r));
                    // The result goes in R(B), then we copy to R(A).
                    if ra != base + b {
                        let val = state.stack_get(base + b);
                        state.stack_set(ra, val);
                    }
                }

                // ----- Comparison -----
                OpCode::Eq => {
                    let b_val = rk(&state.stack, base, &proto.constants, instr.b());
                    let c_val = rk(&state.stack, base, &proto.constants, instr.c());
                    let equal = val_equal(b_val, c_val, &state.gc);
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
                    let result = val_less_than(b_val, c_val, &state.gc, &proto, pc)?;
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
                    let result = val_less_equal(b_val, c_val, &state.gc, &proto, pc)?;
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
                            // After return, restore our frame state.
                        }
                        CallResult::Rust => {
                            // Rust function already completed.
                            // Adjust top if needed for MULTRET.
                            if c == 0 {
                                // top was set by poscall
                            }
                        }
                    }
                    // After call returns, base may have changed if this
                    // was a different frame. Break to reenter outer loop.
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
fn table_get(
    state: &LuaState,
    table_ref: super::gc::arena::GcRef<Table>,
    key: Val,
) -> LuaResult<Val> {
    let table = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    Ok(table.get(key, &state.gc.string_arena))
}

/// Raw table set.
fn table_set(
    state: &mut LuaState,
    table_ref: super::gc::arena::GcRef<Table>,
    key: Val,
    value: Val,
) -> LuaResult<()> {
    let table = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| runtime_error_simple("invalid table reference"))?;
    table.raw_set(key, value, &state.gc.string_arena)
}

// ---------------------------------------------------------------------------
// Comparison helpers
// ---------------------------------------------------------------------------

/// Lua equality comparison (no metamethods).
fn val_equal(a: Val, b: Val, gc: &Gc) -> bool {
    match (&a, &b) {
        (Val::Nil, Val::Nil) => true,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Num(x), Val::Num(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => {
            if x == y {
                return true; // Same GcRef (identity)
            }
            // Interned strings: same content means same ref.
            // But compare content as fallback.
            let sx = gc.string_arena.get(*x);
            let sy = gc.string_arena.get(*y);
            match (sx, sy) {
                (Some(a), Some(b)) => a.data() == b.data(),
                _ => false,
            }
        }
        // Reference types: identity comparison.
        (Val::Table(x), Val::Table(y)) => x == y,
        (Val::Function(x), Val::Function(y)) => x == y,
        (Val::Userdata(x), Val::Userdata(y)) => x == y,
        (Val::Thread(x), Val::Thread(y)) => x == y,
        (Val::LightUserdata(x), Val::LightUserdata(y)) => x == y,
        // Different types are never equal.
        _ => false,
    }
}

/// Lua less-than comparison (no metamethods).
fn val_less_than(a: Val, b: Val, gc: &Gc, proto: &Proto, pc: usize) -> LuaResult<bool> {
    match (&a, &b) {
        (Val::Num(x), Val::Num(y)) => Ok(x < y),
        (Val::Str(x), Val::Str(y)) => {
            let sx = gc
                .string_arena
                .get(*x)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            let sy = gc
                .string_arena
                .get(*y)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            Ok(sx.data() < sy.data())
        }
        _ => Err(compare_error(proto, pc, a, b)),
    }
}

/// Lua less-or-equal comparison (no metamethods).
fn val_less_equal(a: Val, b: Val, gc: &Gc, proto: &Proto, pc: usize) -> LuaResult<bool> {
    match (&a, &b) {
        (Val::Num(x), Val::Num(y)) => Ok(x <= y),
        (Val::Str(x), Val::Str(y)) => {
            let sx = gc
                .string_arena
                .get(*x)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            let sy = gc
                .string_arena
                .get(*y)
                .ok_or_else(|| compare_error(proto, pc, a, b))?;
            Ok(sx.data() <= sy.data())
        }
        _ => Err(compare_error(proto, pc, a, b)),
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
