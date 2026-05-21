//! Debug library: introspection, hooks, and stack inspection.
//!
//! Implements all 14 functions from PUC-Rio's `ldblib.c`:
//! getregistry, getmetatable, setmetatable, getfenv, setfenv,
//! getinfo, getlocal, setlocal, getupvalue, setupvalue,
//! gethook, sethook, debug, traceback.

use std::fmt::Write as _;

use crate::error::{LuaError, LuaResult, RuntimeError};
use crate::vm::callinfo::CallInfo;
use crate::vm::closure::Closure;
use crate::vm::debug_info;
use crate::vm::gc::arena::GcRef;
use crate::vm::state::{
    DebugInfoField, Gc, HookState, LuaState, MASK_CALL, MASK_COUNT, MASK_LINE, MASK_RET,
};
use crate::vm::string::LuaString;
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

#[inline]
fn clear_hook_state(hook: &mut HookState) {
    hook.hook_func = Val::Nil;
    hook.hook_mask = 0;
    hook.base_hook_count = 0;
    hook.hook_count = 0;
}

#[inline]
fn install_hook_state(hook: &mut HookState, hook_func: Val, mask: u8, count: i32) {
    hook.hook_func = hook_func;
    hook.hook_mask = mask;
    hook.base_hook_count = count;
    hook.hook_count = count;
}

#[inline]
fn parse_hook_mask(mask_bytes: &[u8], count: i32) -> u8 {
    let mut mask = 0;
    for byte in mask_bytes {
        match *byte {
            b'c' => mask |= MASK_CALL,
            b'r' => mask |= MASK_RET,
            b'l' => mask |= MASK_LINE,
            _ => {}
        }
    }
    if count > 0 {
        mask |= MASK_COUNT;
    }
    mask
}

// ---------------------------------------------------------------------------
/// Result of resolving a numeric stack level to a call stack position.
///
/// PUC-Rio's `lua_getstack` (ldebug.c:84-103) walks the CI chain and
/// subtracts `ci->tailcalls` from the level for Lua frames. If the level
/// hits zero, we found a real frame. If it goes negative, we landed on
/// a virtual "tail call" frame that was optimized away.
pub(crate) enum StackLevel {
    /// A real frame at the given CI index.
    Real(usize),
    /// A virtual tail call frame (the real frame was optimized away).
    TailCall,
}

/// Resolves a numeric stack level to a `StackLevel`, accounting for
/// tail calls.
///
/// Matches PUC-Rio's `lua_getstack` (ldebug.c:84-103):
/// ```c
/// for (ci = L->ci; level > 0 && ci > L->base_ci; ci--) {
///     level--;
///     if (f_isLua(ci))
///         level -= ci->tailcalls;
/// }
/// ```
///
/// The subtlety: the loop checks `f_isLua(ci)` on the *current* CI
/// before decrementing the pointer. Tail calls subtracted here represent
/// virtual frames that were folded into that CI slot.
pub(crate) fn resolve_stack_level(state: &LuaState, level: usize) -> Option<StackLevel> {
    resolve_stack_level_raw(&state.stack, &state.call_stack, state.ci, &state.gc, level)
}

/// Low-level stack level resolver that works with raw fields.
///
/// Used for both `LuaState` (main thread) and `LuaThread` (coroutines).
fn resolve_stack_level_raw(
    stack: &[Val],
    call_stack: &[CallInfo],
    ci: usize,
    _gc: &Gc,
    level: usize,
) -> Option<StackLevel> {
    let mut ci_idx = ci;
    let mut remaining = level as i64;

    // Matches PUC-Rio's for-loop exactly:
    //   for (ci = L->ci; level > 0 && ci > base_ci; ci--) {
    //       level--;
    //       if (f_isLua(ci)) level -= ci->tailcalls;
    //   }
    // Body executes with current `ci`, then ci is decremented.
    // Uses the cached `is_lua` flag instead of arena lookups.
    while remaining > 0 && ci_idx > 0 {
        remaining -= 1;
        if call_stack[ci_idx].is_lua {
            remaining -= i64::from(call_stack[ci_idx].tail_calls);
        }
        ci_idx -= 1;
    }

    // PUC-Rio: `if (level == 0 && ci > L->base_ci)` — CI[0] (base_ci)
    // is never a valid stack level target.
    //
    // However, PUC-Rio's main entry wraps execution in `lua_cpcall(&pmain)`,
    // which adds a C frame (CI[1]) above base_ci. That C frame shows as
    // `[C]: ?` at the bottom of tracebacks. rilua doesn't have a `pmain`
    // wrapper, so CI[0] serves this role when its function slot is Nil
    // (main thread sentinel). For coroutines, CI[0] holds the real body
    // function and must be excluded.
    if remaining == 0 && ci_idx > 0 {
        Some(StackLevel::Real(ci_idx))
    } else if remaining == 0 && ci_idx == 0 {
        // CI[0] is the base frame. Only return it as a valid level when
        // it doesn't hold a real function (main thread sentinel).
        let func_slot = call_stack[0].func;
        if func_slot < stack.len() && matches!(stack[func_slot], Val::Nil) {
            Some(StackLevel::Real(0))
        } else {
            None
        }
    } else if remaining < 0 {
        Some(StackLevel::TailCall)
    } else {
        None
    }
}

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
    generate_traceback_raw(
        &state.stack,
        &state.call_stack,
        state.ci,
        &state.gc,
        msg,
        start_level,
    )
}

