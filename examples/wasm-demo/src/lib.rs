use std::cell::RefCell;
use std::fmt::Write as _;

use rilua::{Lua, LuaApiMut, LuaResult, StdLib, Val};
use rilua::vm::state::LuaState;
use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Output capture
// ---------------------------------------------------------------------------

thread_local! {
    static OUTPUT: RefCell<String> = RefCell::new(String::new());
}

fn push_output(s: &str) {
    OUTPUT.with(|out| {
        let mut buf = out.borrow_mut();
        buf.push_str(s);
    });
}

fn take_output() -> String {
    OUTPUT.with(|out| out.borrow_mut().split_off(0))
}

// ---------------------------------------------------------------------------
// Custom print — mirrors rilua's lua_print but writes to OUTPUT buffer
// ---------------------------------------------------------------------------

fn wasm_print(state: &mut LuaState) -> LuaResult<u32> {
    let base = state.base;
    let top = state.top;
    let n = top.saturating_sub(base);

    let mut line = String::new();
    for i in 0..n {
        if i > 0 {
            line.push('\t');
        }
        let val = state.stack_get(base + i);
        match val {
            Val::Nil => line.push_str("nil"),
            Val::Bool(b) => {
                if b {
                    line.push_str("true");
                } else {
                    line.push_str("false");
                }
            }
            Val::Str(r) => {
                if let Some(s) = state.gc.string_arena.get(r) {
                    // Lua strings can contain arbitrary bytes; lossy conversion
                    // is acceptable for display in a browser.
                    let text = String::from_utf8_lossy(s.data());
                    line.push_str(&text);
                } else {
                    line.push_str("string: ???");
                }
            }
            _ => {
                let _ = write!(line, "{val}");
            }
        }
    }
    line.push('\n');
    push_output(&line);

    Ok(0)
}

// ---------------------------------------------------------------------------
// WASM entry point
// ---------------------------------------------------------------------------

/// Evaluate a Lua source string and return the captured output.
///
/// Loads base, string, table, math, and coroutine libraries.
/// IO, OS, and package are excluded (no filesystem on WASM).
/// The `print` function is replaced with a version that captures output.
#[wasm_bindgen]
pub fn eval_lua(code: &str) -> String {
    // Clear any leftover output from a previous call.
    take_output();

    let libs = StdLib::BASE | StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::COROUTINE;

    let mut lua = match Lua::new_with(libs) {
        Ok(l) => l,
        Err(e) => return format!("[init error] {e}"),
    };

    // Replace print with our capturing version.
    if let Err(e) = lua.register_function("print", wasm_print) {
        return format!("[init error] {e}");
    }

    match lua.exec(code) {
        Ok(()) => take_output(),
        Err(e) => {
            let mut out = take_output();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!("[error] {e}"));
            out
        }
    }
}
