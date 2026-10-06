use rilua::table_security::{
    is_secret_value, transform_host_secret_string, unwrap_secret, wrap_host_secret_string,
};
use rilua::vm::state::LuaState;
use rilua::{Lua, LuaApiMut, LuaResult, Val};

fn shorten(state: &mut LuaState) -> LuaResult<u32> {
    let input = state.stack_get(state.base);
    let result = transform_host_secret_string(state, input, |bytes| {
        Ok(bytes
            .split(|byte| *byte == b'-')
            .next()
            .unwrap_or_default()
            .to_vec())
    })?;
    assert!(unwrap_secret(state, input).is_err());
    assert!(is_secret_value(state, result));
    state.push(result);
    Ok(1)
}

#[test]
fn host_secret_transform_preserves_opaque_bytes_taint_and_gc() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    rilua::table_security::register_table_security(&mut lua)?;
    lua.register_function("shorten", shorten)?;
    let input = wrap_host_secret_string(lua.state_mut(), "Name\0é-Realm");
    lua.set_global("input", input)?;
    lua.exec(
        r"
        local function addon()
            debug.setstacktaint('Addon')
            local output = shorten(input)
            assert(debug.getstacktaint() == 'Addon')
            assert(not pcall(secretunwrap, output))
            assert(not pcall(secretunwrap, input))
            return output
        end
        output = addon()
        input = nil
        collectgarbage('collect')
        assert(issecretvalue(output))
        assert(secretunwrap(output) == 'Name\000é')
    ",
    )?;
    assert!(transform_host_secret_string(lua.state_mut(), Val::Num(4.0), |_| Ok(vec![])).is_err());
    Ok(())
}
