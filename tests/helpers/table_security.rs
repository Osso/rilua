use rilua::api::state_is_secure;
use rilua::table_security::{
    check_table_access, set_table_security, wrap_host_secret_bool, wrap_secret,
};
use rilua::vm::table::Table;
use rilua::{Lua, LuaApi, LuaApiMut, Val};

fn secure_lua() -> Lua {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua
}

fn host_bool_lua() -> Lua {
    use rilua::LuaResult;
    use rilua::vm::state::LuaState;

    fn host_true(state: &mut LuaState) -> LuaResult<u32> {
        let before = state_is_secure(state);
        let secret = wrap_host_secret_bool(state, true);
        state.push(secret);
        assert_eq!(state_is_secure(state), before);
        Ok(1)
    }
    fn host_false(state: &mut LuaState) -> LuaResult<u32> {
        let before = state_is_secure(state);
        let secret = wrap_host_secret_bool(state, false);
        state.push(secret);
        assert_eq!(state_is_secure(state), before);
        Ok(1)
    }
    let mut lua = secure_lua();
    lua.register_function("host_true", host_true).unwrap();
    lua.register_function("host_false", host_false).unwrap();
    fn api_equal(state: &mut LuaState) -> LuaResult<u32> {
        let result =
            state.api_equal(state.stack_get(state.base), state.stack_get(state.base + 1))?;
        state.push(Val::Bool(result));
        Ok(1)
    }
    fn api_less(state: &mut LuaState) -> LuaResult<u32> {
        let result =
            state.api_lessthan(state.stack_get(state.base), state.stack_get(state.base + 1))?;
        state.push(Val::Bool(result));
        Ok(1)
    }
    lua.register_function("host_api_equal", api_equal).unwrap();
    lua.register_function("host_api_less", api_less).unwrap();
    lua.exec("debug.settaintmode(true); function call_tainted(fn) debug.setstacktaint('TestAddon'); return fn() end")
        .unwrap();
    lua
}

#[test]
fn host_secret_booleans_keep_taint_and_secure_boolean_control_flow() {
    host_bool_lua()
        .exec(
            r#"
        local yes, no = host_true(), host_false()
        assert(issecretvalue(yes) and issecretvalue(no))
        assert(type(yes) == 'userdata' and type(no) == 'userdata')
        assert(secretunwrap(yes) == true and secretunwrap(no) == false)
        local fromTainted = call_tainted(function()
            assert(debug.getstacktaint() == 'TestAddon')
            local value = host_false()
            assert(debug.getstacktaint() == 'TestAddon')
            return value
        end)
        assert(issecretvalue(fromTainted) and secretunwrap(fromTainted) == false)
        if no then error('secret false entered true branch') end
        if not yes then error('secret true entered false branch') end
        assert(not no and not (not yes))
        assert((no or 'fallback') == 'fallback')
        assert((yes and 'selected') == 'selected')
        assert(issecretvalue(yes or no) and issecretvalue(no and yes))
        assert((no and yes) == false and (yes and no) == false)
        assert(yes == true and no == false and yes ~= false and no ~= true)
        assert(rawequal(no, false) and rawequal(yes, true))
        assert(no == host_false() and yes == host_true())
        assert(host_api_equal(no, false) and host_api_equal(no, host_false()))
        assert(not host_api_equal(yes, no) and not host_api_equal(no, true))
        assert(not pcall(assert, no))
        assert(issecretvalue(assert(yes)))
        assert(type(not no) == 'boolean' and type(no == false) == 'boolean')
        local number = secretwrap(0)
        assert(number and number ~= 0 and not pcall(function() return number + 1 end))
        assert(secretunwrap(number) == 0)
        collectgarbage('collect')
        assert(issecretvalue(no) and secretunwrap(no) == false)
    "#,
        )
        .unwrap();
}

#[test]
fn tainted_code_cannot_inspect_secret_booleans_even_by_alias_identity() {
    host_bool_lua()
        .exec(
            r#"
        local yes, no = host_true(), host_false()
        local operations = {
            function() return secretunwrap(no) end,
            function() if no then return 1 end end,
            function() if yes then return 1 end end,
            function() return not no end,
            function() return no and 1 end,
            function() return yes or 1 end,
            function() return no == no end,
            function() return yes == yes end,
            function() return no == false end,
            function() return no == host_false() end,
            function() return rawequal(no, no) end,
            function() return rawequal(yes, true) end,
            function() return host_api_equal(no, no) end,
            function() return host_api_less(no, no) end,
            function() return assert(no) end,
            function() return assert(yes) end,
        }
        for _, operation in ipairs(operations) do
            local ok, message = pcall(call_tainted, operation)
            assert(not ok and string.find(message, 'untainted caller', 1, true), tostring(message))
        end
        local ok = pcall(call_tainted, function() return secretwrap(false) end)
        assert(not ok)
        assert(secretunwrap(no) == false and secretunwrap(yes) == true)
    "#,
        )
        .unwrap();
}

