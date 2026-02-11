//! Base library: print, assert, type, tostring, tonumber, etc.
//!
//! Reference: `lbaselib.c` in PUC-Rio Lua 5.1.1.

use std::io::Write;

use crate::error::{LuaError, LuaResult, RuntimeError};
use crate::vm::callinfo::LUA_MULTRET;
use crate::vm::execute;
use crate::vm::metatable::{self, val_raw_equal};
use crate::vm::state::LuaState;
use crate::vm::table::Table;
use crate::vm::value::{Userdata, Val};

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

/// Number of arguments passed to the current function.
#[inline]
fn nargs(state: &LuaState) -> usize {
    state.top.saturating_sub(state.base)
}

/// Gets argument at position `n` (0-indexed from first arg).
///
/// Returns `Val::Nil` if `n` is past the actual argument count.
#[inline]
fn arg(state: &LuaState, n: usize) -> Val {
    let idx = state.base + n;
    if idx < state.top {
        state.stack_get(idx)
    } else {
        Val::Nil
    }
}

/// Returns "bad argument" error.
fn bad_argument(name: &str, n: usize, msg: &str) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: format!("bad argument #{n} to '{name}' ({msg})"),
        level: 0,
        traceback: vec![],
    })
}

/// Returns a simple runtime error (no source location).
fn simple_error(msg: String) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: msg,
        level: 0,
        traceback: vec![],
    })
}

