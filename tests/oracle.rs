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
