#!/usr/bin/env bash
# Performance regression workflow for rilua.
#
# Modes:
#   smoke                     Run the small stable smoke subset and compare
#                             Criterion smoke benchmarks against a saved baseline.
#   gate                      Run the full all.lua wall-clock gate against
#                             .perf-baseline.
#   all                       Run smoke then gate.
#   diagnose-criterion-baseline
#                             Save a fresh temporary Criterion baseline for one
#                             smoke benchmark, compare against it, and report
#                             whether the saved baseline looks stale.
#   refresh-criterion-baseline
#                             Refresh the named Criterion smoke baseline.
#   show-config               Print the active thresholds, tests, and benchmarks.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:-smoke}"

FULL_BASELINE_FILE="$ROOT/.perf-baseline"
FULL_THRESHOLD_PCT="${PERF_FULL_THRESHOLD_PCT:-5}"
FULL_RUNS="${PERF_FULL_RUNS:-5}"
PUC_SMOKE_RUNS="${PERF_PUC_SMOKE_RUNS:-3}"
CRITERION_BASELINE_NAME="${PERF_CRITERION_BASELINE_NAME:-perf-smoke}"
CRITERION_THRESHOLD_PCT="${PERF_CRITERION_THRESHOLD_PCT:-20}"

PUC_SMOKE_TESTS=(
    constructs.lua
    nextvar.lua
    sort.lua
    db.lua
    verybig.lua
)

CRITERION_SMOKE_BENCHES=(
    control_flow_dispatch
    verybig_loaded_chunk
    next_pairs_mixed_1k
    sort_callback_1k
)

print_config() {
    echo "Mode: $MODE"
    echo "Full-suite threshold: ${FULL_THRESHOLD_PCT}%"
    echo "Full-suite runs: $FULL_RUNS"
    echo "Full-suite baseline file: $FULL_BASELINE_FILE"
    echo "PUC-Rio smoke runs: $PUC_SMOKE_RUNS"
    echo "Criterion baseline name: $CRITERION_BASELINE_NAME"
    echo "Criterion smoke threshold: ${CRITERION_THRESHOLD_PCT}%"
    echo "PUC-Rio smoke subset: ${PUC_SMOKE_TESTS[*]}"
    echo "Criterion smoke subset: ${CRITERION_SMOKE_BENCHES[*]}"
}

build_release() {
    echo "==> Building release binary"
    cargo build --release
}

ensure_full_baseline() {
    if [ ! -f "$FULL_BASELINE_FILE" ]; then
        echo "Error: missing full-suite baseline file: $FULL_BASELINE_FILE" >&2
        echo "Run ./scripts/bench-puc-rio.sh > .perf-baseline after an accepted improvement." >&2
        exit 1
    fi
}

run_puc_smoke_subset() {
    echo "==> PUC-Rio smoke subset (${PUC_SMOKE_RUNS} runs per test)"
    "$ROOT/scripts/benchmark-tests.sh" "$PUC_SMOKE_RUNS" "${PUC_SMOKE_TESTS[@]}"
}

read_point_estimate() {
    local json_file="$1"
    python3 - "$json_file" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    data = json.load(handle)

for key in ("slope", "mean", "median"):
    metric = data.get(key)
    if isinstance(metric, dict) and metric.get("point_estimate") is not None:
        print(metric["point_estimate"])
        raise SystemExit(0)

raise SystemExit(f"no usable point estimate in {sys.argv[1]}")
PY
}

