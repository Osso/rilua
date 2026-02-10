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

    // -- Native function helpers --
    // Used inside closures passed to create_function / register_function.

    /// Get argument at the given 1-based index, converting via FromLua.
    /// Raises an argument error if the value is missing or the wrong type.
    pub fn check_arg<V: FromLua>(&self, index: u32) -> Result<V> { ... }

    /// Push a return value onto the stack, converting via IntoLua.
    pub fn push<V: IntoLua>(&mut self, value: V) -> Result<()> { ... }
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
        lua.push(format!("Hello, {name}!"))?;
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

## Internal Stack Model

Internally, rilua uses a virtual stack similar to PUC-Rio's C API
for stdlib function implementation. Understanding this model is
necessary for implementing stdlib functions and the debug library.

### Stack Index Addressing

Stack indices address values relative to the current call frame:

| Index type | Range | Resolution |
|-----------|-------|------------|
| Positive | 1, 2, 3, ... | Base-relative: `base + index - 1` |
| Negative | -1, -2, -3, ... | Top-relative: `top + index` |
| Pseudo-index | special constants | Registry, globals, environment, upvalues |

**Pseudo-indices** provide access to non-stack locations:

| Pseudo-index | Target |
|-------------|--------|
| `REGISTRY_INDEX` | Global registry table (shared across all threads) |
| `GLOBALS_INDEX` | Current thread's global table |
| `ENVIRON_INDEX` | Current function's environment table |
| Upvalue indices | C closure upvalue slots (one per captured value) |

### Push/Get Protocol

**Push operations** write to `stack[top]` then increment `top`.
Operations that allocate GC objects (strings, tables, closures)
trigger a GC check. Operations on immediate values (nil, boolean,
number) do not.

**Get operations** read from the addressed stack slot. Most are
non-mutating. Exception: number-to-string conversion mutates the
stack slot in place and triggers a GC check (string allocation).

**C closure upvalues** are stored as values directly inside the
closure struct (not as UpVal objects like Lua closures). When
creating a C closure with `n` upvalues, the top `n` stack values
are popped and copied into the closure's upvalue array.

### Table Operations

**With metamethods** (`gettable`/`settable`): invoke the full
`__index`/`__newindex` chain (up to 100 iterations). `gettable`
takes the key from the stack top, replaces it with the result.
`settable` takes key and value from the top, pops both.

**Without metamethods** (`rawget`/`rawset`): bypass the metamethod
chain. Call `luaH_get`/`luaH_set` directly. The raw variants require
the target to be a table (not just anything with a metatable).

**Integer shortcuts** (`rawgeti`/`rawseti`): take the integer key
as a parameter instead of from the stack.

### Call Protocol

1. Push the function onto the stack.
2. Push arguments in order (first argument pushed first).
3. Call with `(nargs, nresults)`.
4. The function and all arguments are removed.
5. `nresults` results are pushed (excess discarded, missing padded
   with nil). `MULTRET` (-1) means push all results.

**Protected calls** return a status code. On error, the function and
arguments are replaced by a single error message on the stack. An
optional error handler function is specified by stack index (converted
to a stack offset internally because the stack may be reallocated).

### Registry and Environments

**The registry** is a global table stored in the shared state. It is
accessible via the registry pseudo-index. C libraries use it to store
private data (metatables, module state, references) that should not
be accessible from Lua code.

**Function environments**: every closure (C or Lua) has an associated
environment table. For Lua closures, the environment is the table
used for global variable lookups. The environment is captured from
the creating function when a new closure is created.

**Thread environments**: each thread has its own global table. The
main thread's global table is the default environment for new
closures.

### Reference System

The reference system provides a way to store Lua values in a table
(typically the registry) and retrieve them later by integer handle.
It is a free-list allocator using integer keys:

- `ref(table)` pops a value from the stack, stores it at an integer
  key. If there is a free slot (from a previous `unref`), it reuses
  it. Otherwise appends at `#table + 1`.
- `unref(table, ref)` adds the slot back to the free list.
- `table[0]` stores the free list head. Each free slot stores the
  index of the next free slot as its value.
- Special constants: `REF_NIL` (-1) for nil values (never stored),
  `NO_REF` (-2) for invalid references.

### Library Registration Protocol

Each standard library is loaded by pushing its opener function,
pushing the library name as argument, then calling. The opener:

1. Creates a table for the library (or reuses an existing one from
   `_LOADED`).
2. Registers all functions via closure creation and field assignment.
3. Stores the table in both `_LOADED[name]` and as a global.

The base library uses an empty name and registers directly into the
global table.

### GC Handle Safety

Values on the Lua stack (between `stack[0]` and `stack[top-1]`) are
marked as reachable during GC traversal. This is the primary
mechanism for protecting values from collection.

**Stack traversal during GC**:

1. Mark the thread's global table.
2. Mark all values from `stack[0]` to `stack[top-1]`.
3. Nil out slots from `top` to the maximum `ci.top` across all call
   frames (clears stale references).

**GC checks** (`checkGC`) run after operations that allocate GC
objects. The check occurs before the allocation in the API function,
which is safe because the new object does not exist yet. After
allocation, the object is immediately placed on the stack (making it
reachable).

**Write barriers** maintain the tri-color invariant during
incremental GC:

- **Forward barrier**: when a black object (already traversed) gets
  a white value (not yet traversed) stored into it, the white value
  is immediately marked.
- **Back barrier** (for tables): the table is re-grayed so it will
  be re-traversed.

C closure upvalues and table entries both require write barriers when
mutated through the API.

## Compatibility Note

While the public API is Rust-idiomatic, internal implementation
uses a lower-level stack-based API for stdlib functions that mirrors
PUC-Rio's `lua_*` conventions. This is an internal detail, not part
of the public API.
