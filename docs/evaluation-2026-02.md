# rilua Compatibility and Performance Evaluation

Date: February 12, 2026
Branch: `rewrite/v2`
rilua version: 0.1.0 (Phase 9d)
Reference: PUC-Rio Lua 5.1.1 (tag v5.1.1)
Platform: AMD Ryzen 7 8840U, Linux 6.18.8, Fedora 43

## Executive Summary

rilua passes all 1262 internal tests (583 unit + 402 integration + 277
oracle). Against the PUC-Rio official test suite, 5 of 20 applicable
tests pass. A single compiler bug (while-true-if-break, #18) causes
all 8 timeout failures and is the highest-impact fix opportunity.
Performance ranges from 0.92x to 2.18x vs PUC-Rio on most benchmarks,
with one outlier (string operations at 9.33x). Memory usage is 1.15x
to 3.98x higher. Binary sizes are comparable (1.0 MB vs 1.1 MB).

## 1. Internal Test Suite

```
Unit tests:        583 passed    (cargo test --lib)
Integration tests: 402 passed    (cargo test --test integration)
Oracle tests:      277 passed    (cargo test --test oracle)
Total:            1262 passed, 0 failed, 0 skipped
```

All tests pass with `cargo nextest run --profile ci` (single-threaded,
deterministic) in 7.0 seconds.

## 2. PUC-Rio Official Test Suite

### Results Matrix

| Test File    | rilua    | PUC-Rio  | Blocker(s)              |
|--------------|----------|----------|-------------------------|
| api          | N/A      | PASS     | Requires testC library  |
| attrib       | FAIL     | FAIL(*)  | File creation infra     |
| big          | FAIL     | FAIL     | Requires checktable (C) |
| calls        | TIMEOUT  | PASS     | Bug #17, #18, #21       |
| checktable   | N/A      | PASS     | Requires testC library  |
| closure      | TIMEOUT  | PASS     | Bug #18, #19            |
| code         | N/A      | PASS     | Requires testC library  |
| constructs   | TIMEOUT  | PASS     | Bug #18                 |
| db           | FAIL     | PASS     | Bug #23                 |
| errors       | FAIL     | PASS     | Bug #22                 |
| events       | TIMEOUT  | PASS     | Bug #18                 |
| **files**    | **PASS** | PASS     |                         |
| **gc**       | **PASS** | PASS     |                         |
| literals     | FAIL     | PASS     | Bug #20                 |
| **locals**   | **PASS** | PASS     |                         |
| main         | FAIL     | FAIL(*)  | CLI subprocess infra    |
| math         | TIMEOUT  | FAIL(*)  | Bug #18                 |
| nextvar      | TIMEOUT  | PASS     | Bug #18                 |
| **pm**       | **PASS** | PASS     |                         |
| **sort**     | **PASS** | PASS     |                         |
| strings      | TIMEOUT  | PASS     | Bug #18                 |
| vararg       | FAIL     | PASS     | Bug #21                 |
| verybig      | TIMEOUT  | FAIL(*)  | Bug #18                 |

(*) PUC-Rio also fails these: attrib/main need writable dirs, big needs
checktable, math needs non-standard printf, verybig is optional.

### Summary

| Result    | Count | Percentage |
|-----------|-------|------------|
| Pass      | 5     | 25%        |
| N/A       | 3     | (excluded) |
| Timeout   | 8     | 40%        |
| Fail      | 7     | 35%        |

### Bugs Discovered (9 new)

| # | Bug | Impact | Affects |
|---|-----|--------|---------|
| 17 | Parenthesized multi-return not truncated | `(f())` keeps multiple returns | calls, vararg |
| 18 | While-true-if-break compiler JMP target | Infinite loop in `while true do ... if ... break` | 8 timeout tests |
| 19 | Repeat-until upvalue scoping | Closures share upvalue across iterations | closure |
| 20 | Coroutine register restoration | Upvalue corruption after yield/resume | literals |
| 21 | Return-from-C stale value | `return f(...)` leaks stale register when f returns 0 | vararg, calls |
| 22 | Loadstring error message format | Source name and error text format mismatch | errors |
| 23 | Debug.getinfo namewhat | Returns "global" instead of "local" for local functions | db |
| 24 | Constant pool dedup gap | ~5% more constants than PUC-Rio (264 vs 252) | cosmetic |
| 25 | Missing constant folding | No compile-time arithmetic folding | cosmetic |

### Priority Fix Order

Fixing bugs in this order maximizes test progress:

1. **Bug #18** (while-true-if-break): Unblocks all 8 timeout tests.
   Root cause: jump backpatching for false branch of comparison in
   constant-true while loops emits self-referencing JMP instead of
   loop-top JMP. Expected fix: ~20 lines in `codegen.rs`.

2. **Bug #17** (parenthesized multi-return): Needed for calls.lua.
   Root cause: `(expr)` in the parser doesn't set a "truncate to 1"
   flag on call expressions.

3. **Bug #19** (repeat-until upvalue scoping): Needed for closure.lua.
   Root cause: locals in repeat body aren't closed/reopened per
   iteration.

4. **Bug #21** (return-from-C stale value): Needed for vararg.lua.
   Root cause: TAILCALL or RETURN for C functions doesn't clear the
   result area when nresults=0.

5. **Bug #20** (coroutine register restoration): Needed for
   literals.lua. Root cause: stack base offset error in resume path.

6. **Bug #22** (loadstring error format): Needed for errors.lua.
   Two sub-issues: source name format and error message text.

7. **Bug #23** (debug.getinfo namewhat): Needed for db.lua. The
   getfuncname resolution doesn't check for local function assignments.

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

Compilation is 10x slower than PUC-Rio for large files. The Lua
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

## 6. Roadmap for Phase 9d/9e Completion

### Phase 9d: Remaining Bug Fixes

Priority order (by impact on PUC-Rio test pass count):

| Priority | Bug | Expected Effort | Tests Unblocked |
|----------|-----|-----------------|-----------------|
| P0 | #18 while-true-if-break | Small (codegen.rs JMP patching) | 8 timeouts |
| P1 | #17 parenthesized multi-return | Small (parser/codegen) | calls |
| P1 | #19 repeat-until upvalue scoping | Medium (codegen.rs upvalue close) | closure |
| P1 | #21 return-from-C stale value | Small (execute.rs return cleanup) | vararg |
| P2 | #20 coroutine register restoration | Medium (state.rs resume path) | literals |
| P2 | #22 loadstring error format | Small (compiler error formatting) | errors |
| P2 | #23 debug.getinfo namewhat | Small (debug_info.rs) | db |

Estimated total: ~7 focused bug-fix sessions.

### Phase 9e: Behavioral Equivalence

After Phase 9d bug fixes:
- Re-run all 20 PUC-Rio tests, expect 15+ to pass
- Fix remaining edge cases iteratively
- Address attrib.lua (file infra) and main.lua (subprocess infra)
- Consider testC equivalent for api/checktable/code coverage

### Performance Optimization Opportunities

| Area | Current | Target | Approach |
|------|---------|--------|----------|
| String operations | 9.33x | <3x | Optimize string.find/rep/sub hot paths |
| Table operations | 2.00x | <1.5x | Reduce per-element GC overhead |
| Function calls | 2.18x | <1.5x | Optimize precall/postcall dispatch |
| Memory (table_ops) | 3.98x | <2x | Compact arena representation |
| Compilation | 10.7x | <3x | Optimize parser/AST allocation |
