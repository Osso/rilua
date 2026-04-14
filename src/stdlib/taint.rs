//! WoW taint tracking API functions.
//!
//! Implements Blizzard's security/taint system: per-call-frame taint tags,
//! per-table-slot taint metadata, and the debug/security API functions.

use crate::error::{LuaResult, RuntimeError, runtime_error};
use crate::vm::closure::{Closure, RustClosure};
use crate::vm::state::LuaState;
use crate::vm::table::Table;
use crate::vm::value::Val;

const CLOSURE_TAINT_KEY: &str = "__closure_taint";

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all taint API functions on the Lua state.
pub fn register_taint_api(state: &mut LuaState) -> LuaResult<()> {
    register_debug_functions(state)?;
    register_global_functions(state)?;
    Ok(())
}

fn register_debug_functions(state: &mut LuaState) -> LuaResult<()> {
    let debug_key = state.gc.intern_string(b"debug");
    let debug_table = {
        let global = state.gc.tables.get(state.global)
            .ok_or_else(|| runtime_error("global table missing"))?;
        match global.get_str(debug_key, &state.gc.string_arena) {
            Val::Table(t) => t,
            _ => return Ok(()), // debug lib not loaded
        }
    };
    set_fn(state, debug_table, "setobjecttaint", setobjecttaint)?;
    set_fn(state, debug_table, "getstacktaint", getstacktaint)?;
    set_fn(state, debug_table, "setstacktaint", setstacktaint)?;
    set_fn(state, debug_table, "settaintmode", settaintmode)?;
    Ok(())
}

fn register_global_functions(state: &mut LuaState) -> LuaResult<()> {
    set_global_fn(state, "issecure", issecure)?;
    set_global_fn(state, "issecurevariable", issecurevariable)?;
    set_global_fn(state, "securecall", securecall)?;
    set_global_fn(state, "securecallfunction", securecall)?; // alias
    set_global_fn(state, "forceinsecure", forceinsecure)?;
    set_global_fn(state, "hooksecurefunc", hooksecurefunc)?;
    set_global_fn(state, "secureexecuterange", secureexecuterange)?;
    Ok(())
}

fn set_fn(
    state: &mut LuaState,
    table: crate::vm::gc::arena::GcRef<Table>,
    name: &str,
    func: fn(&mut LuaState) -> LuaResult<u32>,
) -> LuaResult<()> {
    let key = state.gc.intern_string(name.as_bytes());
    let closure = Closure::Rust(RustClosure::new(func, name));
    let closure_ref = state.gc.alloc_closure(closure);
    if let Some(t) = state.gc.tables.get_mut(table) {
        t.raw_set(Val::Str(key), Val::Function(closure_ref), &state.gc.string_arena)?;
    }
    Ok(())
}

