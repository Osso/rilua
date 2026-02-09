# Testing Strategy

## Decision

**Spec-driven, multi-layer testing. Unit tests for internals,
integration tests for language semantics, PUC-Rio official test
suite as the compatibility target.**

Note: This document describes the planned testing strategy. The
layers below are implemented incrementally as features are added.

## Test Layers

### Layer 1: Unit Tests

In-module `#[cfg(test)]` blocks testing internal components in
isolation.

**Lexer tests** (`src/compiler/lexer.rs`):

- Token type recognition for all keywords and symbols
- Number literal parsing (decimal, hex, float, exponent)
- String literal parsing (escapes, long brackets)
- Comment handling (single-line, long comments)
- Error cases (unterminated strings, invalid escapes)
- Source position tracking

**Parser tests** (`src/compiler/parser.rs`):

- Expression parsing with operator precedence
- Statement parsing for each statement type
- Block and scope handling
- Error recovery and error messages
- AST structure verification

**Compiler tests** (`src/compiler/codegen.rs`):

- Instruction emission for each AST node type
- Register allocation
- Constant pool management
- Upvalue resolution
- Jump backpatching

**VM tests** (`src/vm/`):

- Instruction dispatch correctness
- Stack manipulation
- GC arena allocation and collection
- Table operations (get, set, resize)
- String interning
- Closure creation and upvalue management

### Layer 2: Integration Tests

Lua scripts in `tests/` that exercise language features through the
full pipeline. Each test uses `assert()` to validate behavior.

Organization:

```text
tests/
  integration.rs       Test runner (calls run_file for each .lua)
  test01.lua           Basic operations
  test02.lua           Control flow
  test03.lua           Functions
  ...
```

Each test file focuses on a specific area:

- Arithmetic and comparison operators
- String operations and concatenation
- Table construction and access
- Control flow (if/while/for/repeat)
- Functions (calls, returns, varargs)
- Closures and upvalues
- Metatables and metamethods
- Error handling (pcall, xpcall, error)
- Standard library functions
- Edge cases and corner cases

### Layer 3: PUC-Rio Official Test Suite

The PUC-Rio Lua 5.1.1 test suite (`tests/lua51/`) is the
compatibility target. These are verbatim test files from the
official Lua repository (tag `v5_1_1`).

| Test File | Area |
|-----------|------|
| `all.lua` | Test runner |
| `api.lua` | C API interactions (requires testC) |
| `attrib.lua` | require/package system, assignments, operators |
| `big.lua` | String overflow, large line counts, table constructs |
| `calls.lua` | Function calls and returns |
| `checktable.lua` | Table invariant checker (requires testC) |
| `closure.lua` | Closures and upvalues |
| `code.lua` | Code generation, optimizations (requires testC) |
| `constructs.lua` | Syntax, operator priority, language constructs |
| `db.lua` | Debug library |
| `errors.lua` | Error handling |
| `events.lua` | Metatables and metamethods |
| `files.lua` | I/O library |
| `gc.lua` | Garbage collection |
| `literals.lua` | Scanner/lexer and literal parsing |
| `locals.lua` | Local variables |
| `main.lua` | Standalone interpreter (lua.c) options |
| `math.lua` | Math library |
| `nextvar.lua` | Tables, next(), size operator, for loops |
| `pm.lua` | Pattern matching |
| `sort.lua` | table.sort |
| `strings.lua` | String library |
| `vararg.lua` | Vararg functions |
| `verybig.lua` | Very large programs |

The goal is to pass all official tests. Progress is tracked by
counting passing vs failing test files.

**testC dependency**: Three test files (`api.lua`, `code.lua`,
`checktable.lua`) require the `T` global (a C test library compiled
into PUC-Rio's debug builds). These tests skip or degrade gracefully
when `T` is nil. rilua will need a Rust equivalent of the testC
infrastructure to fully exercise these tests.

**Compatibility flags**: The PUC-Rio test suite was written with
default compat options enabled (e.g., `LUA_COMPAT_VARARG` enables
the `arg` table in vararg functions). WoW's Lua disables some of
these. Tests that depend on compat options may need conditional
handling.

### Layer 4: Behavioral Equivalence Tests

Tests that specifically verify behavioral equivalence with PUC-Rio
Lua 5.1.1. These test edge cases where implementations commonly
diverge:

- Numeric formatting (`tostring(0.1)`, `string.format("%g", 0.1)`)
- Error message wording (programs may match on error strings)
- GC behavior (`collectgarbage` return values)
- Weak table clearing timing
- Finalizer execution order
- String-to-number coercion edge cases
- Integer overflow behavior (all numbers are f64)
- Modulo with negative operands (floor division)
- Concatenation type coercion

## Quality Gate

Every commit must pass:

```bash
cargo fmt -- --check && \
cargo clippy --all-targets && \
cargo test && \
cargo doc --no-deps
```

This ensures:

1. Consistent formatting
2. No lint warnings
3. All tests pass
4. Documentation builds without errors

## Test-Driven Development

New features are implemented test-first where possible:

1. Write a Lua test script that exercises the feature.
2. Run the test against PUC-Rio Lua 5.1.1 to verify expected
   behavior.
3. Implement the feature in rilua.
4. Run the test and fix until it passes.
5. Run the full test suite to check for regressions.

This ensures every feature is validated against the reference
implementation.

## Coverage Tracking

Test coverage is measured by:

1. **Feature coverage** — which Lua 5.1.1 features are implemented
   and tested (tracked in CHANGELOG.md).
2. **PUC-Rio test suite progress** — N of 24 official test files
   passing (tracked in CI).
3. **Code coverage** — `cargo-tarpaulin` or `llvm-cov` for line
   coverage metrics (informational, not a gate).
