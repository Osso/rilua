# rilua

A Rust implementation of [Lua 5.1.1](https://lua.org/manual/5.1/).

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)
[![Rust Version](https://img.shields.io/badge/rust-1.92+-orange.svg)](https://www.rust-lang.org)

> **Note**: This implementation is developed for the [WoW Emulation project][wowemu] and
> has not been used outside of that context. The API may change as the project
> matures.

[wowemu]: https://github.com/wowemulation-dev

## Overview

rilua is a from-scratch Lua 5.1.1 interpreter written in Rust. It targets the
Lua variant embedded in the World of Warcraft game client. Zero external
dependencies -- only Rust's standard library.

The crate exposes both a library and a binary (`rilua`, the interpreter/REPL).

Based on [lua-in-rust](https://github.com/cjneidhart/lua-in-rust) by Chris
Neidhart.

### Why Lua 5.1.1

World of Warcraft's addon system uses Lua 5.1.1. Key 5.1-specific traits:
`unpack` is a global (moved to `table.unpack` in 5.2), all numbers are `f64`
(5.3 added integers), no `goto` keyword (added in 5.2). See
[Warcraft Wiki: Lua](https://warcraft.wiki.gg/wiki/Lua).

## Features

### Language Features

- Arithmetic: `+`, `-`, `*`, `/`, `%`, `^`, unary `-`
- Comparison: `==`, `~=`, `<`, `>`, `<=`, `>=`
- Logical: `and`, `or`, `not`
- String concatenation (`..`) and length (`#`)
- Global and local variables with multiple assignment
- Control flow: `if`/`elseif`/`else`, `while`, `repeat`/`until`, `do`/`end`
- Numeric `for` loops
- Function declarations and calls
- Table constructors (array, keyed, mixed) and field access
- Hexadecimal number literals
- Comments: single-line (`--`) and long brackets (`--[[...]]`)
- Long strings (`[[...]]`, `[=[...]=]`, etc.)
- String escape sequences: named (`\n`, `\t`, etc.) and decimal byte (`\ddd`)

### Runtime

- Single-pass compiler emitting bytecode (no AST)
- Stack-based virtual machine
- Mark-sweep garbage collector with interned strings
- REPL with multi-line input
- File execution mode
- Line and column tracking in error messages

### Standard Library

- `assert`, `print`, `type`, `ipairs`, `unpack`

### Not Yet Implemented

- Multiple return values, `break`, closures, upvalues
- Generic `for` loops, metatables, metamethods
- Method calls (`:` syntax), varargs (`...`)
- Most standard library modules (`string`, `table`, `math`, `os`, `io`,
  `coroutine`, `debug`)
- `pcall`, `xpcall`, `error`, `tostring`, `tonumber`

## Usage

```bash
# Launch the REPL
cargo run

# Execute a Lua file
cargo run -- script.lua
```

### As a Library

```rust
fn main() {
    let mut state = rilua::State::new();
    state.open_libs();
    state.do_string("print('hello from rilua')").unwrap();
}
```

See [`examples/hello.rs`](examples/hello.rs) for a runnable version:

```bash
cargo run --example hello
```

## Building

Development tools (Rust 1.92.0, markdownlint) can be installed automatically
with [Mise](https://mise.jdx.dev/):

```bash
mise install
```

```bash
# Build
cargo build

# Run tests
cargo test

# Run quality gate
cargo fmt -- --check && cargo clippy --all-targets && cargo test && cargo doc --no-deps
```

### Debug Flags

Set before compiling (read at compile time via `option_env!`):

- `LUA_DEBUG_PARSER=1` -- prints compiled chunk output after parsing
- `LUA_DEBUG_VM=1` -- prints each instruction as it executes
- `LUA_DEBUG_GC=1` -- prints GC statistics during collection

Example: `LUA_DEBUG_VM=1 cargo run -- test.lua`

## Architecture

Pipeline: **Source code -> Lexer -> Parser -> Chunk (bytecode) -> VM**

| Module | Description |
|--------|-------------|
| `compiler/lexer.rs` | Tokenizer with one-token lookahead |
| `compiler/parser.rs` | Recursive descent, emits bytecode into Chunk |
| `vm.rs` | State struct, public API (mirrors Lua C API) |
| `vm/frame.rs` | Instruction dispatch loop |
| `vm/object.rs` | GC heap, mark-sweep, string interning |
| `vm/lua_val.rs` | Value types: Nil, Bool, Num, Str, RustFn, Obj |
| `vm/table.rs` | Lua tables as HashMap |
| `lua_std/` | Standard library implementations |
| `instr.rs` | Bytecode instruction enum (40+ variants) |
| `error.rs` | Error types: SyntaxError, TypeError, ArgError |

## Tests

- Integration tests running `.lua` files through the full pipeline
- Unit tests in compiler and VM modules
- PUC-Rio Lua 5.1.1 official test suite (used as compatibility target)

## License

Dual-licensed under either:

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

## Acknowledgments

- Chris Neidhart for [lua-in-rust](https://github.com/cjneidhart/lua-in-rust)
- Roberto Ierusalimschy, Waldemar Celes, and Luiz Henrique de Figueiredo for
  [Lua](https://lua.org)
- Matthew Orlando (cogwheel) for
  [lua-wow](https://github.com/cogwheel/lua-wow), a WoW-compatible Lua 5.1.1
  distribution documenting the WoW client's Lua configuration
- Meorawr for [Elune](https://github.com/Meorawr/elune), a Lua 5.1 fork
  implementing WoW's tainted execution model
- mikeclueby4 for [WoWBench](https://sourceforge.net/projects/wowbench), a
  standalone test harness emulating the WoW Lua addon environment

## Resources

- [Lua 5.1 Reference Manual](https://lua.org/manual/5.1/)
- [PUC-Rio Lua 5.1.1 Source](https://github.com/lua/lua/tree/v5.1.1)
- [Warcraft Wiki: Lua](https://warcraft.wiki.gg/wiki/Lua)