/// Check minimum argument count.
fn check_args(name: &str, state: &LuaState, min: usize) -> LuaResult<()> {
    if nargs(state) < min {
        Err(bad_argument(name, min, "value expected"))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Metafield helpers (for __tostring, __metatable -- not in TMS enum)
// ---------------------------------------------------------------------------

/// Looks up a named metafield in a value's metatable.
///
/// Unlike the TMS-based lookup, this works for any string key
/// (e.g., `__tostring`, `__metatable`).
fn get_metafield(gc: &mut crate::vm::state::Gc, val: Val, name: &[u8]) -> Option<Val> {
    let mt = match val {
        Val::Table(r) => gc.tables.get(r).and_then(Table::metatable)?,
        Val::Userdata(r) => gc.userdata.get(r).and_then(Userdata::metatable)?,
        _ => gc.type_metatables[metatable::type_tag(val)]?,
    };

    let key_ref = gc.intern_string(name);
    let table = gc.tables.get(mt)?;
    let result = table.get_str(key_ref, &gc.string_arena);
    if result.is_nil() { None } else { Some(result) }
}

// ---------------------------------------------------------------------------
// print
// ---------------------------------------------------------------------------

/// Implements Lua's `print(...)`.
///
/// Tab-separated values, newline-terminated. Uses `tostring()` conversion
/// for each argument.
///
/// Reference: `luaB_print` in `lbaselib.c`.
pub fn lua_print(state: &mut LuaState) -> LuaResult<u32> {
    let base = state.base;
    let top = state.top;
    let n = top.saturating_sub(base);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for i in 0..n {
        if i > 0 {
            let _ = out.write_all(b"\t");
        }
        let val = state.stack_get(base + i);
        let _ = match val {
            Val::Nil => out.write_all(b"nil"),
            Val::Bool(b) => {
                if b {
                    out.write_all(b"true")
                } else {
                    out.write_all(b"false")
                }
            }
            Val::Str(r) => {
                if let Some(s) = state.gc.string_arena.get(r) {
                    out.write_all(s.data())
                } else {
                    out.write_all(b"string: ???")
                }
            }
            _ => {
                let s = format!("{val}");
                out.write_all(s.as_bytes())
            }
        };
    }
    let _ = out.write_all(b"\n");
    let _ = out.flush();

    Ok(0)
}

// ---------------------------------------------------------------------------
// type
// ---------------------------------------------------------------------------

/// Implements Lua's `type(v)`.
///
/// Returns the type name as a string.
/// Reference: `luaB_type` in `lbaselib.c`.
pub fn lua_type(state: &mut LuaState) -> LuaResult<u32> {
    check_args("type", state, 1)?;
    let val = arg(state, 0);
    let name = val.type_name();
    let r = state.gc.intern_string(name.as_bytes());
    state.push(Val::Str(r));
    Ok(1)
}

// ---------------------------------------------------------------------------
// tostring
// ---------------------------------------------------------------------------

/// Implements Lua's `tostring(v)`.
///
/// Respects `__tostring` metamethod. Otherwise converts to string:
/// - nil -> "nil", booleans -> "true"/"false"
/// - numbers -> %.14g formatted
/// - strings -> identity
/// - other -> "type: 0xADDR"
///
/// Reference: `luaB_tostring` in `lbaselib.c`.
pub fn lua_tostring(state: &mut LuaState) -> LuaResult<u32> {
    check_args("tostring", state, 1)?;
    let val = arg(state, 0);

    // Check __tostring metamethod.
    if let Some(tm_val) = get_metafield(&mut state.gc, val, b"__tostring") {
        // Call __tostring(val).
        let call_base = state.top;
        state.ensure_stack(call_base + 3);
        state.stack_set(call_base, tm_val);
        state.stack_set(call_base + 1, val);
        state.top = call_base + 2;

        state.call_function(call_base, 1)?;

        // Result is at call_base (poscall put it there).
        let result = state.stack_get(call_base);
        state.push(result);
        return Ok(1);
    }

    let result = val_to_str(state, val);
    state.push(result);
    Ok(1)
}

/// Converts a value to a Lua string without metamethods.
fn val_to_str(state: &mut LuaState, val: Val) -> Val {
    match val {
        Val::Str(_) => val,
        Val::Nil => {
            let r = state.gc.intern_string(b"nil");
            Val::Str(r)
        }
        Val::Bool(b) => {
            let s = if b {
                b"true".as_ref()
            } else {
                b"false".as_ref()
            };
            let r = state.gc.intern_string(s);
            Val::Str(r)
        }
        Val::Num(_) => {
            let s = format!("{val}");
            let r = state.gc.intern_string(s.as_bytes());
            Val::Str(r)
        }
        _ => {
            // "type: 0xADDR" format
            let s = format!("{val}");
            let r = state.gc.intern_string(s.as_bytes());
            Val::Str(r)
        }
    }
}

// ---------------------------------------------------------------------------
// tonumber
// ---------------------------------------------------------------------------

/// Implements Lua's `tonumber(v, base?)`.
///
/// Base 10 (default): converts numbers and numeric strings.
/// Other bases (2-36): converts string to integer in that base.
/// Returns nil if conversion fails.
///
/// Reference: `luaB_tonumber` in `lbaselib.c`.
pub fn lua_tonumber(state: &mut LuaState) -> LuaResult<u32> {
    check_args("tonumber", state, 1)?;
    let val = arg(state, 0);
    let base_arg = arg(state, 1);

    let base = match base_arg {
        Val::Nil => 10,
        Val::Num(n) => n as i64,
        _ => {
            return Err(bad_argument(
                "tonumber",
                2,
                "number expected, got non-number",
            ));
        }
    };

    if base == 10 {
        // Standard conversion.
        if let Some(n) = execute::coerce_to_number(val, &state.gc) {
            state.push(Val::Num(n));
        } else {
            state.push(Val::Nil);
        }
    } else {
        // Non-decimal base.
        if !(2..=36).contains(&base) {
            return Err(bad_argument("tonumber", 2, "base out of range"));
        }
        let Val::Str(r) = val else {
            return Err(bad_argument("tonumber", 1, "string expected"));
        };
        let s = state
            .gc
            .string_arena
            .get(r)
            .map(|ls| String::from_utf8_lossy(ls.data()).to_string())
            .unwrap_or_default();
        let trimmed = s.trim();
        #[allow(clippy::cast_precision_loss)]
        if let Ok(n) = u64::from_str_radix(trimmed, base as u32) {
            state.push(Val::Num(n as f64));
        } else {
            state.push(Val::Nil);
        }
    }
    Ok(1)
}

// ---------------------------------------------------------------------------
// assert
// ---------------------------------------------------------------------------

/// Implements Lua's `assert(v, msg?)`.
///
/// If the first argument is falsy, raises an error with the optional
/// message (default: "assertion failed!"). If truthy, returns ALL arguments.
///
/// Reference: `luaB_assert` in `lbaselib.c`.
pub fn lua_assert(state: &mut LuaState) -> LuaResult<u32> {
    check_args("assert", state, 1)?;
    let val = arg(state, 0);

    if !val.is_truthy() {
        let msg = arg(state, 1);
        let error_msg = if let Val::Str(r) = msg {
            state.gc.string_arena.get(r).map_or_else(
                || "assertion failed!".to_string(),
                |s| String::from_utf8_lossy(s.data()).to_string(),
            )
        } else if msg.is_nil() {
            "assertion failed!".to_string()
        } else {
            format!("{msg}")
        };

        return Err(simple_error(error_msg));
    }

    // Return all arguments.
    let n = nargs(state);
    Ok(n as u32)
}

// ---------------------------------------------------------------------------
// error
// ---------------------------------------------------------------------------

/// Implements Lua's `error(msg, level?)`.
///
/// Raises a runtime error. If `msg` is a string and `level > 0`,
/// prepends source location. The error object (which can be any Lua value)
/// is stored in `state.error_object` for `pcall`/`xpcall` to retrieve.
///
/// Reference: `luaB_error` in `lbaselib.c`.
pub fn lua_error(state: &mut LuaState) -> LuaResult<u32> {
    let msg = arg(state, 0);
    let level_arg = arg(state, 1);

    let level: u32 = match level_arg {
        Val::Num(n) => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let l = n as u32;
            l
        }
        _ => 1, // default level
    };

    // If msg is a string or number (lua_isstring) and level > 0, prepend source location.
    // PUC-Rio's lua_isstring returns true for both strings and numbers.
    let is_stringable = matches!(msg, Val::Str(_) | Val::Num(_));
    let error_val = if is_stringable && level > 0 {
        let where_prefix = execute::get_where(state, level);
        let original = match msg {
            Val::Str(r) => state
                .gc
                .string_arena
                .get(r)
                .map(|s| String::from_utf8_lossy(s.data()).to_string())
                .unwrap_or_default(),
            _ => format!("{msg}"),
        };
        let full_msg = format!("{where_prefix}{original}");
        let new_r = state.gc.intern_string(full_msg.as_bytes());
        Val::Str(new_r)
    } else {
        msg // Non-string/number: throw as-is.
    };

    // Store the error object for pcall to retrieve.
    state.error_object = Some(error_val);

    // Create the error message string for the RuntimeError.
    let display_msg = match error_val {
        Val::Str(r) => state
            .gc
            .string_arena
            .get(r)
            .map(|s| String::from_utf8_lossy(s.data()).to_string())
            .unwrap_or_default(),
        Val::Nil => "nil".to_string(),
        _ => format!("{error_val}"),
    };

    Err(LuaError::Runtime(RuntimeError {
        message: display_msg,
        level,
        traceback: vec![],
    }))
}

// ---------------------------------------------------------------------------
// pcall
// ---------------------------------------------------------------------------

/// Implements Lua's `pcall(f, ...)`.
///
/// Calls `f` with the given arguments in protected mode. On success,
/// returns `true` followed by all return values. On error, returns
/// `false` followed by the error message.
///
/// Reference: `luaB_pcall` in `lbaselib.c`.
pub fn lua_pcall(state: &mut LuaState) -> LuaResult<u32> {
    check_args("pcall", state, 1)?;

    let func_pos = state.base; // The function to call is pcall's first argument.
    let n_call_args = nargs(state) - 1; // Arguments to pass to the function.

    // Save state for error recovery.
    let saved_ci = state.ci;
    let saved_n_ccalls = state.n_ccalls;

    // Clear any stale error object.
    state.error_object = None;

    // Set top to include function and its arguments.
    state.top = func_pos + 1 + n_call_args;

    // Attempt the call (through call_function for C-call boundary tracking).
    let call_result = state.call_function(func_pos, LUA_MULTRET);

    match call_result {
        Ok(()) => {
            // Success: results are at func_pos..state.top.
            let n_inner = state.top - func_pos;

            // Shift results right by 1 to insert "true" prefix.
            state.ensure_stack(state.top + 1);
            for i in (func_pos..state.top).rev() {
                let v = state.stack_get(i);
                state.stack_set(i + 1, v);
            }
            state.stack_set(func_pos, Val::Bool(true));
            state.top = func_pos + 1 + n_inner;

            Ok((1 + n_inner) as u32)
        }
        Err(err) => {
            // Error: restore state and push false + error value.
            state.ci = saved_ci;
            state.base = state.call_stack[state.ci].base;
            state.n_ccalls = saved_n_ccalls;

            // Close upvalues opened during the failed call.
            state.close_upvalues(func_pos);

            // Get the error value. Prefer the stored error object (from error()),
            // falling back to the error message string.
            let error_val = state.error_object.take().unwrap_or_else(|| {
                let r = state.gc.intern_string(err.to_string().as_bytes());
                Val::Str(r)
            });

            state.stack_set(func_pos, Val::Bool(false));
            state.stack_set(func_pos + 1, error_val);
            state.top = func_pos + 2;

            Ok(2)
        }
    }
}

// ---------------------------------------------------------------------------
// xpcall
// ---------------------------------------------------------------------------

/// Implements Lua's `xpcall(f, err)`.
///
/// Calls `f` with no arguments in protected mode. On error, calls the
/// error handler `err` with the error value. Returns `true` + results
/// on success, or `false` + handler return value on error.
///
/// Reference: `luaB_xpcall` in `lbaselib.c`.
pub fn lua_xpcall(state: &mut LuaState) -> LuaResult<u32> {
    check_args("xpcall", state, 2)?;

    let func_val = arg(state, 0);
    let handler_val = arg(state, 1);

    // Set up to call the function with no arguments.
    let func_pos = state.base;

    // Save state for error recovery.
    let saved_ci = state.ci;
    let saved_n_ccalls = state.n_ccalls;
    state.error_object = None;

    // Place function at func_pos, no arguments.
    state.stack_set(func_pos, func_val);
    state.top = func_pos + 1;

    // Attempt the call (through call_function for C-call boundary tracking).
    let call_result = state.call_function(func_pos, LUA_MULTRET);

    match call_result {
        Ok(()) => {
            // Success: results at func_pos..state.top.
            let n_inner = state.top - func_pos;

            state.ensure_stack(state.top + 1);
            for i in (func_pos..state.top).rev() {
                let v = state.stack_get(i);
                state.stack_set(i + 1, v);
            }
            state.stack_set(func_pos, Val::Bool(true));
            state.top = func_pos + 1 + n_inner;

            Ok((1 + n_inner) as u32)
        }
        Err(err) => {
            // Error: restore state.
            state.ci = saved_ci;
            state.base = state.call_stack[state.ci].base;
            state.n_ccalls = saved_n_ccalls;
            state.close_upvalues(func_pos);

            // Get the error value.
            let error_val = state.error_object.take().unwrap_or_else(|| {
                let r = state.gc.intern_string(err.to_string().as_bytes());
                Val::Str(r)
            });

            // Call the error handler with the error value.
            state.stack_set(func_pos, handler_val);
            state.stack_set(func_pos + 1, error_val);
            state.top = func_pos + 2;

            let handler_result = state.call_function(func_pos, 1);

            if handler_result.is_ok() {
                // Handler returned. Result at func_pos.
                let handler_ret = state.stack_get(func_pos);
                state.stack_set(func_pos, Val::Bool(false));
                state.stack_set(func_pos + 1, handler_ret);
            } else {
                // Error in error handler: return false + "error in error handling".
                state.ci = saved_ci;
                state.base = state.call_stack[state.ci].base;
                state.n_ccalls = saved_n_ccalls;
                let msg = state.gc.intern_string(b"error in error handling");
                state.stack_set(func_pos, Val::Bool(false));
                state.stack_set(func_pos + 1, Val::Str(msg));
            }
            state.top = func_pos + 2;
            Ok(2)
        }
    }
}

// ---------------------------------------------------------------------------
// setmetatable / getmetatable
// ---------------------------------------------------------------------------

/// Implements Lua's `setmetatable(table, metatable)`.
///
/// Sets the metatable for a table. The second argument must be nil or a table.
/// If the current metatable has a `__metatable` field, raises an error.
///
/// Reference: `luaB_setmetatable` in `lbaselib.c`.
pub fn lua_setmetatable(state: &mut LuaState) -> LuaResult<u32> {
    check_args("setmetatable", state, 2)?;
    let table_val = arg(state, 0);
    let mt_val = arg(state, 1);

    let Val::Table(table_ref) = table_val else {
        return Err(bad_argument("setmetatable", 1, "table expected"));
    };

    // Validate second argument is nil or table.
    let new_mt = match mt_val {
        Val::Nil => None,
        Val::Table(r) => Some(r),
        _ => {
            return Err(bad_argument("setmetatable", 2, "nil or table expected"));
        }
    };

    // Check for __metatable protection.
    if let Some(existing_mt) = state.gc.tables.get(table_ref).and_then(Table::metatable) {
        let key_ref = state.gc.intern_string(b"__metatable");
        let has_protection = state
            .gc
            .tables
            .get(existing_mt)
            .is_some_and(|t| !t.get_str(key_ref, &state.gc.string_arena).is_nil());
        if has_protection {
            return Err(simple_error(
                "cannot change a protected metatable".to_string(),
            ));
        }
    }

    // Set the metatable.
    if let Some(t) = state.gc.tables.get_mut(table_ref) {
        t.set_metatable(new_mt);
    }

    // Return the table.
    state.push(table_val);
    Ok(1)
}

/// Implements Lua's `getmetatable(obj)`.
///
/// Returns the metatable of the object. If the metatable has a `__metatable`
/// field, returns that field instead of the actual metatable.
///
/// Reference: `luaB_getmetatable` in `lbaselib.c`.
pub fn lua_getmetatable(state: &mut LuaState) -> LuaResult<u32> {
    check_args("getmetatable", state, 1)?;
    let val = arg(state, 0);

    // Get the actual metatable.
    let mt = match val {
        Val::Table(r) => state.gc.tables.get(r).and_then(Table::metatable),
        Val::Userdata(r) => state.gc.userdata.get(r).and_then(Userdata::metatable),
        _ => state.gc.type_metatables[metatable::type_tag(val)],
    };

    let Some(mt_ref) = mt else {
        state.push(Val::Nil);
        return Ok(1);
    };

    // Check for __metatable field.
    if let Some(protection) = get_metafield(&mut state.gc, val, b"__metatable") {
        state.push(protection);
    } else {
        state.push(Val::Table(mt_ref));
    }

    Ok(1)
}

// ---------------------------------------------------------------------------
// rawget / rawset / rawequal
// ---------------------------------------------------------------------------

/// Implements Lua's `rawget(table, index)`.
///
/// Gets a value from a table without invoking metamethods.
/// Reference: `luaB_rawget` in `lbaselib.c`.
pub fn lua_rawget(state: &mut LuaState) -> LuaResult<u32> {
    check_args("rawget", state, 2)?;
    let table_val = arg(state, 0);
    let key = arg(state, 1);

    let Val::Table(table_ref) = table_val else {
        return Err(bad_argument("rawget", 1, "table expected"));
    };

    let result = state
        .gc
        .tables
        .get(table_ref)
        .map_or(Val::Nil, |t| t.get(key, &state.gc.string_arena));
    state.push(result);
    Ok(1)
}

/// Implements Lua's `rawset(table, index, value)`.
///
/// Sets a value in a table without invoking metamethods.
/// Reference: `luaB_rawset` in `lbaselib.c`.
pub fn lua_rawset(state: &mut LuaState) -> LuaResult<u32> {
    check_args("rawset", state, 3)?;
    let table_val = arg(state, 0);
    let key = arg(state, 1);
    let value = arg(state, 2);

    let Val::Table(table_ref) = table_val else {
        return Err(bad_argument("rawset", 1, "table expected"));
    };

    // Need to split the borrow: get table mutably, string_arena immutably.
    // Since these are different arenas in gc, we access them via the public fields.
    let table = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| bad_argument("rawset", 1, "invalid table"))?;
    table.raw_set(key, value, &state.gc.string_arena)?;

    // Return the table.
    state.push(table_val);
    Ok(1)
}

