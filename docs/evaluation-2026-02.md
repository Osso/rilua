# rilua Compatibility and Performance Evaluation

Initial: February 12, 2026
Updated: February 13, 2026 (Phase 9d bug fixes, tarball-based testing)
Branch: `rewrite/v2`
rilua version: 0.1.0 (Phase 9d)
Reference: PUC-Rio Lua 5.1.1 (official tarball at `./lua-5.1.1/`)
Platform: AMD Ryzen 7 8840U, Linux 6.18.8, Fedora 43

## Executive Summary

rilua passes all 1289 internal tests (586 unit + 426 integration + 277
oracle). Against the PUC-Rio official test suite (run from
`./lua-5.1-tests/`), 11 of 20 applicable tests pass with 0 timeouts.
Bugs #15-#28 have been fixed since the initial evaluation; 7 tests
still fail due to call depth limits, streaming reader, per-thread
globals, locale-aware parsing, hook stubs, and CLI subprocess handling.
Performance ranges from 0.92x to 2.18x vs PUC-Rio on most benchmarks,
with one outlier (string operations at 9.33x). Memory usage is 1.15x
to 3.98x higher. Binary sizes are comparable (1.0 MB vs 1.1 MB).

The goal is to reach performance parity with PUC-Rio once all features
are implemented and rilua is behaviorally identical.

## 1. Internal Test Suite

```
Unit tests:        586 passed    (cargo test --lib)
Integration tests: 426 passed    (cargo test --test integration)
Oracle tests:      277 passed    (cargo test --test oracle)
Total:            1289 passed, 0 failed, 0 skipped
```

All tests pass with `cargo test` in under 10 seconds.

## 2. PUC-Rio Official Test Suite

Test suite: `./lua-5.1-tests/` (23 test files + `all.lua` runner).
Run via `scripts/compare.sh ./lua-5.1.1/src/lua ./target/debug/rilua`.
Tests execute from within the test suite directory (required for
relative `dofile()` calls).

### Results Matrix

