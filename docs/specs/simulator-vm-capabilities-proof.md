# Simulator VM capabilities: proof ledger

Verified: 2026-10-06. Base: a76ffa8. Original functional implementation: 6856c39. Final verified Rust revision: dcc5a28. Work confined to sim-vm-capabilities worktree; no simulator edits, push or merge.

## Capability coverage

| Capability | Contract | Implementation commits | Behavioral proof |
| --- | --- | --- | --- |
| Trusted owner instruction budgets | Host-selected owner scopes; independent counters; live-frame exhaustion; exemptions restore after errors/unwind; host reset refills | 1d66886, 6f4bd7a | 2 unit + 2 embedding tests: limits, tampering, exemptions, owners, coroutine/pcall evasion, unwind |
| Secret access revocation | Immediate context and descendants denied through tail calls, securecall, coroutine resume/yield; returns on context end; taint unchanged | e4d4ef7, 0aa93e4, 6856c39 | 1 unit + 3 embedding tests: context/errors, resumers, yields, public struct compatibility |
| Secret kind and table contents | Metadata-only payload tag; flagged tables shallowly wrap stored/returned values; keys, length, missing nil public | 3a39bf9, 59be95d, 3908dbf, 6f4bd7a | 1 unit + 3 embedding tests: all tags, tainted metadata, reads/writes/iterators, redirects, prepared slots, GC |
| Secret byte transforms | Trusted byte closure; authentic secret strings only; always secret output; no Lua plaintext; taint unchanged | b6395a8 | 1 unit + 1 embedding test: binary/NUL strings, errors, tainted shortening, unwrap denial, GC |

## Exact commits

- Budget implementation: 1d66886.
- Revocation: e4d4ef7; public CallInfo compatibility restored in 0aa93e4; lazy resumer tracking in 6856c39.
- Kind/contents: 3a39bf9; inherited-index propagation 59be95d; prepared-slot propagation 3908dbf.
- Byte transform: b6395a8.
- Unused-path specialization and contents-policy fast path: 6f4bd7a.

## Earlier test ledger

- Base a76ffa8 snapshot, extracted under target: `cargo test --manifest-path target/base-a76ffa8/Cargo.toml --target-dir target/base-build --lib`: 777 pass; both interrupt-flag tests pass. Their shared-flag race is not observed.
- Same base, `--test integration error_msg_call_global`: fails with `attempt to call a nil value`, matching branch failure exactly.
- RED feature probes reject missing APIs. Additional behavioral RED/GREEN regressions cover inherited __index secrecy and prepared-slot snapshot secrecy. External old-style CallInfo struct construction failed before sidecar repair and passes afterward.
- `cargo test --no-fail-fast` was invoked once at 1468a04, logged in target/test.out. Library 782 pass, integration 486 pass plus known failure, oracle 277 pass. Arbitrary-byte fuzz stdout broke Pyrun's UTF-8 capture, causing SIGPIPE in fuzz/doc targets, not a VM assertion. Only missing targets were retried with file-backed output: fuzz 5 pass, doc 3 pass.
- After compatibility/performance source changes, final affected-target command at 6f4bd7a: `cargo test --lib --test integration --test oracle --test proptest_fuzz --no-fail-fast`: 782 library, 487 integration, 277 oracle, 5 fuzz pass; only known error_msg_call_global fails. Both interrupt tests pass. Log: target/final-affected-targets.out.
- Final coroutine-only change 6856c39: `cargo test secret_access` passes 1 unit + 3 embedding tests; `cargo test coroutine` passes 1 unit + 28 integration + 13 oracle tests. Includes nested resumers entered before first revocation, plus yield/resume restoration. Other proofs remain applicable to unchanged code.
- `cargo fmt --check`, `cargo check`: pass at 6856c39. All-target Clippy at 6f4bd7a exits 0 with no warning on changed lines; lib/integration Clippy at 6856c39 exits 0 with no sidecar/test warning. Existing lint backlog and Rust 1.99 strlen signature warning remain unsuppressed. Doc tests: 3 pass at 6f4bd7a.

New tests: 5 unit and 9 integration tests. Contracts: [budgets](instruction-budgets.md), [secrets](table-security.md). Fixtures: tests/helpers/instruction_budget.rs, secret_access.rs, secret_kind.rs, host_secret_transform.rs; src/table_security_tests.rs and state/instruction_budget.rs.

## Final verification: dcc5a28

