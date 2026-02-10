//! Base library: print, assert, type, tostring, tonumber, etc.

use std::io::Write;

use crate::error::LuaResult;
use crate::vm::state::LuaState;
use crate::vm::value::Val;

/// Implements Lua's `print(...)`.
///
/// PUC-Rio semantics:
/// - Tab-separated values, newline-terminated.
/// - Calls `tostring()` on each argument (Phase 3e: inline conversion).
/// - Returns 0 values.
///
/// Reference: `luaB_print` in `lbaselib.c`.
pub fn lua_print(state: &mut LuaState) -> LuaResult<u32> {
    let base = state.base;
    let top = state.top;
    let nargs = top.saturating_sub(base);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for i in 0..nargs {
        if i > 0 {
            let _ = out.write_all(b"\t");
        }
        let val = state.stack_get(base + i);
        let _ = match val {
            Val::Nil => out.write_all(b"nil"),
            Val::Bool(b) => {
                if b {
                    out.write_all(b"true")
                } else {
                    out.write_all(b"false")
                }
            }
            Val::Num(_) => {
                // Use Val's Display impl which matches PUC-Rio's %.14g.
                let s = format!("{val}");
                out.write_all(s.as_bytes())
            }
            Val::Str(r) => {
                if let Some(s) = state.gc.string_arena.get(r) {
                    out.write_all(s.data())
                } else {
                    out.write_all(b"string: ???")
                }
            }
            // Other types: "type: 0xADDR" format.
            _ => {
                let s = format!("{val}");
                out.write_all(s.as_bytes())
            }
        };
    }
    let _ = out.write_all(b"\n");
    let _ = out.flush();

    Ok(0)
}