/// Implements Lua's `rawequal(v1, v2)`.
///
/// Compares two values without invoking metamethods.
/// Reference: `luaB_rawequal` in `lbaselib.c`.
pub fn lua_rawequal(state: &mut LuaState) -> LuaResult<u32> {
    check_args("rawequal", state, 2)?;
    let v1 = arg(state, 0);
    let v2 = arg(state, 1);

    let result = val_raw_equal(v1, v2, &state.gc.tables, &state.gc.string_arena);
    state.push(Val::Bool(result));
    Ok(1)
}

// ---------------------------------------------------------------------------
// select
// ---------------------------------------------------------------------------

/// Implements Lua's `select(index, ...)`.
///
/// If `index` is the string "#", returns the number of extra arguments.
/// Otherwise, returns all arguments from `index` onward.
///
/// Reference: `luaB_select` in `lbaselib.c`.
pub fn lua_select(state: &mut LuaState) -> LuaResult<u32> {
    check_args("select", state, 1)?;
    let n = nargs(state);
    let idx_val = arg(state, 0);

    // Check for "#" selector.
    if let Val::Str(r) = idx_val
        && let Some(s) = state.gc.string_arena.get(r)
        && s.data() == b"#"
    {
        #[allow(clippy::cast_precision_loss)]
        let count = (n - 1) as f64;
        state.push(Val::Num(count));
        return Ok(1);
    }

    // Numeric index.
    let Val::Num(idx_f) = idx_val else {
        return Err(bad_argument("select", 1, "number or string expected"));
    };
    #[allow(clippy::cast_possible_truncation)]
    let mut idx = idx_f as i64;
    let n_extra = (n - 1) as i64; // Arguments after the index.

    if idx < 0 {
        idx += n_extra + 1; // Negative indexing from end.
    }
    if idx < 1 || idx > n_extra {
        return Err(bad_argument("select", 1, "index out of range"));
    }

    // Return all arguments from idx onward.
    // The arguments are at state.base+1 through state.base+n-1.
    // We want arguments starting at state.base + idx.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let result_count = (n_extra - idx + 1) as u32;
    Ok(result_count)
}

