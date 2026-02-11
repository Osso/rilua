//! Oracle comparison tests: verify PUC-Rio Lua 5.1.1 reference binary
//! integration and establish baseline for future rilua comparisons.

#[allow(dead_code, unreachable_pub)]
mod helpers;

use helpers::oracle;

#[test]
fn reference_binary_exists() {
    if !oracle::reference_available() {
        eprintln!("skipping: reference Lua binary not available");
        return;
    }
    let bin = oracle::reference_bin();
    assert!(
        bin.exists(),
        "reference binary should exist at {}",
        bin.display()
    );
}

#[test]
fn reference_print_hello() {
    let Some(result) = oracle::run_reference("print('hello')") else {
        eprintln!("skipping: reference Lua binary not available");
        return;
    };
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "hello\n");
    assert!(result.stderr.is_empty());
}

#[test]
fn reference_arithmetic() {
    oracle::assert_reference_output("print(1 + 2)", "3\n");
}

#[test]
fn reference_string_concat() {
    oracle::assert_reference_output("print('foo' .. 'bar')", "foobar\n");
}

#[test]
fn reference_multiple_values() {
    oracle::assert_reference_output("print(1, 2, 3)", "1\t2\t3\n");
}

#[test]
fn reference_tostring_coercion() {
    oracle::assert_reference_output("print(tostring(42))", "42\n");
}

#[test]
fn reference_type_function() {
    oracle::assert_reference_output("print(type(nil))", "nil\n");
}

#[test]
fn reference_syntax_error() {
    let Some(result) = oracle::run_reference("if then") else {
        eprintln!("skipping: reference Lua binary not available");
        return;
    };
    assert_ne!(
        result.exit_code, 0,
        "syntax error should produce non-zero exit"
    );
    assert!(
        result.stderr.contains("'then'") || result.stderr.contains("syntax"),
        "stderr should mention the syntax error: {}",
        result.stderr
    );
}

#[test]
fn reference_runtime_error() {
    let Some(result) = oracle::run_reference("error('boom')") else {
        eprintln!("skipping: reference Lua binary not available");
        return;
    };
    assert_ne!(
        result.exit_code, 0,
        "runtime error should produce non-zero exit"
    );
    assert!(
        result.stderr.contains("boom"),
        "stderr should contain error message: {}",
        result.stderr
    );
}

#[test]
fn reference_version_string() {
    let Some(result) = oracle::run_reference("print(_VERSION)") else {
        eprintln!("skipping: reference Lua binary not available");
        return;
    };
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "Lua 5.1\n");
}

// ---------------------------------------------------------------------------
// Oracle comparison tests: rilua vs PUC-Rio
// ---------------------------------------------------------------------------

#[test]
fn oracle_print_hello() {
    oracle::assert_matches_reference("print('hello')");
}

#[test]
fn oracle_arithmetic() {
    oracle::assert_matches_reference("print(1 + 2)");
}

#[test]
fn oracle_multiple_values() {
    oracle::assert_matches_reference("print(1, 2, 3)");
}

#[test]
fn oracle_print_nil() {
    oracle::assert_matches_reference("print(nil)");
}

#[test]
fn oracle_print_bool() {
    oracle::assert_matches_reference("print(true, false)");
}

#[test]
fn oracle_print_no_args() {
    oracle::assert_matches_reference("print()");
}

#[test]
fn oracle_variable_assignment() {
    oracle::assert_matches_reference("x = 42 print(x)");
}

#[test]
fn oracle_print_negative() {
    oracle::assert_matches_reference("print(-5)");
}

#[test]
fn oracle_print_float() {
    oracle::assert_matches_reference("print(3.14)");
}

// ---------------------------------------------------------------------------
// Phase 4 oracle comparison tests
// ---------------------------------------------------------------------------

#[test]
fn oracle_type_function() {
    oracle::assert_matches_reference(
        "print(type(1), type('s'), type(nil), type(true), type({}), type(print))",
    );
}

#[test]
fn oracle_tostring() {
    oracle::assert_matches_reference("print(tostring(42), tostring(nil), tostring(true))");
}

#[test]
fn oracle_tonumber() {
    oracle::assert_matches_reference("print(tonumber('42'), tonumber('ff', 16))");
}

#[test]
fn oracle_pcall_success() {
    oracle::assert_matches_reference("print(pcall(function() return 1, 2, 3 end))");
}

#[test]
fn oracle_pcall_error() {
    oracle::assert_matches_reference("print(pcall(function() error('boom') end))");
}

#[test]
fn oracle_xpcall() {
    oracle::assert_matches_reference(
        "print(xpcall(function() error('boom') end, function(e) return 'caught: ' .. e end))",
    );
}

#[test]
fn oracle_select_count() {
    oracle::assert_matches_reference("print(select('#', 1, 2, 3))");
}