/// Low-level traceback generator that works with raw fields.
///
/// Used for both `LuaState` (main thread) and `LuaThread` (coroutines).
fn generate_traceback_raw(
    stack: &[Val],
    call_stack: &[CallInfo],
    ci: usize,
    gc: &Gc,
    msg: &str,
    start_level: usize,
) -> String {
    const LEVELS1: usize = 12;
    const LEVELS2: usize = 10;
    let traceback = TracebackContext {
        stack,
        call_stack,
        ci,
        gc,
    };
    let mut result = String::with_capacity(estimate_traceback_capacity(msg, traceback));

    if !msg.is_empty() {
        result.push_str(msg);
        result.push('\n');
    }

    result.push_str("stack traceback:");

    if traceback.has_no_tail_calls() {
        append_traceback_without_tail_calls(&mut result, traceback, start_level, LEVELS1, LEVELS2);
        return result;
    }

    append_traceback_with_tail_calls(&mut result, traceback, start_level, LEVELS1, LEVELS2);
    result
}

#[derive(Clone, Copy)]
struct TracebackContext<'a> {
    stack: &'a [Val],
    call_stack: &'a [CallInfo],
    ci: usize,
    gc: &'a Gc,
}

impl TracebackContext<'_> {
    fn has_no_tail_calls(self) -> bool {
        self.call_stack
            .get(..=self.ci)
            .is_none_or(|frames| frames.iter().all(|frame| frame.tail_calls == 0))
    }

    fn count_frames_without_tail_calls(self) -> usize {
        let real_frames = self.ci;
        let has_main_thread_sentinel = self.call_stack.first().is_some_and(|frame| {
            frame.func < self.stack.len() && matches!(self.stack[frame.func], Val::Nil)
        });
        real_frames + usize::from(has_main_thread_sentinel)
    }

    fn ci_index(self, offset: usize) -> usize {
        self.ci.saturating_sub(offset)
    }
}

fn estimate_traceback_capacity(msg: &str, traceback: TracebackContext<'_>) -> usize {
    let frame_count = traceback
        .ci
        .min(traceback.call_stack.len().saturating_sub(1))
        + 1;
    msg.len() + 32 + frame_count * 48
}

fn append_traceback_without_tail_calls(
    result: &mut String,
    traceback: TracebackContext<'_>,
    start_level: usize,
    levels1: usize,
    levels2: usize,
) {
    let total_frames = traceback.count_frames_without_tail_calls();
    if start_level >= total_frames {
        return;
    }

    let remaining_frames = total_frames - start_level;
    let show_split = remaining_frames > levels1 + levels2;
    let first_frames = if show_split {
        levels1
    } else {
        remaining_frames
    };
    let skipped_frames = remaining_frames.saturating_sub(first_frames + levels2);

    append_traceback_frame_range(result, traceback, start_level, first_frames);

    if skipped_frames == 0 {
        return;
    }

    result.push_str("\n\t...");

    append_traceback_frame_range(result, traceback, total_frames - levels2, levels2);
}

fn append_traceback_frame_range(
    result: &mut String,
    traceback: TracebackContext<'_>,
    start: usize,
    len: usize,
) {
    for offset in start..start + len {
        result.push_str("\n\t");
        format_frame_line(
            result,
            traceback.stack,
            traceback.call_stack,
            traceback.ci_index(offset),
            traceback.gc,
        );
    }
}

fn append_traceback_with_tail_calls(
    result: &mut String,
    traceback: TracebackContext<'_>,
    start_level: usize,
    levels1: usize,
    levels2: usize,
) {
    let resolve = |level| {
        resolve_stack_level_raw(
            traceback.stack,
            traceback.call_stack,
            traceback.ci,
            traceback.gc,
            level,
        )
    };

    let mut level = start_level;
    let mut first_part = true;
    loop {
        let Some(stack_level) = resolve(level) else {
            break;
        };

        if level > levels1 && first_part {
            if resolve(level + levels2).is_some() {
                result.push_str("\n\t...");
                let mut probe = level + levels2;
                while resolve(probe).is_some() {
                    probe += 1;
                }
                level = probe - levels2;
                first_part = false;
                continue;
            }
            first_part = false;
        }

        result.push_str("\n\t");

        match stack_level {
            StackLevel::TailCall => {
                result.push_str("(tail call): ?");
            }
            StackLevel::Real(ci_idx) => {
                format_frame_line(
                    result,
                    traceback.stack,
                    traceback.call_stack,
                    ci_idx,
                    traceback.gc,
                );
            }
        }

        level += 1;
    }
}