// ---------------------------------------------------------------------------
// unpack
// ---------------------------------------------------------------------------

/// Implements Lua's `unpack(list, i, j)`.
///
/// Returns `list[i], list[i+1], ..., list[j]`.
/// Default: `i=1`, `j=#list`.
///
/// Reference: `luaB_unpack` in `lbaselib.c`.
pub fn lua_unpack(state: &mut LuaState) -> LuaResult<u32> {
    check_args("unpack", state, 1)?;
    let list_val = arg(state, 0);
    let i_val = arg(state, 1);
    let j_val = arg(state, 2);

    let Val::Table(table_ref) = list_val else {
        return Err(bad_argument("unpack", 1, "table expected"));
    };

    #[allow(clippy::cast_possible_truncation)]
    let i = match i_val {
        Val::Num(n) => n as i64,
        Val::Nil => 1,
        _ => {
            return Err(bad_argument("unpack", 2, "number expected"));
        }
    };

    #[allow(clippy::cast_possible_truncation)]
    let j = match j_val {
        Val::Nil => {
            // Default: #list (table length).
            let table = state
                .gc
                .tables
                .get(table_ref)
                .ok_or_else(|| bad_argument("unpack", 1, "invalid table"))?;
            table.len(&state.gc.string_arena) as i64
        }
        Val::Num(n) => n as i64,
        _ => {
            return Err(bad_argument("unpack", 3, "number expected"));
        }
    };

    let n = j - i + 1;
    if n <= 0 {
        return Ok(0);
    }

    #[allow(clippy::cast_sign_loss)]
    let n_usize = n as usize;
    state.ensure_stack(state.top + n_usize);

    for k in i..=j {
        #[allow(clippy::cast_precision_loss)]
        let key = Val::Num(k as f64);
        let val = state
            .gc
            .tables
            .get(table_ref)
            .map_or(Val::Nil, |t| t.get(key, &state.gc.string_arena));
        state.push(val);
    }

    Ok(n_usize as u32)
}

