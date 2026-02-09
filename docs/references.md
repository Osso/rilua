# Reference Implementations

Classification of implementations studied during architecture design.
Each was examined for its approach to compilation, VM design, memory
management, and API surface.

## Tier 1 — Primary References

### PUC-Rio Lua 5.1.1 (C)

- **Local path**: `~/Repos/github.com/lua/lua` (tag `v5.1.1`)
- **Role**: The specification. All behavioral questions are answered here.
- **Architecture**: 17,654 lines. Single-pass recursive descent compiler
  emitting 38 register-based opcodes (u32 packed). CallInfo chain for
  call stack. Incremental tri-color mark-sweep GC with write barriers.
  Array+hash tables. Interned strings with cached hash. longjmp/setjmp
  error handling. Stack-based C API.
- **What we take**: Opcode semantics, GC behavioral contract, weak table
  semantics, finalizer protocol, `collectgarbage()` API, standard library
  behavior, error messages.
- **What we change**: Single-pass compilation (we use AST), longjmp (we
  use Result), C API (we use traits), raw pointers (we use arenas).

### Luau (C++)

- **Local path**: `~/Repos/github.com/luau-lang/luau`
- **Role**: Architecture reference. Lua 5.1-compatible scripting language.
- **Architecture**: Lexer -> Parser -> AST -> Compiler -> VM. 83
  register-based opcodes (count evolves with development). Incremental tri-color GC. CallInfo chain.
  Array+hash tables. String interning. No longjmp (C++ exceptions).
- **What we take**: AST-based pipeline design, separation of compiler
  phases, Proto as non-GC value (owned by closures), CallInfo chain
  pattern, incremental GC debt model.
- **What we change**: Opcode count (we use PUC-Rio's 38, not Luau's 83),
  language extensions (we implement standard Lua 5.1.1 only), native
  codegen (out of scope).

## Tier 2 — Selective References

### tsuki (Rust, Lua 5.4)

- **Local path**: `~/Repos/github.com/ultimaweapon/tsuki`
- **Role**: Proves Result-based error handling works for a Lua VM.
- **Architecture**: c2rust translation of Lua 5.4. PUC-Rio file naming
  (llex.rs, lparser.rs, lcode.rs). 83 opcodes. Incremental GC via raw
  pointers. Heavy use of unsafe code. Memory-safe public API over unsafe internals.
- **What we take**: Result-based error propagation pattern, memory-safe
  public API design, packed u32 instruction format for
  serialization.
- **What we avoid**: c2rust code style, heavy unsafe, wrong Lua version
  (5.4 vs 5.1.1).

### full-moon (Rust, parser only)

- **Local path**: `~/Repos/github.com/Kampfkarren/full-moon`
- **Role**: Best Rust patterns for Lua parsing.
- **Architecture**: Hand-written recursive descent parser. Lossless AST
  preserving whitespace and comments. Error recovery with partial AST.
  Multi-dialect support (5.1-5.4, Luau) via feature flags. Sealed
  traits, visitor pattern, builder pattern.
- **What we take**: Recursive descent parsing patterns, AST node design
  idioms, error recovery approach, sealed trait pattern.
- **What we avoid**: Lossless parsing (unnecessary for a VM), multi-dialect
  support (we only target 5.1.1), external dependencies.

### mlua (Rust, FFI wrapper)

- **Local path**: `~/Repos/github.com/mlua-rs/mlua`
- **Role**: API design reference for embedding Lua in Rust.
- **Architecture**: FFI wrapper around C Lua. Memory-safe Rust API over
  unsafe C bindings. Trait-based type conversions (`IntoLua`, `FromLua`).
  Registry-based lifetime management. Scoped values. UserData trait.
- **What we take**: Trait-based API design (`IntoLua`/`FromLua` pattern),
  UserData trait approach, scope-based lifetime management, error type
  design.
- **Not applicable**: FFI wrapping (we implement from scratch), C Lua
  compilation, vendored builds.

## Tier 3 — Limited Relevance

### lua-in-rust (Rust, Lua 5.1.1)

- **Local path**: `~/Repos/github.com/cjneidhart/lua-in-rust`
- **Role**: Previous base for rilua. Studied for lessons learned.
- **Architecture**: Single-pass compiler, stack-based VM (~40 custom
  opcodes), stop-the-world mark-sweep GC with raw pointers, string
  interning, HashMap-only tables. ~5,000 lines. ~40% complete.
- **Lessons**: Stack-based VM diverges too far from PUC-Rio's
  register-based design. Single-pass compilation makes the compiler
  hard to test and extend. Raw pointer GC works but is fragile.
  String interning with pointer equality is the right approach.

### lua-rs (Rust, compiler only)

- **Local path**: `~/Repos/github.com/lonng/lua-rs`
- **Role**: Shows AST-to-bytecode compilation for Lua 5.1.
- **Architecture**: Multi-pass with explicit AST. Scanner -> Parser ->
  AST -> Compiler -> FunctionProto. No VM. Broken build. Zero unsafe.
- **Useful for**: AST node design, constant folding patterns, debug
  info tracking. Not useful for VM or runtime design.

### coppermoon (Rust, mlua wrapper)

- **Local path**: `~/Repos/github.com/coppermoondev/coppermoon`
- **Role**: None for VM/compiler work.
- **Architecture**: Node.js-like runtime wrapping mlua (C Lua 5.4 FFI).
  Not a from-scratch implementation. Provides batteries-included stdlib
  (HTTP, SQLite, WebSocket, etc.) over the C VM.
- **Not applicable**: Everything. This wraps C Lua, not implements it.