#[test]
fn oracle_select_range() {
    oracle::assert_matches_reference("print(select(2, 'a', 'b', 'c'))");
}

#[test]
fn oracle_unpack() {
    oracle::assert_matches_reference("print(unpack({10, 20, 30}))");
}

#[test]
fn oracle_rawequal() {
    oracle::assert_matches_reference("print(rawequal(1, 1), rawequal(1, 2))");
}

#[test]
fn oracle_assert_success() {
    oracle::assert_matches_reference("print(assert(42, 'msg'))");
}

#[test]
fn oracle_metamethod_add() {
    oracle::assert_matches_reference(
        "local t = setmetatable({}, {__add = function(a,b) return 42 end}); print(t + 1)",
    );
}

#[test]
fn oracle_metamethod_index() {
    oracle::assert_matches_reference(
        "local t = setmetatable({}, {__index = function(t,k) return k end}); print(t.hello)",
    );
}

#[test]
fn oracle_metamethod_call() {
    oracle::assert_matches_reference(
        "local t = setmetatable({}, {__call = function(self, a, b) return a + b end}); print(t(3, 4))",
    );
}

#[test]
fn oracle_metamethod_len_table_ignores() {
    // In Lua 5.1.1, __len is NOT called for tables (only for userdata).
    oracle::assert_matches_reference(
        "local t = setmetatable({1, 2, 3}, {__len = function(self) return 99 end}); print(#t)",
    );
}

#[test]
fn oracle_metamethod_unm() {
    oracle::assert_matches_reference(
        "local t = setmetatable({}, {__unm = function(self) return 'negated' end}); print(-t)",
    );
}

#[test]
fn oracle_metamethod_concat() {
    oracle::assert_matches_reference(
        "local t = setmetatable({}, {__concat = function(a,b) return 'joined' end}); print(t .. 'x')",
    );
}

#[test]
fn oracle_nested_pcall() {
    oracle::assert_matches_reference(
        "print(pcall(function() return pcall(function() error('inner') end) end))",
    );
}

#[test]
fn oracle_error_non_string() {
    oracle::assert_matches_reference("print(pcall(function() error(42) end))");
}

#[test]
fn oracle_pcall_no_error() {
    oracle::assert_matches_reference("print(pcall(print, 'hello'))");
}

// ---------------------------------------------------------------------------
// Phase 5a oracle comparison tests
// ---------------------------------------------------------------------------

#[test]
fn oracle_version() {
    oracle::assert_matches_reference("print(_VERSION)");
}

#[test]
fn oracle_g_type() {
    oracle::assert_matches_reference("print(type(_G))");
}

#[test]
fn oracle_g_self_ref() {
    oracle::assert_matches_reference("print(_G == _G)");
}

#[test]
fn oracle_next_basic() {
    // next on single-element table.
    oracle::assert_matches_reference("local k, v = next({x=1}) print(k, v)");
}

#[test]
fn oracle_next_empty() {
    oracle::assert_matches_reference("print(next({}))");
}

#[test]
fn oracle_ipairs() {
    oracle::assert_matches_reference("for i, v in ipairs({10, 20, 30}) do print(i, v) end");
}

#[test]
fn oracle_ipairs_stops_at_nil() {
    oracle::assert_matches_reference(
        "local t = {1, 2, nil, 4} local c = 0 for i, v in ipairs(t) do c = c + 1 end print(c)",
    );
}

#[test]
fn oracle_pairs_sum() {
    oracle::assert_matches_reference(
        "local s = 0 for k, v in pairs({a=1, b=2, c=3}) do s = s + v end print(s)",
    );
}

#[test]
fn oracle_loadstring_success() {
    oracle::assert_matches_reference("local f = loadstring('return 1+2') print(f())");
}

#[test]
fn oracle_loadstring_nil_on_error() {
    oracle::assert_matches_reference("local f, err = loadstring('if then') print(f, type(err))");
}

#[test]
fn oracle_collectgarbage_step() {
    oracle::assert_matches_reference("print(collectgarbage('step'))");
}

#[test]
fn oracle_getfenv_zero() {
    oracle::assert_matches_reference("print(getfenv(0) == _G)");
}

#[test]
fn oracle_setfenv_function() {
    oracle::assert_matches_reference(
        "local f = function() return x end local env = {x = 42} setfenv(f, env) print(f())",
    );
}

#[test]
fn oracle_load_function() {
    oracle::assert_matches_reference(
        "local i = 0 local chunks = {'ret', 'urn ', '42'} local f = load(function() i = i + 1 return chunks[i] end) print(f())",
    );
}

// ---------------------------------------------------------------------------
// String library oracle tests
// ---------------------------------------------------------------------------

#[test]
fn oracle_string_len() {
    oracle::assert_matches_reference("print(string.len('hello'))");
}

