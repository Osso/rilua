use std::time::Duration;

use rilua::vm::callinfo::LUA_MULTRET;
use rilua::vm::state::LuaState;
use rilua::{Lua, LuaApiMut, LuaResult, Val};

fn dispatch_with_timeout(state: &mut LuaState, timeout: Duration) -> LuaResult<u32> {
    let base = state.base;
    state.with_instruction_owner("Addon", |state| {
        state.with_execution_watchdog(timeout, 1, |state| state.call_function(base, LUA_MULTRET))
    })?;
    Ok((state.top - base) as u32)
}

fn dispatch(state: &mut LuaState) -> LuaResult<u32> {
    dispatch_with_timeout(state, Duration::from_secs(60))
}

fn overrun_dispatch(state: &mut LuaState) -> LuaResult<u32> {
    dispatch_with_timeout(state, Duration::ZERO)
}

fn exempt_dispatch(state: &mut LuaState) -> LuaResult<u32> {
    state.with_watchdog_exemption(dispatch)
}

#[test]
fn execution_watchdog_rejects_overrun_and_restores_after_exemption() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    lua.register_function("dispatch", dispatch)?;
    lua.register_function("overrun_dispatch", overrun_dispatch)?;
    lua.register_function("exempt_dispatch", exempt_dispatch)?;
    lua.exec(
        r"
        function work()
            debug.sethook()
            debug.setstacktaint(nil)
            local total = 0
            for i = 1, 1000 do total = total + i end
            return total
        end
        assert(dispatch(work) == 500500)
        local ok, message = pcall(overrun_dispatch, work)
        assert(not ok and string.find(message, 'script ran too long', 1, true))
        assert(exempt_dispatch(work) == 500500)
        assert(not pcall(overrun_dispatch, work))
        assert(1 + 2 == 3)
    ",
    )?;
    Ok(())
}

#[test]
fn execution_watchdog_covers_nested_owners_and_coroutines() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    let function = lua.load(
        "local co = coroutine.create(function() while true do pcall(function() end) end end); coroutine.resume(co); return 4",
    )?;
    let result = lua.state_mut().with_instruction_owner("A", |state| {
        state.with_execution_watchdog(Duration::ZERO, 1, |state| {
            state.with_instruction_owner("B", |state| {
                let base = state.top;
                state.push(Val::Function(function.gc_ref()));
                state.call_function(base, 0)
            })
        })
    });
    assert!(result.is_err_and(|error| error.to_string().contains("script ran too long")));
    lua.exec("assert(2 + 2 == 4)")?;
    Ok(())
}
