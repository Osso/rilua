# Simulator VM capabilities: proof ledger

Verified: 2026-10-06. Base: a76ffa8. Functional implementation: 6856c39. Work confined to sim-vm-capabilities worktree; no simulator edits, push or merge.

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

## Test ledger

- Base a76ffa8 snapshot, extracted under target: `cargo test --manifest-path target/base-a76ffa8/Cargo.toml --target-dir target/base-build --lib`: 777 pass; both interrupt-flag tests pass. Their shared-flag race is not observed.
- Same base, `--test integration error_msg_call_global`: fails with `attempt to call a nil value`, matching branch failure exactly.
- RED feature probes reject missing APIs. Additional behavioral RED/GREEN regressions cover inherited __index secrecy and prepared-slot snapshot secrecy. External old-style CallInfo struct construction failed before sidecar repair and passes afterward.
- `cargo test --no-fail-fast` was invoked once at 1468a04, logged in target/test.out. Library 782 pass, integration 486 pass plus known failure, oracle 277 pass. Arbitrary-byte fuzz stdout broke Pyrun's UTF-8 capture, causing SIGPIPE in fuzz/doc targets, not a VM assertion. Only missing targets were retried with file-backed output: fuzz 5 pass, doc 3 pass.
- After compatibility/performance source changes, final affected-target command at 6f4bd7a: `cargo test --lib --test integration --test oracle --test proptest_fuzz --no-fail-fast`: 782 library, 487 integration, 277 oracle, 5 fuzz pass; only known error_msg_call_global fails. Both interrupt tests pass. Log: target/final-affected-targets.out.
- Final coroutine-only change 6856c39: `cargo test secret_access` passes 1 unit + 3 embedding tests; `cargo test coroutine` passes 1 unit + 28 integration + 13 oracle tests. Includes nested resumers entered before first revocation, plus yield/resume restoration. Other proofs remain applicable to unchanged code.
- `cargo fmt --check`, `cargo check`: pass at 6856c39. All-target Clippy at 6f4bd7a exits 0 with no warning on changed lines; lib/integration Clippy at 6856c39 exits 0 with no sidecar/test warning. Existing lint backlog and Rust 1.99 strlen signature warning remain unsuppressed. Doc tests: 3 pass at 6f4bd7a.

New tests: 5 unit and 9 integration tests. Contracts: [budgets](instruction-budgets.md), [secrets](table-security.md). Fixtures: tests/helpers/instruction_budget.rs, secret_access.rs, secret_kind.rs, host_secret_transform.rs; src/table_security_tests.rs and state/instruction_budget.rs.

## Performance: acceptance gap

No per-instruction budget branch remains in unmetered execution. Before option 2 is used, contents helpers avoid table-arena lookups. Coroutine resumer identity bookkeeping is skipped before revocation. Follow-up optimization makes the revocation sidecar an optional boxed allocation, created only on first revocation; frame push/pop, tail transfer, and coroutine resume use one pointer check before out-of-line vector updates. Public CallInfo and revocation lifetimes are unchanged. Interleaved remeasurement is pending.

Wall-clock Criterion results fluctuate sharply on the shared host, even CPU-pinned. Recorded regressions were not discarded: target/performance-pinned-*.out and performance-optimized-*.out retain them. To distinguish scheduling delay from work, per-case runs pinned CPU 18 used 20 samples, 5 s measurement, 10 ms warmup and 100 resamples; child user CPU time was divided by exact sample.json iteration counts (includes small startup/warmup overhead).

At 6f4bd7a, normalized current/base CPU ratios: control-flow 0.9771, metatable indexing 0.9072, table building 0.9488, coroutine cycle 1.0236. Raw data: target/performance-cpu.json. At final 6856c39, coroutine cycle is **1.0558** (base 36.804 us/iteration, current 38.858 us/iteration), retained in target/performance-coroutine-final.json. Benchmarked non-secret, non-coroutine paths were unchanged by that final optimization.

**Zero unused-feature performance regression is not established; coroutine CPU time remains higher in the final paired sample. This acceptance gate is not satisfied.** All four functional capabilities are implemented and behaviorally tested; no native WoW parity or universally regression-free performance claim is made.
