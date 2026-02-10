# Testing Strategy

## Decision

**Spec-driven, multi-layer testing. Unit tests for internals,
oracle comparison for behavioral equivalence, integration tests
for language semantics, PUC-Rio official test suite as the
compatibility target.**

As of Phase 3 completion: 466 tests (431 unit, 16 integration,
19 oracle). 60/60 oracle test cases pass against PUC-Rio 5.1.1.
Layers 1-3 are active. Layer 4 (PUC-Rio test suite) activates
after Phase 5a (base library). Layer 5 (behavioral equivalence)
is partially active via oracle comparison tests.

## Test Layers

### Layer 1: Unit Tests

In-module `#[cfg(test)]` blocks testing internal components in
isolation. Every implementation chunk includes unit tests.

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

### Layer 2: Oracle Comparison Tests

Oracle comparison tests run the same Lua code in both rilua and
PUC-Rio Lua 5.1.1, comparing output to verify behavioral equivalence.
This catches divergences that unit tests and integration tests might
miss.

#### Reference Binaries

- **lua**: `~/Repos/github.com/lua/lua/lua` (PUC-Rio Lua 5.1.1,
  git tag `v5.1.1`)
- **luac**: built from the Lua 5.1.1 source distribution
  (`https://lua.org/ftp/lua-5.1.1.tar.gz`)

The `lua` binary path is configured via the `LUA_REFERENCE_BIN`
environment variable. Tests that require the reference binary skip
gracefully if it is not available.

#### Oracle Test Framework

Test helpers in `tests/helpers/`:

```rust
// tests/helpers/oracle.rs

/// Run code in PUC-Rio Lua 5.1.1 and return (stdout, stderr, exit_code).
fn run_reference(code: &str) -> (String, String, i32);

/// Run code in rilua and return (stdout, stderr).
fn run_rilua(code: &str) -> (String, String);

/// Assert rilua produces the expected stdout for the given code.
fn assert_output(code: &str, expected: &str);

/// Run in both interpreters, assert stdout matches.
fn assert_matches_reference(code: &str);
```

Usage in tests:

```rust
#[test]
fn arithmetic_matches_reference() {
    assert_matches_reference("print(1 + 2)");
    assert_matches_reference("print(2 ^ 10)");
    assert_matches_reference("print(10 % 3)");
    assert_matches_reference("print(-7 % 3)");  // floor modulo
}
```

#### Bytecode Comparison

After the compiler is implemented, bytecode comparison tests compile
Lua snippets with both rilua and `luac -l`, then compare instruction
output. This verifies the compiler produces correct bytecode before
the VM exists.

```rust
/// Compile code with rilua and return a formatted instruction listing.
fn compile_rilua(code: &str) -> String;

/// Compile code with PUC-Rio luac -l and return the listing.
fn compile_reference(code: &str) -> String;

/// Assert both compilers produce equivalent bytecode.
fn assert_bytecode_matches(code: &str);
```

Bytecode comparison checks instruction opcodes and operands. It does
not compare constant pool ordering or debug info formatting, since
these may differ between implementations without affecting semantics.

#### When Each Test Category Activates

| Category | Activates after | Mechanism |
|----------|----------------|-----------|
| Unit tests | Phase 0 (skeleton) | `cargo test --lib` |
| Bytecode comparison | Phase 2 (compiler) | Compare rilua compiler output with `luac -l` |
| Oracle comparison | Phase 3 + `print` | Compare rilua output with `lua -e` |
| Integration `.lua` tests | Phase 3 + `assert` | `cargo test --test integration` |
| PUC-Rio test suite | Phase 5a (base lib) | `cargo test --test lua51` |

### Layer 3: Integration Tests

Lua scripts in `tests/` that exercise language features through the
full pipeline. Each test uses `assert()` to validate behavior.

Organization mirrors the Lua 5.1 Reference Manual. Language tests
cover Chapter 2 ("The Language"), standard library tests cover
Chapter 5 ("Standard Libraries").

**Language tests** (Chapter 2):

| File | Section | Description |
|------|---------|-------------|
| `lexical.lua` | 2.1 | Lexical conventions (keywords, names, strings, numbers, comments) |
| `types.lua` | 2.2 | Values and types, coercion (2.2.1) |
| `variables.lua` | 2.3 | Global, local, and table field variables |
| `statements.lua` | 2.4 | Chunks, blocks, assignment, control structures, for loops, local declarations |
| `expressions.lua` | 2.5 | Arithmetic, relational, logical operators, concatenation, length, precedence, table constructors, function calls, function definitions |
| `visibility.lua` | 2.6 | Lexical scoping, upvalues, closures |
| `errors.lua` | 2.7 | error(), pcall, xpcall, error objects, stack traces |
| `metatables.lua` | 2.8 | Metamethods for arithmetic, comparison, indexing, call, concatenation, length |
| `environments.lua` | 2.9 | Function environments, setfenv, getfenv |
| `gc.lua` | 2.10 | Garbage collection, finalizers (2.10.1), weak tables (2.10.2) |
| `coroutines.lua` | 2.11 | create, resume, yield, wrap, status, error propagation |