// ---------------------------------------------------------------------------
// next / pairs / ipairs
// ---------------------------------------------------------------------------

/// Implements Lua's `next(table, key)`.
///
/// Returns the next key-value pair after `key` in the table.
/// If `key` is nil, returns the first pair. Returns nil at end.
///
/// Reference: `luaB_next` in `lbaselib.c`.
pub fn lua_next(state: &mut LuaState) -> LuaResult<u32> {
    check_args("next", state, 1)?;
    let table_val = arg(state, 0);
    let key = arg(state, 1); // defaults to nil if omitted

    let Val::Table(table_ref) = table_val else {
        return Err(bad_argument("next", 1, "table expected"));
    };

    let result = state
        .gc
        .tables
        .get(table_ref)
        .ok_or_else(|| bad_argument("next", 1, "invalid table"))?
        .next(key, &state.gc.string_arena)?;

    if let Some((k, v)) = result {
        state.push(k);
        state.push(v);
        Ok(2)
    } else {
        state.push(Val::Nil);
        Ok(1)
    }
}

/// Implements Lua's `pairs(t)`.
///
/// Returns three values: the `next` function, the table, and nil.
/// The generic `for` loop uses these to iterate over all key-value pairs.
///
/// Reference: `luaB_pairs` in `lbaselib.c`.
pub fn lua_pairs(state: &mut LuaState) -> LuaResult<u32> {
    check_args("pairs", state, 1)?;
    let table_val = arg(state, 0);

    let Val::Table(_) = table_val else {
        return Err(bad_argument("pairs", 1, "table expected"));
    };

    // Push the `next` function. We look it up from globals.
    let next_key = state.gc.intern_string(b"next");
    let next_fn = state
        .gc
        .tables
        .get(state.global)
        .map_or(Val::Nil, |t| t.get_str(next_key, &state.gc.string_arena));
    state.push(next_fn);
    state.push(table_val);
    state.push(Val::Nil);
    Ok(3)
}

/// Internal iterator for `ipairs`. Called as `ipairsaux(t, i)`.
///
/// Increments `i`, does `rawget(t, i+1)`, returns `(i+1, value)` or
/// nothing if value is nil.
fn ipairs_aux(state: &mut LuaState) -> LuaResult<u32> {
    let table_val = arg(state, 0);
    let idx_val = arg(state, 1);

    let Val::Table(table_ref) = table_val else {
        return Err(bad_argument("ipairs", 1, "table expected"));
    };

    let Val::Num(idx_f) = idx_val else {
        return Err(bad_argument("ipairs", 2, "number expected"));
    };

    let next_idx = idx_f + 1.0;
    #[allow(clippy::cast_precision_loss)]
    let key = Val::Num(next_idx);
    let val = state
        .gc
        .tables
        .get(table_ref)
        .map_or(Val::Nil, |t| t.get(key, &state.gc.string_arena));

    if val.is_nil() {
        Ok(0)
    } else {
        state.push(Val::Num(next_idx));
        state.push(val);
        Ok(2)
    }
}

