//! End-to-end integration tests for rilua.
//!
//! These tests run Lua code through the full pipeline (compile -> execute)
//! via the `rilua` binary and verify stdout/stderr output.

#![allow(clippy::expect_used)]

use std::process::Command;

/// Helper: run `rilua -e <code>` and return (stdout, stderr, exit_code).
fn run_rilua(code: &str) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_rilua"))
        .arg("-e")
        .arg(code)
        .output()
        .expect("failed to run rilua binary");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

// ---------------------------------------------------------------------------
// print tests
// ---------------------------------------------------------------------------

#[test]
fn print_number() {
    let (stdout, _, code) = run_rilua("print(42)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn print_string() {
    let (stdout, _, code) = run_rilua("print(\"hello\")");
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello\n");
}

#[test]
fn print_multiple() {
    let (stdout, _, code) = run_rilua("print(1, 2, 3)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1\t2\t3\n");
}

#[test]
fn print_nil() {
    let (stdout, _, code) = run_rilua("print(nil)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "nil\n");
}

#[test]
fn print_bool() {
    let (stdout, _, code) = run_rilua("print(true, false)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "true\tfalse\n");
}

#[test]
fn print_no_args() {
    let (stdout, _, code) = run_rilua("print()");
    assert_eq!(code, 0);
    assert_eq!(stdout, "\n");
}

// ---------------------------------------------------------------------------
// Variable and expression tests
// ---------------------------------------------------------------------------

#[test]
fn variable_assignment() {
    let (stdout, _, code) = run_rilua("x = 42 print(x)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn arithmetic_print() {
    let (stdout, _, code) = run_rilua("print(1 + 2)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n");
}

#[test]
fn string_variable() {
    let (stdout, _, code) = run_rilua("x = \"world\" print(\"hello\", x)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello\tworld\n");
}

#[test]
fn multiple_assignments() {
    let (stdout, _, code) = run_rilua("a = 1 b = 2 print(a + b)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n");
}

#[test]
fn print_negative_number() {
    let (stdout, _, code) = run_rilua("print(-5)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "-5\n");
}

#[test]
fn print_float() {
    let (stdout, _, code) = run_rilua("print(3.14)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3.14\n");
}

// ---------------------------------------------------------------------------
// Error handling tests
// ---------------------------------------------------------------------------

#[test]
fn syntax_error() {
    let (_, stderr, code) = run_rilua("if then");
    assert_ne!(code, 0);
    assert!(!stderr.is_empty(), "syntax error should produce stderr");
}

#[test]
fn missing_e_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_rilua"))
        .arg("-e")
        .output()
        .expect("failed to run rilua binary");
    assert_ne!(output.status.code().unwrap_or(-1), 0);
}

#[test]
fn version_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_rilua"))
        .arg("-v")
        .output()
        .expect("failed to run rilua binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Lua 5.1.1"));
    assert_eq!(output.status.code().unwrap_or(-1), 0);
}

#[test]
fn nonexistent_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_rilua"))
        .arg("/tmp/rilua_nonexistent_file.lua")
        .output()
        .expect("failed to run rilua binary");
    assert_ne!(output.status.code().unwrap_or(-1), 0);
}

// ---------------------------------------------------------------------------
// Phase 4: stdlib functions
// ---------------------------------------------------------------------------

#[test]
fn type_function() {
    let (stdout, _, code) =
        run_rilua("print(type(1), type('s'), type(nil), type(true), type({}), type(print))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "number\tstring\tnil\tboolean\ttable\tfunction\n");
}

#[test]
fn tostring_function() {
    let (stdout, _, code) = run_rilua("print(tostring(42), tostring(nil), tostring(true))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\tnil\ttrue\n");
}

#[test]
fn tonumber_function() {
    let (stdout, _, code) = run_rilua("print(tonumber('42'), tonumber('ff', 16))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\t255\n");
}

#[test]
fn assert_function_success() {
    let (stdout, _, code) = run_rilua("print(assert(42, 'msg'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\tmsg\n");
}

#[test]
fn assert_function_failure() {
    let (_, stderr, code) = run_rilua("assert(false, 'test failed')");
    assert_ne!(code, 0);
    assert!(stderr.contains("test failed"));
}

#[test]
fn error_function() {
    let (_, stderr, code) = run_rilua("error('boom')");
    assert_ne!(code, 0);
    assert!(stderr.contains("boom"));
}

#[test]
fn pcall_success() {
    let (stdout, _, code) = run_rilua("print(pcall(function() return 1, 2, 3 end))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "true\t1\t2\t3\n");
}

#[test]
fn pcall_error() {
    let (stdout, _, code) = run_rilua("print(pcall(function() error('boom') end))");
    assert_eq!(code, 0);
    assert!(stdout.starts_with("false\t"));
    assert!(stdout.contains("boom"));
}

#[test]
fn xpcall_with_handler() {
    let (stdout, _, code) = run_rilua(
        "print(xpcall(function() error('boom') end, function(e) return 'caught: ' .. e end))",
    );
    assert_eq!(code, 0);
    assert!(stdout.starts_with("false\t"));
    assert!(stdout.contains("caught:"));
}

#[test]
fn select_count() {
    let (stdout, _, code) = run_rilua("print(select('#', 1, 2, 3))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n");
}

#[test]
fn select_range() {
    let (stdout, _, code) = run_rilua("print(select(2, 'a', 'b', 'c'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "b\tc\n");
}

#[test]
fn unpack_function() {
    let (stdout, _, code) = run_rilua("print(unpack({10, 20, 30}))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "10\t20\t30\n");
}

#[test]
fn rawget_rawset_rawequal() {
    let (stdout, _, code) = run_rilua(
        "local t = {}; rawset(t, 'x', 42); print(rawget(t, 'x'), rawequal(1, 1), rawequal(1, 2))",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\ttrue\tfalse\n");
}

// ---------------------------------------------------------------------------
// Phase 4: metamethods
// ---------------------------------------------------------------------------

#[test]
fn metamethod_add() {
    let (stdout, _, code) = run_rilua(
        "local t = setmetatable({}, {__add = function(a,b) return 42 end}); print(t + 1)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn metamethod_index_function() {
    let (stdout, _, code) = run_rilua(
        "local t = setmetatable({}, {__index = function(t,k) return k end}); print(t.hello)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello\n");
}

#[test]
fn metamethod_index_table() {
    let (stdout, _, code) = run_rilua(
        "local base = {x = 10}; local t = setmetatable({}, {__index = base}); print(t.x)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "10\n");
}

#[test]
fn metamethod_newindex() {
    let (stdout, _, code) = run_rilua(
        "local log = {}; local t = setmetatable({}, {__newindex = function(t,k,v) log[k] = v end}); t.x = 42; print(rawget(t, 'x'), log.x)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "nil\t42\n");
}

#[test]
fn metamethod_call() {
    let (stdout, _, code) = run_rilua(
        "local t = setmetatable({}, {__call = function(self, a, b) return a + b end}); print(t(3, 4))",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "7\n");
}

#[test]
fn metamethod_tostring() {
    let (stdout, _, code) = run_rilua(
        "local t = setmetatable({}, {__tostring = function(self) return 'mytable' end}); print(tostring(t))",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "mytable\n");
}

#[test]
fn metamethod_len_table_ignores() {
    // In Lua 5.1.1, __len is NOT called for tables (only for userdata).
    // Tables always use raw length.
    let (stdout, _, code) = run_rilua(
        "local t = setmetatable({1, 2, 3}, {__len = function(self) return 99 end}); print(#t)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n"); // Raw length, not 99.
}

#[test]
fn metamethod_unm() {
    let (stdout, _, code) = run_rilua(
        "local t = setmetatable({}, {__unm = function(self) return 'negated' end}); print(-t)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "negated\n");
}

#[test]
fn metamethod_concat() {
    let (stdout, _, code) = run_rilua(
        "local t = setmetatable({}, {__concat = function(a,b) return 'joined' end}); print(t .. 'x')",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "joined\n");
}

#[test]
fn metamethod_eq() {
    let (stdout, _, code) = run_rilua(
        "local mt = {__eq = function(a,b) return true end}; local a = setmetatable({}, mt); local b = setmetatable({}, mt); print(a == b, a == {})",
    );
    assert_eq!(code, 0);
    // a == b uses shared __eq, returns true. a == {} has no shared __eq, returns false.
    assert_eq!(stdout, "true\tfalse\n");
}

#[test]
fn metamethod_lt_le() {
    let (stdout, _, code) = run_rilua(
        "local mt = {__lt = function(a,b) return true end, __le = function(a,b) return true end}; local a = setmetatable({}, mt); local b = setmetatable({}, mt); print(a < b, a <= b)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "true\ttrue\n");
}

#[test]
fn setmetatable_getmetatable() {
    let (stdout, _, code) =
        run_rilua("local mt = {}; local t = setmetatable({}, mt); print(getmetatable(t) == mt)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "true\n");
}

#[test]
fn metatable_protection() {
    let (stdout, _, code) = run_rilua(
        "local t = setmetatable({}, {__metatable = 'protected'}); print(getmetatable(t)); print(pcall(setmetatable, t, {}))",
    );
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "protected\nfalse\tcannot change a protected metatable\n"
    );
}

#[test]
fn pcall_nested() {
    let (stdout, _, code) =
        run_rilua("print(pcall(function() return pcall(function() error('inner') end) end))");
    assert_eq!(code, 0);
    assert!(stdout.starts_with("true\tfalse\t"));
    assert!(stdout.contains("inner"));
}

// ---------------------------------------------------------------------------
// Phase 5a: globals, iterators, loading, environments
// ---------------------------------------------------------------------------

#[test]
fn version_global() {
    let (stdout, _, code) = run_rilua("print(_VERSION)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "Lua 5.1\n");
}

#[test]
fn g_global_is_table() {
    let (stdout, _, code) = run_rilua("print(type(_G))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "table\n");
}

#[test]
fn g_self_referential() {
    let (stdout, _, code) = run_rilua("print(_G == _G)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "true\n");
}

#[test]
fn g_contains_print() {
    let (stdout, _, code) = run_rilua("print(_G.print == print)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "true\n");
}

#[test]
fn next_basic() {
    let (stdout, _, code) = run_rilua("local k, v = next({a=1}) print(k, v)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "a\t1\n");
}

#[test]
fn next_nil_at_end() {
    let (stdout, _, code) = run_rilua("local t = {a=1} local k = next(t) print(next(t, k))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "nil\n");
}

#[test]
fn next_empty_table() {
    let (stdout, _, code) = run_rilua("print(next({}))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "nil\n");
}

#[test]
fn pairs_iteration() {
    let (stdout, _, code) = run_rilua(
        "local sum = 0; for k, v in pairs({a=1, b=2, c=3}) do sum = sum + v end; print(sum)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "6\n");
}

#[test]
fn ipairs_basic() {
    let (stdout, _, code) = run_rilua("for i, v in ipairs({10, 20, 30}) do print(i, v) end");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1\t10\n2\t20\n3\t30\n");
}

#[test]
fn ipairs_stops_at_nil() {
    let (stdout, _, code) = run_rilua(
        "local t = {1, 2, nil, 4}; local count = 0; for i, v in ipairs(t) do count = count + 1 end; print(count)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "2\n");
}

#[test]
fn loadstring_success() {
    let (stdout, _, code) = run_rilua("local f = loadstring('return 1+2') print(f())");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n");
}

#[test]
fn loadstring_error() {
    let (stdout, _, code) = run_rilua("local f, err = loadstring('if then') print(f, type(err))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "nil\tstring\n");
}

#[test]
fn loadstring_with_name() {
    let (stdout, _, code) = run_rilua(
        "local f, err = loadstring('error(\"test\")', 'mychunk') local ok, msg = pcall(f) print(msg)",
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("mychunk"));
    assert!(stdout.contains("test"));
}

#[test]
fn loadstring_returns_function() {
    let (stdout, _, code) = run_rilua("local f = loadstring('return 42') print(type(f), f())");
    assert_eq!(code, 0);
    assert_eq!(stdout, "function\t42\n");
}

#[test]
fn collectgarbage_count() {
    let (stdout, _, code) = run_rilua("print(type(collectgarbage('count')))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "number\n");
}

#[test]
fn collectgarbage_collect() {
    let (stdout, _, code) = run_rilua("collectgarbage() print('ok')");
    assert_eq!(code, 0);
    assert_eq!(stdout, "ok\n");
}

#[test]
fn collectgarbage_step() {
    let (stdout, _, code) = run_rilua("print(collectgarbage('step'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "false\n");
}

#[test]
fn getfenv_level_zero() {
    let (stdout, _, code) = run_rilua("print(getfenv(0) == _G)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "true\n");
}

#[test]
fn setfenv_level_zero() {
    let (stdout, _, code) = run_rilua(
        "local env = setmetatable({print=print}, {__index=_G}) setfenv(0, env) print('ok')",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "ok\n");
}

#[test]
fn setfenv_function() {
    let (stdout, _, code) = run_rilua(
        "local f = function() return x end local env = {x = 42} setfenv(f, env) print(f())",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn getfenv_returns_table() {
    let (stdout, _, code) = run_rilua("print(type(getfenv(1)))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "table\n");
}

#[test]
fn newproxy_basic() {
    let (stdout, _, code) = run_rilua("local p = newproxy() print(type(p))");
    assert_eq!(code, 0);
    // Our stub returns table since we don't have userdata yet.
    assert_eq!(stdout, "table\n");
}

#[test]
fn newproxy_with_metatable() {
    let (stdout, _, code) = run_rilua("local p = newproxy(true) print(type(getmetatable(p)))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "table\n");
}

#[test]
fn load_function() {
    let (stdout, _, code) = run_rilua(
        "local i = 0 local chunks = {'ret', 'urn ', '42'} local f = load(function() i = i + 1 return chunks[i] end) print(f())",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

// ---------------------------------------------------------------------------
// string library tests
// ---------------------------------------------------------------------------

#[test]
fn string_len() {
    let (stdout, _, code) = run_rilua("print(string.len('hello'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "5\n");
}

#[test]
fn string_len_empty() {
    let (stdout, _, code) = run_rilua("print(string.len(''))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "0\n");
}

#[test]
fn string_byte_single() {
    let (stdout, _, code) = run_rilua("print(string.byte('A'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "65\n");
}

#[test]
fn string_byte_range() {
    let (stdout, _, code) = run_rilua("print(string.byte('abc', 1, 3))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "97\t98\t99\n");
}

#[test]
fn string_char() {
    let (stdout, _, code) = run_rilua("print(string.char(72, 101))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "He\n");
}

#[test]
fn string_sub() {
    let (stdout, _, code) = run_rilua("print(string.sub('hello', 2, 4))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "ell\n");
}

#[test]
fn string_sub_negative() {
    let (stdout, _, code) = run_rilua("print(string.sub('hello', -3))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "llo\n");
}

#[test]
fn string_rep() {
    let (stdout, _, code) = run_rilua("print(string.rep('ab', 3))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "ababab\n");
}

#[test]
fn string_rep_zero() {
    let (stdout, _, code) = run_rilua("print(string.rep('ab', 0))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "\n");
}

#[test]
fn string_reverse() {
    let (stdout, _, code) = run_rilua("print(string.reverse('hello'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "olleh\n");
}

#[test]
fn string_lower() {
    let (stdout, _, code) = run_rilua("print(string.lower('Hello World'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello world\n");
}

#[test]
fn string_upper() {
    let (stdout, _, code) = run_rilua("print(string.upper('Hello World'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "HELLO WORLD\n");
}

#[test]
fn string_format_basic() {
    let (stdout, _, code) = run_rilua("print(string.format('%d %s', 42, 'hi'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42 hi\n");
}

#[test]
fn string_format_hex() {
    let (stdout, _, code) = run_rilua("print(string.format('%x', 255))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "ff\n");
}

#[test]
fn string_format_float() {
    let (stdout, _, code) = run_rilua("print(string.format('%f', 3.14))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3.140000\n");
}

#[test]
fn string_format_quoted() {
    let (stdout, _, code) = run_rilua(r#"print(string.format('%q', 'hello "world"'))"#);
    assert_eq!(code, 0);
    assert_eq!(stdout, "\"hello \\\"world\\\"\"\n");
}

#[test]
fn string_format_percent() {
    let (stdout, _, code) = run_rilua("print(string.format('100%%'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "100%\n");
}

#[test]
fn string_find_plain() {
    let (stdout, _, code) = run_rilua("print(string.find('hello world', 'world'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "7\t11\n");
}

#[test]
fn string_find_pattern() {
    let (stdout, _, code) = run_rilua("print(string.find('hello123', '%d+'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "6\t8\n");
}

#[test]
fn string_find_not_found() {
    let (stdout, _, code) = run_rilua("print(string.find('hello', 'xyz'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "nil\n");
}

#[test]
fn string_match_captures() {
    let (stdout, _, code) = run_rilua("print(string.match('2024-01-15', '(%d+)-(%d+)-(%d+)'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "2024\t01\t15\n");
}

#[test]
fn string_match_no_capture() {
    let (stdout, _, code) = run_rilua("print(string.match('hello', '%a+'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello\n");
}

#[test]
fn string_gmatch_iteration() {
    let (stdout, _, code) = run_rilua(
        "local t = {} for w in string.gmatch('hello world foo', '%a+') do t[#t+1] = w end print(t[1], t[2], t[3])",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello\tworld\tfoo\n");
}

#[test]
fn string_gsub_basic() {
    let (stdout, _, code) = run_rilua("print(string.gsub('hello', 'l', 'L'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "heLLo\t2\n");
}

#[test]
fn string_gsub_with_limit() {
    let (stdout, _, code) = run_rilua("print(string.gsub('hello', 'l', 'L', 1))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "heLlo\t1\n");
}

#[test]
fn string_gsub_pattern() {
    let (stdout, _, code) = run_rilua("print(string.gsub('abc123', '%d', '*'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "abc***\t3\n");
}

#[test]
fn string_method_upper() {
    let (stdout, _, code) = run_rilua("print(('hello'):upper())");
    assert_eq!(code, 0);
    assert_eq!(stdout, "HELLO\n");
}

#[test]
fn string_method_len() {
    let (stdout, _, code) = run_rilua("print(('abc'):len())");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n");
}

#[test]
fn string_method_sub() {
    let (stdout, _, code) = run_rilua("print(('hello'):sub(1, 3))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "hel\n");
}

#[test]
fn string_method_rep() {
    let (stdout, _, code) = run_rilua("print(('ab'):rep(3))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "ababab\n");
}

#[test]
fn string_method_reverse() {
    let (stdout, _, code) = run_rilua("print(('hello'):reverse())");
    assert_eq!(code, 0);
    assert_eq!(stdout, "olleh\n");
}

#[test]
fn string_method_find() {
    let (stdout, _, code) = run_rilua("print(('hello world'):find('world'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "7\t11\n");
}

#[test]
fn string_method_format() {
    let (stdout, _, code) = run_rilua("print(string.format('%s=%d', 'x', 10))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "x=10\n");
}

#[test]
fn string_method_gsub() {
    let (stdout, _, code) = run_rilua("print(('aaa'):gsub('a', 'b'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "bbb\t3\n");
}

#[test]
fn string_method_byte() {
    let (stdout, _, code) = run_rilua("print(('ABC'):byte(1, 3))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "65\t66\t67\n");
}

#[test]
fn string_method_lower() {
    let (stdout, _, code) = run_rilua("print(('HELLO'):lower())");
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello\n");
}

#[test]
fn string_find_with_captures() {
    let (stdout, _, code) = run_rilua("print(string.find('hello world', '(w%a+)'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "7\t11\tworld\n");
}

#[test]
fn string_format_g() {
    let (stdout, _, code) = run_rilua("print(string.format('%g', 100000))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "100000\n");
}

#[test]
fn string_format_g_scientific() {
    let (stdout, _, code) = run_rilua("print(string.format('%g', 1e10))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1e+10\n");
}

#[test]
fn string_gsub_function_replacement() {
    let (stdout, _, code) =
        run_rilua("print(string.gsub('abc', '%a', function(c) return string.upper(c) end))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "ABC\t3\n");
}

#[test]
fn string_gsub_table_replacement() {
    let (stdout, _, code) =
        run_rilua("local t = {a='A', b='B'} print(string.gsub('aXb', '%a', t))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "AXB\t3\n");
}

#[test]
fn string_find_anchor() {
    let (stdout, _, code) = run_rilua("print(string.find('hello', '^hel'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1\t3\n");
}

#[test]
fn string_find_anchor_no_match() {
    let (stdout, _, code) = run_rilua("print(string.find('hello', '^llo'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "nil\n");
}

#[test]
fn string_match_empty_capture() {
    let (stdout, _, code) = run_rilua("print(string.match('hello', '()h'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1\n");
}

#[test]
fn string_global_type() {
    let (stdout, _, code) = run_rilua("print(type(string))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "table\n");
}

#[test]
fn string_global_has_functions() {
    let (stdout, _, code) = run_rilua("print(type(string.len), type(string.format))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "function\tfunction\n");
}

// ---------------------------------------------------------------------------
// table library tests
// ---------------------------------------------------------------------------

#[test]
fn table_global_type() {
    let (stdout, _, code) = run_rilua("print(type(table))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "table\n");
}

#[test]
fn table_global_has_functions() {
    let (stdout, _, code) =
        run_rilua("print(type(table.concat), type(table.insert), type(table.sort))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "function\tfunction\tfunction\n");
}

// -- table.concat --

#[test]
fn table_concat_basic() {
    let (stdout, _, code) = run_rilua("print(table.concat({1, 2, 3}, ', '))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1, 2, 3\n");
}

#[test]
fn table_concat_strings() {
    let (stdout, _, code) = run_rilua("print(table.concat({'a', 'b', 'c'}))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "abc\n");
}

#[test]
fn table_concat_default_sep() {
    let (stdout, _, code) = run_rilua("print(table.concat({'x', 'y', 'z'}))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "xyz\n");
}

#[test]
fn table_concat_range() {
    let (stdout, _, code) = run_rilua("print(table.concat({'a', 'b', 'c', 'd'}, '-', 2, 3))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "b-c\n");
}

#[test]
fn table_concat_empty() {
    let (stdout, _, code) = run_rilua("print(table.concat({}, ', '))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "\n");
}

#[test]
fn table_concat_single() {
    let (stdout, _, code) = run_rilua("print(table.concat({'only'}, ', '))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "only\n");
}

#[test]
fn table_concat_numbers() {
    let (stdout, _, code) = run_rilua("print(table.concat({10, 20, 30}, '+'))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "10+20+30\n");
}

#[test]
fn table_concat_error_non_string() {
    let (_, stderr, code) = run_rilua("table.concat({1, true, 3}, ', ')");
    assert_ne!(code, 0);
    assert!(
        stderr.contains("table contains non-strings"),
        "stderr: {stderr}"
    );
}

// -- table.insert --

#[test]
fn table_insert_append() {
    let (stdout, _, code) =
        run_rilua("local t = {1, 2, 3}; table.insert(t, 4); print(t[1], t[2], t[3], t[4])");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1\t2\t3\t4\n");
}

#[test]
fn table_insert_at_position() {
    let (stdout, _, code) =
        run_rilua("local t = {1, 2, 3}; table.insert(t, 2, 99); print(t[1], t[2], t[3], t[4])");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1\t99\t2\t3\n");
}

#[test]
fn table_insert_at_start() {
    let (stdout, _, code) =
        run_rilua("local t = {1, 2, 3}; table.insert(t, 1, 0); print(t[1], t[2], t[3], t[4])");
    assert_eq!(code, 0);
    assert_eq!(stdout, "0\t1\t2\t3\n");
}

#[test]
fn table_insert_wrong_args() {
    let (_, stderr, code) = run_rilua("table.insert({})");
    assert_ne!(code, 0);
    assert!(
        stderr.contains("wrong number of arguments to 'insert'"),
        "stderr: {stderr}"
    );
}

// -- table.remove --

#[test]
fn table_remove_last() {
    let (stdout, _, code) =
        run_rilua("local t = {1, 2, 3}; local v = table.remove(t); print(v, #t, t[1], t[2])");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\t2\t1\t2\n");
}

#[test]
fn table_remove_at_position() {
    let (stdout, _, code) =
        run_rilua("local t = {1, 2, 3}; local v = table.remove(t, 2); print(v, #t, t[1], t[2])");
    assert_eq!(code, 0);
    assert_eq!(stdout, "2\t2\t1\t3\n");
}

#[test]
fn table_remove_empty() {
    // table.remove on empty table returns nothing.
    let (stdout, _, code) = run_rilua("local t = {}; print(table.remove(t))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "\n");
}

#[test]
fn table_remove_return_value() {
    let (stdout, _, code) = run_rilua("local t = {'a', 'b', 'c'}; print(table.remove(t, 1))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "a\n");
}

// -- table.sort --

#[test]
fn table_sort_numbers() {
    let (stdout, _, code) = run_rilua(
        "local t = {3, 1, 4, 1, 5, 9, 2, 6}; table.sort(t); print(table.concat(t, ', '))",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "1, 1, 2, 3, 4, 5, 6, 9\n");
}

#[test]
fn table_sort_strings() {
    let (stdout, _, code) = run_rilua(
        "local t = {'banana', 'apple', 'cherry'}; table.sort(t); print(table.concat(t, ', '))",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "apple, banana, cherry\n");
}

#[test]
fn table_sort_custom_comparator() {
    let (stdout, _, code) = run_rilua(
        "local t = {3, 1, 4, 1, 5}; table.sort(t, function(a, b) return a > b end); print(table.concat(t, ', '))",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "5, 4, 3, 1, 1\n");
}

#[test]
fn table_sort_empty() {
    let (stdout, _, code) = run_rilua("local t = {}; table.sort(t); print(#t)");
    assert_eq!(code, 0);
    assert_eq!(stdout, "0\n");
}

#[test]
fn table_sort_single() {
    let (stdout, _, code) = run_rilua("local t = {42}; table.sort(t); print(t[1])");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn table_sort_already_sorted() {
    let (stdout, _, code) =
        run_rilua("local t = {1, 2, 3, 4, 5}; table.sort(t); print(table.concat(t, ', '))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1, 2, 3, 4, 5\n");
}

#[test]
fn table_sort_reverse_sorted() {
    let (stdout, _, code) =
        run_rilua("local t = {5, 4, 3, 2, 1}; table.sort(t); print(table.concat(t, ', '))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1, 2, 3, 4, 5\n");
}

// -- table.maxn --

#[test]
fn table_maxn_basic() {
    let (stdout, _, code) = run_rilua("print(table.maxn({1, 2, 3}))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n");
}

#[test]
fn table_maxn_empty() {
    let (stdout, _, code) = run_rilua("print(table.maxn({}))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "0\n");
}

#[test]
fn table_maxn_float_keys() {
    let (stdout, _, code) =
        run_rilua("local t = {}; t[1] = 'a'; t[3.5] = 'b'; t[100] = 'c'; print(table.maxn(t))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "100\n");
}

// -- table.getn --

#[test]
fn table_getn_basic() {
    let (stdout, _, code) = run_rilua("print(table.getn({10, 20, 30}))");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n");
}

// -- table.setn --

#[test]
fn table_setn_error() {
    let (_, stderr, code) = run_rilua("table.setn({}, 5)");
    assert_ne!(code, 0);
    assert!(stderr.contains("'setn' is obsolete"), "stderr: {stderr}");
}

// -- table.foreach / foreachi --

#[test]
fn table_foreachi_basic() {
    let (stdout, _, code) = run_rilua(
        "local r = '' table.foreachi({10, 20, 30}, function(i, v) r = r .. i .. '=' .. v .. ' ' end) print(r)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "1=10 2=20 3=30 \n");
}

#[test]
fn table_foreachi_early_return() {
    let (stdout, _, code) = run_rilua(
        "local v = table.foreachi({10, 20, 30}, function(i, v) if v == 20 then return 'found' end end) print(v)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "found\n");
}

#[test]
fn table_foreach_basic() {
    // foreach iterates all keys -- order is not guaranteed for hash part,
    // so test with only integer keys where order matches foreachi.
    let (stdout, _, code) = run_rilua(
        "local sum = 0 table.foreach({10, 20, 30}, function(k, v) sum = sum + v end) print(sum)",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "60\n");
}

#[test]
fn table_foreach_early_return() {
    let (stdout, _, code) = run_rilua(
        "local v = table.foreach({a=1, b=2}, function(k, v) if v == 1 then return k end end) print(type(v))",
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, "string\n");
}
