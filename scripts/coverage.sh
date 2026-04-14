#!/usr/bin/env bash
# Run source-based coverage for rilua with a stable local workflow.
#
# Uses cargo-llvm-cov for instrumentation and reporting. Some environments do
# not have rustup's llvm-tools-preview installed, so this script falls back to
# system llvm binaries when LLVM_COV / LLVM_PROFDATA are not already set.
#
# Usage:
#   ./scripts/coverage.sh [summary|html|json|text]
#
# Outputs:
#   summary -> target/llvm-cov/summary.json
#   html    -> target/llvm-cov/html/
#   json    -> target/llvm-cov/coverage.json
#   text    -> target/llvm-cov/text/

set -euo pipefail

MODE="${1:-summary}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$ROOT/target/llvm-cov"

resolve_tool() {
    local env_var="$1"
    local bin_name="$2"
    local current="${!env_var:-}"

    if [ -n "$current" ]; then
        printf '%s\n' "$current"
        return
    fi

    if command -v "$bin_name" >/dev/null 2>&1; then
        command -v "$bin_name"
        return
    fi

    if [ -x "/usr/bin/$bin_name" ]; then
        printf '%s\n' "/usr/bin/$bin_name"
        return
    fi

    echo "Error: could not find $bin_name. Set $env_var explicitly." >&2
    exit 1
}

if ! cargo llvm-cov --version >/dev/null 2>&1; then
    echo "Error: cargo-llvm-cov is required." >&2
    echo "Install with: cargo install cargo-llvm-cov" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

LLVM_COV_BIN="$(resolve_tool LLVM_COV llvm-cov)"
LLVM_PROFDATA_BIN="$(resolve_tool LLVM_PROFDATA llvm-profdata)"

run_cov() {
    (
        cd "$ROOT"
        env LLVM_COV="$LLVM_COV_BIN" LLVM_PROFDATA="$LLVM_PROFDATA_BIN" \
            cargo llvm-cov "$@"
    )
}

case "$MODE" in
    summary)
        run_cov --json --summary-only --output-path "$OUT_DIR/summary.json" --no-fail-fast
        echo "Summary report: $OUT_DIR/summary.json"
        ;;
    html)
        run_cov --html --output-dir "$OUT_DIR/html" --no-fail-fast
        echo "HTML report: $OUT_DIR/html/index.html"
        ;;
    json)
        run_cov --json --output-path "$OUT_DIR/coverage.json" --no-fail-fast
        echo "JSON report: $OUT_DIR/coverage.json"
        ;;
    text)
        run_cov --text --output-dir "$OUT_DIR/text" --no-fail-fast
        echo "Text report directory: $OUT_DIR/text"
        ;;
    *)
        echo "Usage: $0 [summary|html|json|text]" >&2
        exit 1
        ;;
esac