/// Implements Lua's `ipairs(t)`.
///
/// Returns three values: an iterator function, the table, and 0.
/// The iterator returns sequential integer keys 1, 2, 3, ... with
/// their values, stopping at the first nil.
///
/// Reference: `luaB_ipairs` in `lbaselib.c`.
pub fn lua_ipairs(state: &mut LuaState) -> LuaResult<u32> {
    check_args("ipairs", state, 1)?;
    let table_val = arg(state, 0);

    let Val::Table(_) = table_val else {
        return Err(bad_argument("ipairs", 1, "table expected"));
    };

    // Create the ipairs_aux closure and push it.
    let closure = crate::vm::closure::Closure::Rust(crate::vm::closure::RustClosure::new(
        ipairs_aux,
        "ipairs_aux",
    ));
    let closure_ref = state.gc.alloc_closure(closure);
    state.push(Val::Function(closure_ref));
    state.push(table_val);
    state.push(Val::Num(0.0));
    Ok(3)
}

// ---------------------------------------------------------------------------
// loadstring / loadfile / dofile / load
// ---------------------------------------------------------------------------

/// Implements Lua's `loadstring(string [, chunkname])`.
///
/// Compiles a string as a Lua chunk. Returns the compiled function on success,
/// or nil + error message on failure.
///
/// Reference: `luaB_loadstring` in `lbaselib.c`.
pub fn lua_loadstring(state: &mut LuaState) -> LuaResult<u32> {
    check_args("loadstring", state, 1)?;
    let src_val = arg(state, 0);
    let name_val = arg(state, 1);

    let Val::Str(src_ref) = src_val else {
        return Err(bad_argument("loadstring", 1, "string expected"));
    };

    let source = state
        .gc
        .string_arena
        .get(src_ref)
        .map(|s| String::from_utf8_lossy(s.data()).to_string())
        .unwrap_or_default();

    // Chunk name: defaults to the source string itself.
    let chunk_name = if let Val::Str(r) = name_val {
        state.gc.string_arena.get(r).map_or_else(
            || source.clone(),
            |s| String::from_utf8_lossy(s.data()).to_string(),
        )
    } else {
        source.clone()
    };

    load_string_impl(state, &source, &chunk_name)
}

/// Implements Lua's `loadfile([filename])`.
///
/// Reads and compiles a Lua file. If no filename is given, reads from stdin.
/// Returns the compiled function on success, or nil + error message on failure.
///
/// Reference: `luaB_loadfile` in `lbaselib.c`.
pub fn lua_loadfile(state: &mut LuaState) -> LuaResult<u32> {
    let filename_val = arg(state, 0);

    let source = if filename_val.is_nil() {
        // Read from stdin.
        use std::io::Read;
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_err() {
            state.push(Val::Nil);
            let msg = state.gc.intern_string(b"cannot read stdin");
            state.push(Val::Str(msg));
            return Ok(2);
        }
        (buf, "=stdin".to_string())
    } else {
        let Val::Str(r) = filename_val else {
            return Err(bad_argument("loadfile", 1, "string expected"));
        };
        let filename = state
            .gc
            .string_arena
            .get(r)
            .map(|s| String::from_utf8_lossy(s.data()).to_string())
            .unwrap_or_default();

        match std::fs::read_to_string(&filename) {
            Ok(contents) => (contents, format!("@{filename}")),
            Err(e) => {
                state.push(Val::Nil);
                let msg = state
                    .gc
                    .intern_string(format!("cannot open {filename}: {e}").as_bytes());
                state.push(Val::Str(msg));
                return Ok(2);
            }
        }
    };

    load_string_impl(state, &source.0, &source.1)
}

/// Implements Lua's `dofile([filename])`.
///
/// Reads, compiles, and executes a Lua file. Unlike `loadfile`, raises
/// errors instead of returning nil+msg. Returns all values from the chunk.
///
/// Reference: `luaB_dofile` in `lbaselib.c`.
pub fn lua_dofile(state: &mut LuaState) -> LuaResult<u32> {
    let filename_val = arg(state, 0);

    let (source, chunk_name) = if filename_val.is_nil() {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| simple_error(format!("cannot read stdin: {e}")))?;
        (buf, "=stdin".to_string())
    } else {
        let Val::Str(r) = filename_val else {
            return Err(bad_argument("dofile", 1, "string expected"));
        };
        let filename = state
            .gc
            .string_arena
            .get(r)
            .map(|s| String::from_utf8_lossy(s.data()).to_string())
            .unwrap_or_default();

        let contents = std::fs::read_to_string(&filename)
            .map_err(|e| simple_error(format!("cannot open {filename}: {e}")))?;
        (contents, format!("@{filename}"))
    };

    // Compile the source.
    let proto = crate::compiler::compile(&source, &chunk_name)?;

    // Patch string constants.
    let mut proto = std::rc::Rc::try_unwrap(proto).unwrap_or_else(|rc| (*rc).clone());
    crate::patch_string_constants(&mut proto, &mut state.gc);
    let proto = std::rc::Rc::new(proto);

    // Create closure with the current global table as environment.
    let lua_cl = crate::vm::closure::LuaClosure::new(proto, state.global);
    let closure_ref = state
        .gc
        .alloc_closure(crate::vm::closure::Closure::Lua(lua_cl));

    // Set up the call.
    let call_base = state.top;
    state.ensure_stack(call_base + 2);
    state.stack_set(call_base, Val::Function(closure_ref));
    state.top = call_base + 1;

    state.call_function(call_base, LUA_MULTRET)?;

    // Return all results.
    let n_results = state.top - call_base;
    Ok(n_results as u32)
}