#[test]
fn secret_boolean_comparator_results_and_ordinals_enforce_caller_security() {
    host_bool_lua()
        .exec(
            r#"
        local first, second = {}, {}
        local secretFalse = host_false()
        local secretTrue = host_true()
        setmetatable(first, {__lt = function() return secretFalse end,
                             __eq = function() return secretTrue end})
        setmetatable(second, getmetatable(first))
        assert(not (first < second) and first == second)
        assert(not host_api_less(first, second) and host_api_equal(first, second))
        assert(not pcall(call_tainted, function() return first < second end))
        assert(not pcall(call_tainted, function() return first == second end))
        assert(not pcall(call_tainted, function() return host_api_less(first, second) end))
        assert(not pcall(call_tainted, function() return host_api_equal(first, second) end))
        assert(not pcall(call_tainted, function() return secretFalse < secretTrue end))
        assert(not pcall(function() return secretFalse < secretTrue end))
        assert(not pcall(function() return secretFalse <= secretTrue end))
        assert(not pcall(host_api_less, secretFalse, secretTrue))
        local values = {2, 1}
        table.sort(values, function() return host_false() end)
        assert(values[1] == 2 and values[2] == 1)
        assert(not pcall(call_tainted, function()
            table.sort({2, 1}, function() return host_false() end)
        end))
        local left, right = {key=2}, {key=1}
        local mt = {__lt = function(a, b)
            if a.key < b.key then return host_true() end
            return host_false()
        end}
        setmetatable(left, mt); setmetatable(right, mt)
        local ordered = {left, right}
        table.sort(ordered)
        assert(ordered[1] == right and ordered[2] == left)
        assert(not pcall(call_tainted, function() table.sort({left, right}) end))
    "#,
        )
        .unwrap();
}

#[test]
fn secret_boolean_stdlib_truth_checks_guard_tainted_callers() {
    host_bool_lua()
        .exec(
            r#"
        assert(string.find('a+b', 'a.b', 1, host_false()) == 1)
        assert(string.find('a.b', 'a.b', 1, host_true()) == 1)
        assert(not pcall(call_tainted, function()
            return string.find('a+b', 'a.b', 1, host_false())
        end))
        package.loaded['host-secret-false'] = host_false()
        package.preload['host-secret-false'] = function() return 'loaded' end
        assert(require('host-secret-false') == 'loaded')
        package.loaded['host-secret-false'] = host_false()
        assert(not pcall(call_tainted, function() return require('host-secret-false') end))
        assert(({} or false) ~= false)
        local opaque = secretwrap('opaque')
        assert(opaque and opaque == opaque and opaque ~= 'opaque')
        local plain = newproxy(true)
        assert(plain and not (plain == false))
    "#,
        )
        .unwrap();
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

#[test]
fn table_security_frozen_wrapper_graph_keeps_private_payload() {
    let mut lua = secure_lua();
    let root = lua.state_mut().gc.alloc_table(Table::new());
    let payload = lua.state_mut().gc.alloc_table(Table::new());
    let wrapper = wrap_secret(lua.state_mut(), Val::Table(payload)).unwrap();
    {
        let state = lua.state_mut();
        state
            .gc
            .tables
            .get_mut(root)
            .unwrap()
            .raw_set(Val::Num(1.0), wrapper, &state.gc.string_arena)
            .unwrap();
        state.gc.freeze_table(root);
    }
    lua.exec("collectgarbage('collect'); collectgarbage('collect')")
        .unwrap();
    assert!(lua.state().gc.tables.get(payload).is_some());
    assert!(
        matches!(rilua::table_security::unwrap_secret(lua.state(), wrapper).unwrap(), Val::Table(value) if value == payload)
    );
}

#[test]
fn table_security_reused_arena_slots_drop_restrictions_and_secret_payload() {
    let mut lua = secure_lua();
    let state = lua.state_mut();
    let table = state.gc.alloc_table(Table::new());
    set_table_security(state, table, 0).unwrap();
    let wrapped = wrap_secret(state, Val::Table(table)).unwrap();
    let Val::Userdata(secret) = wrapped else {
        panic!("expected wrapper")
    };
    state.gc.userdata.free(secret);
    state.gc.tables.free(table);
    let replacement = state.gc.alloc_table(Table::new());
    let userdata = state
        .gc
        .alloc_userdata(rilua::vm::value::Userdata::new(Box::new(())));
    assert_eq!(table.index(), replacement.index());
    assert_ne!(table.generation(), replacement.generation());
    assert_eq!(secret.index(), userdata.index());
    assert_ne!(secret.generation(), userdata.generation());
    state.call_stack[0].taint = Some("Addon".to_owned());
    assert!(check_table_access(state, replacement, None).is_ok());
    assert!(!rilua::table_security::is_secret_value(
        state,
        Val::Userdata(userdata)
    ));
    assert!(!rilua::table_security::is_secret_value(state, wrapped));
}
