use rilua::table_security::{check_table_access, set_table_security, wrap_secret};
use rilua::vm::table::Table;
use rilua::{Lua, LuaApi, LuaApiMut, Val};

fn secure_lua() -> Lua {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua
}

#[test]
fn table_security_registration_is_opt_in() {
    let mut lua = Lua::new().unwrap();
    lua.exec("assert(settablesecurity == nil and secretwrap == nil)")
        .unwrap();
    let mut opted_in = secure_lua();
    opted_in
        .exec("assert(type(settablesecurity) == 'function')")
        .unwrap();
}

#[test]
fn table_security_options_validate_and_return_no_values() {
    secure_lua()
        .exec(
            r#"
        local t = {}
        assert(select('#', settablesecurity(t, 1)) == 0)
        assert(select('#', settablesecurity(t, 0)) == 0)
        assert(select('#', settablesecurity(t, 1)) == 0)
        for _, option in ipairs({-1, 0.5, 2, 3, math.huge, '0'}) do
            assert(not pcall(settablesecurity, t, option))
        end
        assert(not pcall(settablesecurity, t))
        assert(not pcall(settablesecurity, 12, 0))
        local ok, message = pcall(settablesecurity, t, 2)
        assert(not ok and string.find(message, 'SecretWrapContents', 1, true))
    "#,
        )
        .unwrap();
}

#[test]
fn table_security_secret_wrappers_preserve_unwrapped_keys_and_graph() {
    secure_lua()
        .exec(
            r#"
        local key = { label = 'retained' }
        key.self = key
        local first, second = secretwrap(key), secretwrap(key)
        assert(issecretvalue(first) and issecretvalue(second))
        assert(not issecretvalue(key))
        assert(secretwrap(first) == first)
        assert(secretunwrap(first) == key and secretunwrap(second) == key)
        assert(secretunwrap(first) == secretunwrap(second))
        key = nil
        collectgarbage('collect')
        assert(secretunwrap(first).self == secretunwrap(second))
        assert(secretunwrap(first).label == 'retained')
        local a, b, c = secretwrap(123, nil, false)
        assert(issecretvalue(a) and issecretvalue(b) and issecretvalue(c))
        local x, y, z = secretunwrap(a, b, c)
        assert(x == 123 and y == nil and z == false)
        assert(select('#', secretunwrap(a, b, c)) == 3)
        assert(select('#', secretwrap()) == 0)
        assert(select('#', secretunwrap()) == 0)
        assert(secretunwrap('plain') == 'plain')
        assert(getmetatable(first) == nil)
        assert(not pcall(getfenv, first))
    "#,
        )
        .unwrap();
}

#[test]
fn table_security_core_rejects_tainted_and_secret_key_access_cumulatively() {
    let mut lua = secure_lua();
    let state = lua.state_mut();
    let table = state.gc.alloc_table(Table::new());
    let secret = wrap_secret(state, Val::Num(17.0)).unwrap();
    set_table_security(state, table, 1).unwrap();
    set_table_security(state, table, 0).unwrap();
    assert!(check_table_access(state, table, Some(Val::Num(17.0))).is_ok());
    assert!(check_table_access(state, table, Some(secret)).is_err());
    state.call_stack[0].taint = Some("UntrustedAddon".to_owned());
    assert!(check_table_access(state, table, None).is_err());
    assert!(set_table_security(state, table, 0).is_err());
    assert!(wrap_secret(state, Val::Num(2.0)).is_err());
    assert!(rilua::table_security::unwrap_secret(state, secret).is_err());
    state.call_stack[0].taint = None;
    assert!(check_table_access(state, table, None).is_ok());
}

#[test]
fn table_security_gc_keeps_payload_alive_only_while_wrapper_is_rooted() {
    let mut lua = secure_lua();
    let table = lua.state_mut().gc.alloc_table(Table::new());
    let wrapper = wrap_secret(lua.state_mut(), Val::Table(table)).unwrap();
    let Val::Userdata(wrapper_ref) = wrapper else {
        panic!("wrapper must be userdata")
    };
    lua.set_global_val("kept", wrapper).unwrap();
    lua.exec("collectgarbage('collect'); collectgarbage('collect')")
        .unwrap();
    assert!(lua.state().gc.tables.get(table).is_some());
    lua.exec("kept = nil; collectgarbage('collect'); collectgarbage('collect')")
        .unwrap();
    assert!(lua.state().gc.tables.get(table).is_none());
    assert!(lua.state().gc.userdata.get(wrapper_ref).is_none());
    let next_table = lua.state_mut().gc.alloc_table(Table::new());
    assert!(check_table_access(lua.state(), next_table, None).is_ok());
    assert!(check_table_access(lua.state(), table, None).is_err());
    let plain = lua
        .state_mut()
        .gc
        .alloc_userdata(rilua::vm::value::Userdata::new(Box::new(42)));
    assert!(!rilua::table_security::is_secret_value(
        lua.state(),
        Val::Userdata(plain)
    ));
}

#[test]
fn table_security_flags_do_not_survive_collected_table_slot_reuse() {
    let mut lua = secure_lua();
    let table = lua.state_mut().gc.alloc_table(Table::with_sizes(2, 4));
    set_table_security(lua.state_mut(), table, 0).unwrap();
    lua.exec("collectgarbage('collect'); collectgarbage('collect')")
        .unwrap();
    assert!(lua.state().gc.tables.get(table).is_none());
    let replacement = lua.state_mut().gc.alloc_table(Table::with_sizes(1, 0));
    lua.state_mut().call_stack[0].taint = Some("Addon".to_owned());
    assert!(check_table_access(lua.state(), replacement, Some(Val::Num(1.0))).is_ok());
}