| Target | Base a76ffa8 | Final dcc5a28 |
| --- | --- | --- |
| Library | 777 pass | 782 pass |
| Integration | 478 pass, 1 known failure | 487 pass, same known failure |
| Oracle | 277 pass | 277 pass |
| Fuzz | 5 pass | 5 pass |
| Documentation | 3 pass | 3 pass |

Both interrupt tests pass on both revisions. The only VM assertion failure is error_msg_call_global, with identical `got: (command line):1: attempt to call a nil value`. Default-feature dynmod and binary test targets contain zero tests. Final total: 1,554 pass, 1 known failure; base: 1,540 pass, 1 same failure.

Final full invocation: `cargo test --no-fail-fast --target-dir target/verify-build`, exit 101 solely for the known integration failure. Complete combined raw output: target/test-final.out. The last source changes invalidate the earlier full attempt at 91fa816; only the final dcc5a28 run is final acceptance evidence. Base full run: target/test-base-final.out; its fuzz/doc output was interrupted by Pyrun UTF-8 decoding of arbitrary fuzz bytes. Missing base targets were recovered individually in target/base-fuzz-perf-final.out and target/base-doc-perf-final.out.

Binary-safe capture uses a Python argv wrapper that opens the target log, dup2s stdout/stderr onto it, and execs Cargo. No shell, stdout decoding, or output suppression intervenes. This replaces the requested tee pipeline's logging mechanism, not its test scope. The earlier branch attempt suffered the same runner SIGPIPE; the final raw-file run completes all targets.

At dcc5a28: `cargo clippy` exit 0 (21 existing library warnings, including strlen); `cargo check` exit 0; `cargo fmt --check` exit 0. Logs: target/clippy-perf-final.out and target/check-perf-final.out. No forced-inline warnings remain; no lint suppression added. Changed-code readability audit: short named sidecar/growth operations, existing revocation predicate preserved, no new nesting/duplication or warning suppression. Bench-only rebuilding afterward changes no Rust source and does not invalidate these proofs.

## Interleaved performance investigation

### Method

Existing benches/interpreter.rs is unchanged between a76ffa8 and this branch. Cases: end_to_end/coroutine_cycle (create, 50 yields/resumes, completion), vm_execution/closures_100 (100 closure creations/calls/upvalues), vm_execution/table_build_1k (1,000 stores plus 1,000 reads), vm_execution/string_concat_100, and end_to_end/compile_and_run (fresh VM, compile, map/callback calls, tables/arithmetic). All four new capabilities are unused. These are end-to-end existing benches, not a VM-only dispatch microbenchmark.

Build with the unchanged optimized bench/release profile: LTO, one codegen unit, strip=true, target-cpu=native, same Cargo.lock and .cargo/config.toml. Base is a git archive extracted under target/base-a76ffa8; all builds/artifacts remain under this worktree. Rust 1.99.0; AMD Ryzen AI 9 HX 370; nproc=24. Each phase runs 20 rounds, five cases, both binaries sequentially on logical CPU 18 (SMT sibling CPU 6). Randomized case order uses seed 7608; base/branch order alternates each round. No builds run during measurement; no governor, affinity of other processes, services, or system configuration changed.

Each Criterion invocation uses 20 samples, 100 ms warmup, 1 s measurement, 100 resamples, and --noplot. Entries below are median of the 20 invocation medians in microseconds; brackets are their P25–P75 spread (exclusive quartiles). Raw per-sample time/iteration arrays and min/max remain in target/vm-benchmark-<phase>.json and target/vm-benchmark-<phase>-summary.json. No outlier runs removed. Paired delta is median of 20 within-round branch/base ratios, not ratio of aggregate medians; brackets are a deterministic 10,000-resample percentile bootstrap 95% interval (seed 76).

The host is shared and frequency/scheduling changed markedly: initial 1/5/15-minute load 14.30/13.47/10.57 to 17.36/14.81/11.71; final matched-profile load 10.32/10.30/9.09 to 6.89/9.27/9.00. Do not interpret absolute before/after time differences as speedup; compare each branch with its interleaved base in the same phase.

Reproduce with scripts/bench-vm-capabilities.pyrun.py. Archive the desired base/branch revision under target, build each using `cargo bench --manifest-path <snapshot>/Cargo.toml --bench interpreter --no-run --target-dir <build-dir>` from this worktree, and copy the executable Cargo reports to target/vm-bench-base and target/vm-bench-branch. Then `exec(fs.read('scripts/bench-vm-capabilities.pyrun.py'))` in Pyrun, optionally setting ctx.vm_bench_phase, ctx.vm_bench_base, and ctx.vm_bench_branch. The script fails explicitly on command errors and writes each log and complete sample arrays.

