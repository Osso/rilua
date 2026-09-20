use rilua::Lua;

fn run(code: &str) {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.exec(code).unwrap();
}

#[test]
fn vm_security_rejects_tainted_existing_missing_and_numeric_access() {
    run(r#"
        local protected = { existing = 17, [1] = 23 }
        local ordinary = { existing = 31 }
        settablesecurity(protected, 0)
        assert(protected.existing == 17 and protected[1] == 23)
        local accessors = {
            function() return protected.existing end,
            function() return protected.missing end,
            function() return protected[1] end,
            function() protected.existing = 99 end,
            function() protected.missing = 99 end,
            function() protected[1] = 99 end,
        }
        for _, access in ipairs(accessors) do
            debug.setobjecttaint(access, 'Addon')
            assert(not pcall(access))
        end
        assert(protected.existing == 17 and protected.missing == nil)
        assert(protected[1] == 23)
        local function normal() ordinary.existing = 32; return ordinary.existing end
        debug.setobjecttaint(normal, 'Addon')
        assert(normal() == 32)
    "#);
}

#[test]
fn vm_security_checks_proxy_before_metamethod_and_redirected_target() {
    run(r#"
        local calls = 0
        local protected = setmetatable({}, {
            __index = function() calls = calls + 1; return 17 end,
            __newindex = function() calls = calls + 1 end,
        })
        settablesecurity(protected, 0)
        local function read() return protected.x end
        local function write() protected.x = 2 end
        debug.setobjecttaint(read, 'Addon')
        debug.setobjecttaint(write, 'Addon')
        assert(not pcall(read) and not pcall(write))
        assert(calls == 0)
        local backing = { x = 9 }
        settablesecurity(backing, 0)
        local proxy = setmetatable({}, { __index = backing, __newindex = backing })
        local function redirected_read() return proxy.x end
        local function redirected_write() proxy.y = 2 end
        debug.setobjecttaint(redirected_read, 'Addon')
        debug.setobjecttaint(redirected_write, 'Addon')
        assert(not pcall(redirected_read) and not pcall(redirected_write))
        assert(backing.x == 9 and backing.y == nil)
        local function_proxy = setmetatable({}, {
            __index = function(_, key) return backing[key] end,
            __newindex = function(_, key, value) backing[key] = value end,
        })
        local function function_read() return function_proxy.x end
        local function function_write() function_proxy.z = 2 end
        debug.setobjecttaint(function_read, 'Addon')
        debug.setobjecttaint(function_write, 'Addon')
        assert(not pcall(function_read) and not pcall(function_write))
        assert(backing.z == nil)
    "#);
}

#[test]
fn vm_security_rejects_secret_keys_and_allows_unwrapping_proxy() {
    run(r#"
        local backing = { [42] = 'original' }
        settablesecurity(backing, 1)
        local key = secretwrap(42)
        assert(not pcall(function() return backing[key] end))
        assert(not pcall(function() backing[key] = 'bad' end))
        local proxy = setmetatable({}, {
            __index = function(_, k) return backing[secretunwrap(k)] end,
            __newindex = function(_, k, v) backing[secretunwrap(k)] = v end,
        })
        for i = 1, 50 do
            assert(proxy[secretwrap(42)] == 'original')
            assert(backing[42] == 'original')
        end
        proxy[key] = 'updated'
        assert(proxy[secretwrap(42)] == 'updated' and backing[42] == 'updated')
        local invoked = false
        local guarded = setmetatable({}, { __index = function() invoked = true end })
        settablesecurity(guarded, 1)
        assert(not pcall(function() return guarded[key] end))
        assert(not invoked)
    "#);
}