/// Implements Lua's `load(func [, chunkname])`.
///
/// Loads a chunk from a reader function. The reader is called repeatedly
/// with no arguments; it should return a string each time, or nil to signal
/// end of input.
///
/// Reference: `luaB_load` in `lbaselib.c`.
pub fn lua_load(state: &mut LuaState) -> LuaResult<u32> {
    check_args("load", state, 1)?;
    let func_val = arg(state, 0);
    let name_val = arg(state, 1);

    if !matches!(func_val, Val::Function(_)) {
        return Err(bad_argument("load", 1, "function expected"));
    }

    let chunk_name = if let Val::Str(r) = name_val {
        state.gc.string_arena.get(r).map_or_else(
            || "=(load)".to_string(),
            |s| String::from_utf8_lossy(s.data()).to_string(),
        )
    } else {
        "=(load)".to_string()
    };

    // Call the reader function repeatedly to collect source.
    let mut collected = String::new();
    loop {
        let call_base = state.top;
        state.ensure_stack(call_base + 2);
        state.stack_set(call_base, func_val);
        state.top = call_base + 1;

        state.call_function(call_base, 1)?;

        let result = state.stack_get(call_base);
        state.top = call_base; // Clean up.

        match result {
            Val::Nil => break, // End of input.
            Val::Str(r) => {
                let chunk = state
                    .gc
                    .string_arena
                    .get(r)
                    .map(|s| String::from_utf8_lossy(s.data()).to_string())
                    .unwrap_or_default();
                if chunk.is_empty() {
                    break; // Empty string also signals end.
                }
                collected.push_str(&chunk);
            }
            _ => {
                return Err(simple_error(
                    "reader function must return a string".to_string(),
                ));
            }
        }
    }

    load_string_impl(state, &collected, &chunk_name)
}

/// Shared implementation for loadstring/loadfile/load: compiles source
/// and pushes either the function or nil+error.
#[allow(clippy::unnecessary_wraps)]
fn load_string_impl(state: &mut LuaState, source: &str, name: &str) -> LuaResult<u32> {
    match crate::compiler::compile(source, name) {
        Ok(proto) => {
            let mut proto = std::rc::Rc::try_unwrap(proto).unwrap_or_else(|rc| (*rc).clone());
            crate::patch_string_constants(&mut proto, &mut state.gc);
            let proto = std::rc::Rc::new(proto);

            let lua_cl = crate::vm::closure::LuaClosure::new(proto, state.global);
            let closure_ref = state
                .gc
                .alloc_closure(crate::vm::closure::Closure::Lua(lua_cl));
            state.push(Val::Function(closure_ref));
            Ok(1)
        }
        Err(e) => {
            state.push(Val::Nil);
            let msg = state.gc.intern_string(e.to_string().as_bytes());
            state.push(Val::Str(msg));
            Ok(2)
        }
    }
}

// ---------------------------------------------------------------------------
// collectgarbage
// ---------------------------------------------------------------------------

/// Implements Lua's `collectgarbage([opt [, arg]])`.
///
/// GC control interface. Until Phase 7 (GC collector), most operations
/// are stubs that return 0.
///
/// Reference: `luaB_collectgarbage` in `lbaselib.c`.
pub fn lua_collectgarbage(state: &mut LuaState) -> LuaResult<u32> {
    let opt_val = arg(state, 0);

    let opt = if opt_val.is_nil() {
        "collect"
    } else if let Val::Str(r) = opt_val {
        // We need to match the string. Extract it temporarily.
        let s = state
            .gc
            .string_arena
            .get(r)
            .map(|s| String::from_utf8_lossy(s.data()).to_string())
            .unwrap_or_default();
        // Match against known options.
        return collectgarbage_dispatch(state, &s);
    } else {
        return Err(bad_argument("collectgarbage", 1, "string expected"));
    };

    collectgarbage_dispatch(state, opt)
}

fn collectgarbage_dispatch(state: &mut LuaState, opt: &str) -> LuaResult<u32> {
    match opt {
        "collect" | "stop" | "restart" => {
            // Stubs until Phase 7.
            state.push(Val::Num(0.0));
            Ok(1)
        }
        "count" => {
            // Return approximate memory usage in KB.
            // Stub: report sum of arena sizes as rough estimate.
            let kb = (f64::from(state.gc.string_arena.len())
                + f64::from(state.gc.tables.len())
                + f64::from(state.gc.closures.len())
                + f64::from(state.gc.upvalues.len()))
                / 1024.0;
            state.push(Val::Num(kb));
            Ok(1)
        }
        "step" => {
            // Stub: report cycle not completed (matches PUC-Rio default behavior).
            state.push(Val::Bool(false));
            Ok(1)
        }
        "setpause" | "setstepmul" => {
            // Stub: return previous value (0).
            state.push(Val::Num(0.0));
            Ok(1)
        }
        _ => Err(bad_argument(
            "collectgarbage",
            1,
            &format!("invalid option '{opt}'"),
        )),
    }
}

// ---------------------------------------------------------------------------
// setfenv / getfenv
// ---------------------------------------------------------------------------

/// Implements Lua's `getfenv([f])`.
///
/// Returns the environment table of a function. If `f` is a number,
/// returns the environment of the function at that stack level.
/// Default level is 1 (the calling function).
///
/// Reference: `luaB_getfenv` in `lbaselib.c`.
pub fn lua_getfenv(state: &mut LuaState) -> LuaResult<u32> {
    let val = arg(state, 0);

    // Default: level 1.
    let func_val = match val {
        Val::Nil => get_func_at_level(state, 1)?,
        Val::Num(n) => {
            #[allow(clippy::cast_possible_truncation)]
            let level = n as i64;
            if level == 0 {
                // Level 0: return the thread's global environment.
                state.push(Val::Table(state.global));
                return Ok(1);
            }
            get_func_at_level(state, level as u32)?
        }
        Val::Function(_) => val,
        _ => {
            return Err(bad_argument("getfenv", 1, "number or function expected"));
        }
    };

    // For Rust functions, return the global environment.
    // For Lua functions, return the closure's environment.
    if let Val::Function(r) = func_val {
        let env = state.gc.closures.get(r).map(|cl| match cl {
            crate::vm::closure::Closure::Lua(lua_cl) => lua_cl.env,
            crate::vm::closure::Closure::Rust(_) => state.global,
        });
        state.push(Val::Table(env.unwrap_or(state.global)));
    } else {
        state.push(Val::Table(state.global));
    }
    Ok(1)
}

