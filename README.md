# rilua

A Rust implementation of [Lua 5.1.1](https://lua.org/manual/5.1/).

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)
[![Rust Version](https://img.shields.io/badge/rust-1.92+-orange.svg)](https://www.rust-lang.org)

## Overview

rilua is a from-scratch Lua 5.1.1 interpreter written in Rust. It targets
behavioral equivalence with the PUC-Rio reference interpreter -- executed
Lua code must produce identical results.

Part of the [WoW Emulation project](https://github.com/wowemulation-dev).
Zero external dependencies -- only Rust's standard library.

### Use Cases

rilua is built for the World of Warcraft emulation ecosystem:

- **Addon development and testing** -- Run and test WoW addons outside the
  game client without launching WoW
- **Server-side scripting** -- Embed in private server emulators (CMaNGOS,
  TrinityCore, AzerothCore) for scripted encounters, quests, and NPC
  behavior
- **Client Lua environment emulation** -- Reproduce the WoW client's Lua
  sandbox including restricted stdlib, taint system, and WoW-specific
  extensions (bit library, string functions, global aliases)
- **Addon compatibility testing** -- Automated test harness for verifying
  addons against the Lua 5.1.1 spec

It also serves as an embeddable Lua 5.1.1 interpreter for Rust applications
and as a readable reference implementation for studying Lua internals.
See `docs/use-cases.md` for details.

### Why Lua 5.1.1

World of Warcraft's addon system uses Lua 5.1.1. Key 5.1-specific traits:
`unpack` is a global (moved to `table.unpack` in 5.2), all numbers are `f64`
(5.3 added integers), no `goto` keyword (added in 5.2). See
[Warcraft Wiki: Lua](https://warcraft.wiki.gg/wiki/Lua).

## Usage

### Standalone Interpreter

`rilua` reproduces the PUC-Rio `lua` command-line interface:

```bash
# Run a Lua script
rilua script.lua

# Execute a string
rilua -e 'print("hello")'

# Interactive REPL
rilua -i

# All flags: -e stat, -l name, -i, -v, --, -
rilua -v
# Lua 5.1.1  Copyright (C) 1994-2006 Lua.org, PUC-Rio
```

### Bytecode Compiler

`riluac` reproduces the PUC-Rio `luac` bytecode compiler and lister:

```bash
# Compile to bytecode
riluac -o output.luac script.lua

# List bytecode instructions
riluac -l script.lua

# Detailed listing (constants, locals, upvalues)
riluac -l -l script.lua

# Syntax check only
riluac -p script.lua
```

Binary chunks are cross-compatible with PUC-Rio in both directions.

### Embedding in Rust

rilua provides a Rust-idiomatic API with `IntoLua`/`FromLua` conversion
traits (inspired by [mlua](https://github.com/mlua-rs/mlua)):

```rust
use rilua::{Lua, StdLib};

// Create interpreter with all standard libraries
let mut lua = Lua::new_with(StdLib::ALL)?;

// Execute Lua code
lua.exec("x = 1 + 2")?;

// Read and write globals with automatic type conversion
let x: f64 = lua.global("x")?;
assert_eq!(x, 3.0);
lua.set_global("greeting", "hello")?;

// Selective library loading for sandboxing
let mut sandbox = Lua::new_with(StdLib::BASE | StdLib::STRING | StdLib::TABLE)?;
```

See `docs/api.md` for the full API reference.

## Supported Features

### Language

All Lua 5.1.1 language features are implemented:

- Variables, assignments, local declarations
- Control flow: `if`/`elseif`/`else`, `while`, `repeat`/`until`, numeric
  `for`, generic `for`, `break`, `return`
- Functions: closures, varargs (`...`), multiple return values, tail calls,
  method syntax (`obj:method()`)
- Tables: array and hash parts, constructors (`{1, 2, key = "val"}`)
- Metatables: all 17 metamethods (`__index`, `__newindex`, `__call`,
  `__add`, `__sub`, `__mul`, `__div`, `__mod`, `__pow`, `__unm`, `__eq`,
  `__lt`, `__le`, `__concat`, `__len`, `__gc`, `__tostring`)
- String metatable: method syntax (`("hello"):upper()`)
- Coroutines: `create`, `resume`, `yield`, `wrap`, `status`, `running`
- Environments: `setfenv`/`getfenv`, per-closure global tables
- Protected calls: `pcall`, `xpcall` with error objects and stack traces
- Error messages with variable names (matching PUC-Rio format)

### Standard Libraries

All 9 standard libraries with all functions:

| Library | Functions | Notes |
|---------|-----------|-------|
| base | 29 | `print`, `assert`, `type`, `tostring`, `tonumber`, `pairs`, `ipairs`, `next`, `select`, `unpack`, `pcall`, `xpcall`, `error`, `loadstring`, `loadfile`, `dofile`, `load`, `setmetatable`, `getmetatable`, `rawget`, `rawset`, `rawequal`, `setfenv`, `getfenv`, `collectgarbage`, `newproxy`, `_G`, `_VERSION` |
| string | 14 | `len`, `byte`, `char`, `sub`, `rep`, `reverse`, `lower`, `upper`, `format`, `find`, `match`, `gmatch`, `gsub`, `dump`. Pattern matching with all Lua 5.1.1 features. `gfind` alias included. |
| table | 9 | `concat`, `insert`, `remove`, `sort`, `maxn`, `getn`, `setn`, `foreach`, `foreachi`. Sort uses PUC-Rio's median-of-three quicksort. |
| math | 28 | `abs` through `tanh`, `pi`, `huge`, `mod` alias. |
| io | 18 | 11 library functions + 7 file methods. `stdin`/`stdout`/`stderr` handles. |
| os | 11 | `clock`, `date`, `difftime`, `execute`, `exit`, `getenv`, `remove`, `rename`, `setlocale`, `time`, `tmpname`. |
| debug | 14 | `getinfo`, `getlocal`, `setlocal`, `getupvalue`, `setupvalue`, `traceback`, `getregistry`, `getmetatable`, `setmetatable`, `getfenv`, `setfenv`, `gethook`, `sethook`, `debug`. |
| package | 9 | `require`, `module`, `loaded`, `preload`, `loaders`, `config`, `path`, `cpath`, `seeall`, `loadlib`. |
| coroutine | 6 | `create`, `resume`, `yield`, `wrap`, `status`, `running`. |

### Bytecode and Compatibility

- 38 register-based opcodes matching PUC-Rio encoding
- `string.dump` and binary chunk loading
- Binary chunks are cross-compatible with PUC-Rio (byte-identical output
  for simple programs, loadable in both directions)
- Non-UTF-8 source files supported (`\255`, `\0` in string literals)

### Garbage Collector

Arena-based incremental mark-sweep with generational indices:

- 5-state incremental collection (Pause, Propagate, SweepString, Sweep,
  Finalize)
- Write barriers (backward for tables, forward for upvalues)
- `__gc` finalizers with error propagation
- Weak tables (`__mode` = "k", "v", or "kv")
- `collectgarbage()` API: collect, stop, restart, count, step, setpause,
  setstepmul

## Known Limitations

### Not Yet Implemented

- **`debug.debug()` interactive mode**: Stub (returns immediately).
- **C library loading**: `package.loadlib` returns "not supported"
  (incompatible ABI). Lua file loading via `require` works.
- **SIGINT handling**: No signal-based interruption of running code.

### PUC-Rio Test Suite Compatibility

All 23 official Lua 5.1.1 test files pass, including the `all.lua`
runner which executes all tests sequentially with aggressive GC settings.
Tests: api, attrib, big, calls, checktable, closure, code, constructs,
db, errors, events, files, gc, literals, locals, main, math, nextvar,
pm, sort, strings, vararg, verybig.

The `all.lua` runner completes in ~17 seconds.

See `docs/testing.md` for details on running modes and the comparison
script.

## Architecture

Pipeline: **Source -> Lexer -> Parser -> AST -> Compiler -> Proto -> VM**

| Component | Description |
|-----------|-------------|
| Lexer | Tokenizer with one-token lookahead, byte-based (`&[u8]`) |
| Parser | Recursive descent producing typed AST |
| Compiler | AST walker emitting register-based bytecode into Proto |
| VM | Register-based dispatch, PUC-Rio's 38 opcodes, CallInfo chain |
| GC | Arena-based incremental mark-sweep, write barriers, finalizers |
| API | Trait-based Rust-idiomatic embedding (`IntoLua`/`FromLua`) |

See `docs/architecture.md` for design documentation.

## Building

Development tools (Rust 1.92.0, markdownlint) can be installed automatically
with [Mise](https://mise.jdx.dev/):

```bash
mise install
```

```bash
# Build
cargo build

# Run the interpreter
cargo run -- script.lua

# Run tests
cargo test

# Run quality gate
cargo fmt -- --check && cargo clippy --all-targets && cargo test && cargo doc --no-deps
```

## Testing

Five test layers: unit tests inside compiler and VM modules,
integration tests (Lua scripts with `assert()`), oracle comparison
tests (same Lua code run in both rilua and PUC-Rio, comparing output),
the PUC-Rio official test suite as a compatibility target, and
behavioral equivalence tests for edge cases.

PUC-Rio tests pass both individually and through the `all.lua` runner.
See `docs/testing.md` for the testing strategy and
[lua.org/tests/](https://lua.org/tests/) for the official test
documentation.

## License

Dual-licensed under either:

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

## Acknowledgments

- Roberto Ierusalimschy, Waldemar Celes, and Luiz Henrique de Figueiredo for
  [Lua](https://lua.org)
- The [Luau](https://github.com/luau-lang/luau) team at Roblox for
  demonstrating AST-based Lua compilation at scale
- The [mlua](https://github.com/mlua-rs/mlua) project for Rust-idiomatic
  Lua API patterns
- Matthew Orlando (cogwheel) for
  [lua-wow](https://github.com/cogwheel/lua-wow), documenting the WoW
  client's Lua configuration

## Resources

- [Lua 5.1 Reference Manual](https://lua.org/manual/5.1/)
- [PUC-Rio Lua 5.1.1 Source](https://github.com/lua/lua/tree/v5.1.1)
- [Warcraft Wiki: Lua](https://warcraft.wiki.gg/wiki/Lua)
