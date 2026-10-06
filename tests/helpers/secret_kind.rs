use rilua::table_security::{SecretPayloadKind, is_secret_table, secret_payload_kind};
use rilua::vm::state::LuaState;
use rilua::{Lua, LuaApiMut, LuaResult, Val};

fn secret_table(state: &mut LuaState) -> LuaResult<u32> {
    state.push(Val::Bool(is_secret_table(
        state,
        state.stack_get(state.base),
    )));
    Ok(1)
}
fn kind(state: &mut LuaState) -> LuaResult<u32> {
    let kind = secret_payload_kind(state, state.stack_get(state.base));
    state.push(Val::Bool(kind == Some(SecretPayloadKind::Table)));
    Ok(1)
}

#[test]
fn secret_kind_metadata_and_contents_are_opaque_to_tainted_callers() {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.register_function("secret_table", secret_table).unwrap();
    lua.register_function("table_kind", kind).unwrap();
    lua.exec(
        r#"
        local plain = { child = secretwrap(3) }
        local wrapped = secretwrap({ field = 'private' })
        local contents = { 'one', 'two', n = 12, child = {} }
        local retained_next, retained_table = pairs(contents)
        settablesecurity(contents, 2)
        settablesecurity(contents, 2)
        assert(not secret_table(plain))
        assert(secret_table(wrapped) and secret_table(contents))
        assert(not table_kind(contents) and table_kind(wrapped))
        assert(not table_kind(secretwrap('text')))
        local function addon()
            debug.setstacktaint('Addon')
            assert(table_kind(wrapped) and secret_table(contents))
            assert(not pcall(secretunwrap, wrapped))
            assert(not pcall(function() return wrapped.field end))
            assert(issecretvalue(contents[1]))
            assert(issecretvalue(rawget(contents, 'n')))
            assert(issecretvalue(contents.child))
            assert(not pcall(secretunwrap, contents.n))
            contents.n = 13
            rawset(contents, 'new', 'new payload')
            table.insert(contents, 'three')
            assert(issecretvalue(contents.n) and issecretvalue(contents.new))
            local first, second, third = unpack(contents)
            assert(issecretvalue(first) and issecretvalue(second) and issecretvalue(third))
            for key, value in pairs(contents) do assert(issecretvalue(value)) end
            local key, value = retained_next(retained_table)
            assert(key ~= nil and issecretvalue(value))
            assert(issecretvalue(table.remove(contents)))
            assert(#contents == 2 and contents.missing == nil)
            return contents.new
        end
        local output = addon()
        assert(secretunwrap(output) == 'new payload')
        assert(secretunwrap(contents.n) == 13)
        collectgarbage('collect')
        assert(secretunwrap(contents[1]) == 'one')
    "#,
    )
    .unwrap();
}

#[test]
fn secret_contents_wraps_index_metamethod_results_and_checked_host_writes() {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.exec(
        r#"
        contents = setmetatable({}, {__index = function() return 'inherited' end})
        settablesecurity(contents, 2)
        assert(issecretvalue(contents.key))
        assert(secretunwrap(contents.key) == 'inherited')
        debug.setstacktaint('Addon')
        assert(issecretvalue(contents.key))
        assert(not pcall(secretunwrap, contents.key))
    "#,
    )
    .unwrap();
    let table: rilua::Table = lua.global("contents").unwrap();
    table
        .raw_set(lua.state_mut(), Val::Num(1.0), Val::Num(17.0))
        .unwrap();
    lua.exec("assert(issecretvalue(rawget(contents, 1)))")
        .unwrap();
}
