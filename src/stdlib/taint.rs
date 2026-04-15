//! WoW taint tracking API functions.
//!
//! Implements Blizzard's security/taint system: per-call-frame taint tags,
//! per-table-slot taint metadata, and the debug/security API functions.

use crate::error::{LuaResult, runtime_error};
use crate::vm::callinfo::LUA_MULTRET;
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

fn ensure_runtime_tables(state: &LuaState) -> LuaResult<()> {
    state
        .gc
        .tables
        .get(state.global)
        .ok_or_else(|| runtime_error("global table missing"))?;
    state
        .gc
        .tables
        .get(state.registry)
        .ok_or_else(|| runtime_error("registry table missing"))?;
    Ok(())
}

fn decode_taint_name(state: &LuaState, value: Val) -> Option<String> {
    let Val::Str(string_ref) = value else {
        return None;
    };

    state
        .gc
        .string_arena
        .get(string_ref)
        .map(|lua_string| String::from_utf8_lossy(lua_string.data()).into_owned())
}

fn numeric_slot_index(value: f64) -> Option<i64> {
    if !value.is_finite() {
        return None;
    }

    let whole = value.trunc();
    if (value - whole).abs() > f64::EPSILON {
        return None;
    }

    format!("{whole:.0}").parse().ok()
}