#[test]
fn oracle_string_len_empty() {
    oracle::assert_matches_reference("print(string.len(''))");
}

#[test]
fn oracle_string_byte() {
    oracle::assert_matches_reference("print(string.byte('A'))");
}

#[test]
fn oracle_string_byte_range() {
    oracle::assert_matches_reference("print(string.byte('abc', 1, 3))");
}

#[test]
fn oracle_string_char() {
    oracle::assert_matches_reference("print(string.char(72, 101, 108, 108, 111))");
}

#[test]
fn oracle_string_sub() {
    oracle::assert_matches_reference("print(string.sub('hello', 2, 4))");
}

#[test]
fn oracle_string_sub_negative() {
    oracle::assert_matches_reference("print(string.sub('hello', -3))");
}

#[test]
fn oracle_string_rep() {
    oracle::assert_matches_reference("print(string.rep('ab', 3))");
}

#[test]
fn oracle_string_rep_zero() {
    oracle::assert_matches_reference("print(string.rep('xy', 0))");
}

#[test]
fn oracle_string_reverse() {
    oracle::assert_matches_reference("print(string.reverse('hello'))");
}

#[test]
fn oracle_string_lower() {
    oracle::assert_matches_reference("print(string.lower('Hello World'))");
}

#[test]
fn oracle_string_upper() {
    oracle::assert_matches_reference("print(string.upper('Hello World'))");
}

#[test]
fn oracle_string_format_d() {
    oracle::assert_matches_reference("print(string.format('%d %s', 42, 'hi'))");
}

#[test]
fn oracle_string_format_x() {
    oracle::assert_matches_reference("print(string.format('%x', 255))");
}

#[test]
fn oracle_string_format_f() {
    oracle::assert_matches_reference("print(string.format('%f', 3.14))");
}

#[test]
fn oracle_string_format_g() {
    oracle::assert_matches_reference("print(string.format('%g', 100000))");
}

#[test]
fn oracle_string_format_g_sci() {
    oracle::assert_matches_reference("print(string.format('%g', 1e10))");
}

