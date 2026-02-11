//! rilua — Lua 5.1.1 implemented in Rust.
//!
//! A from-scratch implementation targeting behavioral equivalence with
//! the PUC-Rio reference interpreter. Designed for embedding in Rust
//! applications, with a focus on the World of Warcraft addon variant.
//!
//! # Architecture
//!
//! Pipeline: Source -> Lexer -> Parser -> AST -> Compiler -> Proto -> VM
//!
//! See `docs/architecture.md` for design documentation.

pub mod compiler;
pub mod error;
pub mod stdlib;
pub mod vm;

use std::rc::Rc;

use error::LuaResult;
use vm::callinfo::LUA_MULTRET;
use vm::closure::{Closure, LuaClosure};
use vm::execute::{CallResult, execute};
use vm::proto::Proto;
use vm::state::{Gc, LuaState};
use vm::value::Val;

/// Executes a Lua source string as `"=(string)"`.
///
/// Compiles the source, registers the standard library, and runs the
/// resulting chunk. Equivalent to `exec_with_name(source, "=(string)")`.
pub fn exec(source: &str) -> LuaResult<()> {
    exec_with_name(source, "=(string)")
}

/// Executes a Lua source string with the given chunk name.
///
/// Pipeline: compile -> patch string constants -> create state ->
/// register stdlib -> create main closure -> precall -> execute.
pub fn exec_with_name(source: &str, name: &str) -> LuaResult<()> {
    // 1. Compile source to Proto.
    let proto = compiler::compile(source, name)?;

    // 2. Create VM state.
    let mut state = LuaState::new();

    // 3. Register standard library.
    stdlib::open_libs(&mut state)?;

    // 4. Patch string constants: resolve nil placeholders to real GC strings.
    let mut proto = Rc::try_unwrap(proto).unwrap_or_else(|rc| (*rc).clone());
    patch_string_constants(&mut proto, &mut state.gc);
    let proto = Rc::new(proto);

    // 5. Create main closure with proto and global table as environment.
    let lua_cl = LuaClosure::new(proto, state.global);
    let closure_ref = state.gc.alloc_closure(Closure::Lua(lua_cl));

    // 6. Set closure at stack[0], set up for call.
    state.stack_set(0, Val::Function(closure_ref));
    state.top = 1;
    state.base = 1;

    // 7. precall + execute.
    match state.precall(0, LUA_MULTRET)? {
        CallResult::Lua => execute(&mut state)?,
        CallResult::Rust => {} // shouldn't happen for main chunk
    }

    Ok(())
}

/// Resolves string constant placeholders in a Proto using the GC.
///
/// During compilation, string constants are stored as `Val::Nil` with
/// their raw bytes recorded in `proto.string_pool`. This function
/// interns each string via the GC and replaces the placeholder with
/// the real `Val::Str` value. Recurses into child protos.
pub fn patch_string_constants(proto: &mut Proto, gc: &mut Gc) {
    for (idx, bytes) in proto.string_pool.drain(..) {
        let str_ref = gc.intern_string(&bytes);
        proto.constants[idx as usize] = Val::Str(str_ref);
    }
    for child in &mut proto.protos {
        patch_string_constants(Rc::make_mut(child), gc);
    }
}
