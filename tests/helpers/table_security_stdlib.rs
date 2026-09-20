use rilua::Lua;

fn secure_lua() -> Lua {
    let mut lua = Lua::new().unwrap();
    rilua::table_security::register_table_security(&mut lua).unwrap();
    lua.exec("debug.settaintmode(true); function call_tainted(fn) debug.setstacktaint('TestAddon'); return fn() end").unwrap();
    lua
}

#[test]
fn protected_tables_reject_tainted_stdlib_access() {
    secure_lua()
        .exec(
            r#"
        local t = {3, 1, 2, label = 'private', year = 2026, month = 9, day = 19}
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
            function() return debug.getmetatable(t) end,
            function() debug.setmetatable(t, {}) end,
            function() secureexecuterange(t, function() end) end,
            function() return os.time(t) end,
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
            local ok, message = pcall(call_tainted, operation)
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
        assert(not pcall(call_tainted, iterate))
        assert(not pcall(call_tainted, pair))
    "#,
        )
        .unwrap();
}

#[test]
fn host_functions_using_handles_cannot_bypass_caller_security() {
    use rilua::vm::state::LuaState;
    use rilua::{FromLua, LuaApiMut, LuaResult, Val};
    fn read(state: &mut LuaState) -> LuaResult<u32> {
        let Val::Table(reference) = state.stack_get(state.base) else {
            panic!("table fixture")
        };
        let handle = rilua::Table::from_lua(Val::Table(reference), state)?;
        let value = handle.raw_get(state, Val::Num(1.0))?;
        state.push(value);
        Ok(1)
    }
    fn write(state: &mut LuaState) -> LuaResult<u32> {
        let Val::Table(reference) = state.stack_get(state.base) else {
            panic!("table fixture")
        };
        rilua::Table::from_lua(Val::Table(reference), state)?.raw_set(
            state,
            Val::Num(1.0),
            Val::Num(99.0),
        )?;
        Ok(0)
    }
    fn metatable(state: &mut LuaState) -> LuaResult<u32> {
        let Val::Table(reference) = state.stack_get(state.base) else {
            panic!("table fixture")
        };
        rilua::Table::from_lua(Val::Table(reference), state)?.set_metatable(state, None)?;
        Ok(0)
    }
    let mut lua = secure_lua();
    lua.register_function("host_read", read).unwrap();
    lua.register_function("host_write", write).unwrap();
    lua.register_function("host_metatable", metatable).unwrap();
    lua.exec(
        r#"
        local t = {42}
        settablesecurity(t, 0)
        assert(host_read(t) == 42)
        for _, operation in ipairs({host_read, host_write, host_metatable}) do
            local function invoke() return operation(t) end
            assert(not pcall(call_tainted, invoke))
        end
        assert(host_read(t) == 42)
    "#,
    )
    .unwrap();
}
