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
| CPU | AMD Ryzen 7 8840U w/ Radeon 780M Graphics |
| OS | Fedora Linux 43 (kernel 6.18) |
| Rust | Edition 2024, `--release` profile |
| PUC-Rio | Lua 5.1.1, compiled with `gcc -O2 -DLUA_USE_LINUX` |
| Runs | 10 per test, median reported |
| Date | 2026-02-23 |

### Per-Test Results (ms, median of 10 runs)

Tests from the [PUC-Rio test suite](https://lua.org/tests/) run
individually. `main.lua` and `big.lua` are excluded: `main.lua` tests
CLI features via `os.execute` (environment-dependent), and `big.lua`
requires a coroutine wrapper set by `all.lua`.

| Test | PUC-Rio | rilua | Ratio |
|------|--------:|------:|------:|
| gc.lua | 72 | 86 | 1.19x |
| db.lua | 17 | 30 | 1.76x |
| calls.lua | 7 | 9 | 1.29x |
| strings.lua | 2 | 3 | 1.50x |
| literals.lua | 3 | 3 | 1.00x |
| attrib.lua | 4 | 5 | 1.25x |
| locals.lua | 5 | 7 | 1.40x |
| constructs.lua | 251 | 601 | 2.39x |
| code.lua | 2 | 2 | 1.00x |
| nextvar.lua | 13 | 31 | 2.38x |
| pm.lua | 10 | 11 | 1.10x |
| api.lua | 3 | 3 | 1.00x |
| events.lua | 2 | 3 | 1.50x |
| vararg.lua | 2 | 2 | 1.00x |
| closure.lua | 5 | 8 | 1.60x |
| errors.lua | 139 | 144 | 1.04x |
| math.lua | 5 | 6 | 1.20x |
| sort.lua | 51 | 93 | 1.82x |
| verybig.lua | 124 | 225 | 1.81x |
| files.lua | 12 | 13 | 1.08x |
| **Sum** | **729** | **1285** | **1.76x** |

### Interpretation

rilua is 1.76x slower than PUC-Rio Lua overall. Most tests are within
1.0-1.5x. Three tests account for the majority of the gap:

- **constructs.lua** (2.39x, +350ms): heavy control-flow constructs,
  deeply nested loops and conditionals. This test stresses the VM
  dispatch loop.
- **nextvar.lua** (2.38x, +18ms): table iteration (`next`, `pairs`),
  global table manipulation. Stresses table hash traversal.
- **sort.lua** (1.82x, +42ms): `table.sort` with comparison callbacks.
  Function call overhead per comparison.
- **verybig.lua** (1.81x, +101ms): large function compilation and
  execution with many locals and upvalues.

Tests at or near parity (1.0-1.1x): `literals.lua`, `code.lua`,
`api.lua`, `vararg.lua`, `errors.lua`, `pm.lua`, `files.lua`.

### Combined Runner

`bench-all.lua` runs all 20 standalone tests sequentially in a single
interpreter session (like `all.lua` but without `main.lua`/`big.lua`
and without the dump/undump `dofile` override).

| Runner | PUC-Rio | rilua | Ratio |
|--------|--------:|------:|------:|
| bench-all.lua | 811 | N/A* | - |

\* rilua fails `bench-all.lua` due to a GC bug: after running
`constructs.lua` + `nextvar.lua`, a subsequent GC cycle during
`pm.lua` incorrectly collects the global `assert` function. Each test
passes individually; the bug only manifests under accumulated GC
pressure across multiple `dofile` calls. This is a known regression.

### Reproducing

Build both interpreters and run the benchmark script:

```sh
# Build PUC-Rio Lua 5.1.1
cd lua-5.1.1 && make linux && cd ..

# Build rilua
cargo build --release

# Run benchmarks (default: 10 runs per test)
./scripts/benchmark-tests.sh [runs]
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

## Benchmarks

### Criterion Microbenchmarks

`benches/interpreter.rs` contains criterion benchmarks covering:

- **State creation**: empty, base libs, full stdlib
- **Compilation**: minimal, loops, functions, tables
- **VM execution**: arithmetic loops, fibonacci, string concat, tables,
  closures, metatable dispatch
- **GC**: full collect, allocation churn, incremental stepping
- **String interning**: unique strings, dedup hits
- **Table operations**: integer keys, string keys, mixed Lua ops
- **End-to-end**: compile+run, coroutine cycles

Run with:

```sh
cargo bench
```

Results go to `target/criterion/`. Use `--save-baseline` and
`--baseline` flags to compare across changes.

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

## Regression Gate

`scripts/perf-gate.sh` compares the current build against the stored
baseline with a configurable threshold (default 5%).

```sh
./scripts/perf-gate.sh [baseline_ms] [threshold_pct]
```

If no arguments are given, reads `.perf-baseline` and uses 5%.

The script:
1. Builds release
2. Runs `bench-puc-rio.sh` with 5 iterations
3. Compares median against `baseline + baseline * threshold / 100`
4. Exits 0 (pass) or 1 (regression detected)

After a confirmed improvement, update the baseline:

```sh
./scripts/bench-puc-rio.sh > .perf-baseline
```

## Optimization Priorities

Based on the per-test benchmarks, these areas offer the largest
potential gains, ordered by impact:

### 1. VM Dispatch (constructs.lua: 2.39x, +350ms)

`constructs.lua` is the heaviest test and the largest absolute gap.
It exercises the main `execute()` loop with deeply nested control flow.

- **Instruction dispatch**: the `match`-based dispatch in `execute()`
  is the hot path. Layout optimization, opcode reordering to improve
  branch prediction, and reducing per-instruction overhead would have
  the highest impact.
- **FORPREP/FORLOOP specialization**: integer-only fast path for
  numeric `for` loops when bounds are integers.

### 2. Table Operations (nextvar.lua: 2.38x, sort.lua: 1.82x)

- **Hash traversal**: `next()` and `pairs()` iteration speed.
  `nextvar.lua` hammers these.
- **Comparison callback overhead**: `sort.lua` calls a Lua comparison
  function per element pair. Reducing function call setup/teardown cost
  would help.

### 3. Compilation (verybig.lua: 1.81x, +101ms)

- **AST allocation**: heap-allocated AST nodes dropped after
  compilation. A pool or arena built from `Vec`-based storage could
  reduce allocation pressure.
- **Constant folding**: limited constant folding during compilation
  could reduce VM work for arithmetic-heavy code.

### 4. GC Correctness (bench-all.lua: fails)

Before further optimization, the GC bug that causes global collection
under accumulated pressure must be fixed. This blocks the combined
`bench-all.lua` runner.

### 5. Lower-Priority Opportunities

- **String concatenation**: batching consecutive `CONCAT` operations
  to reduce intermediate allocations.
- **Generational GC**: nursery for young objects, tenured for
  survivors. Would reduce per-cycle work for allocation-heavy programs.
- **Hash function**: alternative hash functions could reduce collision
  rates for specific workloads.
