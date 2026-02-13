//! Debug library: introspection, hooks, and stack inspection.
//!
//! Implements all 14 functions from PUC-Rio's `ldblib.c`:
//! getregistry, getmetatable, setmetatable, getfenv, setfenv,
//! getinfo, getlocal, setlocal, getupvalue, setupvalue,
//! gethook, sethook, debug, traceback.
//!
//! Hooks (`sethook`/`gethook`) are stubbed -- they require VM execution
//! loop integration (per-instruction mask checking). The `debug` function
//! is also a stub (interactive mode needs a persistent stdin loop).

use std::fmt::Write as _;

use crate::error::{LuaError, LuaResult, RuntimeError};
use crate::vm::closure::Closure;
use crate::vm::debug_info;
use crate::vm::gc::arena::GcRef;
use crate::vm::state::LuaState;
use crate::vm::table::Table;
use crate::vm::value::Val;

// ---------------------------------------------------------------------------
// Helpers (same pattern as os.rs / base.rs)
// ---------------------------------------------------------------------------

#[inline]
fn nargs(state: &LuaState) -> usize {
    state.top.saturating_sub(state.base)
}

#[inline]
fn arg(state: &LuaState, n: usize) -> Val {
    let idx = state.base + n;
    if idx < state.top {
        state.stack_get(idx)
    } else {
        Val::Nil
    }
}

fn bad_argument(name: &str, n: usize, msg: &str) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: format!("bad argument #{n} to '{name}' ({msg})"),
        level: 0,
        traceback: vec![],
    })
}

fn simple_error(msg: String) -> LuaError {
    LuaError::Runtime(RuntimeError {
        message: msg,
        level: 0,
        traceback: vec![],
    })
}

fn check_number(state: &LuaState, name: &str, n: usize) -> LuaResult<f64> {
    match arg(state, n) {
        Val::Num(v) => Ok(v),
        Val::Str(r) => {
            let s = state
                .gc
                .string_arena
                .get(r)
                .map(|s| s.data().to_vec())
                .ok_or_else(|| bad_argument(name, n + 1, "number expected"))?;
            let text = String::from_utf8_lossy(&s);
            text.trim()
                .parse::<f64>()
                .map_err(|_| bad_argument(name, n + 1, "number expected"))
        }
        _ => Err(bad_argument(name, n + 1, "number expected")),
    }
}

/// Returns the thread-offset: 1 if arg(0) is a Thread, else 0.
fn get_thread_offset(state: &LuaState) -> usize {
    usize::from(nargs(state) >= 1 && matches!(arg(state, 0), Val::Thread(_)))
}

// ---------------------------------------------------------------------------
pub(crate) use crate::error::chunkid;

/// Gets the current line from a Lua `CallInfo`.
pub(crate) fn current_line(state: &LuaState, ci_idx: usize) -> i32 {
    let ci = &state.call_stack[ci_idx];
    let func_val = state.stack_get(ci.func);
    if let Val::Function(r) = func_val
        && let Some(Closure::Lua(lcl)) = state.gc.closures.get(r)
    {
        let pc = ci.saved_pc;
        if pc > 0 && pc <= lcl.proto.line_info.len() {
            return lcl.proto.line_info[pc - 1] as i32;
        } else if !lcl.proto.line_info.is_empty() {
            return lcl.proto.line_info[0] as i32;
        }
    }
    -1
}

