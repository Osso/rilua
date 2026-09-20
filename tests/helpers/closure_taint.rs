use rilua::Lua;

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
