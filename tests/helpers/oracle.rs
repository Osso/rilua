//! Oracle comparison framework: runs Lua code in both rilua and PUC-Rio
//! Lua 5.1.1, comparing output for behavioral equivalence.
//!
//! The PUC-Rio `lua` binary path is read from the `LUA_REFERENCE_BIN`
//! environment variable. If not set, defaults to
//! `~/Repos/github.com/lua/lua/lua`. Tests that require the reference
//! binary skip gracefully if it is not available.

use std::path::PathBuf;
use std::process::Command;

/// Default path to the PUC-Rio Lua 5.1.1 binary.
const DEFAULT_LUA_BIN: &str = concat!(env!("HOME"), "/Repos/github.com/lua/lua/lua");

/// Returns the path to the PUC-Rio Lua 5.1.1 reference binary.
///
/// Reads `LUA_REFERENCE_BIN` from the environment, falling back to
/// the default path if not set.
pub fn reference_bin() -> PathBuf {
    std::env::var("LUA_REFERENCE_BIN")
        .map_or_else(|_| PathBuf::from(DEFAULT_LUA_BIN), PathBuf::from)
}

/// Returns `true` if the reference Lua binary exists and is executable.
pub fn reference_available() -> bool {
    let bin = reference_bin();
    bin.exists()
}

/// Result of running Lua code in an interpreter.
#[derive(Debug)]
pub struct LuaOutput {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Process exit code (0 = success).
    pub exit_code: i32,
}

/// Run Lua code in the PUC-Rio reference interpreter via `lua -e`.
///
/// Returns `None` if the reference binary is not available.
pub fn run_reference(code: &str) -> Option<LuaOutput> {
    let bin = reference_bin();
    if !bin.exists() {
        return None;
    }

    let output = Command::new(&bin).arg("-e").arg(code).output().ok()?;

    Some(LuaOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// Run a Lua file in the PUC-Rio reference interpreter.
///
/// Returns `None` if the reference binary is not available.
pub fn run_reference_file(path: &str) -> Option<LuaOutput> {
    let bin = reference_bin();
    if !bin.exists() {
        return None;
    }

    let output = Command::new(&bin).arg(path).output().ok()?;

    Some(LuaOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// Assert that the reference interpreter produces the expected stdout
/// for the given Lua code.
///
/// Skips the test if the reference binary is not available.
#[allow(dead_code)]
pub fn assert_reference_output(code: &str, expected_stdout: &str) {
    let Some(result) = run_reference(code) else {
        eprintln!("skipping: reference Lua binary not available");
        return;
    };
    assert_eq!(
        result.stdout, expected_stdout,
        "Reference Lua output mismatch for code: {code}\nstderr: {}",
        result.stderr,
    );
}

/// Run Lua code in rilua via `rilua -e`.
#[allow(dead_code, clippy::expect_used)]
pub fn run_rilua(code: &str) -> LuaOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_rilua"))
        .arg("-e")
        .arg(code)
        .output()
        .expect("failed to run rilua binary");

    LuaOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    }
}

/// Assert that rilua and PUC-Rio produce identical stdout for the given code.
///
/// Skips the test if the reference binary is not available.
#[allow(dead_code)]
pub fn assert_matches_reference(code: &str) {
    let Some(reference) = run_reference(code) else {
        eprintln!("skipping: reference Lua binary not available");
        return;
    };
    let rilua = run_rilua(code);
    assert_eq!(
        rilua.stdout, reference.stdout,
        "Output mismatch for code: {code}\n  rilua stdout: {:?}\n  ref   stdout: {:?}\n  rilua stderr: {}\n  ref   stderr: {}",
        rilua.stdout, reference.stdout, rilua.stderr, reference.stderr,
    );
    assert_eq!(
        rilua.exit_code, reference.exit_code,
        "Exit code mismatch for code: {code}\n  rilua: {}\n  ref:   {}",
        rilua.exit_code, reference.exit_code,
    );
}
