# Comparison with Other Implementations

How rilua compares to PUC-Rio Lua, mlua, and Luau in architecture,
performance, API design, and trade-offs.

## Overview

| | rilua | PUC-Rio Lua | mlua | Luau |
|---|---|---|---|---|
| **Language** | Rust | C | Rust (FFI to C) | C++ |
| **Lua Version** | 5.1.1 | 5.1 - 5.5 | 5.1 - 5.5, Luau | 5.1 derivative |
| **Type** | Native interpreter | Reference impl | Binding layer | Native interpreter |
| **License** | MIT | MIT | MIT | MIT |
| **Dependencies** | 0 | 0 (libc) | 7+ runtime | 0 (C++ stdlib) |
| **WASM** | `wasm32-unknown-unknown` | No | `wasm32-unknown-emscripten` | No |
| **Unsafe Code** | 0 in core VM/GC | N/A (C) | Extensive (FFI) | N/A (C++) |

## Architecture

### rilua

Pipeline: Lexer -> Parser -> AST -> Compiler -> Bytecode -> VM.

38 register-based opcodes matching PUC-Rio's instruction set. Values
are a Rust enum (`Val`) with GcRef indices into typed arenas. Arena-based
incremental mark-sweep GC with generational indices. No unsafe code in
the VM, compiler, or GC. Errors propagate via `Result<T, LuaError>` --
no `setjmp`/`longjmp`, no panics in library code.

Protos (compiled function bodies) are reference-counted (`Rc<Proto>`)
rather than GC-managed, reducing GC traversal cost for immutable data.

### PUC-Rio Lua

The reference implementation and baseline for all Lua interpreters.
Same 38-opcode register-based VM. Values are tagged unions (`TValue`)
with a mark-sweep GC using linked lists of `GCObject` pointers. Error
handling uses `setjmp`/`longjmp`. Written in ANSI C for portability.

PUC-Rio's GC walks linked lists of heap-allocated objects. Each object
carries color bits inline. The design prioritizes simplicity and
portability over cache locality.

### mlua

mlua is not a Lua implementation. It is a Rust binding layer over
PUC-Rio's C implementation (or LuaJIT, or Luau). The actual Lua
execution happens in C/C++ code linked via FFI.

Every C API call that can trigger a Lua error is wrapped in
`lua_pcall` to prevent `longjmp` from unwinding Rust stack frames.
The library states it contains "a huge amount of unsafe code" to
bridge the C/Rust boundary. Users do not write `unsafe` in normal
usage, but the FFI boundary is inherently fragile.

