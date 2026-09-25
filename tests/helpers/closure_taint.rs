use rilua::{Function, Lua, LuaApi, LuaApiMut, stdlib::taint};

#[test]
fn stamped_closures_protect_tables_across_call_boundaries() {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.exec(
        r#"
        debug.settaintmode(true)
        local protected = { value = 17 }
        settablesecurity(protected, 0)
        local read = function() return protected.value end
        local write = function() protected.value = 99 end
        debug.setobjecttaint(read, 'Addon')
        debug.setobjecttaint(write, 'Addon')
        assert(not pcall(read), 'stamped read bypassed security')
        assert(not pcall(write), 'stamped write bypassed security')
        local function tail() return read() end
        assert(not pcall(tail), 'tail call bypassed security')
        local ok, allowed = pcall(function() return pcall(read) end)
        assert(ok and not allowed, 'nested pcall bypassed security')
        local callbacks = { read, write }
        collectgarbage('collect')
        for _, callback in ipairs(callbacks) do
            assert(not pcall(callback), 'delayed callback bypassed security')
        end
        local co = coroutine.create(read)
        assert(not coroutine.resume(co), 'coroutine bypassed security')
        assert(protected.value == 17, 'failed write changed table')
        assert(debug.getstacktaint() == nil, 'callee taint escaped to caller')
        debug.setobjecttaint(read, nil)
        assert(read() == 17, 'cleared closure stayed tainted')
    "#,
    )
    .unwrap();
}

#[test]
fn secure_calls_clear_caller_taint_but_not_callee_stamps() {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.exec(
        r#"
        debug.settaintmode(true)
        local protected = { value = 17 }
        settablesecurity(protected, 0)
        local function clean() return protected.value end
        local function addon() return securecallfunction(clean) end
        debug.setobjecttaint(addon, 'Addon')
        assert(addon() == 17, 'secure call failed to clear caller taint')
        local function stamped() return debug.getstacktaint() end
        debug.setobjecttaint(stamped, 'OtherAddon')
        assert(securecallfunction(stamped) == 'OtherAddon', 'secure call erased callee stamp')
    "#,
    )
    .unwrap();
}

#[test]
fn nested_closures_created_by_addon_remain_tainted_across_secure_calls_and_gc() {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.exec(
        r#"
        debug.settaintmode(true)
        local protected = {value = 17}
        settablesecurity(protected, 0)
        local function make() return function() return protected.value end end
        debug.setobjecttaint(make, 'Addon')
        local callback = make()
        assert(not pcall(securecallfunction, callback))
        collectgarbage('collect')
        assert(not pcall(securecallfunction, callback))
        local function trusted() return function() return protected.value end end
        assert(securecallfunction(trusted()) == 17)
        callback = nil
        collectgarbage('collect')
        collectgarbage('collect')
        assert(securecallfunction(trusted()) == 17)
    "#,
    )
    .unwrap();
}

#[test]
fn trusted_factory_inherits_tainted_caller_unless_securely_called() {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.exec(
        r#"
        debug.settaintmode(true)
        local protected = {value = 17}
        settablesecurity(protected, 0)
        local function factory()
            return function() return protected.value end
        end
        local function addon()
            local inherited = factory()
            local clean = securecallfunction(factory)
            return inherited, clean
        end
        debug.setobjecttaint(addon, 'Addon')
        local inherited, clean = addon()
        assert(not pcall(securecallfunction, inherited), 'trusted factory lost caller taint')
        assert(securecallfunction(clean) == 17, 'secure factory inherited suspended caller taint')
        collectgarbage('collect')
        assert(not pcall(securecallfunction, inherited), 'inherited stamp lost after GC')
        assert(securecallfunction(clean) == 17, 'clean stamp changed after GC')
    "#,
    )
    .unwrap();
}

#[test]
fn collected_closure_stamp_does_not_transfer_to_reused_slot() {
    let mut lua = Lua::new().unwrap();
    lua.exec(
        r#"
        local function make() return function() return debug.getstacktaint() end end
        for i = 1, 12 do
            local stale = make()
            debug.setobjecttaint(stale, 'CollectedClosureProbe')
            assert(securecallfunction(stale) == 'CollectedClosureProbe')
            stale = nil
            collectgarbage('collect')
            local fresh = make()
            assert(securecallfunction(fresh) == nil, 'collected stamp leaked at iteration ' .. i)
        end
        "#,
    )
    .unwrap();
}

#[test]
fn host_and_lua_share_live_closure_stamps_across_gc() {
    let mut lua = Lua::new().unwrap();
    lua.exec("function callback() return debug.getstacktaint() end")
        .unwrap();
    let callback: Function = lua.global("callback").unwrap();
    taint::set_closure_taint(lua.state_mut(), callback.gc_ref(), Some("HostAddon")).unwrap();
    assert_eq!(
        taint::get_closure_taint(lua.state_mut(), callback.gc_ref()).as_deref(),
        Some("HostAddon")
    );
    lua.exec("collectgarbage('collect'); assert(securecallfunction(callback) == 'HostAddon'); debug.setobjecttaint(callback, 'LuaAddon')")
        .unwrap();
    assert_eq!(
        taint::get_closure_taint(lua.state_mut(), callback.gc_ref()).as_deref(),
        Some("LuaAddon")
    );
    taint::set_closure_taint(lua.state_mut(), callback.gc_ref(), None).unwrap();
    lua.exec("assert(securecallfunction(callback) == nil)")
        .unwrap();
}

#[test]
fn collected_host_stamped_closure_is_not_kept_alive() {
    let mut lua = Lua::new().unwrap();
    lua.exec("function callback() return debug.getstacktaint() end")
        .unwrap();
    let callback: Function = lua.global("callback").unwrap();
    taint::set_closure_taint(lua.state_mut(), callback.gc_ref(), Some("HostAddon")).unwrap();
    lua.exec("callback = nil; collectgarbage('collect'); collectgarbage('collect')")
        .unwrap();
    assert!(lua.state().gc.closures.get(callback.gc_ref()).is_none());
    assert_eq!(
        taint::get_closure_taint(lua.state_mut(), callback.gc_ref()),
        None
    );
    lua.exec(
        "for key in pairs(debug.getregistry().__closure_taint) do error('dead stamp retained') end",
    )
    .unwrap();
    assert!(taint::set_closure_taint(lua.state_mut(), callback.gc_ref(), Some("Stale")).is_err());
}