fn register_debug_functions(state: &mut LuaState) -> LuaResult<()> {
    let debug_key = state.gc.intern_string(b"debug");
    let debug_table = {
        let global = state
            .gc
            .tables
            .get(state.global)
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
        t.raw_set(
            Val::Str(key),
            Val::Function(closure_ref),
            &state.gc.string_arena,
        )?;
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
        g.raw_set(
            Val::Str(key),
            Val::Function(closure_ref),
            &state.gc.string_arena,
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// debug.setobjecttaint(func, taint)
// ---------------------------------------------------------------------------

fn setobjecttaint(state: &mut LuaState) -> LuaResult<u32> {
    ensure_runtime_tables(state)?;
    let func_val = state.stack_get(state.base);
    let taint_val = state.stack_get(state.base + 1);

    let Val::Function(cl_ref) = func_val else {
        return Ok(0);
    };

    // Store taint in registry: __closure_taint[cl_index] = taint_string
    let taint_table = get_or_create_closure_taint_table(state);
    let key = Val::Num(f64::from(cl_ref.index()));
    if let Some(t) = state.gc.tables.get_mut(taint_table) {
        let _ = t.raw_set(key, taint_val, &state.gc.string_arena);
    }
    Ok(0)
}

fn get_or_create_closure_taint_table(state: &mut LuaState) -> crate::vm::gc::arena::GcRef<Table> {
    let key = state.gc.intern_string(CLOSURE_TAINT_KEY.as_bytes());
    if let Some(reg) = state.gc.tables.get(state.registry)
        && let Val::Table(t) = reg.get_str(key, &state.gc.string_arena)
    {
        return t;
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
    ensure_runtime_tables(state)?;
    // Report the deepest taint visible to the caller — matches the Elune
    // `L->stacktaint` semantics where taint persists across calls. Our
    // per-CI storage requires walking the stack to find it.
    let taint = (0..=state.ci)
        .rev()
        .filter_map(|depth| state.call_stack.get(depth))
        .find_map(|ci| ci.taint.as_ref());
    match taint {
        Some(name) => {
            let s = state.gc.intern_string(name.as_bytes());
            state.push(Val::Str(s));
            Ok(1)
        }
        None => Ok(0),
    }
}

// ---------------------------------------------------------------------------
// debug.setstacktaint(taint)
// ---------------------------------------------------------------------------

fn setstacktaint(state: &mut LuaState) -> LuaResult<u32> {
    ensure_runtime_tables(state)?;
    let val = state.stack_get(state.base);
    let taint = match val {
        Val::Str(_) => decode_taint_name(state, val),
        _ => None,
    };
    // Apply the taint to the CALLER's CallInfo, not our own. Elune keeps
    // taint on a single per-thread slot (`L->stacktaint`) that persists
    // across the call; rilua uses per-CI storage, so the equivalent
    // "persist past this call" behavior requires writing to the frame
    // that will keep executing after setstacktaint returns.
    let target = state.ci.checked_sub(1).unwrap_or(state.ci);
    if let Some(ci) = state.call_stack.get_mut(target) {
        ci.taint = taint;
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// debug.settaintmode(mode)
// ---------------------------------------------------------------------------

fn settaintmode(state: &mut LuaState) -> LuaResult<u32> {
    ensure_runtime_tables(state)?;
    let val = state.stack_get(state.base);
    state.taint_mode = !matches!(val, Val::Nil | Val::Bool(false) | Val::Num(0.0));
    Ok(0)
}

// ---------------------------------------------------------------------------
// issecure()
// ---------------------------------------------------------------------------

fn issecure(state: &mut LuaState) -> LuaResult<u32> {
    ensure_runtime_tables(state)?;
    // Walk the live call stack (0..=ci) and report secure only when every
    // frame is clean. Rilua stores taint per-CallInfo and doesn't auto-
    // propagate it through `CallInfo::new`, so checking just the current
    // frame would miss a tainted caller.
    let secure = crate::api::state_is_secure(state);
    state.push(Val::Bool(secure));
    Ok(1)
}

// ---------------------------------------------------------------------------
// issecurevariable(table, key)
// ---------------------------------------------------------------------------

fn issecurevariable(state: &mut LuaState) -> LuaResult<u32> {
    ensure_runtime_tables(state)?;
    let table_val = state.stack_get(state.base);
    let key_val = state.stack_get(state.base + 1);

    let Val::Table(table_ref) = table_val else {
        state.push(Val::Bool(true));
        return Ok(1);
    };

    let taint = match key_val {
        Val::Str(s) => {
            let bytes = state.gc.string_arena.get(s).map(|ls| ls.data().to_vec());
            bytes.and_then(|b| {
                state
                    .gc
                    .tables
                    .get(table_ref)
                    .and_then(|t| t.get_slot_taint_str(&b).map(str::to_string))
            })
        }
        Val::Num(n) => numeric_slot_index(n).and_then(|index| {
            state
                .gc
                .tables
                .get(table_ref)
                .and_then(|t| t.get_slot_taint_int(index).map(str::to_string))
        }),
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
    ensure_runtime_tables(state)?;
    if state.base >= state.top {
        return Ok(0);
    }

    let saved_taint = state
        .call_stack
        .get(state.ci)
        .and_then(|ci| ci.taint.clone());

    // Clear taint for the duration of the call
    if let Some(ci) = state.call_stack.get_mut(state.ci) {
        ci.taint = None;
    }

    let func_pos = state.base;
    let result = state.call_function(func_pos, LUA_MULTRET);
    let n_results = state.top.saturating_sub(func_pos) as u32;

    if let Some(ci) = state.call_stack.get_mut(state.ci) {
        ci.taint = saved_taint;
    }

    result?;
    Ok(n_results)
}

// ---------------------------------------------------------------------------
// forceinsecure()
// ---------------------------------------------------------------------------

fn forceinsecure(state: &mut LuaState) -> LuaResult<u32> {
    ensure_runtime_tables(state)?;
    if let Some(ci) = state.call_stack.get_mut(state.ci) {
        ci.taint = Some(String::new());
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// hooksecurefunc(table_or_name, name_or_hook, hook?)
// ---------------------------------------------------------------------------

fn hooksecurefunc(state: &mut LuaState) -> LuaResult<u32> {
    ensure_runtime_tables(state)?;
    // TODO: full implementation needs wrapping original + hook
    // For now, no-op stub matching WoW's permissive behavior
    Ok(0)
}

// ---------------------------------------------------------------------------
// secureexecuterange(func, start, end, ...)
// ---------------------------------------------------------------------------

fn secureexecuterange(state: &mut LuaState) -> LuaResult<u32> {
    ensure_runtime_tables(state)?;
    // TODO: loop start..=end calling func with clean taint per index
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::stdlib::{StdLib, open_libs_selective};

    fn new_state_with_taint_api() -> LuaState {
        let mut state = LuaState::new();
        open_libs_selective(&mut state, StdLib::BASE | StdLib::DEBUG)
            .expect("failed to open stdlib with taint api");
        state
    }

    fn string_val(state: &mut LuaState, value: &str) -> Val {
        Val::Str(state.gc.intern_string(value.as_bytes()))
    }

    fn decode_string(state: &LuaState, value: Val) -> String {
        match value {
            Val::Str(string_ref) => state
                .gc
                .string_arena
                .get(string_ref)
                .map(|s| String::from_utf8_lossy(s.data()).into_owned())
                .expect("missing string ref"),
            other => panic!("expected string, got {other:?}"),
        }
    }

    fn set_args(state: &mut LuaState, args: &[Val]) {
        state.base = 0;
        state.call_stack[state.ci].base = 0;
        state.top = 0;
        state.ensure_stack(args.len());
        for (idx, arg) in args.iter().enumerate() {
            state.stack_set(idx, *arg);
        }
        state.top = args.len();
    }

    fn assert_table_has_function(
        state: &mut LuaState,
        table_ref: crate::vm::gc::arena::GcRef<Table>,
        name: &str,
    ) {
        let key = state.gc.intern_string(name.as_bytes());
        let table = state.gc.tables.get(table_ref).expect("missing table");
        assert!(
            matches!(
                table.get(Val::Str(key), &state.gc.string_arena),
                Val::Function(_)
            ),
            "expected {name} to be a function"
        );
    }

    fn noop_callback(_state: &mut LuaState) -> LuaResult<u32> {
        Ok(0)
    }

    fn count_args_callback(state: &mut LuaState) -> LuaResult<u32> {
        let nargs = state.top.saturating_sub(state.base);
        state.push(Val::Num(nargs as f64));
        Ok(1)
    }

    #[test]
    fn open_libs_selective_registers_taint_api() {
        let mut state = new_state_with_taint_api();
        let global_ref = state.global;
        assert_table_has_function(&mut state, global_ref, "issecure");
        assert_table_has_function(&mut state, global_ref, "issecurevariable");
        assert_table_has_function(&mut state, global_ref, "securecall");
        assert_table_has_function(&mut state, global_ref, "securecallfunction");
        assert_table_has_function(&mut state, global_ref, "forceinsecure");
        assert_table_has_function(&mut state, global_ref, "hooksecurefunc");
        assert_table_has_function(&mut state, global_ref, "secureexecuterange");

        let debug_key = state.gc.intern_string(b"debug");
        let debug_table_ref = match state
            .gc
            .tables
            .get(state.global)
            .expect("missing global table")
            .get(Val::Str(debug_key), &state.gc.string_arena)
        {
            Val::Table(table_ref) => table_ref,
            other => panic!("expected debug table, got {other:?}"),
        };

        assert_table_has_function(&mut state, debug_table_ref, "setobjecttaint");
        assert_table_has_function(&mut state, debug_table_ref, "getstacktaint");
        assert_table_has_function(&mut state, debug_table_ref, "setstacktaint");
        assert_table_has_function(&mut state, debug_table_ref, "settaintmode");
    }

    #[test]
    fn stack_taint_round_trips_and_updates_issecure() {
        let mut state = new_state_with_taint_api();

        let addon = string_val(&mut state, "MyAddon");
        set_args(&mut state, &[addon]);
        assert_eq!(setstacktaint(&mut state).expect("setstacktaint failed"), 0);
        assert_eq!(state.call_stack[state.ci].taint.as_deref(), Some("MyAddon"));

        set_args(&mut state, &[]);
        let result_start = state.top;
        assert_eq!(getstacktaint(&mut state).expect("getstacktaint failed"), 1);
        assert_eq!(
            decode_string(&state, state.stack_get(result_start)),
            "MyAddon"
        );

        set_args(&mut state, &[]);
        let result_start = state.top;
        assert_eq!(issecure(&mut state).expect("issecure failed"), 1);
        assert_eq!(state.stack_get(result_start), Val::Bool(false));

        set_args(&mut state, &[Val::Nil]);
        assert_eq!(setstacktaint(&mut state).expect("clear taint failed"), 0);
        assert_eq!(state.call_stack[state.ci].taint, None);

        set_args(&mut state, &[]);
        assert_eq!(getstacktaint(&mut state).expect("getstacktaint failed"), 0);

        set_args(&mut state, &[]);
        let result_start = state.top;
        assert_eq!(issecure(&mut state).expect("issecure failed"), 1);
        assert_eq!(state.stack_get(result_start), Val::Bool(true));
    }

    #[test]
    fn settaintmode_toggles_falsey_and_truthy_values() {
        let mut state = new_state_with_taint_api();

        set_args(&mut state, &[Val::Bool(false)]);
        assert_eq!(settaintmode(&mut state).expect("settaintmode failed"), 0);
        assert!(!state.taint_mode);

        set_args(&mut state, &[Val::Num(0.0)]);
        assert_eq!(settaintmode(&mut state).expect("settaintmode failed"), 0);
        assert!(!state.taint_mode);

        let addon = string_val(&mut state, "enabled");
        set_args(&mut state, &[addon]);
        assert_eq!(settaintmode(&mut state).expect("settaintmode failed"), 0);
        assert!(state.taint_mode);
    }

    #[test]
    fn issecurevariable_reports_tainted_slots() {
        let mut state = new_state_with_taint_api();
        let table_ref = state.gc.alloc_table(Table::new());
        {
            let table = state.gc.tables.get_mut(table_ref).expect("missing table");
            table.set_slot_taint_str(b"foo", "AddonA");
            table.set_slot_taint_int(7, "AddonB");
        }

        let foo = string_val(&mut state, "foo");
        set_args(&mut state, &[Val::Table(table_ref), foo]);
        let result_start = state.top;
        assert_eq!(
            issecurevariable(&mut state).expect("issecurevariable failed"),
            2
        );
        assert_eq!(state.stack_get(result_start), Val::Bool(false));
        assert_eq!(
            decode_string(&state, state.stack_get(result_start + 1)),
            "AddonA"
        );

        set_args(&mut state, &[Val::Table(table_ref), Val::Num(7.0)]);
        let result_start = state.top;
        assert_eq!(
            issecurevariable(&mut state).expect("issecurevariable failed"),
            2
        );
        assert_eq!(state.stack_get(result_start), Val::Bool(false));
        assert_eq!(
            decode_string(&state, state.stack_get(result_start + 1)),
            "AddonB"
        );

        let bar = string_val(&mut state, "bar");
        set_args(&mut state, &[Val::Table(table_ref), bar]);
        let result_start = state.top;
        assert_eq!(
            issecurevariable(&mut state).expect("issecurevariable failed"),
            1
        );
        assert_eq!(state.stack_get(result_start), Val::Bool(true));
    }

    #[test]
    fn setobjecttaint_stores_function_taint_in_registry() {
        let mut state = new_state_with_taint_api();
        let closure_ref = state
            .gc
            .alloc_closure(Closure::Rust(RustClosure::new(noop_callback, "noop")));
        let addon = string_val(&mut state, "RegistryAddon");

        set_args(&mut state, &[Val::Function(closure_ref), addon]);
        assert_eq!(
            setobjecttaint(&mut state).expect("setobjecttaint failed"),
            0
        );

        let taint_table = get_or_create_closure_taint_table(&mut state);
        let stored = state
            .gc
            .tables
            .get(taint_table)
            .expect("missing taint table")
            .get(Val::Num(closure_ref.index() as f64), &state.gc.string_arena);
        assert_eq!(decode_string(&state, stored), "RegistryAddon");
    }

    #[test]
    fn securecall_returns_results_and_restores_taint() {
        let mut state = new_state_with_taint_api();
        state.call_stack[state.ci].taint = Some("CallerAddon".to_string());
        let closure_ref = state.gc.alloc_closure(Closure::Rust(RustClosure::new(
            count_args_callback,
            "count_args",
        )));

        set_args(
            &mut state,
            &[Val::Function(closure_ref), Val::Num(1.0), Val::Num(2.0)],
        );
        assert_eq!(securecall(&mut state).expect("securecall failed"), 1);
        assert_eq!(state.stack_get(0), Val::Num(2.0));
        assert_eq!(
            state.call_stack[state.ci].taint.as_deref(),
            Some("CallerAddon")
        );
    }

    #[test]
    fn taint_stub_functions_remain_permissive_noops() {
        let mut state = new_state_with_taint_api();
        assert_eq!(
            hooksecurefunc(&mut state).expect("hooksecurefunc failed"),
            0
        );
        assert_eq!(
            secureexecuterange(&mut state).expect("secureexecuterange failed"),
            0
        );
        assert_eq!(forceinsecure(&mut state).expect("forceinsecure failed"), 0);
        assert_eq!(state.call_stack[state.ci].taint.as_deref(), Some(""));
    }
}