| Test File      | rilua      | PUC-Rio  | Blocker(s)                        |
|----------------|------------|----------|-----------------------------------|
| api            | N/A        | PASS     | Requires testC library (T==nil)   |
| attrib         | FAIL       | PASS     | File creation infra (writes libs/) |
| big            | FAIL       | FAIL     | Requires checktable (C library)   |
| calls          | FAIL       | PASS     | `load()` reader streaming (#29)   |
| checktable     | N/A        | PASS     | Requires testC library (T==nil)   |
| closure        | FAIL       | PASS     | Per-thread global tables          |
| code           | N/A        | PASS     | Requires testC library (T==nil)   |
| **constructs** | **PASS**   | PASS     |                                   |
| db             | FAIL       | PASS     | Hook stubs (sethook)              |
| **errors**     | **PASS**   | PASS     |                                   |
| **events**     | **PASS**   | PASS     |                                   |
| **files**      | **PASS**   | PASS     |                                   |
| **gc**         | **PASS**   | PASS     |                                   |
| literals       | FAIL       | PASS     | Locale-aware number parsing       |
| **locals**     | **PASS**   | PASS     |                                   |
| main           | FAIL       | FAIL     | CLI subprocess infra              |
| **math**       | **PASS**   | PASS     |                                   |
| **nextvar**    | **PASS**   | PASS     |                                   |
| pm             | FAIL       | PASS     | Call depth limit (#29)            |
| **sort**       | **PASS**   | PASS     |                                   |
| **strings**    | **PASS**   | PASS     |                                   |
| **vararg**     | **PASS**   | PASS     |                                   |
| verybig        | TIMEOUT    | PASS     | Expression combinations           |

N/A: api, checktable, code trivially pass (T==nil skips all assertions)
but do not exercise rilua. They require a testC equivalent to test.

### Summary

| Result    | Count | Of applicable |
|-----------|-------|---------------|
| Pass      | 11    | 55%           |
| N/A       | 3     | (excluded)    |
| Timeout   | 1     | 5%            |
| Fail      | 8     | 40%           |

### Progress Since Initial Evaluation (Feb 12)

The initial evaluation found 5/20 passing with 8 timeouts. After
fixing bugs #15-#28:

- **8 timeouts resolved**: All caused by Bug #18 (while-true-if-break
  compiler JMP target). Fix: `compile_while` used `patch_jump` instead
  of `patch_list`.
- **6 new passes**: constructs, errors, events, math, nextvar, strings,
  vararg (11 total, up from 5).
- **1 regression identified**: pm.lua now fails due to call depth
  limit (Bug #29, see below). Previously reported as passing but not
  verified with tarball-based test execution.

### Bugs Discovered

Bugs #15-#28 were discovered during the initial evaluation and Phase
9d. All have been fixed except #24-#25 (cosmetic).

| #  | Bug | Status | Affects |
|----|-----|--------|---------|
| 15 | code_not used PC as register | FIXED | constructs |
| 16 | EQ metamethod result position | FIXED | events |
| 17 | Parenthesized multi-return not truncated | FIXED | calls |
| 18 | While-true-if-break JMP target | FIXED | 8 timeouts |
| 19 | Repeat-until upvalue scoping | FIXED | closure |
| 20 | Coroutine register restoration | FIXED | literals |
| 21 | Return-from-C stale value | FIXED | vararg, calls |
| 22 | Loadstring error message format | FIXED | errors |
| 23 | Debug.getinfo namewhat | FIXED | db |
| 24 | Constant pool dedup gap | Open | cosmetic |
| 25 | Missing constant folding | Open | cosmetic |
| 26 | Raw byte error messages | FIXED | errors |
| 27 | Syntax nesting limits | FIXED | errors |
| 28 | Parser-level local/upvalue limits | FIXED | errors |

### Remaining Blockers

| Test | Blocker | Description |
|------|---------|-------------|
| attrib | File infra | Test writes `libs/B.lua` etc. to disk |
| calls | Streaming reader | `load()` reads entire input before parsing; PUC-Rio streams incrementally |
| closure | Per-thread globals | `setfenv` on coroutine threads not supported; rilua has a single global table |
| db | Hook stubs | `debug.sethook` line hook execution not implemented |
| literals | Locale parsing | Rust `f64::parse()` ignores C locale; needs libc `strtod` FFI |
| main | CLI subprocess | `require` path search fails for `/tmp/lua_*` temp files |
| pm | Call depth limit (#29) | `range(0,255)` recurses 256 levels; rilua's `call_depth` limit is 200 for all calls |
| verybig | Expression combos | Generated code triggers expression evaluation edge cases |

**Bug #29 (call depth model)**: rilua increments `call_depth` for
every call (Lua and Rust) and checks against `MAXCCALLS` (200).
PUC-Rio only increments `nCcalls` in `luaD_call` (C entry point);
Lua-to-Lua calls within `luaV_execute` use `goto reentry` and don't
increment the counter. This means PUC-Rio Lua functions can recurse
thousands of levels deep without hitting the 200-call limit. rilua
needs to separate Lua call depth from the Rust stack overflow guard.

## 3. Bytecode Comparison

### Instruction-Level Comparison (18 test cases)

| Category            | Match | Diff  | Notes |
|---------------------|-------|-------|-------|
| Simple programs     | 12    |       | Instructions identical |
| Constant folding    |       | 2     | rilua doesn't fold (`1+2*3`, `-1`) |
| Pointer addresses   |       | 3     | Expected (CLOSURE addresses differ) |
| Listing annotations |       | 1     | VARARG comment format |

Real instruction differences: 2 of 18 (constant folding only).
Cosmetic differences: 4 of 18 (addresses, annotations).

### Binary Chunk Compatibility

| Test | Result |
|------|--------|
| Simple program: byte-identical | PASS |
| Complex program: same size, cross-compatible | PASS |
| rilua chunk runs in PUC-Rio | PASS |
| PUC-Rio chunk runs in rilua | PASS |
| Debug info: minor line number differences | Expected |

Binary chunks are cross-compatible in both directions. Simple programs
produce byte-identical output. Complex programs differ only in debug
info (line number mappings).

## 4. Performance Benchmarks

Both interpreters built from tarballs with default optimization flags
(PUC-Rio: `-O2`, rilua: `cargo build --release`). Microbenchmarks
from February 12; test suite timings from February 13.

### Execution Time (best of 3 runs)

| Benchmark     | PUC-Rio (s) | rilua (s)  | Ratio  | Notes |
|---------------|-------------|------------|--------|-------|
| fib(30) x5    | 0.3906      | 0.7724     | 1.98x  | Pure recursion |
| table_ops     | 0.0319      | 0.0639     | 2.00x  | Insert + sum 10k x100 |
| string_ops    | 0.0027      | 0.0252     | 9.33x  | find + sub + rep x5000 |
| calls         | 0.0759      | 0.1657     | 2.18x  | 1M function calls |
| closures      | 0.0118      | 0.0139     | 1.18x  | 100k closure creation |
| patterns      | 0.0215      | 0.0258     | 1.20x  | find + gsub + match |
| **sort**      | **0.1662**  | **0.1527** | **0.92x** | 5k random sort x100 |
| coroutines    | 0.0093      | 0.0192     | 2.06x  | 100k resume/yield |
| gc_pressure   | 0.0377      | 0.0445     | 1.18x  | 100k alloc + collect |
| metatables    | 0.0574      | 0.0616     | 1.07x  | 100k __add + __index |

**Summary**: Most benchmarks run 1.1x-2.2x slower than PUC-Rio.
Table sort is faster (0.92x). String operations are the outlier at
9.33x slower, indicating the string library needs optimization.
Metatable and closure operations are close to parity (1.07x-1.18x).

### Compilation Speed

| File (10k lines) | PUC-Rio | rilua  | Ratio |
|-------------------|---------|--------|-------|
| large.lua         | 0.007s  | 0.075s | 10.7x |

Compilation is 10x slower than PUC-Rio for large files. The rilua
pipeline has an additional AST intermediate step (Luau-style) that
PUC-Rio doesn't have, which adds overhead. For typical program sizes
(<1000 lines), both compile in under 5ms.

### Memory Usage (Peak RSS)

| Benchmark    | PUC-Rio (kB) | rilua (kB) | Ratio |
|--------------|--------------|------------|-------|
| fib          | 3,036        | 3,484      | 1.15x |
| gc_pressure  | 3,068        | 3,656      | 1.19x |
| metatables   | 3,072        | 3,644      | 1.19x |
| sort         | 5,552        | 16,344     | 2.94x |
| table_ops    | 7,336        | 29,192     | 3.98x |

Baseline memory (fib, gc, metatables) is 15-19% higher than PUC-Rio.
For allocation-heavy workloads (sort, table_ops), memory usage is
3-4x higher, indicating the arena-based GC has higher per-object
overhead than PUC-Rio's `GCObject` union layout.

### Binary Size

| Binary    | Size      |
|-----------|-----------|
| PUC-Rio lua | 999 KB  |
| rilua       | 1.1 MB  |

### Startup Time

| Test (single invocation) | PUC-Rio | rilua |
|--------------------------|---------|-------|
| Empty program            | ~2 ms   | ~2 ms |

Startup is equivalent.

### PUC-Rio Test Suite Execution Time

Best of 3 runs per test file, release builds (`-O2` / `--release`).
Tests execute from within `./lua-5.1-tests/` via `scripts/compare.sh`.
Only the 11 currently passing test files are measured.

| Test         | PUC-Rio (s) | rilua (s) | Ratio  | Notes |
|--------------|-------------|-----------|--------|-------|
| constructs   | 0.227       | 0.521     | 2.29x  | Largest test, syntax + operator coverage |
| errors       | 0.119       | 0.005     | 0.04x  | rilua Result-based errors vs setjmp/longjmp |
| events       | 0.002       | 0.002     | --     | < 5ms both |
| files        | 0.009       | 0.011     | 1.22x  | I/O operations, temp files |
| gc           | 0.063       | 0.075     | 1.19x  | Allocation + collection cycles |
| locals       | 0.003       | 0.004     | --     | < 5ms both |
| math         | 0.003       | 0.005     | --     | < 5ms both |
| nextvar      | 0.012       | 0.022     | 1.83x  | Table iteration, next(), length |
| sort         | 0.047       | 0.086     | 1.82x  | table.sort with comparators |
| strings      | 0.002       | 0.002     | --     | < 5ms both |
| vararg       | 0.001       | 0.001     | --     | < 5ms both |

Tests under 5ms are too short for meaningful ratio comparison. Of the
tests with measurable runtime, `constructs` (2.29x), `nextvar` (1.83x),
and `sort` (1.82x) show where dispatch and table overhead matters.
`errors` is 25x faster in rilua because `Result<T, E>` propagation
uses normal control flow, while PUC-Rio's `pcall` saves the entire
register state via `setjmp` on every protected call.

## 5. Feature Coverage Summary

### Standard Library Completeness

| Library    | Functions | Status |
|------------|-----------|--------|
| base       | 29/29     | All implemented |
| string     | 14/14     | All implemented (gfind alias included) |
| table      | 9/9       | All implemented (deprecated functions included) |
| math       | 28/28     | All implemented (mod alias included) |
| io         | 18/18     | All implemented (11 lib + 7 methods) |
| os         | 11/11     | All implemented |
| debug      | 14/14     | All implemented (sethook/gethook are stubs) |
| package    | 9/9       | All implemented (C loaders stub "not supported") |
| coroutine  | 6/6       | All implemented |

### CLI Completeness

| Feature | Status |
|---------|--------|
| `-e stat` | Working |
| `-l name` | Working |
| `-i` interactive | Working |
| `-v` version | Working |
| `--` stop options | Working |
| `-` stdin | Working |
| LUA_INIT | Working |
| arg table | Working |
| REPL multiline | Working |
| `=expr` shorthand | Working |
| SIGINT handling | Stub |

### Bytecode Tools

| Feature | Status |
|---------|--------|
| riluac -l listing | Working |
| riluac -l -l full listing | Working |
| riluac -p parse-only | Working |
| riluac -o output file | Working |
| riluac -s strip debug | Working |
| string.dump | Working |
| Binary chunk loading | Working |
| Cross-compatibility | Working (both directions) |

## 6. Remaining Work

### Compatibility Fixes (by impact)

| Priority | Issue | Tests Unblocked |
|----------|-------|-----------------|
| P0 | Call depth model (Bug #29): separate Lua depth from Rust guard | pm |
| P1 | `load()` streaming reader | calls |
| P1 | Per-thread global tables (`setfenv` on threads) | closure |
| P1 | Locale-aware number parsing (libc `strtod` FFI) | literals |
| P2 | `debug.sethook` line hook execution | db |
| P2 | attrib.lua file creation infrastructure | attrib |
| P2 | main.lua CLI subprocess infrastructure | main |
| P3 | verybig.lua expression edge cases | verybig |

### Performance Targets

The goal is performance parity with PUC-Rio Lua 5.1.1 once all
features are implemented and rilua is behaviorally identical.

| Area | Current | Target | Approach |
|------|---------|--------|----------|
| String operations | 9.33x | <2x | Optimize string.find/rep/sub hot paths |
| Table operations | 2.00x | <1.5x | Reduce per-element GC overhead |
| Function calls | 2.18x | <1.5x | Optimize precall/postcall dispatch |
| Memory (table_ops) | 3.98x | <2x | Compact arena representation |
| Compilation | 10.7x | <3x | Optimize parser/AST allocation |
| Recursion | 1.98x | <1.5x | Optimize call/return dispatch |

Performance optimization is deferred until behavioral equivalence is
achieved. Premature optimization risks making bug fixes harder.
