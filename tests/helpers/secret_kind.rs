use rilua::table_security::{SecretPayloadKind, is_secret_table, secret_payload_kind};
use rilua::{Lua, LuaApiMut, LuaResult, Val};

#[test]
fn secret_contents_global_slot_snapshot_is_not_public() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    let state = lua.state_mut();
    let global = state.global;
    let global_key = state.gc.intern_string(b"_G");
    let value_key = state.gc.intern_string(b"payload");
    let shadow_key = state.gc.intern_string(b"_test_shadow");
    let shadow = state.gc.alloc_table(rilua::vm::table::Table::new());
    rilua::Table::from_gc_ref(state.registry).raw_set(
        state,
        Val::Str(shadow_key),
        Val::Table(shadow),
    )?;
    state.install_global_slots(
        vec![Val::Table(global), Val::Num(17.0)].into_boxed_slice(),
        vec![global_key, value_key].into_boxed_slice(),
        Some(shadow_key),
    );
    let function = lua.load("return payload")?;
    rilua::table_security::set_table_security(lua.state_mut(), global, 2)?;
    let result = lua.call_function(&function, &[])?;
    assert_eq!(result.len(), 1);
    assert!(rilua::table_security::is_secret_value(
        lua.state_mut(),
        result[0]
    ));
    assert_eq!(
        rilua::table_security::unwrap_secret(lua.state_mut(), result[0])?,
        Val::Num(17.0)
    );
    Ok(())
}

#[test]
fn secret_kind_metadata_and_contents_are_opaque_to_tainted_callers() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    rilua::table_security::register_table_security(&mut lua)?;
    lua.register_function("secret_table", |state| {
        state.push(Val::Bool(is_secret_table(
            state,
            state.stack_get(state.base),
        )));
        Ok(1)
    })?;
    lua.register_function("table_kind", |state| {
        let kind = secret_payload_kind(state, state.stack_get(state.base));
        state.push(Val::Bool(kind == Some(SecretPayloadKind::Table)));
        Ok(1)
    })?;
    lua.exec(
        r"
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
    ",
    )?;
    Ok(())
}

#[test]
fn secret_contents_wraps_index_metamethod_results_and_checked_host_writes() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    rilua::table_security::register_table_security(&mut lua)?;
    lua.exec(
        r"
        contents = setmetatable({}, {__index = function() return 'inherited' end})
        settablesecurity(contents, 2)
        assert(issecretvalue(contents.key))
        assert(secretunwrap(contents.key) == 'inherited')
        local proxy = setmetatable({}, {__index = contents})
        assert(issecretvalue(proxy.key))
        assert(secretunwrap(proxy.key) == 'inherited')
        debug.setstacktaint('Addon')
        assert(issecretvalue(contents.key))
        assert(issecretvalue(proxy.key))
        assert(not pcall(secretunwrap, proxy.key))
        assert(not pcall(secretunwrap, contents.key))
    ",
    )?;
    let table: rilua::Table = lua.global("contents")?;
    let key = Val::Str(lua.state_mut().gc.intern_string(b"key"));
    let result = lua.state_mut().gettable(Val::Table(table.gc_ref()), key)?;
    assert!(rilua::table_security::is_secret_value(
        lua.state_mut(),
        result
    ));
    table.raw_set(lua.state_mut(), Val::Num(1.0), Val::Num(17.0))?;
    lua.exec("assert(issecretvalue(rawget(contents, 1)))")?;
    Ok(())
}
