//! Handle types for Lua objects.
//!
//! Lightweight `Copy` newtypes over `GcRef<T>` that provide a typed,
//! public API for interacting with GC-managed Lua objects. All mutating
//! operations require `&mut Lua`, ensuring the borrow checker prevents
//! aliased mutation.

use crate::error::{LuaError, LuaResult, RuntimeError};
use crate::vm::closure::Closure;
use crate::vm::gc::arena::GcRef;
use crate::vm::state::{LuaState, LuaThread, ThreadStatus};
use crate::vm::value::Val;

/// Handle to a Lua table in the GC arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table(pub(crate) GcRef<crate::vm::table::Table>);

/// Handle to a Lua function (closure) in the GC arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function(pub(crate) GcRef<Closure>);

/// Handle to a Lua coroutine thread in the GC arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thread(pub(crate) GcRef<LuaThread>);

impl Table {
    /// Gets a value by key without metamethod dispatch.
    pub fn raw_get(&self, state: &LuaState, key: Val) -> LuaResult<Val> {
        let table = state.gc.tables.get(self.0).ok_or_else(|| {
            LuaError::Runtime(RuntimeError {
                message: "table has been collected".into(),
                level: 0,
                traceback: vec![],
            })
        })?;
        Ok(table.get(key, &state.gc.string_arena))
    }

    /// Sets a value by key without metamethod dispatch.
    pub fn raw_set(&self, state: &mut LuaState, key: Val, value: Val) -> LuaResult<()> {
        let table = state.gc.tables.get_mut(self.0).ok_or_else(|| {
            LuaError::Runtime(RuntimeError {
                message: "table has been collected".into(),
                level: 0,
                traceback: vec![],
            })
        })?;
        table.raw_set(key, value, &state.gc.string_arena)
    }

    /// Returns the raw length of the table (no `__len` metamethod).
    pub fn raw_len(&self, state: &LuaState) -> i64 {
        state
            .gc
            .tables
            .get(self.0)
            .map_or(0, |t| t.len(&state.gc.string_arena) as i64)
    }

    /// Sets or clears the metatable for this table.
    pub fn set_metatable(&self, state: &mut LuaState, mt: Option<Self>) -> LuaResult<()> {
        let table = state.gc.tables.get_mut(self.0).ok_or_else(|| {
            LuaError::Runtime(RuntimeError {
                message: "table has been collected".into(),
                level: 0,
                traceback: vec![],
            })
        })?;
        table.set_metatable(mt.map(|t| t.0));
        Ok(())
    }

    /// Returns the underlying `GcRef` for internal use.
    pub fn gc_ref(self) -> GcRef<crate::vm::table::Table> {
        self.0
    }
}

impl Function {
    /// Returns the underlying `GcRef` for internal use.
    pub fn gc_ref(self) -> GcRef<Closure> {
        self.0
    }
}

impl Thread {
    /// Returns the status of this coroutine thread.
    pub fn status(&self, state: &LuaState) -> ThreadStatus {
        state
            .gc
            .threads
            .get(self.0)
            .map_or(ThreadStatus::Dead, |t| t.status)
    }

    /// Returns the underlying `GcRef` for internal use.
    pub fn gc_ref(self) -> GcRef<LuaThread> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_raw_get_set() {
        let mut state = LuaState::new();
        let t = state.gc.alloc_table(crate::vm::table::Table::new());
        let handle = Table(t);

        let key = Val::Num(1.0);
        let value = Val::Num(42.0);
        handle.raw_set(&mut state, key, value).ok();
        let got = handle.raw_get(&state, key).ok();
        assert_eq!(got, Some(Val::Num(42.0)));
    }

    #[test]
    fn table_raw_len_empty() {
        let mut state = LuaState::new();
        let t = state.gc.alloc_table(crate::vm::table::Table::new());
        let handle = Table(t);
        assert_eq!(handle.raw_len(&state), 0);
    }

    #[test]
    fn function_gc_ref_round_trip() {
        let mut state = LuaState::new();
        let cl = Closure::Rust(crate::vm::closure::RustClosure::new(|_| Ok(0), "test"));
        let r = state.gc.alloc_closure(cl);
        let handle = Function(r);
        assert_eq!(handle.gc_ref(), r);
    }

    #[test]
    fn table_set_metatable() {
        let mut state = LuaState::new();
        let t = state.gc.alloc_table(crate::vm::table::Table::new());
        let mt = state.gc.alloc_table(crate::vm::table::Table::new());
        let handle = Table(t);
        let mt_handle = Table(mt);
        handle.set_metatable(&mut state, Some(mt_handle)).ok();
        // Verify the metatable was set by checking the table's metatable field.
        let table = state.gc.tables.get(t);
        assert!(table.is_some());
        assert_eq!(table.and_then(crate::vm::table::Table::metatable), Some(mt));
    }
}
