#!/usr/bin/env bash
# Phase 0a: run a fresh baseline benchmark + bench rerun (for noise floor),
# then capture bench-list and profiler-hotspots JSON.
#
# Inputs (from the framework env file, usually <FRAMEWORK_ROOT>/.env):
#   SOURCE_DIR, STACKS_BENCH_NETWORK, STACKS_BENCH_START_AT, STACKS_BENCH_COUNT,
#   STACKS_BENCH_DATA_DIR, BENCH_LOCK, BASE
#
# Outputs (in SESSION_DIR):
#   baseline-bench-run.json[.stderr.log]
#   baseline-rerun.json[.stderr.log]
#   baseline-run-id            (numeric, single line)
#   baseline-rerun-id
#   bench-list.json
#   baseline-profiler-hotspots.json
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"

COMMON_RANGE_ARGS=(
  --source "$SOURCE_DIR"
  --network "$STACKS_BENCH_NETWORK"
  --start-at "$STACKS_BENCH_START_AT"
  --count   "$STACKS_BENCH_COUNT"
)

cd "$BASE"

flock "$BENCH_LOCK" cargo stacks-bench --db "$STACKS_BENCH_DATA_DIR" --json \
  bench run "${COMMON_RANGE_ARGS[@]}" --name "baseline-$OPT_SESSION_ID" \
  > "$OPT_SESSION_DIR/baseline-bench-run.json" \
  2> "$OPT_SESSION_DIR/baseline-bench-run.stderr.log"
BASELINE_RUN_ID=$(extract_run_id "$OPT_SESSION_DIR/baseline-bench-run.json")
echo "$BASELINE_RUN_ID" > "$OPT_SESSION_DIR/baseline-run-id"

flock "$BENCH_LOCK" cargo stacks-bench --db "$STACKS_BENCH_DATA_DIR" --json \
  bench rerun --run-id "$BASELINE_RUN_ID" \
  > "$OPT_SESSION_DIR/baseline-rerun.json" \
  2> "$OPT_SESSION_DIR/baseline-rerun.stderr.log"
BASELINE_RERUN_ID=$(extract_run_id "$OPT_SESSION_DIR/baseline-rerun.json")
echo "$BASELINE_RERUN_ID" > "$OPT_SESSION_DIR/baseline-rerun-id"

cargo stacks-bench --db "$STACKS_BENCH_DATA_DIR" --json \
  bench list --all --with-args --limit 100 \
  > "$OPT_SESSION_DIR/bench-list.json"
cargo stacks-bench --db "$STACKS_BENCH_DATA_DIR" --json \
  bench show --run-id "$BASELINE_RUN_ID" --profiler-hot 50 \
  > "$OPT_SESSION_DIR/baseline-profiler-hotspots.json"

echo "baseline-run-id   : $BASELINE_RUN_ID"
echo "baseline-rerun-id : $BASELINE_RERUN_ID"
