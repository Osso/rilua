# Performance

Performance characteristics, profiling workflow, and optimization history
for rilua.

## Current Baseline

The primary benchmark is the full PUC-Rio Lua 5.1.1 test suite
(`lua-5.1-tests/all.lua`), run via `scripts/bench-puc-rio.sh`.

| Metric          | Value     |
|-----------------|-----------|
| Median time     | ~2630 ms  |
| Runs per sample | 5         |
| Build           | `--release` |
| Test suite      | 23/23 PUC-Rio tests via `all.lua` |

The baseline is stored in `.perf-baseline` as a single integer
(milliseconds). Update it after confirmed improvements:

```sh
cargo build --release
./scripts/bench-puc-rio.sh > .perf-baseline
```

## Optimization History

Starting from ~15.4s on the full suite, four optimization phases reduced
runtime to ~2.6s (83% total reduction).

### Phase 1: Lexer and Parser (~7% improvement)

- Keyword lookup: `match` dispatch replacing binary search on sorted array
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
- `sweep_partial`: direct assignment replacing `mem::replace` on dead path
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

## Comparing Against PUC-Rio Lua

`scripts/compare.sh` runs each test file through both PUC-Rio Lua and
rilua, producing a markdown compatibility table:

```sh
# Build PUC-Rio Lua first (see lua-5.1.1/README)
scripts/compare.sh lua-5.1.1/src/lua target/release/rilua
```

Output: per-test PASS/FAIL/TIMEOUT comparison with match column.

## Remaining Optimization Opportunities

These are areas where further gains are possible within the project's
constraints (zero external dependencies, zero unsafe).

### Compiler

- **AST allocation**: AST nodes are heap-allocated and dropped after
  compilation. A pool or arena built from `Vec`-based storage could
  reduce allocation pressure without external crates.
- **Constant folding**: limited constant folding during compilation
  could reduce VM work for arithmetic-heavy code.

### VM

- **Instruction dispatch**: the main `execute()` loop uses `match`.
  Computed-goto equivalents are not available in safe Rust, but layout
  and branch prediction hints (`likely`/`unlikely` when stabilized)
  could help.
- **FORPREP/FORLOOP specialization**: integer-only fast path for
  numeric `for` loops when bounds are integers.
- **String concatenation**: batching consecutive `CONCAT` operations
  to reduce intermediate allocations.

### GC

- **Generational collection**: the current incremental mark-sweep
  scans all live objects. A generational scheme (nursery for young
  objects, tenured for survivors) would reduce per-cycle work for
  programs with high allocation churn.
- **Sweep batch tuning**: `GCSWEEPMAX` is currently 80. Larger batches
  trade latency for throughput; workload-specific tuning may help.

### Tables

- **Hash function**: the current hash follows PUC-Rio's approach.
  Alternative hash functions could reduce collision rates for specific
  workloads.
- **Array part growth**: more aggressive pre-sizing based on
  constructor analysis could reduce resize operations.
