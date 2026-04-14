# Performance

Performance characteristics, benchmarks against PUC-Rio Lua 5.1.1, and
optimization history.

## Goal: PUC-Rio Parity

The target is matching PUC-Rio Lua 5.1.1 (compiled with `-O2`) on the
official test suite. PUC-Rio Lua is written in C and represents the
performance floor for a Lua 5.1 implementation.

### Benchmark Environment

| Property | Value |
|----------|-------|
| CPU | AMD Ryzen AI 9 HX PRO 370 w/ Radeon 890M |
| OS | Arch Linux (kernel 6.19.11-arch1-1) |
| Rust | `rustc 1.94.1`, edition 2024, `--release` profile |
| PUC-Rio | Lua 5.1.1, compiled with `gcc -O2 -DLUA_USE_LINUX` |
| Runs | 10 per standalone test, 5 for `all.lua`, Criterion default 100 samples |
| Date | 2026-04-14 |

### Per-Test Results (ms, median of 10 runs)

Tests from the [PUC-Rio test suite](https://lua.org/tests/) run
individually. `main.lua` and `big.lua` are excluded: `main.lua` tests
CLI features via `os.execute` (environment-dependent), and `big.lua`
requires a coroutine wrapper set by `all.lua`.

| Test | PUC-Rio | rilua | Ratio |
|------|--------:|------:|------:|
| gc.lua | 68 | 79 | 1.16x |
| db.lua | 22 | 32 | 1.45x |
| calls.lua | 6 | 12 | 2.00x |
| strings.lua | 2 | 2 | 1.00x |
| literals.lua | 2 | 2 | 1.00x |
| attrib.lua | 3 | 4 | 1.33x |
| locals.lua | 4 | 5 | 1.25x |
| constructs.lua | 262 | 592 | 2.26x |
| code.lua | 2 | 2 | 1.00x |
| nextvar.lua | 16 | 33 | 2.06x |
| pm.lua | 13 | 13 | 1.00x |
| api.lua | 2 | 3 | 1.50x |
| events.lua | 2 | 3 | 1.50x |
| vararg.lua | 1 | 1 | 1.00x |
| closure.lua | 4 | 8 | 2.00x |
| errors.lua | 121 | 188 | 1.55x |
| math.lua | 4 | 5 | 1.25x |
| sort.lua | 65 | 105 | 1.62x |
| verybig.lua | 114 | 176 | 1.54x |
| files.lua | 7 | 7 | 1.00x |
| **Sum** | **720** | **1272** | **1.77x** |

### Interpretation

rilua is 1.77x slower than PUC-Rio Lua overall on the current 2026-04-14
snapshot. Most tests remain within 1.0-1.6x. The largest current gaps are:

- **constructs.lua** (2.26x, +330ms): heavy control-flow constructs,
  deeply nested loops and conditionals. This test stresses the VM
  dispatch loop.
- **nextvar.lua** (2.06x, +17ms): table iteration (`next`, `pairs`),
  global table manipulation. Stresses table hash traversal.
- **calls.lua** and **closure.lua** (2.00x each): function/call-frame
  overhead is still visible in the call-heavy parts of the suite.
- **sort.lua** (1.62x, +40ms): `table.sort` with comparison callbacks.
  Function call overhead per comparison.
- **errors.lua** (1.55x, +67ms) and **verybig.lua** (1.54x, +62ms):
  error-path formatting/traceback work and large compile/execute
  workloads are still materially slower than PUC-Rio.

Tests at or near parity (1.0-1.16x): `strings.lua`, `literals.lua`,
`code.lua`, `pm.lua`, `vararg.lua`, `files.lua`, and `gc.lua`.

The exact absolute times are not directly comparable to the 2026-02-23
snapshot because the machine and OS changed. The ratio against PUC-Rio
is the durable signal.

### Full-Suite Runner

The repo's regression-gate harness uses the official `all.lua` runner:

| Runner | rilua |
|--------|------:|
| `all.lua` median of 5 runs | 1959 ms |
| Min / Max | 1892 ms / 2302 ms |

This value comes from `./scripts/bench-puc-rio.sh target/release/rilua 5`
and is the current baseline used by the wall-clock regression workflow.

The current `lua5.1-tests` corpus downloaded from `lua.org/tests/` does not
ship a `bench-all.lua` helper, so the 2026-04-14 refresh uses the official
`all.lua` runner instead of the older combined-runner file.

### Reproducing

Build both interpreters and run the benchmark script:

```sh
# Build PUC-Rio Lua 5.1.1
cd lua-5.1.1 && make linux && cd ..

# Build test helper shared libraries for the complete suite
cd lua-5.1-tests/libs
gcc -Wall -O2 -I../../lua-5.1.1/src -ansi -shared -o lib1.so lib1.c
gcc -Wall -O2 -I../../lua-5.1.1/src -ansi -shared -o lib11.so lib11.c
gcc -Wall -O2 -I../../lua-5.1.1/src -ansi -shared -o lib2.so lib2.c
gcc -Wall -O2 -I../../lua-5.1.1/src -ansi -shared -o lib21.so lib21.c
cp lib2.so ./-lib2.so
cd ../..

# Build rilua
cargo build --release

# Run standalone per-file benchmarks (default: 10 runs per test)
./scripts/benchmark-tests.sh [runs]

# Run the full all.lua wall-clock benchmark (default: 5 runs)
./scripts/bench-puc-rio.sh [binary] [runs]
```

## Optimization History

Starting from ~15.4s on the full suite, four optimization phases
reduced runtime to ~2.6s (83% total reduction).

### Phase 1: Lexer and Parser (~7% improvement)

- Keyword lookup: `match` dispatch replacing binary search on sorted
  array
- Parser advance: `mem::replace` replacing `Token::clone`
- Lexer: fast-path byte-slice scanning for common characters
- GC traverse: zero-allocation indexed access for tables and closures

### Phase 2: Constant Pool (~68% reduction)

- Hash-based constant pool deduplication replacing O(n) linear scan
- Mirrors PUC-Rio's `addk` approach using `luaH_set` on `fs->h`
- `ConstantKey` enum: `Num(u64)` / `Bool(bool)` / `Str(Vec<u8>)`
- 15.4s -> 4.9s

### Phase 3: GC and VM Inlining (~12% reduction)

- `#[inline]` on hot GC arena and collector methods
- `sweep_partial`: direct assignment replacing `mem::replace` on dead
  path
- `GCSWEEPMAX`: 40 -> 80 to amortize dispatch overhead
- `traverse_thread`: indexed access replacing `Vec` clone allocation
- `CallInfo.is_lua` cache: eliminates arena lookups in traceback
- 4.9s -> 4.3s

### Phase 4: SoA Sweep Layout (~46% reduction)

- Parallel `Vec<u8>` color array (Structure-of-Arrays layout)
- Sweep reads 1 byte per slot instead of loading full `Entry<T>` (~72
  bytes for tables)
- Iterator-based sweep: eliminates per-access bounds checks
- 4.9s -> 2.6s (10-run median)

## Profiling

### Requirements

- Linux with `perf` installed (`linux-tools-common` or equivalent)
- [`cargo-flamegraph`](https://github.com/flamegraph-rs/flamegraph):
  `cargo install flamegraph`

### Generating Flamegraphs

Build with debug symbols in release mode (already configured in
`Cargo.toml` via `[profile.release] debug = true` if needed):

```sh
# Profile a specific test file
cargo flamegraph -- -e "dofile('lua-5.1-tests/constructs.lua')"

# Profile the full test suite
cd lua-5.1-tests
RILUA_TEST_LIB=1 cargo flamegraph -- all.lua
```

Flamegraph SVGs are interactive. Open them in a browser to click-zoom
into specific call stacks and search for function names.

Generated flamegraphs go in `flamegraphs/` (gitignored).

### Using `perf` Directly

```sh
cargo build --release
perf record -g --call-graph dwarf target/release/rilua lua-5.1-tests/constructs.lua
perf report
```

### Current Perf Snapshot (2026-04-14)

Before changing runtime code, the current hot cases were profiled with
`perf` against an unstripped release build:

```sh
CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --release
```

Perf data was captured into `target/perf/*.sym.data` for:
`constructs.lua`, `nextvar.lua`, `sort.lua`, `db.lua`, and `verybig.lua`.

#### `constructs.lua`

The profile is split between runtime dispatch and front-end work. The
largest named entries were:

- `rilua::vm::execute::execute`
- `rilua::compiler::codegen::compile_expr`
- `rilua::compiler::parser::Parser::parse_suffixed_expr`
- `rilua::compiler::lexer::Lexer::scan`
- `rilua::compiler::parser::Parser::advance`
- `rilua::vm::string::StringTable::intern`
- `alloc::raw_vec::RawVecInner<A>::finish_grow`

Read: the official `constructs.lua` timing is not a pure VM-dispatch
benchmark. It still spends meaningful time in lex/parse/codegen and
vector growth on the compile path.

Candidate fixes from this stack:

- Keep VM-dispatch work aimed at `execute()`, but do not expect it alone
  to close the full `constructs.lua` gap.
- Pre-size parser/codegen vectors used by nested block and expression
  construction to reduce `RawVec` growth churn.
- Reduce compile-path string/hash traffic in large nested sources before
  doing deeper readability refactors in the compiler.

#### `nextvar.lua`

Top entries were much more specific:

- `rilua::vm::execute::execute` (`17.58%`)
- `rilua::vm::gc::collector::<impl rilua::vm::state::Gc>::traverse_table` (`7.94%`)
- `rilua::vm::gc::collector::<impl rilua::vm::state::LuaState>::gc_singlestep` (`7.87%`)
- `core::num::flt2dec::strategy::dragon::format_exact` (`6.36%` and `3.61%`)
- `rilua::vm::gc::collector::<impl rilua::vm::state::Gc>::mark_value` (`5.46%`)
- `rilua::vm::table::Table::get` (`4.94%`)
- `rilua::vm::execute::<impl rilua::vm::state::LuaState>::precall` (`5.51%`)
- `rilua::vm::execute::<impl rilua::vm::state::LuaState>::poscall` (`4.56%`)

Read: table iteration is paying for GC traversal and mark work in the
middle of the test, and number formatting is also visible in the hot
path instead of being background noise.

Candidate fixes from this stack:

- Audit GC debt/step pacing around iteration-heavy workloads before
  changing table layout again; this profile says collector work is part
  of the `nextvar.lua` gap right now.
- Investigate where numeric formatting enters this path and remove
  redundant `tostring`/formatting work if it is avoidable.
- Only after that, revisit `Table::get` / iteration helpers.

#### `sort.lua`

The hot stack is dominated by comparator call overhead:

- `rilua::vm::execute::execute` (`33.86%`)
- `rilua::stdlib::table::sort_comp` (`9.09%`)
- `rilua::vm::execute::<impl rilua::vm::state::LuaState>::precall` (`8.67%`)
- `rilua::vm::execute::<impl rilua::vm::state::LuaState>::poscall` (`4.52%`)
- `rilua::vm::execute::vm_gettable` (`4.16%`)
- `rilua::vm::table::Table::get` (`4.05%`)
- `rilua::stdlib::table::auxsort` (`2.46%`)
- `rilua::vm::state::LuaState::push_ci` (`1.95%`)

Read: `sort.lua` is mostly a Lua callback trampoline benchmark. The
sort algorithm itself (`auxsort`) is visible, but the heavier cost is
`sort_comp -> call_function -> precall/execute/poscall`.

Candidate fixes from this stack:

- Optimize the Lua-to-Lua comparator path first: cheaper `precall`,
  `push_ci`, and `poscall` matter more here than changing quicksort.
- Reduce repeated table access and swap overhead inside `auxsort` only
  after the call-frame path is cheaper.
- Keep the default-comparator fast path separate from callback work so
  future optimizations do not penalize the common no-callback case.

#### `db.lua`

This case is dominated by debug table construction:

- `rilua::vm::table::Table::raw_set_impl` (`20.54%`)
- `rilua::vm::table::Table::new_key` (`9.68%`)
- `rilua::vm::table::Table::rehash` / `resize` under `raw_set_impl`
- `rilua::stdlib::debug::set_table_str` on the `new_key` path
- `rilua::vm::execute::execute` (`7.91%`)

Read: the debug library is spending a large share of time allocating and
rehashing small result tables rather than in stack inspection itself.

Candidate fixes from this stack:

- Pre-size the tables returned by `debug.getinfo`, `debug.gethook`, and
  similar helpers so repeated `raw_set_impl -> rehash -> resize` work
  disappears.
- Avoid repeated `set_table_str` insertion churn when the result shape is
  fixed and known up front.
- Treat debug metadata lookup itself as secondary until table-building
  overhead is reduced.

#### `verybig.lua`

This case mixes execution, hashing, and front-end work:

- `rilua::vm::execute::execute` (`16.34%`)
- `<std::hash::random::DefaultHasher as core::hash::Hasher>::write` (`5.00%`)
- `core::hash::BuildHasher::hash_one` (`4.31%`)
- `rilua::vm::table::Table::get` (`4.97%`)
- `rilua::vm::string::StringTable::intern` (`3.54%`)
- `rilua::vm::execute::vm_gettable` (`3.21%`)
- `rilua::vm::table::Table::raw_set_impl` (`2.74%`)
- `rilua::compiler::lexer::Lexer::scan` (`1.80%`)
- `rilua::compiler::codegen::compile_expr` (`1.70%`)
- `rilua::compiler::codegen::compile_table_ctor` (`1.30%`)

Read: large-source pressure is not just dispatch. Hashing, interning,
and compiler work are materially present in the hot path.

Candidate fixes from this stack:

- Reduce hashing and interning pressure in the compiler/string table
  path before attempting another broad VM rewrite.
- Pre-size compiler-owned tables/vectors for large constructors and
  large source files.
- Revisit `Table::get` / `vm_gettable` after compile-path churn is
  lower, because both compile and execute work are contributing here.

### Prioritized Runtime Work From Actual Stacks

If the next optimization pass is limited to one hotspot at a time, the
profiles point to this order:

1. `sort.lua`: comparator call-frame overhead (`sort_comp`, `precall`,
   `poscall`, `push_ci`)
2. `db.lua`: debug result-table construction and rehashing
3. `nextvar.lua`: GC pacing during dense iteration, then unexpected
   number-formatting work
4. `verybig.lua`: hashing/interning plus compiler allocation churn
5. `constructs.lua`: mixed compile + dispatch work, not dispatch alone

## Benchmarks

### Criterion Microbenchmarks

`benches/interpreter.rs` contains criterion benchmarks covering:

- **State creation**: empty, base libs, full stdlib
- **Compilation**: minimal, loops, functions, tables, large compile workloads
- **VM execution**: arithmetic loops, fibonacci, string concat, tables,
  closures, metatable dispatch, control-flow dispatch, large execute workloads
- **Debug API**: `getinfo`, locals/upvalues, traceback-heavy metadata access
- **GC**: full collect, allocation churn, incremental stepping
- **String interning**: unique strings, dedup hits
- **Table operations**: integer keys, string keys, mixed Lua ops, `next`/`pairs`,
  sort callback overhead
- **End-to-end**: compile+run, coroutine cycles

Run with:

```sh
cargo bench
```

Results go to `target/criterion/`. Use `--save-baseline` and
`--baseline` flags to compare across changes. For a smaller stable
subset, use `./scripts/perf-regression.sh smoke` and
`./scripts/perf-regression.sh refresh-criterion-baseline`.

### Current Criterion Snapshot (2026-04-14)

Point estimates below are the middle values from Criterion's reported time
intervals (`cargo bench --bench interpreter -- --noplot`):

| Benchmark | Estimate |
|-----------|---------:|
| `state_creation/new_empty` | 888.81 ns |
| `state_creation/new_with_base` | 8.4429 us |
| `state_creation/new_full` | 44.075 us |
| `compilation/compile_minimal` | 444.12 ns |
| `compilation/compile_loop` | 2.4741 us |
| `compilation/compile_functions` | 7.2293 us |
| `compilation/compile_tables` | 6.8442 us |
| `vm_execution/loop_sum_1k` | 13.690 us |
| `vm_execution/fib_20` | 1.3495 ms |
| `vm_execution/string_concat_100` | 9.8796 us |
| `vm_execution/table_build_1k` | 53.682 us |
| `vm_execution/closures_100` | 23.634 us |
| `vm_execution/metatable_index_1k` | 67.211 us |
| `gc/collect_10k_tables` | 250.47 us |
| `gc/churn_alloc_collect` | 556.51 us |
| `gc/step_incremental` | 253.70 ns |
| `string_interning/intern_unique_1k` | 60.987 us |
| `string_interning/intern_dedup_1k` | 1.2659 us |
| `table_ops/raw_set_int_1k` | 8.3288 us |
| `table_ops/raw_set_str_1k` | 50.445 us |
| `table_ops/mixed_ops_lua` | 680.32 us |
| `end_to_end/compile_and_run` | 75.966 us |
| `end_to_end/coroutine_cycle` | 28.718 us |

The slowest current Criterion points are still the same categories the
PUC-Rio suite highlights: recursive call overhead (`fib_20`), mixed Lua table
workloads, GC churn, metatable dispatch, and string-heavy bulk operations.

### PUC-Rio Full Suite Benchmark

The primary wall-clock benchmark:

```sh
cargo build --release
./scripts/bench-puc-rio.sh [binary] [runs]
```

Arguments:
- `binary`: path to rilua binary (default: `target/release/rilua`)
- `runs`: number of runs (default: 5)

Output: min, median, and max times. Prints median to stdout.

## Regression Workflow

The repo now has a single entrypoint for routine perf checks:

```sh
./scripts/perf-regression.sh [smoke|gate|all|refresh-criterion-baseline|show-config]
```

### Stable Smoke Subset

`./scripts/perf-regression.sh smoke` runs two fast checks:

1. A small official-test smoke subset through `scripts/benchmark-tests.sh`:
   `constructs.lua`, `nextvar.lua`, `sort.lua`, `db.lua`, `verybig.lua`
2. A matching Criterion smoke subset against a saved baseline:
   - `control_flow_dispatch`
   - `verybig_loaded_chunk`
   - `next_pairs_mixed_1k`
   - `sort_callback_1k`

The PUC-Rio smoke subset is trend-only output. The explicit gate is the
Criterion comparison, which fails if any smoke benchmark regresses by
more than `20%` against the saved baseline (`perf-smoke` by default).
`db.lua` keeps the debug-library hotspot in the smoke workflow; the
Criterion smoke list stays intentionally narrower so the gate remains
stable on repeat local runs. `verybig.lua` keeps the large compile/execute
path in the official smoke subset, while Criterion smoke focuses on the
lower-noise VM and table hot paths.

### Full Gate

`./scripts/perf-regression.sh gate` runs the full `all.lua` wall-clock
gate against `.perf-baseline`.

- Baseline source: `.perf-baseline`
- Default runs: `5`
- Default threshold: `5%`
- Pass condition: current median `<= baseline + 5%`

This is the strict regression guard for the full suite. Keep
`.perf-baseline` conservative and only refresh it after an accepted,
measured improvement.

### Refreshing Baselines

Refresh the local Criterion smoke baseline after a confirmed improvement:

```sh
./scripts/perf-regression.sh refresh-criterion-baseline
```

This saves the smoke baseline under `target/criterion/**/perf-smoke/`
(gitignored). The compare path uses `cargo bench --baseline perf-smoke`
and reads Criterion's `base/new` estimate files to enforce the numeric
threshold.

After a confirmed full-suite improvement, refresh `.perf-baseline`:

```sh
./scripts/bench-puc-rio.sh > .perf-baseline
```

### Legacy Full-Suite Helper

`scripts/perf-gate.sh` still exists as a small standalone wall-clock
check, but `scripts/perf-regression.sh` is the preferred routine
workflow because it combines the full-suite gate with the smoke subset
and Criterion baseline comparison.

## Optimization Priorities

Based on the per-test benchmarks, these areas offer the largest
potential gains, ordered by impact:

### 1. VM Dispatch (constructs.lua: 2.31x, +331ms)

`constructs.lua` is the heaviest test and the largest absolute gap.
It exercises the main `execute()` loop with deeply nested control flow.

- **Instruction dispatch**: the `match`-based dispatch in `execute()`
  is the hot path. Layout optimization, opcode reordering to improve
  branch prediction, and reducing per-instruction overhead would have
  the highest impact.
- **FORPREP/FORLOOP specialization**: integer-only fast path for
  numeric `for` loops when bounds are integers.

### 2. Table Operations (nextvar.lua: 2.15x, sort.lua: 1.78x)

- **Hash traversal**: `next()` and `pairs()` iteration speed.
  `nextvar.lua` hammers these.
- **Comparison callback overhead**: `sort.lua` calls a Lua comparison
  function per element pair. Reducing function call setup/teardown cost
  would help.

### 3. Compilation (verybig.lua: 1.89x, +102ms)

- **AST allocation**: heap-allocated AST nodes dropped after
  compilation. A pool or arena built from `Vec`-based storage could
  reduce allocation pressure.
- **Constant folding**: limited constant folding during compilation
  could reduce VM work for arithmetic-heavy code.

### 4. GC Under Sustained Load (bench-all.lua: 1.93x)

The combined runner is 10% slower relative to PUC-Rio than the sum of
individual tests (1.93x vs 1.75x). This indicates GC overhead grows
disproportionately with accumulated state. Incremental GC tuning and
sweep efficiency under high object counts are the targets here.

### 5. Lower-Priority Opportunities

- **String concatenation**: batching consecutive `CONCAT` operations
  to reduce intermediate allocations.
- **Generational GC**: nursery for young objects, tenured for
  survivors. Would reduce per-cycle work for allocation-heavy programs.
- **Hash function**: alternative hash functions could reduce collision
  rates for specific workloads.
