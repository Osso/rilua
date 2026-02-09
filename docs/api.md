# Public API

## Decision

**Rust-idiomatic, trait-based API inspired by mlua's design. Not a
1:1 mirror of the Lua C API.**

## Overview

PUC-Rio Lua exposes a stack-based C API where values are pushed and
popped from a virtual stack. This works well in C but is unergonomic
in Rust — it lacks type safety, requires manual stack management,
and does not leverage Rust's trait system.

rilua provides a Rust-idiomatic API using traits for type conversion,
methods for common operations, and the type system for safety. The
API is designed for embedding Lua in Rust applications.

## Core Type: Lua

```rust
/// A Lua interpreter instance.
///
/// This is the main entry point for the rilua API. It owns all
/// Lua state: the value stack, GC heap, global table, registry,
/// and loaded libraries.
pub struct Lua {
    // Internal state (not public)
}

impl Lua {
    /// Create a new Lua state with standard libraries loaded.
    pub fn new() -> Self { ... }

    /// Create a new Lua state without any libraries.
    pub fn new_empty() -> Self { ... }

    /// Load and execute a string of Lua code.
    pub fn exec(&mut self, source: &str) -> Result<()> { ... }

    /// Load a string of Lua code and return a callable function.
    pub fn load(&mut self, source: &str) -> Result<Function> { ... }

    /// Get a global variable.
    pub fn global<V: FromLua>(&self, name: &str) -> Result<V> { ... }

    /// Set a global variable.
    pub fn set_global<V: IntoLua>(
        &mut self, name: &str, value: V,
    ) -> Result<()> { ... }

    /// Create a new empty table.
    pub fn create_table(&mut self) -> Result<Table> { ... }

    /// Create a Lua-callable function from a Rust closure.
    pub fn create_function<F>(&mut self, func: F) -> Result<Function>
    where
        F: Fn(&mut Lua) -> Result<u32> + 'static,
    { ... }

    /// Register a Rust function as a global.
    pub fn register_function<F>(
        &mut self,
        name: &str,
        func: F,
    ) -> Result<()>
    where
        F: Fn(&mut Lua) -> Result<u32> + 'static,
    { ... }

    /// Load and execute a file of Lua code.
    pub fn exec_file(&mut self, path: &str) -> Result<()> { ... }

    /// Load a file and return a callable function.
    pub fn load_file(&mut self, path: &str) -> Result<Function> { ... }

    /// Load a string with a chunk name (used in error messages).
    pub fn load_named(
        &mut self, source: &str, name: &str,
    ) -> Result<Function> { ... }

    /// Call a Lua function with arguments and return results.
    pub fn call<A, R>(&mut self, func: Function, args: A) -> Result<R>
    where
        A: IntoLuaMulti,
        R: FromLuaMulti,
    { ... }

    /// Protected call — catches Lua errors.
    pub fn pcall<A, R>(
        &mut self, func: Function, args: A,
    ) -> std::result::Result<R, Error>
    where
        A: IntoLuaMulti,
        R: FromLuaMulti,
    { ... }

    /// Force a full garbage collection cycle.
    pub fn gc_collect(&mut self) { ... }

    /// Get current memory usage in bytes.
    pub fn gc_count(&self) -> usize { ... }

    /// Stop automatic garbage collection.
    pub fn gc_stop(&mut self) { ... }

    /// Restart automatic garbage collection.
    pub fn gc_restart(&mut self) { ... }

    /// Perform an incremental GC step. Returns true if a cycle completed.
    pub fn gc_step(&mut self) -> bool { ... }

    /// Set GC pause parameter. Returns the previous value.
    pub fn gc_set_pause(&mut self, pause: u32) -> u32 { ... }

    /// Set GC step multiplier. Returns the previous value.
    pub fn gc_set_step_multiplier(&mut self, mul: u32) -> u32 { ... }

    /// Create a new coroutine (Lua thread) from a function.
    pub fn create_thread(&mut self, func: Function) -> Result<Thread> { ... }
}
```

## Conversion Traits

### IntoLua / FromLua

```rust
/// Convert a Rust value into a Lua value.
pub trait IntoLua {
    fn into_lua(self, lua: &mut Lua) -> Result<Val>;
}

/// Convert a Lua value into a Rust value.
pub trait FromLua: Sized {
    fn from_lua(val: Val, lua: &Lua) -> Result<Self>;
}
```

