//! Lua VM state: the main execution context.
//!
//! `LuaState` holds the value stack, call stack, global table, and GC
//! heap. Each coroutine gets its own `LuaThread` with a separate stack
//! but sharing the GC heap with all other threads.
//!
//! Full implementation in Phase 3a.

/// A Lua thread (coroutine) with its own stack and call stack.
///
/// Placeholder -- fields will be added in Phase 3a.
pub struct LuaThread;