/// Generates a stack traceback string from the current call stack.
///
/// This is the shared logic used by both `debug.traceback()` and the CLI
/// error handler. Matches PUC-Rio's `luaL_traceback` / the traceback
/// function in `lua.c`.
///
/// `msg` is prepended (with a newline separator) if non-empty.
/// `start_level` controls how many frames to skip from the top
/// (PUC-Rio uses 2 to skip the traceback function and the error handler).
pub(crate) fn generate_traceback(state: &LuaState, msg: &str, start_level: usize) -> String {
    const LEVELS1: usize = 12;
    const LEVELS2: usize = 10;

    let mut result = String::new();

    if !msg.is_empty() {
        result.push_str(msg);
        result.push('\n');
    }

    result.push_str("stack traceback:");

    let mut level = start_level;
    let mut first_part = true;
    loop {
        if level > state.ci {
            break;
        }
        let ci_idx = state.ci - level;

        if level > LEVELS1 && first_part {
            if state.ci > level + LEVELS2 + 1 {
                result.push_str("\n\t...");
                let mut total = level;
                while state.ci > total + 1 {
                    total += 1;
                }
                level = total - LEVELS2;
                first_part = false;
                continue;
            }
            first_part = false;
        }

        result.push_str("\n\t");

        let ci = &state.call_stack[ci_idx];
        let func_val = state.stack_get(ci.func);

        // Try to resolve function name from the calling frame.
        let func_name = debug_info::getfuncname(state, ci_idx, &state.gc.string_arena);

        if let Val::Function(r) = func_val {
            if let Some(cl) = state.gc.closures.get(r) {
                match cl {
                    Closure::Lua(lcl) => {
                        let short_src = chunkid(&lcl.proto.source);
                        result.push_str(&short_src);
                        result.push(':');
                        let line = current_line(state, ci_idx);
                        if line > 0 {
                            let _ = write!(result, "{line}:");
                        }
                        if let Some((_kind, name)) = &func_name {
                            let _ = write!(result, " in function '{name}'");
                        } else if lcl.proto.line_defined == 0 {
                            result.push_str(" in main chunk");
                        } else {
                            let _ = write!(
                                result,
                                " in function <{}:{}>",
                                short_src, lcl.proto.line_defined
                            );
                        }
                    }
                    Closure::Rust(rcl) => {
                        result.push_str("[C]:");
                        if let Some((_kind, name)) = &func_name {
                            let _ = write!(result, " in function '{name}'");
                        } else if rcl.name.is_empty() {
                            result.push_str(" ?");
                        } else {
                            let _ = write!(result, " in function '{}'", rcl.name);
                        }
                    }
                }
            } else {
                result.push_str("[C]: ?");
            }
        } else {
            result.push_str("[C]: ?");
        }

        level += 1;
    }

    result
}

