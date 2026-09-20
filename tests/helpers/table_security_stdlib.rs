use rilua::Lua;

fn secure_lua() -> Lua {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.exec("debug.settaintmode(true)").unwrap();
    lua
}

#[test]
fn protected_tables_reject_tainted_stdlib_access() {
    secure_lua()
        .exec(
            r#"
        local t = {3, 1, 2, label = 'private'}
        settablesecurity(t, 0)
        local operations = {
            function() return rawget(t, 1) end,
            function() rawset(t, 1, 9) end,
            function() return next(t) end,
            function() return pairs(t) end,
            function() return ipairs(t) end,
            function() return unpack(t) end,
            function() return getmetatable(t) end,
            function() setmetatable(t, {}) end,
            function() return table.getn(t) end,
            function() return table.maxn(t) end,
            function() return table.concat(t, ',') end,
            function() table.insert(t, 4) end,
            function() table.remove(t) end,
            function() table.sort(t) end,
            function() table.foreach(t, function() end) end,
            function() table.foreachi(t, function() end) end,
        }
        for _, operation in ipairs(operations) do
            debug.setobjecttaint(operation, 'TestAddon')
            local ok, message = pcall(operation)
            assert(not ok and string.find(message, 'taint'), tostring(message))
        end
        assert(rawget(t, 1) == 3 and rawget(t, 'label') == 'private')
        table.sort(t)
        assert(table.concat(t, ',') == '1,2,3')
    "#,
        )
        .unwrap();
}

#[test]
fn raw_secret_keys_are_rejected_but_unwrapped_keys_work() {
    secure_lua()
        .exec(
            r#"
        local t = {}
        settablesecurity(t, 1)
        local wrapped = secretwrap('key')
        assert(not pcall(rawset, t, wrapped, 1))
        assert(not pcall(rawget, t, wrapped))
        assert(not pcall(next, t, wrapped))
        rawset(t, secretunwrap(wrapped), 42)
        assert(rawget(t, 'key') == 42)
    "#,
        )
        .unwrap();
}

#[test]
fn captured_iterators_recheck_tainted_callers() {
    secure_lua()
        .exec(
            r#"
        local t = {42}
        local iterator, target, index = ipairs(t)
        local next_pair = next
        settablesecurity(t, 0)
        local function iterate()
            return iterator(target, index)
        end
        local function pair()
            return next_pair(t)
        end
        debug.setobjecttaint(iterate, 'TestAddon')
        debug.setobjecttaint(pair, 'TestAddon')
        assert(not pcall(iterate))
        assert(not pcall(pair))
    "#,
        )
        .unwrap();
}
