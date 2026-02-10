# rilua

A Rust implementation of [Lua 5.1.1](https://lua.org/manual/5.1/).

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)
[![Rust Version](https://img.shields.io/badge/rust-1.92+-orange.svg)](https://www.rust-lang.org)

> **Status**: Early development. Architecture defined, implementation
> in progress.

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

## Architecture

Pipeline: **Source -> Lexer -> Parser -> AST -> Compiler -> Proto -> VM**

| Component | Description |
|-----------|-------------|
| Lexer | Tokenizer with one-token lookahead |
| Parser | Recursive descent, produces AST |
| Compiler | Walks AST, emits register-based bytecode into Proto |
| VM | Register-based, PUC-Rio's 38 opcodes, CallInfo chain |
| GC | Arena-based mark-sweep with generational indices |
| API | Trait-based, Rust-idiomatic (inspired by mlua) |

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

# Run tests
cargo test

# Run quality gate
cargo fmt -- --check && cargo clippy --all-targets && cargo test && cargo doc --no-deps
```

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