normalize_path() {
    local path="$1"
    case "$path" in
        /*) echo "$path" ;;
        *) echo "$ROOT/$path" ;;
    esac
}

find_criterion_estimates() {
    local bench_name="$1"
    local phase="$2"
    local path
    path="$(
        rg --files "$ROOT/target/criterion" \
            | rg "/${bench_name}/${phase}/estimates\\.json$" \
            | head -n 1
    )"
    if [ -z "$path" ]; then
        echo "Error: missing Criterion ${phase} estimates for ${bench_name}" >&2
        exit 1
    fi
    normalize_path "$path"
}

compare_criterion_bench() {
    local bench_name="$1"
    local baseline_file
    local new_file
    local baseline_estimate
    local new_estimate
    local delta_pct

    echo "==> Criterion compare: ${bench_name}"
    cargo bench --bench interpreter -- --noplot --baseline "$CRITERION_BASELINE_NAME" "$bench_name"

    baseline_file="$(find_criterion_estimates "$bench_name" "$CRITERION_BASELINE_NAME")"
    new_file="$(find_criterion_estimates "$bench_name" new)"
    baseline_estimate="$(read_point_estimate "$baseline_file")"
    new_estimate="$(read_point_estimate "$new_file")"
    delta_pct="$(
        python3 - "$baseline_estimate" "$new_estimate" <<'PY'
import sys

base = float(sys.argv[1])
new = float(sys.argv[2])
print(((new - base) * 100.0) / base)
PY
    )"

    python3 - "$bench_name" "$baseline_estimate" "$new_estimate" "$delta_pct" "$CRITERION_THRESHOLD_PCT" <<'PY'
import sys

bench_name = sys.argv[1]
base = float(sys.argv[2])
new = float(sys.argv[3])
delta = float(sys.argv[4])
threshold = float(sys.argv[5])

status = "PASS"
if delta > threshold:
    status = "FAIL"

print(
    f"{status}: {bench_name} base={base:.2f} new={new:.2f} delta={delta:+.2f}% "
    f"(threshold +{threshold:.2f}%)"
)

if status == "FAIL":
    raise SystemExit(1)
PY
}

refresh_criterion_baseline() {
    echo "==> Refreshing Criterion smoke baseline: ${CRITERION_BASELINE_NAME}"
    for bench_name in "${CRITERION_SMOKE_BENCHES[@]}"; do
        echo "--> Saving baseline for ${bench_name}"
        cargo bench --bench interpreter -- --noplot --save-baseline "$CRITERION_BASELINE_NAME" "$bench_name"
    done
}

compare_criterion_smoke() {
    local baseline_dir
    baseline_dir="$(
        rg --files "$ROOT/target/criterion" \
            | rg "/${CRITERION_BASELINE_NAME}/estimates\\.json$" \
            | head -n 1
    )"
    if [ -z "$baseline_dir" ]; then
        echo "Error: missing Criterion smoke baseline '${CRITERION_BASELINE_NAME}' in target/criterion." >&2
        echo "Run ./scripts/perf-regression.sh refresh-criterion-baseline first." >&2
        exit 1
    fi

    echo "==> Criterion smoke compare (threshold +${CRITERION_THRESHOLD_PCT}%)"
    for bench_name in "${CRITERION_SMOKE_BENCHES[@]}"; do
        compare_criterion_bench "$bench_name"
    done
}

sanitize_baseline_name() {
    printf '%s' "$1" | tr -cs '[:alnum:]._' '-'
}

diagnose_criterion_baseline() {
    local bench_name="${1:-}"
    local temp_baseline
    local saved_file
    local temp_file
    local new_file
    local saved_estimate
    local temp_estimate
    local new_estimate

    if [ -z "$bench_name" ]; then
        echo "Error: diagnose-criterion-baseline requires a benchmark name." >&2
        echo "Example: ./scripts/perf-regression.sh diagnose-criterion-baseline control_flow_dispatch" >&2
        exit 1
    fi

    saved_file="$(find_criterion_estimates "$bench_name" "$CRITERION_BASELINE_NAME")"
    temp_baseline="${CRITERION_BASELINE_NAME}-recheck-$(sanitize_baseline_name "$bench_name")"

    echo "==> Saving temporary Criterion baseline for ${bench_name}: ${temp_baseline}"
    cargo bench --bench interpreter -- --noplot --save-baseline "$temp_baseline" "$bench_name"

    echo "==> Comparing ${bench_name} against temporary baseline ${temp_baseline}"
    cargo bench --bench interpreter -- --noplot --baseline "$temp_baseline" "$bench_name"

    temp_file="$(find_criterion_estimates "$bench_name" "$temp_baseline")"
    new_file="$(find_criterion_estimates "$bench_name" new)"
    saved_estimate="$(read_point_estimate "$saved_file")"
    temp_estimate="$(read_point_estimate "$temp_file")"
    new_estimate="$(read_point_estimate "$new_file")"

    python3 - "$bench_name" "$CRITERION_BASELINE_NAME" "$temp_baseline" \
        "$saved_estimate" "$temp_estimate" "$new_estimate" "$CRITERION_THRESHOLD_PCT" <<'PY'
import sys

bench_name = sys.argv[1]
saved_name = sys.argv[2]
temp_name = sys.argv[3]
saved = float(sys.argv[4])
temp = float(sys.argv[5])
new = float(sys.argv[6])
threshold = float(sys.argv[7])

saved_to_temp = ((temp - saved) * 100.0) / saved
temp_to_new = ((new - temp) * 100.0) / temp
saved_to_new = ((new - saved) * 100.0) / saved

print(
    f"Saved baseline ({saved_name}): {saved:.2f}\n"
    f"Fresh baseline ({temp_name}): {temp:.2f}\n"
    f"Fresh compare (new): {new:.2f}\n"
    f"Delta saved->fresh: {saved_to_temp:+.2f}%\n"
    f"Delta fresh->new:   {temp_to_new:+.2f}%\n"
    f"Delta saved->new:   {saved_to_new:+.2f}%"
)

if abs(saved_to_temp) > threshold and abs(temp_to_new) <= threshold:
    print(
        f"LIKELY STALE BASELINE: {bench_name} differs from the saved baseline by more than "
        f"{threshold:.2f}% while a fresh same-tree rerun stays within threshold."
    )
    raise SystemExit(0)

if abs(temp_to_new) > threshold:
    print(
        f"UNSTABLE OR REAL REGRESSION: same-tree reruns for {bench_name} still move by more than "
        f"{threshold:.2f}%."
    )
    raise SystemExit(1)

print(
    f"REPRESENTATIVE BASELINE: {bench_name} stayed within {threshold:.2f}% on both the saved "
    f"baseline and the fresh same-tree rerun."
)
PY
}

run_full_gate() {
    local baseline
    local current
    local max_allowed

    ensure_full_baseline
    baseline="$(<"$FULL_BASELINE_FILE")"

    echo "==> Full-suite gate (${FULL_RUNS} runs, threshold +${FULL_THRESHOLD_PCT}%)"
    current="$("$ROOT/scripts/bench-puc-rio.sh" target/release/rilua "$FULL_RUNS")"
    max_allowed=$(( baseline + baseline * FULL_THRESHOLD_PCT / 100 ))

    echo "Full-suite baseline: ${baseline}ms"
    echo "Full-suite current:  ${current}ms"
    echo "Full-suite limit:    ${max_allowed}ms"

    if [ "$current" -gt "$max_allowed" ]; then
        echo "FAIL: full-suite regression exceeded threshold" >&2
        exit 1
    fi

    echo "PASS: full-suite result is within threshold"
}

usage() {
    cat <<'EOF'
Usage: ./scripts/perf-regression.sh [smoke|gate|all|diagnose-criterion-baseline|refresh-criterion-baseline|show-config] [bench-name]

Modes:
  smoke                      Build release, run the PUC-Rio smoke subset, and compare
                             the Criterion smoke subset against a saved baseline.
  gate                       Build release and run the full all.lua wall-clock gate.
  all                        Run smoke and gate in sequence.
  diagnose-criterion-baseline
                             Save a fresh temporary Criterion baseline for one benchmark,
                             compare against it, and report whether the saved baseline
                             still looks representative.
  refresh-criterion-baseline Refresh the named Criterion smoke baseline.
  show-config                Print the active thresholds, tests, and benchmarks.
EOF
}

case "$MODE" in
    smoke)
        build_release
        run_puc_smoke_subset
        compare_criterion_smoke
        ;;
    gate)
        build_release
        run_full_gate
        ;;
    all)
        build_release
        run_puc_smoke_subset
        compare_criterion_smoke
        run_full_gate
        ;;
    diagnose-criterion-baseline)
        diagnose_criterion_baseline "${2:-}"
        ;;
    refresh-criterion-baseline)
        refresh_criterion_baseline
        ;;
    show-config)
        print_config
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