Standard implementations:

| Rust Type | Lua Type |
|-----------|----------|
| `()` | nil |
| `bool` | boolean |
| `f64`, `f32` | number |
| `i8`..`i64`, `u8`..`u64` | number (with range check) |
| `String`, `&str` | string |
| `Vec<T>` | table (array) |
| `HashMap<K, V>` | table |
| `Option<T>` | T or nil |

### IntoLuaMulti / FromLuaMulti

For functions with multiple arguments or return values:

```rust
pub trait IntoLuaMulti {
    fn into_lua_multi(self, lua: &mut Lua) -> Result<Vec<Val>>;
}

pub trait FromLuaMulti: Sized {
    fn from_lua_multi(values: Vec<Val>, lua: &Lua) -> Result<Self>;
}
```

Implemented for tuples up to reasonable arity:

```rust
impl<A: IntoLua, B: IntoLua> IntoLuaMulti for (A, B) { ... }
impl<A: FromLua, B: FromLua> FromLuaMulti for (A, B) { ... }
```

## Table API

```rust
pub struct Table { /* GcRef<table::Table> */ }

impl Table {
    pub fn get<K: IntoLua, V: FromLua>(
        &self, lua: &Lua, key: K,
    ) -> Result<V> { ... }

    pub fn set<K: IntoLua, V: IntoLua>(
        &self, lua: &mut Lua, key: K, value: V,
    ) -> Result<()> { ... }

    pub fn raw_get<K: IntoLua, V: FromLua>(
        &self, lua: &Lua, key: K,
    ) -> Result<V> { ... }

    pub fn raw_set<K: IntoLua, V: IntoLua>(
        &self, lua: &mut Lua, key: K, value: V,
    ) -> Result<()> { ... }

    pub fn len(&self, lua: &Lua) -> Result<i64> { ... }

    pub fn raw_len(&self, lua: &Lua) -> i64 { ... }

    pub fn next<K: IntoLua, NK: FromLua, NV: FromLua>(
        &self, lua: &Lua, key: K,
    ) -> Result<Option<(NK, NV)>> { ... }

    pub fn pairs<K: FromLua, V: FromLua>(
        &self, lua: &Lua,
    ) -> TablePairs<K, V> { ... }

    pub fn set_metatable(
        &self, lua: &mut Lua, mt: Option<Table>,
    ) -> Result<()> { ... }
}
```

## Function API

```rust
pub struct Function { /* GcRef<Closure> or RustFn */ }

impl Function {
    pub fn call<A: IntoLuaMulti, R: FromLuaMulti>(
        &self, lua: &mut Lua, args: A,
    ) -> Result<R> { ... }
}
```

## UserData

Custom Rust types can be exposed to Lua via the `UserData` trait:

```rust
pub trait UserData {
    fn add_methods(methods: &mut UserDataMethods<Self>);
    fn add_fields(fields: &mut UserDataFields<Self>);
}
```

This follows mlua's pattern. A Rust type implementing `UserData`
gets a Lua-visible metatable with methods and fields.

## Thread (Coroutine) API

```rust
pub struct Thread { /* GcRef<LuaThread> */ }

impl Thread {
    pub fn resume<A: IntoLuaMulti, R: FromLuaMulti>(
        &self, lua: &mut Lua, args: A,
    ) -> Result<R> { ... }

    pub fn status(&self, lua: &Lua) -> ThreadStatus { ... }
}

pub enum ThreadStatus {
    Running,
    Suspended,
    Normal,
    Dead,
}
```

## Embedding Example

```rust
use rilua::Lua;

fn main() -> rilua::Result<()> {
    let mut lua = Lua::new();

    // Register a Rust closure as a global
    lua.register_function("greet", |lua| {
        let name: String = lua.check_arg(1)?;
        lua.push_string(&format!("Hello, {name}!"))?;
        Ok(1)
    })?;

    // Execute Lua code
    lua.exec(r#"
        msg = greet("World")
        print(msg)
    "#)?;

    // Read a Lua global from Rust
    let result: String = lua.global("msg")?;
    assert_eq!(result, "Hello, World!");

    Ok(())
}
```

## Compatibility Note

While the public API is Rust-idiomatic, internal implementation
may expose a lower-level stack-based API for stdlib functions
that mirrors PUC-Rio's `lua_*` conventions. This is an internal
detail, not part of the public API.