fn set_global_fn(
    state: &mut LuaState,
    name: &str,
    func: fn(&mut LuaState) -> LuaResult<u32>,
) -> LuaResult<()> {
    let key = state.gc.intern_string(name.as_bytes());
    let closure = Closure::Rust(RustClosure::new(func, name));
    let closure_ref = state.gc.alloc_closure(closure);
    if let Some(g) = state.gc.tables.get_mut(state.global) {
        g.raw_set(Val::Str(key), Val::Function(closure_ref), &state.gc.string_arena)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// debug.setobjecttaint(func, taint)
// ---------------------------------------------------------------------------

fn setobjecttaint(state: &mut LuaState) -> LuaResult<u32> {
    let func_val = state.stack_get(state.base);
    let taint_val = state.stack_get(state.base + 1);

    let Val::Function(cl_ref) = func_val else {
        return Ok(0);
    };

    // Store taint in registry: __closure_taint[cl_index] = taint_string
    let taint_table = get_or_create_closure_taint_table(state);
    let key = Val::Num(cl_ref.index() as f64);
    if let Some(t) = state.gc.tables.get_mut(taint_table) {
        let _ = t.raw_set(key, taint_val, &state.gc.string_arena);
    }
    Ok(0)
}

fn get_or_create_closure_taint_table(
    state: &mut LuaState,
) -> crate::vm::gc::arena::GcRef<Table> {
    let key = state.gc.intern_string(CLOSURE_TAINT_KEY.as_bytes());
    if let Some(reg) = state.gc.tables.get(state.registry) {
        if let Val::Table(t) = reg.get_str(key, &state.gc.string_arena) {
            return t;
        }
    }
    let new_table = state.gc.alloc_table(Table::new());
    if let Some(reg) = state.gc.tables.get_mut(state.registry) {
        let _ = reg.raw_set(Val::Str(key), Val::Table(new_table), &state.gc.string_arena);
    }
    new_table
}

// ---------------------------------------------------------------------------
// debug.getstacktaint()
// ---------------------------------------------------------------------------

fn getstacktaint(state: &mut LuaState) -> LuaResult<u32> {
    let taint = state.call_stack.get(state.ci).and_then(|ci| ci.taint.as_ref());
    match taint {
        Some(name) => {
            let s = state.gc.intern_string(name.as_bytes());
            state.push(Val::Str(s));
            Ok(1)
        }
        None => Ok(0), // nil
    }
}

// ---------------------------------------------------------------------------
// debug.setstacktaint(taint)
// ---------------------------------------------------------------------------

fn setstacktaint(state: &mut LuaState) -> LuaResult<u32> {
    let val = state.stack_get(state.base);
    let taint = match val {
        Val::Str(s) => {
            let data = state.gc.string_arena.get(s)
                .map(|ls| String::from_utf8_lossy(ls.data()).into_owned());
            data
        }
        Val::Nil => None,
        _ => None,
    };
    if let Some(ci) = state.call_stack.get_mut(state.ci) {
        ci.taint = taint;
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// debug.settaintmode(mode)
// ---------------------------------------------------------------------------

fn settaintmode(state: &mut LuaState) -> LuaResult<u32> {
    let val = state.stack_get(state.base);
    state.taint_mode = match val {
        Val::Nil | Val::Bool(false) => false,
        Val::Num(n) if n == 0.0 => false,
        _ => true,
    };
    Ok(0)
}

// ---------------------------------------------------------------------------
// issecure()
// ---------------------------------------------------------------------------

fn issecure(state: &mut LuaState) -> LuaResult<u32> {
    let secure = state.call_stack.get(state.ci)
        .map(|ci| ci.taint.is_none())
        .unwrap_or(true);
    state.push(Val::Bool(secure));
    Ok(1)
}

// ---------------------------------------------------------------------------
// issecurevariable(table, key)
// ---------------------------------------------------------------------------

fn issecurevariable(state: &mut LuaState) -> LuaResult<u32> {
    let table_val = state.stack_get(state.base);
    let key_val = state.stack_get(state.base + 1);

    let Val::Table(table_ref) = table_val else {
        state.push(Val::Bool(true));
        Ok(1)
    };

    let taint = match key_val {
        Val::Str(s) => {
            let bytes = state.gc.string_arena.get(s).map(|ls| ls.data().to_vec());
            bytes.and_then(|b| {
                state.gc.tables.get(table_ref)
                    .and_then(|t| t.get_slot_taint_str(&b).map(|s| s.to_string()))
            })
        }
        Val::Num(n) if n as i64 as f64 == n => {
            state.gc.tables.get(table_ref)
                .and_then(|t| t.get_slot_taint_int(n as i64).map(|s| s.to_string()))
        }
        _ => None,
    };

    match taint {
        None => {
            state.push(Val::Bool(true));
            Ok(1)
        }
        Some(addon_name) => {
            let s = state.gc.intern_string(addon_name.as_bytes());
            state.push(Val::Bool(false));
            state.push(Val::Str(s));
            Ok(2)
        }
    }
}

// ---------------------------------------------------------------------------
// securecall(func, ...)
// ---------------------------------------------------------------------------

fn securecall(state: &mut LuaState) -> LuaResult<u32> {
    let saved_taint = state.call_stack.get(state.ci)
        .and_then(|ci| ci.taint.clone());

    // Clear taint for the duration of the call
    if let Some(ci) = state.call_stack.get_mut(state.ci) {
        ci.taint = None;
    }

    // Get the function to call
    let func_val = state.stack_get(state.base);
    let nargs = state.top.saturating_sub(state.base + 1);

    // Forward args and call
    // TODO: proper protected call through rilua's pcall mechanism
    // For now, just restore taint and return 0
    if let Some(ci) = state.call_stack.get_mut(state.ci) {
        ci.taint = saved_taint;
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// forceinsecure()
// ---------------------------------------------------------------------------

fn forceinsecure(state: &mut LuaState) -> LuaResult<u32> {
    if let Some(ci) = state.call_stack.get_mut(state.ci) {
        ci.taint = Some(String::new());
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// hooksecurefunc(table_or_name, name_or_hook, hook?)
// ---------------------------------------------------------------------------

fn hooksecurefunc(state: &mut LuaState) -> LuaResult<u32> {
    // TODO: full implementation needs wrapping original + hook
    // For now, no-op stub matching WoW's permissive behavior
    Ok(0)
}

// ---------------------------------------------------------------------------
// secureexecuterange(func, start, end, ...)
// ---------------------------------------------------------------------------

fn secureexecuterange(state: &mut LuaState) -> LuaResult<u32> {
    // TODO: loop start..=end calling func with clean taint per index
    Ok(0)
}
