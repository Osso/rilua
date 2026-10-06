use rilua::table_security::{can_access_secrets, revoke_secret_access};
use rilua::vm::state::LuaState;
use rilua::{Lua, LuaApiMut, LuaResult};

fn drop_access(state: &mut LuaState) -> LuaResult<u32> {
    revoke_secret_access(state)?;
    assert!(!can_access_secrets(state));
    Ok(0)
}

#[test]
fn secret_access_revocation_is_scoped_inherited_and_not_taint() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    rilua::table_security::register_table_security(&mut lua)?;
    lua.register_function("drop_access", drop_access)?;
    lua.exec(
        r"
        local secret = secretwrap(false)
        local function read() return secretunwrap(secret) end
        local function dropped()
            drop_access()
            assert(issecure())
            assert(not pcall(read))
            assert(not pcall(function() return not secret end))
            assert(not pcall(function() return securecall(read) end))
            local co = coroutine.create(read)
            assert(not coroutine.resume(co))
            return read()
        end
        assert(not pcall(dropped))
        assert(read() == false)
        local function ordinary_error()
            drop_access()
            error('after revoke')
        end
        assert(not pcall(ordinary_error))
        assert(read() == false)
        local function tainted()
            debug.setstacktaint('Addon')
            drop_access()
            assert(debug.getstacktaint() == 'Addon')
            debug.setstacktaint(nil)
            assert(not pcall(read))
        end
        tainted()
        assert(read() == false)
    ",
    )?;
    assert!(can_access_secrets(lua.state_mut()));
    assert!(revoke_secret_access(lua.state_mut()).is_err());
    Ok(())
}

#[test]
fn secret_access_revocation_survives_coroutine_yield_and_returns_at_context_end() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    rilua::table_security::register_table_security(&mut lua)?;
    lua.register_function("drop_access", drop_access)?;
    lua.exec(
        r"
        local secret = secretwrap('payload')
        local co = coroutine.create(function()
            drop_access()
            coroutine.yield('paused')
            assert(not pcall(secretunwrap, secret))
            return 'done'
        end)
        local ok, result = coroutine.resume(co)
        assert(ok and result == 'paused')
        assert(secretunwrap(secret) == 'payload')
        ok, result = coroutine.resume(co)
        assert(ok and result == 'done')
        assert(secretunwrap(secret) == 'payload')
    ",
    )?;
    Ok(())
}
