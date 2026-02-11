//! Virtual machine: bytecode execution and runtime state.

pub mod callinfo;
pub mod closure;
pub mod execute;
pub mod gc;
pub mod instructions;
pub mod metatable;
pub mod proto;
pub mod state;
pub mod string;
pub mod table;
pub mod value;