#[test]
fn oracle_string_format_q() {
    oracle::assert_matches_reference(r#"print(string.format('%q', 'hello "world"'))"#);
}

#[test]
fn oracle_string_format_percent() {
    oracle::assert_matches_reference("print(string.format('100%%'))");
}

#[test]
fn oracle_string_find_plain() {
    oracle::assert_matches_reference("print(string.find('hello world', 'world'))");
}

#[test]
fn oracle_string_find_pattern() {
    oracle::assert_matches_reference("print(string.find('hello123', '%d+'))");
}

#[test]
fn oracle_string_find_not_found() {
    oracle::assert_matches_reference("print(string.find('hello', 'xyz'))");
}

#[test]
fn oracle_string_find_captures() {
    oracle::assert_matches_reference("print(string.find('hello world', '(w%a+)'))");
}

#[test]
fn oracle_string_find_anchor() {
    oracle::assert_matches_reference("print(string.find('hello', '^hel'))");
}

#[test]
fn oracle_string_match_captures() {
    oracle::assert_matches_reference("print(string.match('2024-01-15', '(%d+)-(%d+)-(%d+)'))");
}

#[test]
fn oracle_string_match_no_capture() {
    oracle::assert_matches_reference("print(string.match('hello', '%a+'))");
}

#[test]
fn oracle_string_match_empty_capture() {
    oracle::assert_matches_reference("print(string.match('hello', '()h'))");
}

#[test]
fn oracle_string_gmatch() {
    oracle::assert_matches_reference(
        "local t = {} for w in string.gmatch('hello world foo', '%a+') do t[#t+1] = w end print(t[1], t[2], t[3])",
    );
}

#[test]
fn oracle_string_gsub_basic() {
    oracle::assert_matches_reference("print(string.gsub('hello', 'l', 'L'))");
}

#[test]
fn oracle_string_gsub_limit() {
    oracle::assert_matches_reference("print(string.gsub('hello', 'l', 'L', 1))");
}

#[test]
fn oracle_string_gsub_pattern() {
    oracle::assert_matches_reference("print(string.gsub('abc123', '%d', '*'))");
}

#[test]
fn oracle_string_gsub_function() {
    oracle::assert_matches_reference(
        "print(string.gsub('abc', '%a', function(c) return string.upper(c) end))",
    );
}

#[test]
fn oracle_string_gsub_table() {
    oracle::assert_matches_reference("local t = {a='A', b='B'} print(string.gsub('aXb', '%a', t))");
}

#[test]
fn oracle_string_method_upper() {
    oracle::assert_matches_reference("print(('hello'):upper())");
}

#[test]
fn oracle_string_method_len() {
    oracle::assert_matches_reference("print(('abc'):len())");
}

#[test]
fn oracle_string_method_sub() {
    oracle::assert_matches_reference("print(('hello'):sub(1, 3))");
}

#[test]
fn oracle_string_method_find() {
    oracle::assert_matches_reference("print(('hello world'):find('world'))");
}

#[test]
fn oracle_string_method_gsub() {
    oracle::assert_matches_reference("print(('aaa'):gsub('a', 'b'))");
}

#[test]
fn oracle_string_type() {
    oracle::assert_matches_reference("print(type(string))");
}

#[test]
fn oracle_string_format_leading_zero() {
    oracle::assert_matches_reference("print(string.format('%05d', 42))");
}

#[test]
fn oracle_string_format_left_align() {
    oracle::assert_matches_reference("print(string.format('%-10s|', 'hi'))");
}

#[test]
fn oracle_string_find_plain_flag() {
    oracle::assert_matches_reference("print(string.find('hello%world', '%', 1, true))");
}

// ---------------------------------------------------------------------------
// Table library oracle tests
// ---------------------------------------------------------------------------

#[test]
fn oracle_table_concat_basic() {
    oracle::assert_matches_reference("print(table.concat({1, 2, 3}, ', '))");
}

#[test]
fn oracle_table_concat_default_sep() {
    oracle::assert_matches_reference("print(table.concat({'a', 'b', 'c'}))");
}

#[test]
fn oracle_table_concat_range() {
    oracle::assert_matches_reference("print(table.concat({'a', 'b', 'c', 'd'}, '-', 2, 3))");
}

#[test]
fn oracle_table_concat_empty() {
    oracle::assert_matches_reference("print(table.concat({}, ', '))");
}

#[test]
fn oracle_table_insert_append() {
    oracle::assert_matches_reference(
        "local t = {1, 2, 3}; table.insert(t, 4); print(t[1], t[2], t[3], t[4])",
    );
}

#[test]
fn oracle_table_insert_at_position() {
    oracle::assert_matches_reference(
        "local t = {1, 2, 3}; table.insert(t, 2, 99); print(t[1], t[2], t[3], t[4])",
    );
}

#[test]
fn oracle_table_remove_last() {
    oracle::assert_matches_reference(
        "local t = {1, 2, 3}; local v = table.remove(t); print(v, #t)",
    );
}

#[test]
fn oracle_table_remove_at_position() {
    oracle::assert_matches_reference(
        "local t = {1, 2, 3}; local v = table.remove(t, 1); print(v, t[1], t[2])",
    );
}

#[test]
fn oracle_table_sort_numbers() {
    oracle::assert_matches_reference(
        "local t = {3, 1, 4, 1, 5, 9, 2, 6}; table.sort(t); print(table.concat(t, ', '))",
    );
}

#[test]
fn oracle_table_sort_strings() {
    oracle::assert_matches_reference(
        "local t = {'banana', 'apple', 'cherry'}; table.sort(t); print(table.concat(t, ', '))",
    );
}

#[test]
fn oracle_table_sort_custom() {
    oracle::assert_matches_reference(
        "local t = {3, 1, 4}; table.sort(t, function(a, b) return a > b end); print(table.concat(t, ', '))",
    );
}

#[test]
fn oracle_table_maxn() {
    oracle::assert_matches_reference("print(table.maxn({1, 2, 3}))");
}

#[test]
fn oracle_table_maxn_empty() {
    oracle::assert_matches_reference("print(table.maxn({}))");
}

#[test]
fn oracle_table_maxn_float_keys() {
    oracle::assert_matches_reference(
        "local t = {}; t[1] = 'a'; t[3.5] = 'b'; t[100] = 'c'; print(table.maxn(t))",
    );
}

#[test]
fn oracle_table_getn() {
    oracle::assert_matches_reference("print(table.getn({10, 20, 30}))");
}

#[test]
fn oracle_table_foreachi() {
    oracle::assert_matches_reference(
        "local r = '' table.foreachi({10, 20, 30}, function(i, v) r = r .. i .. '=' .. v .. ' ' end) print(r)",
    );
}

#[test]
fn oracle_table_foreachi_early_return() {
    oracle::assert_matches_reference(
        "local v = table.foreachi({10, 20, 30}, function(i, v) if v == 20 then return 'found' end end) print(v)",
    );
}

#[test]
fn oracle_table_sort_empty() {
    oracle::assert_matches_reference("local t = {}; table.sort(t); print(#t)");
}

#[test]
fn oracle_table_sort_single() {
    oracle::assert_matches_reference("local t = {42}; table.sort(t); print(t[1])");
}

#[test]
fn oracle_table_insert_remove_sequence() {
    oracle::assert_matches_reference(
        "local t = {1, 2, 3}; table.insert(t, 2, 99); table.remove(t, 3); print(table.concat(t, ', '))",
    );
}