/// Formats a single traceback frame line for a real CI frame.
fn format_frame_line(
    result: &mut String,
    stack: &[Val],
    call_stack: &[CallInfo],
    ci_idx: usize,
    gc: &Gc,
) {
    let ci = &call_stack[ci_idx];
    let func_val = if ci.func < stack.len() {
        stack[ci.func]
    } else {
        Val::Nil
    };

    // Resolve function name using the raw variant (works for both
    // main thread and coroutine threads).
    let func_name = debug_info::getfuncname_raw(call_stack, stack, gc, ci_idx, &gc.string_arena);

    if let Val::Function(r) = func_val {
        if let Some(cl) = gc.closures.get(r) {
            match cl {
                Closure::Lua(lcl) => {
                    result.push_str(&lcl.proto.short_source);
                    result.push(':');
                    let line = current_line_raw(call_stack, stack, gc, ci_idx);
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
                            lcl.proto.short_source, lcl.proto.line_defined
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
}

/// `current_line` that works with raw fields (no `&LuaState` needed).
fn current_line_raw(call_stack: &[CallInfo], stack: &[Val], gc: &Gc, ci_idx: usize) -> i32 {
    let ci = &call_stack[ci_idx];
    let func_val = if ci.func < stack.len() {
        stack[ci.func]
    } else {
        return -1;
    };
    if let Val::Function(r) = func_val
        && let Some(Closure::Lua(lcl)) = gc.closures.get(r)
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

/// Like `get_local_name` but works with raw fields (for coroutines).
fn get_local_name_raw(
    call_stack: &[CallInfo],
    stack: &[Val],
    gc: &Gc,
    ci_idx: usize,
    local_number: usize,
) -> Option<String> {
    let ci = &call_stack[ci_idx];
    let func_val = if ci.func < stack.len() {
        stack[ci.func]
    } else {
        return None;
    };
    if let Val::Function(r) = func_val
        && let Some(Closure::Lua(lcl)) = gc.closures.get(r)
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
            Closure::Rust(rcl) => rcl.env.unwrap_or(state.global),
        }),
        Val::Userdata(r) => {
            let e = state
                .gc
                .userdata
                .get(r)
                .and_then(crate::vm::value::Userdata::env);
            Some(e.unwrap_or(state.global))
        }
        Val::Thread(r) => {
            // Active thread uses state.global; suspended/initial uses stored global.
            if state.current_thread == Some(r) {
                Some(state.global)
            } else {
                state.gc.threads.get(r).map(|t| t.global)
            }
        }
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
                Closure::Rust(rcl) => rcl.env = Some(new_env),
            }
        }
        Val::Userdata(r) => {
            let ud = state.gc.userdata.get_mut(r).ok_or_else(|| {
                simple_error("'setfenv' cannot change environment of given object".into())
            })?;
            ud.set_env(Some(new_env));
        }
        Val::Thread(r) => {
            // For the active thread, set state.global directly.
            // For a suspended/initial thread, set its stored global.
            if state.current_thread == Some(r) {
                state.global = new_env;
            } else if let Some(thread) = state.gc.threads.get_mut(r) {
                thread.global = new_env;
            } else {
                return Err(simple_error(
                    "'setfenv' cannot change environment of given object".into(),
                ));
            }
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

fn getinfo_field_count(options: &str) -> usize {
    let mut count = 0;

    if options.contains('S') {
        count += 5;
    }
    if options.contains('l') {
        count += 1;
    }
    if options.contains('u') {
        count += 1;
    }
    if options.contains('n') {
        count += 2;
    }
    if options.contains('f') {
        count += 1;
    }
    if options.contains('L') {
        count += 1;
    }

    count
}

/// Extract closure metadata into an owned struct so we release the
/// immutable borrow on `state.gc.closures` before mutating state.
fn extract_closure_info(
    state: &LuaState,
    cl_ref: GcRef<crate::vm::closure::Closure>,
    need_activelines: bool,
) -> Option<ClosureInfo> {
    let cl = state.gc.closures.get(cl_ref)?;
    Some(match cl {
        Closure::Lua(lcl) => ClosureInfo {
            is_lua: true,
            source: lcl.proto.source.clone(),
            short_src: lcl.proto.short_source.clone(),
            line_defined: i64::from(lcl.proto.line_defined),
            last_line_defined: i64::from(lcl.proto.last_line_defined),
            what: if lcl.proto.line_defined == 0 {
                "main"
            } else {
                "Lua"
            },
            nups: i64::from(lcl.proto.num_upvalues),
            name: String::new(),
            line_info: if need_activelines {
                lcl.proto.line_info.clone()
            } else {
                Vec::new()
            },
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

    // Determine which thread to inspect.
    let co_ref = if func_arg_idx == 1 {
        if let Val::Thread(r) = arg(state, 0) {
            Some(r)
        } else {
            None
        }
    } else {
        None
    };

    // Resolve the argument to a (ci_idx, closure_ref) pair, or a tail call.
    // When a coroutine is passed, resolve against its CI stack.
    let (ci_idx, closure_ref, is_tail_call) = match func_val {
        Val::Num(n) => {
            #[allow(clippy::cast_possible_truncation)]
            let level = n as usize;

            let resolved = if let Some(cr) = co_ref {
                if let Some(thread) = state.gc.threads.get(cr) {
                    resolve_stack_level_raw(
                        &thread.stack,
                        &thread.call_stack,
                        thread.ci,
                        &state.gc,
                        level,
                    )
                } else {
                    resolve_stack_level(state, level)
                }
            } else {
                resolve_stack_level(state, level)
            };

            match resolved {
                Some(StackLevel::Real(target)) => {
                    // Get the function from the correct thread's stack.
                    let func = if let Some(cr) = co_ref {
                        state.gc.threads.get(cr).map_or(Val::Nil, |t| {
                            let idx = t.call_stack[target].func;
                            if idx < t.stack.len() {
                                t.stack[idx]
                            } else {
                                Val::Nil
                            }
                        })
                    } else {
                        state.stack_get(state.call_stack[target].func)
                    };
                    let cl_ref = match func {
                        Val::Function(r) => Some(r),
                        _ => None,
                    };
                    (Some(target), cl_ref, false)
                }
                Some(StackLevel::TailCall) => (None, None, true),
                None => {
                    state.push(Val::Nil);
                    return Ok(1);
                }
            }
        }
        Val::Function(r) => (None, Some(r), false),
        _ => {
            return Err(bad_argument(
                "getinfo",
                func_arg_idx + 1,
                "function or level expected",
            ));
        }
    };

    let result_table = state
        .gc
        .alloc_table(Table::with_sizes(0, getinfo_field_count(&options)));

    // PUC-Rio: when ar->i_ci == 0 (tail call), info_tailcall fills in
    // placeholder values and returns immediately.
    if is_tail_call {
        fill_tail_call_info(state, result_table, &options)?;
        state.push(Val::Table(result_table));
        return Ok(1);
    }

    let info = closure_ref.and_then(|r| extract_closure_info(state, r, options.contains('L')));

    if let Some(info) = &info {
        if options.contains('S') {
            set_table_str(state, result_table, DebugInfoField::Source, &info.source)?;
            set_table_str(
                state,
                result_table,
                DebugInfoField::ShortSrc,
                &info.short_src,
            )?;
            set_table_int(
                state,
                result_table,
                DebugInfoField::LineDefined,
                info.line_defined,
            )?;
            set_table_int(
                state,
                result_table,
                DebugInfoField::LastLineDefined,
                info.last_line_defined,
            )?;
            set_table_str(state, result_table, DebugInfoField::What, info.what)?;
        }

        if options.contains('l') {
            let line = ci_idx.map_or(-1, |ci_i| {
                if let Some(cr) = co_ref {
                    state.gc.threads.get(cr).map_or(-1, |t| {
                        current_line_raw(&t.call_stack, &t.stack, &state.gc, ci_i)
                    })
                } else {
                    current_line(state, ci_i)
                }
            });
            set_table_int(
                state,
                result_table,
                DebugInfoField::CurrentLine,
                i64::from(line),
            )?;
        }

        if options.contains('u') {
            set_table_int(state, result_table, DebugInfoField::Nups, info.nups)?;
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
            // PUC-Rio: only sets "name" if non-NULL; nil otherwise.
            if name.is_empty() {
                set_table_val(state, result_table, DebugInfoField::Name, Val::Nil)?;
            } else {
                set_table_str(state, result_table, DebugInfoField::Name, &name)?;
            }
            set_table_str(state, result_table, DebugInfoField::NameWhat, &namewhat)?;
        }

        if let (true, Some(cl_ref)) = (options.contains('f'), closure_ref) {
            set_table_val(
                state,
                result_table,
                DebugInfoField::Func,
                Val::Function(cl_ref),
            )?;
        }

        if options.contains('L') && info.is_lua {
            let lines_table = state
                .gc
                .alloc_table(Table::with_sizes(info.line_info.len(), 0));
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
            set_table_val(
                state,
                result_table,
                DebugInfoField::ActiveLines,
                Val::Table(lines_table),
            )?;
        }
    }

    state.push(Val::Table(result_table));
    Ok(1)
}

/// Fills a result table with tail call placeholder info.
///
/// Matches PUC-Rio's `info_tailcall` (ldebug.c:167-174).
fn fill_tail_call_info(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    options: &str,
) -> LuaResult<()> {
    if options.contains('S') {
        set_table_str(state, table_ref, DebugInfoField::Source, "=(tail call)")?;
        set_table_str(state, table_ref, DebugInfoField::ShortSrc, "(tail call)")?;
        set_table_int(state, table_ref, DebugInfoField::LineDefined, -1)?;
        set_table_int(state, table_ref, DebugInfoField::LastLineDefined, -1)?;
        set_table_str(state, table_ref, DebugInfoField::What, "tail")?;
    }
    if options.contains('l') {
        set_table_int(state, table_ref, DebugInfoField::CurrentLine, -1)?;
    }
    if options.contains('u') {
        set_table_int(state, table_ref, DebugInfoField::Nups, 0)?;
    }
    if options.contains('n') {
        set_table_str(state, table_ref, DebugInfoField::Name, "")?;
        set_table_str(state, table_ref, DebugInfoField::NameWhat, "")?;
    }
    if options.contains('f') {
        // func is nil for tail call frames.
        set_table_val(state, table_ref, DebugInfoField::Func, Val::Nil)?;
    }
    Ok(())
}

/// Helper: set a string field in a table.
fn set_table_str(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: DebugInfoField,
    value: &str,
) -> LuaResult<()> {
    let k = state.debug_info_field_key(key);
    let v = intern_debug_info_value(state, value);
    let t = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| simple_error("table not found".into()))?;
    t.raw_set(Val::Str(k), Val::Str(v), &state.gc.string_arena)?;
    state.gc.barrier_back(table_ref);
    Ok(())
}

fn intern_debug_info_value(state: &mut LuaState, value: &str) -> GcRef<LuaString> {
    match value {
        "" => state.gc.intern_string_static(b""),
        "Lua" => state.gc.intern_string_static(b"Lua"),
        "main" => state.gc.intern_string_static(b"main"),
        "C" => state.gc.intern_string_static(b"C"),
        "tail" => state.gc.intern_string_static(b"tail"),
        "global" => state.gc.intern_string_static(b"global"),
        "local" => state.gc.intern_string_static(b"local"),
        "field" => state.gc.intern_string_static(b"field"),
        "method" => state.gc.intern_string_static(b"method"),
        "=(string)" => state.gc.intern_string_static(b"=(string)"),
        "(string)" => state.gc.intern_string_static(b"(string)"),
        "=[C]" => state.gc.intern_string_static(b"=[C]"),
        "[C]" => state.gc.intern_string_static(b"[C]"),
        "=(tail call)" => state.gc.intern_string_static(b"=(tail call)"),
        "(tail call)" => state.gc.intern_string_static(b"(tail call)"),
        _ => state.gc.intern_string(value.as_bytes()),
    }
}

/// Helper: set an integer field in a table.
fn set_table_int(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: DebugInfoField,
    value: i64,
) -> LuaResult<()> {
    let k = state.debug_info_field_key(key);
    let t = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| simple_error("table not found".into()))?;
    t.raw_set(Val::Str(k), Val::Num(value as f64), &state.gc.string_arena)?;
    state.gc.barrier_back(table_ref);
    Ok(())
}

/// Helper: set an arbitrary Val field in a table.
fn set_table_val(
    state: &mut LuaState,
    table_ref: GcRef<Table>,
    key: DebugInfoField,
    value: Val,
) -> LuaResult<()> {
    let k = state.debug_info_field_key(key);
    let t = state
        .gc
        .tables
        .get_mut(table_ref)
        .ok_or_else(|| simple_error("table not found".into()))?;
    t.raw_set(Val::Str(k), value, &state.gc.string_arena)?;
    state.gc.barrier_back(table_ref);
    Ok(())
}

// ---------------------------------------------------------------------------
// 7. debug.getlocal([thread,] level, local)
// ---------------------------------------------------------------------------

pub fn db_getlocal(state: &mut LuaState) -> LuaResult<u32> {
    let arg_offset = get_thread_offset(state);

    let level = check_number(state, "getlocal", arg_offset)? as usize;
    let local_idx = check_number(state, "getlocal", arg_offset + 1)? as usize;

    // Determine which thread to inspect.
    let co_ref = if arg_offset == 1 {
        if let Val::Thread(r) = arg(state, 0) {
            Some(r)
        } else {
            None
        }
    } else {
        None
    };

    // Resolve level against the appropriate thread's CI stack.
    let resolved = if let Some(cr) = co_ref {
        if let Some(thread) = state.gc.threads.get(cr) {
            resolve_stack_level_raw(
                &thread.stack,
                &thread.call_stack,
                thread.ci,
                &state.gc,
                level,
            )
        } else {
            resolve_stack_level(state, level)
        }
    } else {
        resolve_stack_level(state, level)
    };

    let target_ci = match resolved {
        Some(StackLevel::Real(ci)) => ci,
        Some(StackLevel::TailCall) | None => {
            state.push(Val::Nil);
            return Ok(1);
        }
    };

    // Get local name and value from the correct thread.
    if let Some(cr) = co_ref {
        // Coroutine path: read from the thread's stack/call_stack.
        if let Some(thread) = state.gc.threads.get(cr) {
            let name = get_local_name_raw(
                &thread.call_stack,
                &thread.stack,
                &state.gc,
                target_ci,
                local_idx,
            );
            let ci_base = thread.call_stack[target_ci].base;
            let stack_idx = ci_base + local_idx - 1;

            let limit = if target_ci == thread.ci {
                thread.top
            } else if target_ci + 1 < thread.call_stack.len() {
                thread.call_stack[target_ci + 1].func
            } else {
                thread.top
            };

            if let Some(name) = name {
                let val = if stack_idx < thread.stack.len() {
                    thread.stack[stack_idx]
                } else {
                    Val::Nil
                };
                let name_ref = state.gc.intern_string(name.as_bytes());
                state.push(Val::Str(name_ref));
                state.push(val);
                return Ok(2);
            } else if local_idx > 0 && stack_idx < limit {
                let val = if stack_idx < thread.stack.len() {
                    thread.stack[stack_idx]
                } else {
                    Val::Nil
                };
                let name_ref = state.gc.intern_string_static(b"(*temporary)");
                state.push(Val::Str(name_ref));
                state.push(val);
                return Ok(2);
            }
            state.push(Val::Nil);
            return Ok(1);
        }
        // Fallthrough: invalid thread ref, treat as nil.
        state.push(Val::Nil);
        return Ok(1);
    }

    // Main thread path.
    let name = get_local_name(state, target_ci, local_idx);

    let ci_base = state.call_stack[target_ci].base;
    let stack_idx = ci_base + local_idx - 1;

    let limit = if target_ci == state.ci {
        state.top
    } else if target_ci + 1 < state.call_stack.len() {
        state.call_stack[target_ci + 1].func
    } else {
        state.top
    };

    if let Some(name) = name {
        let val = state.stack_get(stack_idx);
        let name_ref = state.gc.intern_string(name.as_bytes());
        state.push(Val::Str(name_ref));
        state.push(val);
        Ok(2)
    } else if local_idx > 0 && stack_idx < limit {
        let val = state.stack_get(stack_idx);
        let name_ref = state.gc.intern_string_static(b"(*temporary)");
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

    // Determine which thread to inspect.
    let co_ref = if arg_offset == 1 {
        if let Val::Thread(r) = arg(state, 0) {
            Some(r)
        } else {
            None
        }
    } else {
        None
    };

    let resolved = if let Some(cr) = co_ref {
        if let Some(thread) = state.gc.threads.get(cr) {
            resolve_stack_level_raw(
                &thread.stack,
                &thread.call_stack,
                thread.ci,
                &state.gc,
                level,
            )
        } else {
            resolve_stack_level(state, level)
        }
    } else {
        resolve_stack_level(state, level)
    };

    let target_ci = match resolved {
        Some(StackLevel::Real(ci)) => ci,
        Some(StackLevel::TailCall) | None => {
            return Ok(0);
        }
    };

    // Coroutine path: modify the thread's stack.
    // Split into two phases to avoid borrow conflicts:
    // 1. Read-only: resolve name and compute stack index.
    // 2. Mutable: set the value.
    if let Some(cr) = co_ref {
        let info = state.gc.threads.get(cr).map(|thread| {
            let name = get_local_name_raw(
                &thread.call_stack,
                &thread.stack,
                &state.gc,
                target_ci,
                local_idx,
            );
            let ci_base = thread.call_stack[target_ci].base;
            let stack_idx = ci_base + local_idx - 1;
            let limit = if target_ci == thread.ci {
                thread.top
            } else if target_ci + 1 < thread.call_stack.len() {
                thread.call_stack[target_ci + 1].func
            } else {
                thread.top
            };
            (name, stack_idx, limit)
        });

        if let Some((name, stack_idx, limit)) = info {
            let name_ref = if let Some(name) = name {
                // Mutable phase: set the value.
                if let Some(thread) = state.gc.threads.get_mut(cr)
                    && stack_idx < thread.stack.len()
                {
                    thread.stack[stack_idx] = new_val;
                }
                state.gc.intern_string(name.as_bytes())
            } else if local_idx > 0 && stack_idx < limit {
                if let Some(thread) = state.gc.threads.get_mut(cr)
                    && stack_idx < thread.stack.len()
                {
                    thread.stack[stack_idx] = new_val;
                }
                state.gc.intern_string_static(b"(*temporary)")
            } else {
                return Ok(0);
            };
            state.push(Val::Str(name_ref));
            return Ok(1);
        }
        return Ok(0);
    }

    // Main thread path.
    let name = get_local_name(state, target_ci, local_idx);

    let ci_base = state.call_stack[target_ci].base;
    let stack_idx = ci_base + local_idx - 1;

    let limit = if target_ci == state.ci {
        state.top
    } else if target_ci + 1 < state.call_stack.len() {
        state.call_stack[target_ci + 1].func
    } else {
        state.top
    };

    let name_ref = if let Some(name) = name {
        if stack_idx < state.stack.len() {
            state.stack[stack_idx] = new_val;
        }
        state.gc.intern_string(name.as_bytes())
    } else if local_idx > 0 && stack_idx < limit {
        if stack_idx < state.stack.len() {
            state.stack[stack_idx] = new_val;
        }
        state.gc.intern_string_static(b"(*temporary)")
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

    let is_lua = state.gc.closures.get(cl_ref).is_some_and(Closure::is_lua);

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
// 11. debug.gethook([thread])
// ---------------------------------------------------------------------------

pub fn db_gethook(state: &mut LuaState) -> LuaResult<u32> {
    let arg_offset = get_thread_offset(state);

    // Determine which thread's hook to query.
    let (hook_func, hook_mask, base_hook_count) = if arg_offset == 1 {
        // Thread argument provided.
        if let Val::Thread(co_ref) = arg(state, 0) {
            // If this thread is currently running, use state.hook.
            if state.current_thread == Some(co_ref) {
                (
                    state.hook.hook_func,
                    state.hook.hook_mask,
                    state.hook.base_hook_count,
                )
            } else if let Some(thread) = state.gc.threads.get(co_ref) {
                (
                    thread.hook.hook_func,
                    thread.hook.hook_mask,
                    thread.hook.base_hook_count,
                )
            } else {
                (Val::Nil, 0, 0)
            }
        } else {
            (Val::Nil, 0, 0)
        }
    } else {
        // No thread argument: use the current (main) thread's hook state.
        (
            state.hook.hook_func,
            state.hook.hook_mask,
            state.hook.base_hook_count,
        )
    };

    // Push the hook function (or nil).
    if hook_func.is_nil() || hook_mask == 0 {
        state.push(Val::Nil);
    } else {
        state.push(hook_func);
    }

    state.push(state.hook_mask_string(hook_mask));

    // Push the count.
    #[allow(clippy::cast_precision_loss)]
    state.push(Val::Num(f64::from(base_hook_count)));

    Ok(3)
}

// ---------------------------------------------------------------------------
// 12. debug.sethook([thread,] hook, mask [, count])
// ---------------------------------------------------------------------------

pub fn db_sethook(state: &mut LuaState) -> LuaResult<u32> {
    let arg_offset = get_thread_offset(state);
    let hook_arg = arg(state, arg_offset);

    if hook_arg.is_nil() || nargs(state) <= arg_offset {
        // Turn off hooks. Matches PUC-Rio: `lua_isnoneornil(L, arg+1)`.
        if arg_offset == 1 {
            // Thread variant.
            if let Val::Thread(co_ref) = arg(state, 0) {
                if state.current_thread == Some(co_ref) {
                    clear_hook_state(&mut state.hook);
                } else if let Some(thread) = state.gc.threads.get_mut(co_ref) {
                    clear_hook_state(&mut thread.hook);
                }
            }
        } else {
            clear_hook_state(&mut state.hook);
        }
        return Ok(0);
    }

    // Parse mask string.
    let mask_bytes = match arg(state, arg_offset + 1) {
        Val::Str(r) => state
            .gc
            .string_arena
            .get(r)
            .map_or_else(<&[u8]>::default, crate::vm::string::LuaString::data),
        _ => return Err(bad_argument("sethook", arg_offset + 2, "string expected")),
    };

    // Parse optional count.
    #[allow(clippy::cast_possible_truncation)]
    let count = if nargs(state) > arg_offset + 2 {
        match arg(state, arg_offset + 2) {
            Val::Num(n) => n as i32,
            _ => 0,
        }
    } else {
        0
    };

    // Validate that hook is a function.
    if !matches!(hook_arg, Val::Function(_)) {
        return Err(bad_argument("sethook", arg_offset + 1, "function expected"));
    }

    let mask = parse_hook_mask(mask_bytes, count);

    // Apply to the appropriate thread.
    if arg_offset == 1 {
        if let Val::Thread(co_ref) = arg(state, 0) {
            if state.current_thread == Some(co_ref) {
                install_hook_state(&mut state.hook, hook_arg, mask, count);
            } else if let Some(thread) = state.gc.threads.get_mut(co_ref) {
                install_hook_state(&mut thread.hook, hook_arg, mask, count);
            }
        }
    } else {
        install_hook_state(&mut state.hook, hook_arg, mask, count);
    }

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
    } else {
        usize::from(arg_offset == 0)
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

    let msg_str = msg.as_deref().unwrap_or("");

    let result = if arg_offset == 1 {
        // Thread argument: generate traceback from the coroutine's state.
        if let Val::Thread(co_ref) = arg(state, 0) {
            if let Some(thread) = state.gc.threads.get(co_ref) {
                generate_traceback_raw(
                    &thread.stack,
                    &thread.call_stack,
                    thread.ci,
                    &state.gc,
                    msg_str,
                    start_level,
                )
            } else {
                generate_traceback(state, msg_str, start_level)
            }
        } else {
            generate_traceback(state, msg_str, start_level)
        }
    } else {
        generate_traceback(state, msg_str, start_level)
    };

    let result_ref = state.gc.intern_string(result.as_bytes());
    state.push(Val::Str(result_ref));
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::Lua;
    use crate::api::{LuaApi, LuaApiMut};
    use crate::stdlib::StdLib;

    fn new_lua() -> Lua {
        Lua::new_with(StdLib::BASE | StdLib::DEBUG).expect("failed to create Lua state")
    }

    fn decode_string(lua: &Lua, value: Val) -> String {
        match value {
            Val::Str(string_ref) => lua
                .state()
                .gc
                .string_arena
                .get(string_ref)
                .map(|s| String::from_utf8_lossy(s.data()).into_owned())
                .expect("missing string ref"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    fn global_bool(lua: &mut Lua, name: &str) -> bool {
        match lua.get_global_val(name) {
            Val::Bool(value) => value,
            other => panic!("expected boolean in {name}, got {other:?}"),
        }
    }

    fn global_number(lua: &mut Lua, name: &str) -> f64 {
        match lua.get_global_val(name) {
            Val::Num(value) => value,
            other => panic!("expected number in {name}, got {other:?}"),
        }
    }

    fn global_string(lua: &mut Lua, name: &str) -> String {
        let value = lua.get_global_val(name);
        decode_string(lua, value)
    }

    #[test]
    fn metatable_helpers_bypass_protection_and_support_type_metatables() {
        let mut lua = new_lua();

        lua.exec(
            r#"
            local mt = { __metatable = "protected" }
            local t = {}
            set_ok = debug.setmetatable(t, mt)
            raw_meta_same = debug.getmetatable(t) == mt
            protected_meta = getmetatable(t) == "protected"

            local num_mt = {}
            number_set_ok = debug.setmetatable(1, num_mt)
            number_meta_same = debug.getmetatable(1) == num_mt
            debug.setmetatable(1, nil)
            "#,
        )
        .expect("metatable helper script failed");

        assert!(global_bool(&mut lua, "set_ok"));
        assert!(global_bool(&mut lua, "raw_meta_same"));
        assert!(global_bool(&mut lua, "protected_meta"));
        assert!(global_bool(&mut lua, "number_set_ok"));
        assert!(global_bool(&mut lua, "number_meta_same"));
    }

    #[test]
    fn getinfo_reports_function_metadata_and_active_lines() {
        let mut lua = new_lua();

        lua.exec(
            r#"
            function sample(a)
              local info = debug.getinfo(sample, "SufL")
              info_source = info.source
              info_what = info.what
              info_nups = info.nups
              info_func_same = info.func == sample
              info_has_lines = type(info.activelines) == "table"
              info_line_defined_positive = info.linedefined > 0
              info_last_line_defined_valid = info.lastlinedefined >= info.linedefined
            end

            function caller()
              local info = debug.getinfo(1, "n")
              level_name = info.name
              level_namewhat = info.namewhat
            end

            sample(1)
            caller()
            "#,
        )
        .expect("getinfo script failed");

        assert_eq!(global_string(&mut lua, "info_source"), "=(string)");
        assert_eq!(global_string(&mut lua, "info_what"), "Lua");
        assert_eq!(global_number(&mut lua, "info_nups"), 0.0);
        assert!(global_bool(&mut lua, "info_func_same"));
        assert!(global_bool(&mut lua, "info_has_lines"));
        assert!(global_bool(&mut lua, "info_line_defined_positive"));
        assert!(global_bool(&mut lua, "info_last_line_defined_valid"));
        assert_eq!(global_string(&mut lua, "level_name"), "caller");
        assert_eq!(global_string(&mut lua, "level_namewhat"), "global");
    }

    #[test]
    fn getinfo_without_l_option_skips_activelines_table() {
        let mut lua = new_lua();

        lua.exec(
            r#"
            function sample()
              local info = debug.getinfo(sample, "Sufn")
              info_has_active_lines = info.activelines ~= nil
              info_has_func = info.func == sample
              info_namewhat_type = type(info.namewhat)
            end

            sample()
            "#,
        )
        .expect("getinfo without L script failed");

        assert!(!global_bool(&mut lua, "info_has_active_lines"));
        assert!(global_bool(&mut lua, "info_has_func"));
        assert_eq!(global_string(&mut lua, "info_namewhat_type"), "string");
    }

    #[test]
    fn locals_and_upvalues_expose_live_names_and_values() {
        let mut lua = new_lua();

        lua.exec(
            r#"
            function make_probe(seed)
              local captured = seed + 1
              local probe
              probe = function(arg)
                local local_name, local_val = debug.getlocal(1, 1)
                local missing_local_name = select(1, debug.getlocal(1, 99))

                local found_name, found_val = nil, nil
                local _ = captured
                for i = 1, 10 do
                  local name, value = debug.getupvalue(probe, i)
                  if name == "captured" then
                    found_name, found_val = name, value
                  end
                end

                _G.local_name = local_name
                _G.local_val = local_val
                _G.missing_local_name = missing_local_name
                _G.upvalue_name = found_name
                _G.upvalue_val = found_val
                _G.missing_rust_upvalue = select(1, debug.getupvalue(print, 1))
              end
              return probe
            end

            make_probe(4)(9)
            "#,
        )
        .expect("locals/upvalues script failed");

        assert_eq!(global_string(&mut lua, "local_name"), "arg");
        assert_eq!(global_number(&mut lua, "local_val"), 9.0);
        assert_eq!(lua.get_global_val("missing_local_name"), Val::Nil);
        assert_eq!(global_string(&mut lua, "upvalue_name"), "captured");
        assert_eq!(global_number(&mut lua, "upvalue_val"), 5.0);
        assert_eq!(lua.get_global_val("missing_rust_upvalue"), Val::Nil);
    }

    #[test]
    fn traceback_includes_message_and_frame_names() {
        let mut lua = new_lua();

        lua.exec(
            r#"
            function alpha()
              local value = beta()
              return value
            end

            function beta()
              return debug.traceback("boom", 1)
            end

            trace = alpha()
            passthrough_nil = debug.traceback(nil)
            "#,
        )
        .expect("traceback script failed");

        let trace = global_string(&mut lua, "trace");
        assert!(trace.starts_with("boom\nstack traceback:"), "got: {trace}");
        assert!(trace.contains("in function 'beta'"), "got: {trace}");
        assert!(trace.contains("in function 'alpha'"), "got: {trace}");
        assert_eq!(lua.get_global_val("passthrough_nil"), Val::Nil);
    }

    #[test]
    fn traceback_elides_middle_frames_for_long_stacks() {
        let mut lua = new_lua();

        lua.exec(
            r#"
            local function recurse(n)
              if n > 0 then
                local value = recurse(n - 1)
                return value
              end
              return debug.traceback("boom", 1)
            end

            trace = recurse(24)
            "#,
        )
        .expect("long traceback script failed");

        let trace = global_string(&mut lua, "trace");
        assert!(trace.contains("\n\t..."), "got: {trace}");
        assert!(trace.contains("in function 'recurse'"), "got: {trace}");
        assert!(trace.contains("in main chunk"), "got: {trace}");
    }

    #[test]
    fn debug_stub_remains_a_noop() {
        let mut lua = new_lua();

        lua.exec(
            r"
            local results = { debug.debug() }
            stub_result_count = #results
            ",
        )
        .expect("debug.debug script failed");

        assert_eq!(global_number(&mut lua, "stub_result_count"), 0.0);
    }

    #[test]
    fn sethook_and_gethook_roundtrip_state() {
        let mut lua = new_lua();

        lua.exec(
            r#"
            local function hook(event, line)
              return event, line
            end

            debug.sethook(hook, "lcr", 7)
            local current_hook, mask, count = debug.gethook()
            hook_same = current_hook == hook
            hook_mask = mask
            hook_count = count

            debug.sethook()
            local off_hook, off_mask, off_count = debug.gethook()
            hook_cleared = off_hook == nil
            cleared_mask = off_mask
            cleared_count = off_count
            "#,
        )
        .expect("sethook/gethook script failed");

        assert!(global_bool(&mut lua, "hook_same"));
        assert_eq!(global_string(&mut lua, "hook_mask"), "crl");
        assert_eq!(global_number(&mut lua, "hook_count"), 7.0);
        assert!(global_bool(&mut lua, "hook_cleared"));
        assert_eq!(global_string(&mut lua, "cleared_mask"), "");
        assert_eq!(global_number(&mut lua, "cleared_count"), 0.0);
    }

    #[test]
    fn getinfo_f_field_returns_same_function_handle() {
        let mut lua = new_lua();

        lua.exec(
            r#"
            function sample()
              return 1
            end

            info_func = debug.getinfo(sample, "f").func
            "#,
        )
        .expect("getinfo f-field script failed");

        let info_func: crate::handles::Function = lua.global("info_func").expect("missing func");
        let sample: crate::handles::Function = lua.global("sample").expect("missing sample");
        assert_eq!(info_func, sample);
    }

    #[test]
    fn getinfo_field_count_matches_option_sets() {
        assert_eq!(getinfo_field_count(""), 0);
        assert_eq!(getinfo_field_count("S"), 5);
        assert_eq!(getinfo_field_count("ln"), 3);
        assert_eq!(getinfo_field_count("flnSuL"), 11);
    }
}