### Changes and diagnostic evidence

- cc43ade: Option<Box<SecretAccessContexts>> allocated only on first revocation; out-of-line cold vector pruning, tail transfer, resumer push/pop. Public CallInfo unchanged. Lazy allocation alone does not remove the measured regression.
- 6c7a6d8: frame push/pop inlining probe. Base assembly has no push_ci call; branch precall has two outlined sites. Profiling shows push_ci at 3.69% of coroutine samples.
- 63f31fc: numeric-loop inlining probe. Base has no integer_for_loop_state calls; metered/unmetered branch dispatch has two ABI calls spilling loop tuples. Perf attributes 8.82% of table samples to this helper.
- 91fa816: conversion/concat inlining probe. Branch has six exact_integer_number and two vm_concat call sites versus none on base; string profile attributes 2.35% to conversion and 27.51% to concat.
- 29c09da, dcc5a28: remove forced-inline directives (Clippy rejected their heuristic use), retain ordinary inline hints, cold-outline frame-vector growth, and use semantically identical saturating sentinel decrement. Final symbol inspection has no outlined push_ci/pop_ci or per-iteration integer helper; conversion/preparation/concat decisions remain compiler-controlled. Residual overhead is not isolated to a proven remaining cause.

Unmetered dispatch remains const-specialized, with no per-instruction metering check. Revocation's unused frame/coroutine path uses an optional-pointer check and allocates nothing. The remaining performance requirement is not thereby proved.

### Numbers

Matched-profile acceptance phases: before=c05cb0e, after-lazy=cc43ade, after-matched=dcc5a28. Diagnostic phases after-inline=6c7a6d8, after-loops=63f31fc, after-final=91fa816, after-cleanup=29c09da, and after-saturating=dcc5a28 retained symbols only on the branch while using the stripped base. Their .text hashes differ from the corresponding stripped build, so those phases are **not matching-configuration acceptance evidence**. They are retained, not discarded; the last phase rebuilds both with original strip=true for acceptance. Profiling/assembly comparisons themselves use matching unstripped base/branch binaries.