/// Implements Lua's `setfenv(f, table)`.
///
/// Sets the environment table of a function. `f` can be a function or
/// a stack level number. Level 0 changes the thread's global environment.
///
/// Reference: `luaB_setfenv` in `lbaselib.c`.
pub fn lua_setfenv(state: &mut LuaState) -> LuaResult<u32> {
    check_args("setfenv", state, 2)?;
    let f_val = arg(state, 0);
    let table_val = arg(state, 1);

    let Val::Table(new_env) = table_val else {
        return Err(bad_argument("setfenv", 2, "table expected"));
    };

    match f_val {
        Val::Num(n) => {
            #[allow(clippy::cast_possible_truncation)]
            let level = n as i64;
            if level == 0 {
                // Change thread's global environment.
                state.global = new_env;
                return Ok(0);
            }

            // Get the function at this stack level and set its env.
            let func_val = get_func_at_level(state, level as u32)?;
            set_func_env(state, func_val, new_env)?;
            state.push(func_val);
            Ok(1)
        }
        Val::Function(_) => {
            set_func_env(state, f_val, new_env)?;
            state.push(f_val);
            Ok(1)
        }
        _ => Err(bad_argument("setfenv", 1, "number or function expected")),
    }
}

/// Gets the function value at the given call stack level.
///
/// Level 1 = the direct caller, level 2 = its caller, etc.
fn get_func_at_level(state: &LuaState, level: u32) -> LuaResult<Val> {
    // Walk back from current ci.
    let mut ci_idx = state.ci;
    let mut remaining = level;

    while remaining > 0 && ci_idx > 0 {
        ci_idx -= 1;
        remaining -= 1;
    }

    if remaining > 0 {
        return Err(bad_argument(
            "getfenv",
            1,
            &format!("invalid level {level}"),
        ));
    }

    let func_idx = state.call_stack[ci_idx].func;
    Ok(state.stack_get(func_idx))
}

/// Sets the environment of a function value.
fn set_func_env(
    state: &mut LuaState,
    func_val: Val,
    new_env: crate::vm::gc::arena::GcRef<Table>,
) -> LuaResult<()> {
    let Val::Function(closure_ref) = func_val else {
        return Err(simple_error(
            "'setfenv' cannot change environment of given object".to_string(),
        ));
    };

    let cl = state.gc.closures.get_mut(closure_ref).ok_or_else(|| {
        simple_error("'setfenv' cannot change environment of given object".to_string())
    })?;

    match cl {
        crate::vm::closure::Closure::Lua(lua_cl) => {
            lua_cl.env = new_env;
            Ok(())
        }
        crate::vm::closure::Closure::Rust(_) => Err(simple_error(
            "'setfenv' cannot change environment of given object".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// newproxy
// ---------------------------------------------------------------------------

/// Implements Lua's `newproxy([boolean])`.
///
/// Creates a zero-size userdata. If `true` is passed, attaches an empty
/// metatable. This is an undocumented but present function in Lua 5.1.1.
///
/// Creates a zero-size userdata. With `true` argument, attaches an empty
/// metatable. With another proxy as argument, shares its metatable.
///
/// Reference: `luaB_newproxy` in `lbaselib.c`.
pub fn lua_newproxy(state: &mut LuaState) -> LuaResult<u32> {
    let arg_val = arg(state, 0);

    // Create a zero-size userdata.
    let ud = Userdata::new(Box::new(()));
    let ud_ref = state.gc.alloc_userdata(ud);

    if arg_val == Val::Bool(true) {
        // Attach an empty metatable.
        let mt = state.gc.alloc_table(Table::new());
        if let Some(u) = state.gc.userdata.get_mut(ud_ref) {
            u.set_metatable(Some(mt));
        }
    } else if let Val::Userdata(other_ref) = arg_val {
        // Share the metatable from an existing proxy.
        let other_mt = state
            .gc
            .userdata
            .get(other_ref)
            .and_then(Userdata::metatable);
        if let (Some(mt), Some(u)) = (other_mt, state.gc.userdata.get_mut(ud_ref)) {
            u.set_metatable(Some(mt));
        }
    }

    state.push(Val::Userdata(ud_ref));
    Ok(1)
}

// ---------------------------------------------------------------------------
// gcinfo (deprecated)
// ---------------------------------------------------------------------------

/// Implements Lua's deprecated `gcinfo()`.
///
/// Returns the total memory in use (in Kbytes).
/// Equivalent to `collectgarbage("count")` but returns an integer.
///
/// Reference: `luaB_gcinfo` in `lbaselib.c`.
pub fn lua_gcinfo(state: &mut LuaState) -> LuaResult<u32> {
    // Stub: report approximate arena object count / 1024.
    let kb = (f64::from(state.gc.string_arena.len())
        + f64::from(state.gc.tables.len())
        + f64::from(state.gc.closures.len())
        + f64::from(state.gc.upvalues.len())
        + f64::from(state.gc.userdata.len()))
        / 1024.0;
    // gcinfo returns an integer (floor).
    #[allow(clippy::cast_possible_truncation)]
    state.push(Val::Num(f64::from(kb.floor() as i32)));
    Ok(1)
}
