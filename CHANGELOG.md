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
- Compatibility CI workflow: builds PUC-Rio Lua 5.1.1 alongside rilua and
  runs all test files through both interpreters, producing a PASS/FAIL
  comparison table as a GitHub job summary. Informational only, never
  blocks PRs.
- Lua 5.1.1 "Values and Types" (section 2.2) semantics:
  - String-to-number coercion in arithmetic: `"3" + 5` evaluates to `8`
  - Number-to-string coercion in concatenation: `3 .. " apples"` evaluates
    to `"3 apples"`
  - String comparison with byte ordering: `"a" < "b"` works correctly
  - Number formatting matching C's `%.14g` (`lua_fmt_number`)
  - `tonumber(e [, base])` with base 2-36 support
  - `tostring(e)` for all current types
  - Numeric `for` loop string coercion: `for i = "1", "10", "1" do ... end`
- Integration test `test14.lua` covering type coercion and comparison
- Lua 5.1.1 "Variables" (section 2.3) semantics:
  - `break` statement with backpatching: terminates innermost `while`,
    `repeat`, or numeric `for` loop. Syntax error when used outside a loop.
    Must be the last statement in a block (Lua 5.1 constraint).
  - Upvalues and closures: inner functions capture outer local variables as
    shared references. Two-state upvalue model (open: points to stack slot;
    closed: owns value after enclosing scope exits). Multiple closures share
    the same upvalue when capturing the same variable.
  - Three-stage variable resolution: local, upvalue, global (mirrors
    PUC-Rio's `singlevaraux` in `lparser.c`)
  - `UpvalueDesc` metadata in `Chunk` for compile-time upvalue tracking
  - `LuaClosure` type replacing `LuaFn`, carrying `Chunk` + upvalue vector
  - `GetUpval`, `SetUpval`, `Close` VM instructions
  - Per-iteration upvalue closing in `for` loops: each iteration snapshots
    captured variables so closures created in different iterations hold
    independent values
  - Open upvalue registry (`BTreeMap`) in VM state for upvalue sharing and
    scope-exit closing
- Integration test `test15.lua` covering `break` in `while`, `repeat`, and
  numeric `for` loops
- Integration test `test16.lua` covering closures: basic counter, independent
  instances, shared upvalues, three-level chains, for-loop per-iteration
  capture, break with upvalues, while-loop closures, multiple upvalues,
  shadowing, and accumulator patterns
- Lua 5.1.1 "Statements" (section 2.4) semantics:
  - Multiple return values: `eval_chunk` handles arbitrary return counts
    (previously panicked on >1)
  - `local function` syntax: `local function f() ... end` with name visible
    in body for recursion
  - Unparenthesized function calls: `f "string"` and `f {table}` as single-
    argument call forms
  - Method call syntax: `obj:method(args)` with `Self_` instruction that
    pushes method function and receiver. Method declarations
    (`function t:m() ... end`) inject implicit `self` parameter.
  - Generic `for` loop: `for k, v in explist do ... end` with `TForLoop`
    instruction implementing the iterator protocol (generator, state,
    control)
  - `pairs(t)` and `next(table [, key])` standard library functions for
    table iteration
  - Table `#` length operator for sequence-style tables
  - Two-token lookahead in table constructors for `Name '='` vs expression
    disambiguation (mirrors PUC-Rio's `luaX_lookahead`)
- Integration test `test17.lua` covering multiple return values, `local
  function` recursion, upvalue capture, and unparenthesized calls
- Integration test `test18.lua` covering method calls, generic `for` with
  `pairs`/`ipairs`, table length operator, and `next()` function
- Lua 5.1.1 "Expressions" (section 2.5) semantics:
  - Varargs (`...`): variadic function parameters and expression position.
    `is_vararg` flag on `Chunk`, `VarArg(u8)` instruction, vararg storage
    in `Frame`. Top-level chunks are implicitly vararg (matches PUC-Rio).
  - Multi-return expansion in table constructors: `{f()}` and `{1, 2, f()}`
    correctly expand all return values of the last expression via
    `SetListMulti` instruction
  - Multi-return expansion in function call arguments: `g(a, f())` expands
    all return values of the last argument via `CallVar` instruction with
    variable argument count protocol (`num_args = 255`)
  - `select('#', ...)` returns count of varargs; `select(n, ...)` returns
    the n-th value onward
- Integration test `test19.lua` covering modulo semantics, varargs,
  multi-return expansion in table constructors and function calls, and
  `select()`

### Fixed

- Modulo operator uses floor division semantics per Lua 5.1.1 spec:
  `a % b == a - math.floor(a/b)*b`. Previously used Rust's truncated
  remainder, giving wrong results for negative operands (e.g. `-5 % 3`
  returned `-2` instead of `1`).

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
- Generic `for` loops (`pairs`, iterators)
- Metatables and metamethods
- Method calls (`:` syntax)
- Varargs (`...`)
- Most standard library functions (`string`, `table`, `math`, `os`, `io`,
  `coroutine`, `debug`)
- `pcall`, `xpcall`, `error`
- `loadstring`, `loadfile`, `dofile`
- `tostring` with `__tostring` metamethod support
- `select`, `rawget`, `rawset`, `rawequal`
- `setmetatable`, `getmetatable`
- `collectgarbage` API
