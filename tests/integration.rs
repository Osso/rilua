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