| Phase | Workload | Base median [P25–P75] us | Branch median [P25–P75] us | Paired delta [95% interval] |
| --- | --- | --- | --- | --- |
| before | Coroutines | 154.686 [138.886–164.022] | 162.864 [151.478–171.902] | +6.58% [+5.12–+8.02%] |
| before | Calls/closures | 169.758 [150.056–176.133] | 177.600 [155.288–187.220] | +4.96% [+1.95–+6.96%] |
| before | Tables | 431.549 [390.231–441.798] | 443.857 [407.048–469.545] | +4.99% [+4.32–+7.06%] |
| before | Strings | 67.689 [63.136–70.073] | 67.053 [62.535–70.855] | +0.03% [-0.21–+2.08%] |
| before | Mixed | 483.441 [412.975–502.740] | 486.431 [424.506–514.987] | +3.20% [+1.01–+6.02%] |
| after-lazy | Coroutines | 139.118 [134.045–153.764] | 147.449 [143.750–158.926] | +6.88% [+5.29–+8.59%] |
| after-lazy | Calls/closures | 147.528 [143.304–161.704] | 153.520 [151.094–172.000] | +5.61% [+3.86–+6.00%] |
| after-lazy | Tables | 387.773 [384.388–437.993] | 408.326 [401.020–428.824] | +4.08% [+3.95–+4.66%] |
| after-lazy | Strings | 60.633 [60.168–64.563] | 61.492 [60.861–64.256] | +1.07% [-0.16–+2.05%] |
| after-lazy | Mixed | 397.289 [384.794–463.135] | 407.151 [398.652–499.789] | +3.91% [+2.40–+4.78%] |
| after-inline | Coroutines | 31.272 [29.887–34.465] | 32.366 [30.515–34.992] | +1.99% [+1.19–+3.85%] |
| after-inline | Calls/closures | 33.237 [32.270–36.382] | 34.289 [33.049–36.752] | +3.07% [+0.25–+5.20%] |
| after-inline | Tables | 87.522 [84.869–96.526] | 94.585 [89.667–100.674] | +5.07% [+3.17–+6.73%] |
| after-inline | Strings | 13.787 [13.336–14.497] | 14.184 [13.661–14.928] | +2.70% [+1.29–+4.07%] |
| after-inline | Mixed | 94.402 [85.364–393.859] | 92.903 [89.332–100.682] | +1.60% [+1.08–+3.32%] |
| after-loops | Coroutines | 28.674 [28.347–29.487] | 28.637 [28.121–29.005] | -0.82% [-2.11–-0.31%] |
| after-loops | Calls/closures | 31.419 [30.537–31.920] | 31.310 [30.586–31.745] | +0.80% [-1.32–+2.10%] |
| after-loops | Tables | 82.100 [80.967–84.540] | 84.111 [82.678–85.773] | +2.04% [+1.39–+2.53%] |
| after-loops | Strings | 12.531 [12.324–12.944] | 12.888 [12.656–13.498] | +2.84% [+2.09–+3.49%] |
| after-loops | Mixed | 82.067 [80.658–84.140] | 84.152 [82.868–86.081] | +2.48% [+0.67–+3.04%] |
| after-final | Coroutines | 29.689 [28.742–30.227] | 30.024 [29.200–31.157] | +1.65% [+1.18–+2.10%] |
| after-final | Calls/closures | 31.287 [30.721–32.790] | 31.098 [30.602–32.065] | -0.71% [-1.98–+1.33%] |
| after-final | Tables | 82.944 [82.008–87.160] | 84.561 [82.541–88.776] | +0.99% [+0.76–+1.73%] |
| after-final | Strings | 12.662 [12.516–13.544] | 13.012 [12.692–13.928] | +1.83% [+1.30–+3.32%] |
| after-final | Mixed | 82.996 [80.947–85.223] | 83.418 [82.106–86.368] | +1.25% [+0.11–+1.51%] |
| after-cleanup | Coroutines | 29.404 [28.893–30.793] | 31.360 [30.356–32.545] | +5.48% [+4.56–+6.98%] |
| after-cleanup | Calls/closures | 31.628 [30.673–32.598] | 32.533 [31.667–33.957] | +3.72% [+3.11–+4.31%] |
| after-cleanup | Tables | 86.787 [83.772–88.645] | 86.508 [84.047–90.430] | +1.05% [-0.46–+2.13%] |
| after-cleanup | Strings | 12.998 [12.572–13.416] | 13.181 [12.799–13.530] | +1.31% [+0.94–+2.60%] |
| after-cleanup | Mixed | 84.335 [81.862–87.561] | 85.677 [82.880–87.428] | +1.52% [+0.40–+2.18%] |
| after-saturating | Coroutines | 34.042 [30.493–36.338] | 34.621 [32.320–37.451] | +5.09% [+2.97–+6.78%] |
| after-saturating | Calls/closures | 36.108 [33.676–38.886] | 36.570 [34.434–39.807] | +2.45% [+1.06–+4.72%] |
| after-saturating | Tables | 95.492 [89.718–102.320] | 97.931 [89.218–104.041] | +0.73% [-0.91–+2.00%] |
| after-saturating | Strings | 14.870 [13.833–15.517] | 14.838 [14.149–15.808] | +1.69% [-0.21–+2.50%] |
| after-saturating | Mixed | 95.123 [88.836–102.991] | 95.659 [90.255–104.066] | +2.07% [+0.71–+3.15%] |
| after-matched | Coroutines | 28.439 [27.132–32.548] | 29.954 [28.365–34.894] | +4.85% [+4.26–+6.33%] |
| after-matched | Calls/closures | 30.419 [28.176–35.132] | 31.522 [29.596–37.058] | +3.65% [+2.61–+4.60%] |
| after-matched | Tables | 81.095 [78.801–90.872] | 82.451 [79.401–92.733] | +2.45% [+1.57–+3.55%] |
| after-matched | Strings | 12.614 [12.436–15.301] | 12.780 [12.641–15.372] | +0.84% [-1.14–+2.99%] |
| after-matched | Mixed | 80.991 [78.504–94.419] | 82.977 [81.108–96.042] | +1.83% [+0.37–+2.78%] |

**Acceptance remains unmet.** Matching-profile final paired medians: coroutine +4.85% (95% interval +4.26–+6.33%), calls +3.65% (+2.61–+4.60%), tables +2.45% (+1.57–+3.55%), mixed +1.83% (+0.37–+2.78%). Strings +0.84% (-1.14–+2.99%) are inconclusive. This confirms real residual overhead, not zero regression. Some overhead is reduced, but the remaining cause is unresolved; no universally regression-free performance or native WoW parity claim is made.

Historical single-sample CPU estimate at 6856c39 was coroutine +5.58% (36.804 versus 38.858 us/iteration), retained in target/performance-coroutine-final.json. Earlier control/table samples and their noisy wall-clock counterparts remain in target/performance-*.out and target/performance-cpu.json; the matched interleaved comparison above supersedes them for this follow-up.