**Standard library tests** (Chapter 5):

| File | Section | Description |
|------|---------|-------------|
| `stdlib-base.lua` | 5.1 | Base library (assert, type, tonumber, tostring, select, unpack, etc.) |
| `stdlib-package.lua` | 5.3 | Package/module library (require, module, loaders, etc.) |
| `stdlib-string.lua` | 5.4 | String library (find, format, gmatch, gsub, etc.) |
| `stdlib-table.lua` | 5.5 | Table library (concat, insert, remove, sort, maxn) |
| `stdlib-math.lua` | 5.6 | Math library (abs, floor, ceil, random, sin, cos, etc.) |
| `stdlib-io.lua` | 5.7 | I/O library (open, read, write, lines, etc.) |
| `stdlib-os.lua` | 5.8 | OS library (clock, date, time, execute, etc.) |
| `stdlib-debug.lua` | 5.9 | Debug library (getinfo, getlocal, sethook, traceback, etc.) |

```text
tests/
  helpers/
    mod.rs               Shared test utilities
    oracle.rs            PUC-Rio comparison functions
  integration.rs         Test runner (calls run_file for each .lua)
  lua51.rs               PUC-Rio test suite runner
  lexical.lua            2.1  Lexical conventions
  types.lua              2.2  Values and types, coercion
  variables.lua          2.3  Variables
  statements.lua         2.4  Statements and control flow
  expressions.lua        2.5  Expressions and operators
  visibility.lua         2.6  Scoping and closures
  errors.lua             2.7  Error handling
  metatables.lua         2.8  Metatables and metamethods
  environments.lua       2.9  Environments
  gc.lua                 2.10 Garbage collection
  coroutines.lua         2.11 Coroutines
  stdlib-base.lua        5.1  Base library
  stdlib-package.lua     5.3  Package library
  stdlib-string.lua      5.4  String library
  stdlib-table.lua       5.5  Table library
  stdlib-math.lua        5.6  Math library
  stdlib-io.lua          5.7  I/O library
  stdlib-os.lua          5.8  OS library
  stdlib-debug.lua       5.9  Debug library
  lua51/                 PUC-Rio official test suite (verbatim)
```

### Layer 4: PUC-Rio Official Test Suite

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
| `closure.lua` | Closures, upvalues, and coroutines |
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

#### Progressive Test Suite Unlocking

PUC-Rio test files become passable as features are implemented.
This table tracks which test files should pass after each phase
(see `docs/roadmap.md` for phase definitions).

| After | Test files expected to pass |
|-------|-----------------------------|
| Phase 3 (core VM) | -- (test files require stdlib for assertions) |
| Phase 5a (base lib) | `literals.lua` (partial), `constructs.lua` (partial) |
| Phase 4 + 5a | `calls.lua`, `locals.lua`, `vararg.lua` |
| Phase 4 + 5a | `events.lua`, `errors.lua` |
| Phase 5b (string lib) | `strings.lua`, `pm.lua` |
| Phase 5c (table lib) | `sort.lua`, `nextvar.lua` |
| Phase 5d (math lib) | `math.lua` |
| Phase 5e (io lib) | `files.lua` |
| Phase 5g (package lib) | `attrib.lua` |
| Phase 5h (debug lib) | `db.lua` |
| Phase 6 (coroutines) | `closure.lua` |
| Phase 7 (GC) | `gc.lua` |
| Phase 8c (CLI) | `main.lua` |
| All phases | `big.lua`, `verybig.lua` |

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

### Layer 5: Behavioral Equivalence Tests

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

These tests use the oracle comparison framework to run each case in
both rilua and PUC-Rio, comparing exact output. They are the last
line of defense before the PUC-Rio test suite.

## Test Workflow

### Test-Driven Development

New features are implemented test-first where possible:

1. Write a Lua test script that exercises the feature.
2. Run the test against PUC-Rio Lua 5.1.1 to verify expected
   behavior.
3. Implement the feature in rilua.
4. Run the test and fix until it passes.
5. Run the oracle comparison to verify matching output.
6. Run the full test suite to check for regressions.

This ensures every feature is validated against the reference
implementation.

### Quality Gate

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
3. All tests pass (unit, integration, oracle, PUC-Rio suite)
4. Documentation builds without errors

### Coverage Tracking

Test coverage is measured by:

1. **Feature coverage** -- which Lua 5.1.1 features are implemented
   and tested (tracked in CHANGELOG.md).
2. **PUC-Rio test suite progress** -- N of 24 official test files
   passing (tracked in CI).
3. **Oracle comparison count** -- number of Lua snippets verified
   against PUC-Rio output.
4. **Code coverage** -- `cargo-tarpaulin` or `llvm-cov` for line
   coverage metrics (informational, not a gate).