/// PUC-Rio's `luaF_getlocalname` equivalent -- finds active local variable
/// name at the given 1-based local index and PC position.
fn get_local_name(state: &LuaState, ci_idx: usize, local_number: usize) -> Option<String> {
    let ci = &state.call_stack[ci_idx];
    let func_val = state.stack_get(ci.func);
    if let Val::Function(r) = func_val
        && let Some(Closure::Lua(lcl)) = state.gc.closures.get(r)
    {
        let pc = if ci.saved_pc > 0 { ci.saved_pc - 1 } else { 0 };
        let mut n = local_number;
        for lv in &lcl.proto.local_vars {
            if lv.start_pc as usize <= pc && pc < lv.end_pc as usize {
                n -= 1;
                if n == 0 {
                    return Some(lv.name.clone());
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 1. debug.getregistry()
// ---------------------------------------------------------------------------

pub fn db_getregistry(state: &mut LuaState) -> LuaResult<u32> {
    state.push(Val::Table(state.registry));
    Ok(1)
}

// ---------------------------------------------------------------------------
// 2. debug.getmetatable(obj) -- raw metatable, bypasses __metatable
// ---------------------------------------------------------------------------

pub fn db_getmetatable(state: &mut LuaState) -> LuaResult<u32> {
    if nargs(state) < 1 {
        return Err(bad_argument("getmetatable", 1, "value expected"));
    }
    let val = arg(state, 0);
    let mt = match val {
        Val::Table(r) => state.gc.tables.get(r).and_then(Table::metatable),
        Val::Userdata(r) => state
            .gc
            .userdata
            .get(r)
            .and_then(crate::vm::value::Userdata::metatable),
        _ => {
            let tag = crate::vm::metatable::type_tag(val);
            state.gc.type_metatables[tag]
        }
    };
    match mt {
        Some(mt_ref) => state.push(Val::Table(mt_ref)),
        None => state.push(Val::Nil),
    }
    Ok(1)
}

// ---------------------------------------------------------------------------
// 3. debug.setmetatable(obj, mt) -- returns obj
// ---------------------------------------------------------------------------

pub fn db_setmetatable(state: &mut LuaState) -> LuaResult<u32> {
    if nargs(state) < 2 {
        return Err(bad_argument("setmetatable", 2, "nil or table expected"));
    }
    let obj = arg(state, 0);
    let mt_val = arg(state, 1);

    let mt_ref = match mt_val {
        Val::Nil => None,
        Val::Table(r) => Some(r),
        _ => return Err(bad_argument("setmetatable", 2, "nil or table expected")),
    };

    match obj {
        Val::Table(r) => {
            let t = state
                .gc
                .tables
                .get_mut(r)
                .ok_or_else(|| simple_error("table not found".into()))?;
            t.set_metatable(mt_ref);
        }
        Val::Userdata(r) => {
            let ud = state
                .gc
                .userdata
                .get_mut(r)
                .ok_or_else(|| simple_error("userdata not found".into()))?;
            ud.set_metatable(mt_ref);
        }
        _ => {
            let tag = crate::vm::metatable::type_tag(obj);
            state.gc.type_metatables[tag] = mt_ref;
        }
    }
    state.push(Val::Bool(true));
    Ok(1)
}

// ---------------------------------------------------------------------------
// 4. debug.getfenv(obj)
// ---------------------------------------------------------------------------

pub fn db_getfenv(state: &mut LuaState) -> LuaResult<u32> {
    if nargs(state) < 1 {
        return Err(bad_argument("getfenv", 1, "value expected"));
    }
    let val = arg(state, 0);
    let env = match val {
        Val::Function(r) => state.gc.closures.get(r).map(|cl| match cl {
            Closure::Lua(lcl) => lcl.env,
            Closure::Rust(_) => state.global,
        }),
        Val::Userdata(r) => {
            let e = state
                .gc
                .userdata
                .get(r)
                .and_then(crate::vm::value::Userdata::env);
            Some(e.unwrap_or(state.global))
        }
        Val::Thread(_) => Some(state.global),
        _ => Some(state.global),
    };
    state.push(Val::Table(env.unwrap_or(state.global)));
    Ok(1)
}

// ---------------------------------------------------------------------------
// 5. debug.setfenv(obj, table)
// ---------------------------------------------------------------------------

pub fn db_setfenv(state: &mut LuaState) -> LuaResult<u32> {
    if nargs(state) < 2 {
        return Err(bad_argument("setfenv", 2, "table expected"));
    }
    let obj = arg(state, 0);
    let Val::Table(new_env) = arg(state, 1) else {
        return Err(bad_argument("setfenv", 2, "table expected"));
    };

    match obj {
        Val::Function(r) => {
            let cl = state.gc.closures.get_mut(r).ok_or_else(|| {
                simple_error("'setfenv' cannot change environment of given object".into())
            })?;
            match cl {
                Closure::Lua(lcl) => lcl.env = new_env,
                Closure::Rust(_) => {
                    return Err(simple_error(
                        "'setfenv' cannot change environment of given object".into(),
                    ));
                }
            }
        }
        Val::Userdata(r) => {
            let ud = state.gc.userdata.get_mut(r).ok_or_else(|| {
                simple_error("'setfenv' cannot change environment of given object".into())
            })?;
            ud.set_env(Some(new_env));
        }
        _ => {
            return Err(simple_error(
                "'setfenv' cannot change environment of given object".into(),
            ));
        }
    }
    state.push(obj);
    Ok(1)
}

// ---------------------------------------------------------------------------
// 6. debug.getinfo([thread,] function [, what])
// ---------------------------------------------------------------------------

/// Extracted closure metadata for `getinfo`, avoiding borrow conflicts.
struct ClosureInfo {
    is_lua: bool,
    source: String,
    short_src: String,
    line_defined: i64,
    last_line_defined: i64,
    what: &'static str,
    nups: i64,
    name: String,
    line_info: Vec<u32>,
}

/// Extract closure metadata into an owned struct so we release the
/// immutable borrow on `state.gc.closures` before mutating state.
fn extract_closure_info(
    state: &LuaState,
    cl_ref: GcRef<crate::vm::closure::Closure>,
) -> Option<ClosureInfo> {
    let cl = state.gc.closures.get(cl_ref)?;
    Some(match cl {
        Closure::Lua(lcl) => ClosureInfo {
            is_lua: true,
            source: lcl.proto.source.clone(),
            short_src: chunkid(&lcl.proto.source),
            line_defined: i64::from(lcl.proto.line_defined),
            last_line_defined: i64::from(lcl.proto.last_line_defined),
            what: if lcl.proto.line_defined == 0 {
                "main"
            } else {
                "Lua"
            },
            nups: i64::from(lcl.proto.num_upvalues),
            name: String::new(),
            line_info: lcl.proto.line_info.clone(),
        },
        Closure::Rust(rcl) => ClosureInfo {
            is_lua: false,
            source: "=[C]".into(),
            short_src: "[C]".into(),
            line_defined: -1,
            last_line_defined: -1,
            what: "C",
            nups: rcl.upvalues.len() as i64,
            name: rcl.name.clone(),
            line_info: Vec::new(),
        },
    })
}

pub fn db_getinfo(state: &mut LuaState) -> LuaResult<u32> {
    let func_arg_idx = get_thread_offset(state);

    let options = if nargs(state) > func_arg_idx + 1 {
        match arg(state, func_arg_idx + 1) {
            Val::Str(r) => state.gc.string_arena.get(r).map_or_else(
                || "flnSu".into(),
                |s| String::from_utf8_lossy(s.data()).to_string(),
            ),
            _ => "flnSu".into(),
        }
    } else {
        "flnSu".into()
    };

    let func_val = arg(state, func_arg_idx);

    let (ci_idx, closure_ref) = match func_val {
        Val::Num(n) => {
            #[allow(clippy::cast_possible_truncation)]
            let level = n as usize;
            if level >= state.ci {
                state.push(Val::Nil);
                return Ok(1);
            }
            let target = state.ci - level;
            let func = state.stack_get(state.call_stack[target].func);
            let cl_ref = match func {
                Val::Function(r) => Some(r),
                _ => None,
            };
            (Some(target), cl_ref)
        }
        Val::Function(r) => (None, Some(r)),
        _ => {
            return Err(bad_argument(
                "getinfo",
                func_arg_idx + 1,
                "function or level expected",
            ));
        }
    };

    let info = closure_ref.and_then(|r| extract_closure_info(state, r));

    let result_table = state.gc.alloc_table(Table::new());

    if let Some(info) = &info {
        if options.contains('S') {
            set_table_str(state, result_table, "source", &info.source)?;
            set_table_str(state, result_table, "short_src", &info.short_src)?;
            set_table_int(state, result_table, "linedefined", info.line_defined)?;
            set_table_int(
                state,
                result_table,
                "lastlinedefined",
                info.last_line_defined,
            )?;
            set_table_str(state, result_table, "what", info.what)?;
        }

        if options.contains('l') {
            let line = ci_idx.map_or(-1, |ci| current_line(state, ci));
            set_table_int(state, result_table, "currentline", i64::from(line))?;
        }

        if options.contains('u') {
            set_table_int(state, result_table, "nups", info.nups)?;
        }

        if options.contains('n') {
            // Use getfuncname to resolve the name from the calling instruction.
            let (name, namewhat) = if let Some(ci) = ci_idx {
                if ci > 0 {
                    debug_info::getfuncname(state, ci, &state.gc.string_arena).map_or_else(
                        || (info.name.clone(), String::new()),
                        |(kind, n)| (n, kind.to_string()),
                    )
                } else {
                    (info.name.clone(), String::new())
                }
            } else {
                // Function passed directly (not a stack level).
                let namewhat = if info.name.is_empty() {
                    String::new()
                } else {
                    "global".to_string()
                };
                (info.name.clone(), namewhat)
            };
            set_table_str(state, result_table, "name", &name)?;
            set_table_str(state, result_table, "namewhat", &namewhat)?;
        }

        if let (true, Some(cl_ref)) = (options.contains('f'), closure_ref) {
            set_table_val(state, result_table, "func", Val::Function(cl_ref))?;
        }

        if options.contains('L') && info.is_lua {
            let lines_table = state.gc.alloc_table(Table::new());
            for &line in &info.line_info {
                let lt = state
                    .gc
                    .tables
                    .get_mut(lines_table)
                    .ok_or_else(|| simple_error("lines table not found".into()))?;
                lt.raw_set(
                    Val::Num(f64::from(line)),
                    Val::Bool(true),
                    &state.gc.string_arena,
                )?;
            }
            set_table_val(state, result_table, "activelines", Val::Table(lines_table))?;
        }
    }

    state.push(Val::Table(result_table));
    Ok(1)
}

/// Helper: set a string field in a table.
fn set_table_str(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: &str,
    value: &str,
) -> LuaResult<()> {
    let k = state.gc.intern_string(key.as_bytes());
    let v = state.gc.intern_string(value.as_bytes());
    let t = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| simple_error("table not found".into()))?;
    t.raw_set(Val::Str(k), Val::Str(v), &state.gc.string_arena)?;
    Ok(())
}

/// Helper: set an integer field in a table.
fn set_table_int(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: &str,
    value: i64,
) -> LuaResult<()> {
    let k = state.gc.intern_string(key.as_bytes());
    let t = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| simple_error("table not found".into()))?;
    t.raw_set(Val::Str(k), Val::Num(value as f64), &state.gc.string_arena)?;
    Ok(())
}

/// Helper: set an arbitrary Val field in a table.
fn set_table_val(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: &str,
    value: Val,
) -> LuaResult<()> {
    let k = state.gc.intern_string(key.as_bytes());
    let t = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| simple_error("table not found".into()))?;
    t.raw_set(Val::Str(k), value, &state.gc.string_arena)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 7. debug.getlocal([thread,] level, local)
// ---------------------------------------------------------------------------

pub fn db_getlocal(state: &mut LuaState) -> LuaResult<u32> {
    let arg_offset = get_thread_offset(state);

    let level = check_number(state, "getlocal", arg_offset)? as usize;
    let local_idx = check_number(state, "getlocal", arg_offset + 1)? as usize;

    if level >= state.ci {
        return Err(bad_argument(
            "getlocal",
            arg_offset + 1,
            "level out of range",
        ));
    }
    let target_ci = state.ci - level;

    let name = get_local_name(state, target_ci, local_idx);

    if let Some(name) = name {
        let ci_base = state.call_stack[target_ci].base;
        let stack_idx = ci_base + local_idx - 1;
        let val = state.stack_get(stack_idx);

        let name_ref = state.gc.intern_string(name.as_bytes());
        state.push(Val::Str(name_ref));
        state.push(val);
        Ok(2)
    } else {
        state.push(Val::Nil);
        Ok(1)
    }
}

// ---------------------------------------------------------------------------
// 8. debug.setlocal([thread,] level, local, value)
// ---------------------------------------------------------------------------

pub fn db_setlocal(state: &mut LuaState) -> LuaResult<u32> {
    let arg_offset = get_thread_offset(state);

    let level = check_number(state, "setlocal", arg_offset)? as usize;
    let local_idx = check_number(state, "setlocal", arg_offset + 1)? as usize;
    let new_val = arg(state, arg_offset + 2);

    if level >= state.ci {
        return Err(bad_argument(
            "setlocal",
            arg_offset + 1,
            "level out of range",
        ));
    }
    let target_ci = state.ci - level;

    let name = get_local_name(state, target_ci, local_idx);

    let name_ref = if let Some(name) = name {
        let ci_base = state.call_stack[target_ci].base;
        let stack_idx = ci_base + local_idx - 1;
        if stack_idx < state.stack.len() {
            state.stack[stack_idx] = new_val;
        }
        state.gc.intern_string(name.as_bytes())
    } else {
        return Ok(0);
    };
    state.push(Val::Str(name_ref));
    Ok(1)
}

// ---------------------------------------------------------------------------
// 9. debug.getupvalue(func, n)
// ---------------------------------------------------------------------------

pub fn db_getupvalue(state: &mut LuaState) -> LuaResult<u32> {
    let Val::Function(cl_ref) = arg(state, 0) else {
        return Err(bad_argument("getupvalue", 1, "function expected"));
    };
    let n = check_number(state, "getupvalue", 1)? as usize;

    let cl = state
        .gc
        .closures
        .get(cl_ref)
        .ok_or_else(|| simple_error("closure not found".into()))?;

    match cl {
        Closure::Lua(lcl) => {
            if n < 1 || n > lcl.upvalues.len() {
                return Ok(0);
            }
            let uv_name = if n <= lcl.proto.upvalue_names.len() {
                lcl.proto.upvalue_names[n - 1].clone()
            } else {
                String::new()
            };
            let uv_ref = lcl.upvalues[n - 1];

            let uv_val = state
                .gc
                .upvalues
                .get(uv_ref)
                .map_or(Val::Nil, |uv| uv.get(&state.stack));

            let name_ref = state.gc.intern_string(uv_name.as_bytes());
            state.push(Val::Str(name_ref));
            state.push(uv_val);
            Ok(2)
        }
        Closure::Rust(_) => Ok(0),
    }
}

// ---------------------------------------------------------------------------
// 10. debug.setupvalue(func, n, value)
// ---------------------------------------------------------------------------

pub fn db_setupvalue(state: &mut LuaState) -> LuaResult<u32> {
    let Val::Function(cl_ref) = arg(state, 0) else {
        return Err(bad_argument("setupvalue", 1, "function expected"));
    };
    let n = check_number(state, "setupvalue", 1)? as usize;
    let new_val = arg(state, 2);

    let is_lua = state.gc.closures.get(cl_ref).map_or(false, Closure::is_lua);

    if !is_lua {
        return Ok(0);
    }

    let (uv_name, uv_ref) = {
        let cl = state
            .gc
            .closures
            .get(cl_ref)
            .ok_or_else(|| simple_error("closure not found".into()))?;
        let lcl = cl
            .as_lua()
            .ok_or_else(|| simple_error("expected Lua closure".into()))?;
        if n < 1 || n > lcl.upvalues.len() {
            return Ok(0);
        }
        let name = if n <= lcl.proto.upvalue_names.len() {
            lcl.proto.upvalue_names[n - 1].clone()
        } else {
            String::new()
        };
        (name, lcl.upvalues[n - 1])
    };

    if let Some(uv) = state.gc.upvalues.get_mut(uv_ref) {
        uv.set(&mut state.stack, new_val);
    }

    let name_ref = state.gc.intern_string(uv_name.as_bytes());
    state.push(Val::Str(name_ref));
    Ok(1)
}

// ---------------------------------------------------------------------------
// 11. debug.gethook([thread]) -- stub
// ---------------------------------------------------------------------------

pub fn db_gethook(state: &mut LuaState) -> LuaResult<u32> {
    state.push(Val::Nil);
    let empty = state.gc.intern_string(b"");
    state.push(Val::Str(empty));
    state.push(Val::Num(0.0));
    Ok(3)
}

// ---------------------------------------------------------------------------
// 12. debug.sethook([thread,] hook, mask [, count]) -- stub
// ---------------------------------------------------------------------------

pub fn db_sethook(state: &mut LuaState) -> LuaResult<u32> {
    let _ = state;
    Ok(0)
}

// ---------------------------------------------------------------------------
// 13. debug.debug() -- stub (interactive mode)
// ---------------------------------------------------------------------------

pub fn db_debug(state: &mut LuaState) -> LuaResult<u32> {
    let _ = state;
    Ok(0)
}

// ---------------------------------------------------------------------------
// 14. debug.traceback([thread,] [message [, level]])
// ---------------------------------------------------------------------------

pub fn db_traceback(state: &mut LuaState) -> LuaResult<u32> {
    let arg_offset = get_thread_offset(state);

    // Level argument (arg_offset + 1).
    #[allow(clippy::cast_possible_truncation)]
    let start_level = if nargs(state) > arg_offset + 1 {
        match arg(state, arg_offset + 1) {
            Val::Num(n) => n as usize,
            _ => 1,
        }
    } else if arg_offset == 0 {
        1
    } else {
        0
    };

    // Message argument (arg_offset + 0).
    // PUC-Rio: lua_isstring returns true for BOTH strings and numbers.
    // - No args: empty prefix, build traceback
    // - String/number: use as message prefix with "\n" separator
    // - Other (including nil): return the value as-is
    let msg: Option<String> = if nargs(state) <= arg_offset {
        // No message argument at all: empty prefix.
        Some(String::new())
    } else {
        match arg(state, arg_offset) {
            Val::Str(r) => {
                let s = state
                    .gc
                    .string_arena
                    .get(r)
                    .map(|s| String::from_utf8_lossy(s.data()).to_string())
                    .unwrap_or_default();
                Some(s)
            }
            Val::Num(n) => Some(format!("{}", Val::Num(n))),
            other => {
                // Non-string, non-number (including nil): return as-is.
                state.push(other);
                return Ok(1);
            }
        }
    };

    let result = generate_traceback(state, msg.as_deref().unwrap_or(""), start_level);

    let result_ref = state.gc.intern_string(result.as_bytes());
    state.push(Val::Str(result_ref));
    Ok(1)
}
