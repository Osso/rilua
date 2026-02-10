//! Standard library: modular implementation of Lua 5.1.1's built-in libraries.

pub mod base;
pub mod coroutine;
pub mod debug;
pub mod io;
pub mod math;
pub mod os;
pub mod package;
pub mod string;
pub mod table;

use crate::error::LuaResult;
use crate::vm::closure::{Closure, RustClosure, RustFn};
use crate::vm::state::LuaState;
use crate::vm::value::Val;

/// Registers all standard library functions into the global table.
///
/// Phase 3e: only `print` is registered. Other functions will be added
/// in subsequent phases.
pub fn open_libs(state: &mut LuaState) -> LuaResult<()> {
    register_global_fn(state, "print", base::lua_print)?;
    Ok(())
}

/// Creates a `RustClosure`, interns the name string, and sets it
/// in the global table.
fn register_global_fn(state: &mut LuaState, name: &str, func: RustFn) -> LuaResult<()> {
    // Create and allocate the closure.
    let closure = Closure::Rust(RustClosure::new(func, name));
    let closure_ref = state.gc.alloc_closure(closure);

    // Intern the function name and set it in globals.
    let key_ref = state.gc.intern_string(name.as_bytes());
    let key = Val::Str(key_ref);
    let val = Val::Function(closure_ref);

    let global = state.global;
    let table = state.gc.tables.get_mut(global).ok_or_else(|| {
        crate::error::LuaError::Runtime(crate::error::RuntimeError {
            message: "global table not found".into(),
            level: 0,
            traceback: vec![],
        })
    })?;
    table.raw_set(key, val, &state.gc.string_arena)?;

    Ok(())
}
