//! Escaped closures must survive error recovery and subsequent stack reuse.
use rilua::{Lua, LuaApiMut, LuaResult, Val};

const INSTALL: &str = r#"
local captured = "original"
saved = function() return captured end
error("deliberate")
"#;
const CHECK: &str = r#"
local function reuse(n)
    local a,b,c,d,e,f,g,h = "replacement",2,3,4,5,6,7,8
    if n > 0 then reuse(n - 1) end
    return a,b,c,d,e,f,g,h
end
reuse(12)
assert(saved() == "original", "escaped upvalue changed after unwind")
"#;

fn call(lua: &mut Lua, source: &str) -> LuaResult<Vec<Val>> {
    let function = lua.load(source)?;
    lua.call_function(&function, &[])
}

fn assert_deliberate<T>(result: LuaResult<T>) {
    assert!(result.is_err(), "call must fail");
    assert!(
        result
            .err()
            .is_some_and(|e| e.to_string().contains("deliberate"))
    );
}

fn check_recovery(source: &str) -> LuaResult<()> {
    let mut lua = Lua::new()?;
    let results = call(&mut lua, &format!("{source}\n{CHECK}\nreturn 42"))?;
    assert_eq!(
        results,
        vec![Val::Num(42.0)],
        "recovery must resume the caller"
    );
    Ok(())
}

#[test]
fn host_call_closes_escaped_upvalues() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    assert_deliberate(call(&mut lua, INSTALL));
    call(&mut lua, CHECK)?;
    Ok(())
}

#[test]
fn traced_host_call_closes_escaped_upvalues() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    let function = lua.load(INSTALL)?;
    assert_deliberate(lua.call_function_traced(&function, &[]));
    call(&mut lua, CHECK)?;
    Ok(())
}

#[test]
fn host_exec_closes_escaped_upvalues() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    assert_deliberate(lua.exec(INSTALL));
    lua.exec(CHECK)
}

#[test]
fn rust_callback_error_closes_escaped_upvalues() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    lua.register_function("fail_from_host", |_| {
        Err(rilua::runtime_error("deliberate"))
    })?;
    assert_deliberate(call(
        &mut lua,
        &INSTALL.replace("error(\"deliberate\")", "fail_from_host()"),
    ));
    call(&mut lua, CHECK)?;
    Ok(())
}

#[test]
fn metamethod_error_closes_escaped_upvalues() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    let source = format!(
        "local t = setmetatable({{}}, {{__index = function() {INSTALL} end}}); return t.missing"
    );
    assert_deliberate(call(&mut lua, &source));
    call(&mut lua, CHECK)?;
    Ok(())
}

#[test]
fn failed_xpcall_handler_closes_escaped_upvalues() -> LuaResult<()> {
    check_recovery(&format!(
        r#"
local ok, message = xpcall(function() error("initial") end, function() {INSTALL} end)
assert(not ok and message == "error in error handling")
"#
    ))
}

#[test]
fn first_load_reader_error_closes_escaped_upvalues() -> LuaResult<()> {
    check_recovery(&format!(
        r#"
local f, message = load(function() {INSTALL} end)
assert(f == nil and string.find(message, "deliberate"))
"#
    ))
}

#[test]
fn streaming_load_reader_error_closes_escaped_upvalues() -> LuaResult<()> {
    check_recovery(&format!(
        r#"
local count = 0
local f, message = load(function()
    count = count + 1
    if count == 1 then return "local x = " end
    {INSTALL}
end)
assert(f == nil and string.find(message, "deliberate"))
"#
    ))
}

#[test]
fn binary_load_reader_error_closes_escaped_upvalues() -> LuaResult<()> {
    check_recovery(&format!(
        r#"
local count = 0
local f, message = load(function()
    count = count + 1
    if count == 1 then return string.char(27) .. "Lua" end
    {INSTALL}
end)
assert(f == nil and string.find(message, "deliberate"))
"#
    ))
}

#[test]
fn finalizer_error_closes_escaped_upvalues() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    lua.register_function("collect_from_host", |state| {
        assert_deliberate(state.full_gc());
        Ok(0)
    })?;
    call(
        &mut lua,
        &format!(
            r#"
collectgarbage("stop")
local object = newproxy(true)
getmetatable(object).__gc = function() {INSTALL} end
object = nil
collect_from_host()
{CHECK}
"#
        ),
    )?;
    Ok(())
}

#[test]
fn hook_error_closes_escaped_upvalues() -> LuaResult<()> {
    let mut lua = Lua::new()?;
    let source = format!(
        r#"
debug.sethook(function() debug.sethook(); {INSTALL} end, "", 1)
local trigger = 1
"#
    );
    assert_deliberate(call(&mut lua, &source));
    call(&mut lua, CHECK)?;
    Ok(())
}

#[test]
fn pcall_preserves_outer_open_upvalues() -> LuaResult<()> {
    check_recovery(&format!(
        r#"
local outer = "before"
local read_outer = function() return outer end
local ok, message = pcall(function() {INSTALL} end)
assert(not ok and string.find(message, "deliberate"))
outer = "after"
assert(read_outer() == "after", "caller upvalue was closed too early")
"#
    ))
}

#[test]
fn coroutine_error_closes_escaped_upvalues() -> LuaResult<()> {
    check_recovery(&format!(
        r#"
local co = coroutine.create(function() {INSTALL} end)
local ok, message = coroutine.resume(co)
assert(not ok and string.find(message, "deliberate"))
assert(coroutine.status(co) == "dead")
"#
    ))
}

#[test]
fn remote_state_error_closes_escaped_upvalues() -> LuaResult<()> {
    let mut lua = Lua::new_with(rilua::StdLib::ALL | rilua::StdLib::TEST)?;
    call(
        &mut lua,
        &format!(
            r#"
local remote = T.newstate()
T.loadlib(remote)
T.doremote(remote, "baselibopen(); strlibopen()")
local result, code, message = T.doremote(remote, [=[{INSTALL}]=])
assert(result == nil and code == 2 and string.find(message, "deliberate"))
local checked = T.doremote(remote, [=[{CHECK} return "checked"]=])
T.closestate(remote)
assert(checked == "checked", "remote escaped upvalue changed after unwind")
"#
        ),
    )?;
    Ok(())
}
