use rilua::vm::callinfo::LUA_MULTRET;
use rilua::vm::state::LuaState;
use rilua::{Lua, LuaApiMut, LuaResult, Val};

fn dispatch(state: &mut LuaState) -> LuaResult<u32> {
    let base = state.base;
    state.with_instruction_owner("Addon", |state| state.call_function(base, LUA_MULTRET))?;
    Ok((state.top - base) as u32)
}

fn exempt_dispatch(state: &mut LuaState) -> LuaResult<u32> {
    state.with_instruction_budget_exemption(dispatch)
}

#[test]
fn instruction_budget_cannot_be_disabled_by_lua_and_recovers_after_exemption_error() {
    let mut lua = Lua::new().unwrap();
    lua.state_mut().set_instruction_budget("Addon", Some(80));
    lua.register_function("dispatch", dispatch).unwrap();
    lua.register_function("exempt_dispatch", exempt_dispatch)
        .unwrap();
    lua.exec(
        r#"
        function work()
            debug.sethook()
            debug.setstacktaint(nil)
            local total = 0
            for i = 1, 1000 do total = total + i end
            return total
        end
        local ok, message = pcall(dispatch, work)
        assert(not ok and string.find(message, 'Addon', 1, true))
        assert(exempt_dispatch(work) == 500500)
        assert(not pcall(exempt_dispatch, function() error('event failed') end))
        assert(not pcall(dispatch, work))
        assert(1 + 2 == 3)
    "#,
    )
    .unwrap();
    assert_eq!(
        lua.state_mut().instruction_budget("Addon").unwrap().used,
        80
    );
    lua.state_mut().reset_instruction_usage("Addon").unwrap();
    lua.exec("assert(dispatch(function() return 42 end) == 42)")
        .unwrap();
    assert!(lua.state_mut().instruction_budget("Addon").unwrap().used > 0);
}

#[test]
fn instruction_budget_accounts_nested_owners_coroutines_and_caught_errors() {
    let mut lua = Lua::new().unwrap();
    lua.state_mut().set_instruction_budget("A", Some(30));
    lua.state_mut().set_instruction_budget("B", Some(100));
    let function = lua.load("local co = coroutine.create(function() while true do pcall(function() end) end end); coroutine.resume(co); return 4").unwrap();
    lua.state_mut()
        .with_instruction_owner("A", |state| {
            state.with_instruction_owner("B", |state| {
                let base = state.top;
                state.push(Val::Function(function.gc_ref()));
                state.call_function(base, 0)
            })
        })
        .unwrap_err();
    assert_eq!(lua.state_mut().instruction_budget("A").unwrap().used, 0);
    assert_eq!(lua.state_mut().instruction_budget("B").unwrap().used, 100);
    lua.exec("assert(2 + 2 == 4)").unwrap();
    assert!(
        lua.state_mut()
            .with_instruction_owner("unknown", |_| Ok(()))
            .is_err()
    );
}
