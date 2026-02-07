//! rilua: Lua 5.1.1 implemented in Rust.

mod compiler;
mod instr;
mod lua_std;
mod vm;
mod vm_aux;

pub mod error;

pub use vm::LuaType;
pub use vm::RustFunc;
pub use vm::State;

use compiler::Chunk;
use instr::Instr;

/// Custom result type for evaluating Lua.
pub type Result<T> = std::result::Result<T, error::Error>;
