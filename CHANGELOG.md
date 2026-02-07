# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

## [Unreleased]

### Added

- Forked from [lua-in-rust](https://github.com/cjneidhart/lua-in-rust) by
  Chris Neidhart and ported to the rilua project
- Dual MIT/Apache-2.0 license (upstream was MIT only)
- `README.md` written for the WoW Emulation project context
- `CLAUDE.md` with project-specific guidance for AI-assisted development
- GitHub Actions CI workflow: format, clippy, test (MSRV 1.92.0 + stable),
  docs, with branch protection gate
- Short string escape sequence processing (Lua 5.1.1 `llex.c` `read_string`):
  named escapes (`\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`, `\\`, `\"`,
  `\'`), decimal byte escapes (`\ddd`, 0-255), backslash-newline, and
  unknown escape passthrough (backslash dropped)
- `EscapeTooLarge` syntax error for `\ddd` values above 255
- Uppercase hex literal prefix (`0X`) support alongside existing `0x`
- Carriage return (`\r`) line ending support throughout the lexer:
  `\r`, `\r\n`, and `\n\r` recognized as line endings in whitespace,
  comments, short strings (rejected as unescaped newlines), and long
  bracket bodies (leading newline skip and body normalization to `\n`)
- Reference: `~/Repos/github.com/cogwheel/lua-wow` (WoW-compatible Lua
  5.1.1 distribution documenting WoW client configuration)
- Acknowledgments in `README.md` for lua-wow, Elune, and WoWBench
  reference projects

### Fixed

- `#` operator on strings with bytes 128-255 returned incorrect length
  (e.g. `#"\255"` returned 2 instead of 1) because Rust `String` encoded
  high bytes as multi-byte UTF-8

### Changed

- Internal string representation changed from `String` to `Vec<u8>` for
  binary-safe Lua string semantics. Lua strings are arbitrary byte
  sequences, not UTF-8 text. Affects `MarkedString.data`,
  `Chunk.string_literals`, `process_escapes`, and all string-handling paths
  through the compiler and VM.
- Crate renamed from `lua` to `rilua`
- Package metadata updated: author, description, repository URL
- REPL banner changed from "Lua in Rust by Chris Neidhart" to
  "rilua {version} -- Lua 5.1.1 in Rust"
- Crate-level doc comment updated to reference rilua
- `.cargo/config.toml` header comment updated to reference rilua
- `get_literal_string_contents` returns `Result<Vec<u8>>` instead of
  `&str` to support escape processing, CR normalization, and binary content
- PUC-Rio Lua 5.1.1 test suite tests marked `#[ignore]` (skip in CI,
  run with `cargo test -- --ignored`)

---

The following sections document the history of the upstream
[lua-in-rust](https://github.com/cjneidhart/lua-in-rust) codebase from which
rilua was forked. All code below was written by Chris Neidhart unless otherwise
noted.

## lua-in-rust [Unreleased]

### Added

- Long bracket support: long strings (`[[...]]`, `[=[...]=]`, `[==[...]==]`,
  etc.) and long comments (`--[[...]]`, `--[=[...]=]`, etc.) at all bracket
  levels, matching Lua 5.1.1 semantics
- `UnfinishedLongString` and `UnfinishedLongComment` error variants with
  messages matching PUC-Rio Lua. Both are recoverable for REPL multi-line input.
- Integration test `test12.lua` covering all comment and long bracket edge cases,
  verified against the reference PUC-Rio Lua 5.1.1 interpreter
- Project configuration: `.editorconfig`, `.gitattributes`, `.cargo/config.toml`,
  `rust-toolchain.toml`, `.mise.toml`, markdownlint config
- PUC-Rio Lua 5.1.1 official test suite: 15 verbatim test files from the
  official test repository (tag `v5_1_1`). All 15 currently fail due to
  unimplemented features.
- `Cargo.lock` tracked in version control
- Lint configuration in `Cargo.toml` (`[lints.rust]` and `[lints.clippy]`
  sections), replacing inline `#![warn(...)]` attributes in `src/lib.rs`
- Release profile with LTO, single codegen unit, and symbol stripping

### Changed

- Rust edition upgraded from 2021 to 2024
- Rust toolchain pinned to 1.92.0
- `.gitignore` rewritten with structured patterns
- README updated with Lua 5.1.1 targeting description and edition requirement
- Source files reformatted for edition 2024 import ordering
- Clippy fixes applied: inlined format arguments, `match` replaced with `if let`
  and `matches!()` where appropriate, `Self` used instead of concrete type names,
  let-chains in lexer, trailing semicolons on expression statements

## lua-in-rust 0.1.0-dev (2018-11-27 -- 2024-05-24)

Initial development by Chris Neidhart. No tagged release exists.

### Language Features

- Arithmetic operators: `+`, `-`, `*`, `/`, `%`, `^`, unary `-`
- Comparison operators: `==`, `~=`, `<`, `>`, `<=`, `>=`
- Logical operators: `and`, `or`, `not`
- String concatenation: `..`
- String length: `#`
- Global and local variables
- Multiple assignment
- Control flow: `if`/`elseif`/`else`, `while`, `repeat`/`until`, `do`/`end`
- Numeric `for` loops (ascending and descending)
- Function declarations (including on table fields)
- Function calls with arguments and return values
- Table constructors: array-style, keyed, and mixed
- Table field access: dot notation and bracket notation
- Hexadecimal number literals
- Single-line comments (`--`)

### Runtime

- Stack-based virtual machine
- Garbage collector: mark-sweep with interned strings
- Standard library: `assert`, `print`, `type`, `ipairs`, `unpack`
- REPL with multi-line input support
- File execution mode
- Error types: `SyntaxError`, `TypeError`, `ArgError`
- Compile-time debug flags: `LUA_DEBUG_PARSER`, `LUA_DEBUG_VM`, `LUA_DEBUG_GC`

### Not Yet Implemented

- Multiple return values
- `break`
- Closures and upvalues
- Generic `for` loops (`pairs`, iterators)
- Metatables and metamethods
- Method calls (`:` syntax)
- Varargs (`...`)
- Most standard library functions (`string`, `table`, `math`, `os`, `io`,
  `coroutine`, `debug`)
- `pcall`, `xpcall`, `error`
- `loadstring`, `loadfile`, `dofile`
- `tostring`, `tonumber`
- `select`, `rawget`, `rawset`, `rawequal`
- `setmetatable`, `getmetatable`
- `collectgarbage` API