When using the `vendored` feature, mlua compiles PUC-Rio's C source
(or Luau's C++ source) from `lua-src-rs` during build. This requires
a C/C++ compiler in the toolchain.

### Luau

Roblox's derivative of Lua 5.1 with a rewritten interpreter, optional
native code generation (x64/ARM64), and a gradual type system. The
bytecode format differs from PUC-Rio. The interpreter uses inline
caching for table field access.

Luau removes several Lua 5.1 features for sandboxing: `string.dump`,
`loadstring` with bytecode, `__gc` metamethods, `setfenv`/`getfenv`.
It adds `buffer` (typed byte arrays), `table.freeze`, `table.clone`,
string interpolation, compound assignments, and `continue`.

## Performance

### Relative Speed

| Implementation | vs PUC-Rio 5.1 (interpreted) | Notes |
|---|---|---|
| **PUC-Rio Lua 5.1** | 1.0x (baseline) | C, `-O2` |
| **rilua** | ~1.7x slower | Pure Rust, `--release` |
| **mlua** | ~1.0x (wraps PUC-Rio) | FFI overhead at boundaries only |
| **Luau (interpreted)** | Faster than PUC-Rio | Optimized dispatch, inline caching |
| **Luau (native codegen)** | 1.5-2.5x faster than Luau interpreted | x64/ARM64 only |

Measured on AMD Ryzen 7 8840U, release builds, median of 10 runs.
Sum of 20 individual PUC-Rio test files: PUC-Rio 696ms, rilua
1167ms, mlua 211ms (8 tests that passed).

rilua's overhead comes from four areas: VM dispatch loop
(`constructs.lua` 2.26x), table hash traversal (`nextvar.lua` 2.0x),
compilation cost (`verybig.lua` 1.87x), and function call overhead
in sorting callbacks (`sort.lua` 1.76x). Tests that do not stress
these paths run at or near parity.

mlua adds minimal overhead because execution happens in PUC-Rio's C
VM. The FFI crossing cost exists at every Rust<->Lua boundary call
but is small relative to VM execution time. Micro-benchmarks confirm
mlua matches PUC-Rio within noise (1.0-1.2x).

Luau's interpreter is faster than PUC-Rio through instruction-level
optimizations, inline caching, and tuned memory allocation. With
native code generation enabled, compute-heavy code sees an additional
1.5-2.5x speedup.

### Where rilua is Competitive

For workloads dominated by string operations, pattern matching, file
I/O, and simple control flow, rilua matches PUC-Rio. The overhead is
concentrated in tight loops with many VM dispatch cycles, table
iteration, and deep function call chains.

For embedding scenarios where Lua execution is a small fraction of
total runtime (e.g., configuration evaluation, scripting hooks), the
1.7x factor is unlikely to be noticeable.

### Micro-Benchmarks (All Implementations)

Minimal Lua scripts that run on incomplete implementations too:

| Test | PUC-Rio | rilua | mlua | lua-in-rust |
|---|---:|---:|---:|---:|
| fib.lua (recursive fib(35)) | 647ms | 1629ms (2.52x) | 652ms (1.01x) | 3784ms (5.85x) |
| loop.lua (1M iterations) | 6ms | 14ms (2.33x) | 7ms (1.17x) | 22ms (3.67x) |
| tables.lua (100K insert+read) | 5ms | 7ms (1.40x) | 5ms (1.00x) | 23ms (4.60x) |
| closures.lua (500K calls) | 13ms | 38ms (2.92x) | 13ms (1.00x) | --- |
| nested_loops.lua (1Mx1K) | 9ms | 24ms (2.67x) | 11ms (1.22x) | 34ms (3.78x) |

Ratios are vs PUC-Rio. lua-in-rust could not run closures.lua
(runtime crash on upvalue access). Benchmark script and runner:
`scripts/benchmark-implementations.sh`.

## Rust API

### rilua

Trait-based API inspired by mlua's design:

```rust
use rilua::{Lua, IntoLua, FromLua};

let lua = Lua::new();
lua.set_global("x", 42)?;
let val: i32 = lua.global("x")?;

// Register a Rust function
lua.register_function("add", |state| {
    let a: f64 = state.arg(1)?;
    let b: f64 = state.arg(2)?;
    state.push(a + b)?;
    Ok(1)
})?;

// UserData
lua.create_typed_userdata::<MyType>(value)?;
```

Key characteristics:
- Handle types (`Table`, `Function`, `Thread`, `AnyUserData`) are
  `Copy` -- they are u32 indices into GC arenas
- `IntoLua` / `FromLua` traits for type conversion
- `RustFn` is `fn(&mut LuaState) -> LuaResult<u32>` (push returns,
  return count)
- GC control: `gc_collect()`, `gc_stop()`, `gc_step()`, etc.
- No lifetime parameters on handles -- arena indices remain valid
  until the Lua state is dropped or GC collects the object

### mlua

More feature-rich API with closures, async, scoped borrows:

```rust
use mlua::prelude::*;

let lua = Lua::new();
lua.globals().set("x", 42)?;
let val: i32 = lua.globals().get("x")?;

// Closure-based function creation
let add = lua.create_function(|_, (a, b): (f64, f64)| {
    Ok(a + b)
})?;
lua.globals().set("add", add)?;

// UserData via derive or trait impl
impl UserData for MyType {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("foo", |_, this, ()| Ok(this.value));
    }
}
```

Key characteristics:
- Closures as Lua functions (captures state, not just `fn` pointers)
- `UserData` trait with method/field registration
- `Scope` for non-`'static` borrows in callbacks
- Async function support (`create_async_function`)
- Serde integration (`serialize`/`deserialize` Lua values)
- `RegistryKey` for persistent references across calls
- Module authoring (`#[mlua::lua_module]` proc macro)

### API Comparison

| Feature | rilua | mlua |
|---|---|---|
| Function registration | `fn` pointers | Closures (captures) |
| UserData | `create_typed_userdata` | `UserData` trait + derive |
| Async | No | Yes (`async` feature) |
| Serde | No | Yes (`serde` feature) |
| Scoped borrows | No | Yes (`Scope`) |
| Module authoring | No | Yes (proc macro) |
| Handle model | Copy indices | Reference-counted |
| Error type | `LuaError` enum | `mlua::Error` enum |
| Dependencies | 0 | 7+ |
| C compiler needed | No | Yes (vendored) or system Lua |

rilua's API is intentionally smaller. It covers the embedding use case
(create state, load code, call functions, exchange data) without the
framework features mlua provides. The trade-off is fewer capabilities
but zero external dependencies and no C toolchain requirement.

## Safety

### Memory Safety

| | rilua | mlua | Luau |
|---|---|---|---|
| **Unsafe in core** | None | Extensive (FFI) | N/A (C++) |
| **User-facing unsafe** | None | None (normal usage) | N/A |
| **Error model** | `Result<T>` | `Result<T>` (wraps longjmp) | longjmp (C++) |
| **GC safety** | Generational indices | C GC + prevent-collection guards | C++ GC |
| **Use-after-free** | Impossible (index validation) | Possible if guards misused | Possible (C++) |

rilua's arena-based GC uses generational indices: each arena slot has
a generation counter incremented on free. A `GcRef` stores both the
slot index and the generation it was created with. Accessing a freed
slot returns an error rather than corrupted data. This provides
use-after-free protection without `unsafe` code.

mlua wraps every error-capable C API call in `lua_pcall` to catch
`longjmp`. This prevents Rust stack unwinding but adds overhead and
complexity. The library acknowledges the approach cannot guarantee
100% safety due to the fundamental tension between `longjmp` and
Rust's ownership model.

### Sandboxing

Luau has the strongest sandboxing story: no bytecode loading, no
`__gc` metamethods, restricted `collectgarbage`, per-script global
isolation (`safeenv`), and a VM interrupt mechanism for terminating
runaway scripts.

rilua and PUC-Rio Lua 5.1 expose `string.dump`, `loadstring` with
bytecode, `__gc` metamethods, and `setfenv`/`getfenv`. These are
faithful to the Lua 5.1 specification but provide less isolation
than Luau.

## Send/Sync (Thread Safety)

| | rilua | mlua |
|---|---|---|
| **Default** | `!Send`, `!Sync` | `!Send`, `!Sync` |
| **With feature** | `Send` (feature = `send`) | `Send + Sync` (feature = `send`) |
| **Mechanism** | GcRef is u32 (trivially Send) | Reentrant mutex around VM |
| **Overhead** | None (index-based handles) | Lock acquisition on every access |
| **Constraint** | `UserData: Send` required | `UserData: Send` required |

rilua's `send` feature works because `GcRef<T>` values are plain
`u32` indices -- they contain no pointers and are trivially `Send`.
The `Lua` struct gets `unsafe impl Send` gated on the feature flag.
There is no synchronization overhead.

mlua's `send` feature wraps the Lua VM in a `parking_lot` reentrant
mutex, making it `Send + Sync`. Every VM access acquires the lock.
This is correct but adds per-operation overhead.

Neither approach makes concurrent access safe without external
synchronization. Both assume single-threaded access to the `Lua`
state and use `Send` to allow moving the state between threads.

## Feature Differences

### What rilua Has That mlua Does Not

- **Zero dependencies**: no C compiler, no system libraries, no
  `pkg-config`
- **`wasm32-unknown-unknown` support**: compiles without Emscripten
- **Behavioral equivalence with PUC-Rio 5.1**: bytecode-compatible,
  same 38 opcodes, same GC states, same stdlib edge cases
- **No unsafe in core**: the VM, GC, compiler, and stdlib contain
  zero `unsafe` blocks

### What mlua Has That rilua Does Not

- **Multiple Lua versions**: 5.1 through 5.5, LuaJIT, Luau
- **Closure-based function creation**: captures arbitrary state
- **Async support**: `create_async_function`, `AsyncThread`
- **Serde integration**: serialize/deserialize Lua values
- **Scoped borrows**: non-`'static` references in callbacks
- **Module authoring**: build `.so`/`.dll` loadable by Lua
- **`UserData` derive macro**: declarative method registration
- **PUC-Rio C performance**: execution at native C speed

### What Luau Has That Both Lack

- **Gradual type system**: optional type annotations with inference
- **Native code generation**: AOT compilation for x64/ARM64
- **`buffer` type**: typed byte array operations
- **String interpolation**: backtick template literals
- **`table.freeze` / `table.clone`**: immutable tables, shallow copy
- **Compound assignments**: `+=`, `-=`, `*=`, `/=`, `..=`
- **`continue` statement**: in loops
- **Sandbox isolation**: per-script globals, VM interrupts

## When to Use What

### Use rilua When

- You need Lua 5.1.1 behavioral equivalence (WoW addon compatibility,
  legacy Lua code)
- You want zero external dependencies and no C toolchain
- You are targeting `wasm32-unknown-unknown`
- Memory safety guarantees in the interpreter matter (no unsafe in
  core)
- The embedding scenario has modest performance requirements (scripting
  hooks, configuration, game logic at moderate scale)

### Use mlua When

- You need production-grade Lua at C execution speed
- You want async Rust integration with Lua coroutines
- You need Serde serialization of Lua values
- You are building a Lua module (`.so`/`.dll`) rather than embedding
- You need multiple Lua version support (5.1 through 5.5)
- Your build environment has a C compiler available

### Use Luau When

- You need sandboxed execution of untrusted scripts
- Performance is critical (native codegen for hot paths)
- You want gradual typing for large Lua codebases
- You need Roblox ecosystem compatibility
- You can accept the divergence from standard Lua 5.1 semantics
  (`setfenv`/`getfenv` removed, no `__gc`, no `string.dump`)

### Use PUC-Rio Lua Directly When

- You are writing C/C++ and do not need Rust integration
- You need the absolute reference behavior
- You are extending Lua at the C API level with existing C libraries
