#!/usr/bin/env bash
# Phase 0b: import an existing baseline run id (and optionally a rerun id)
# instead of running a fresh benchmark. Populates the same artifact files
# `run-baseline.sh` would, by querying `bench show --json` against the
# already-recorded runs.
#
# Usage:
#   import-baseline.sh SESSION_DIR RUN_ID [RERUN_ID]
#
# If RERUN_ID is omitted, RUN_ID is used for both. The downstream noise-floor
# computation then sees zero variance — finalize-session.sh treats every
# experiment improvement as significant. That's fine for a demo where you
# just want to see the rest of the pipeline run; for accept/reject decisions
# in production you should always supply a real RERUN_ID.
#
# Outputs (in SESSION_DIR):
#   same as run-baseline.sh
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"

if [ "$#" -lt 2 ]; then
  echo "usage: $0 SESSION_DIR RUN_ID [RERUN_ID]" >&2
  exit 2
fi

init_session "$1"
RUN_ID="$2"
RERUN_ID="${3:-$RUN_ID}"

mapfile -t BENCH < <(resolve_bench_bin)

# Reconstruct baseline-bench-run.json / baseline-rerun.json from
# `bench show --json --run-id N`. The bench-show envelope shape differs
# slightly from `bench run` / `bench rerun` envelopes:
#   - bench run     → `.data` has run_id, blocks, summary, ...
#   - bench show    → `.data` has run_id, name, start_time, end_time,
#                     git_hash, summary, profiler_hot
# Fields used downstream (`.data.run_id`, `.data.summary.total_duration_us`)
# are present in both, but if you add a downstream reader that depends on
# `.data.blocks` or another bench-run-specific field, double-check shape
# compatibility before assuming this import path is interchangeable.
"${BENCH[@]}" --db "$STACKS_BENCH_DATA_DIR" --json \
  bench show --run-id "$RUN_ID" \
  > "$OPT_SESSION_DIR/baseline-bench-run.json" \
  2> "$OPT_SESSION_DIR/baseline-bench-run.stderr.log"

"${BENCH[@]}" --db "$STACKS_BENCH_DATA_DIR" --json \
  bench show --run-id "$RERUN_ID" \
  > "$OPT_SESSION_DIR/baseline-rerun.json" \
  2> "$OPT_SESSION_DIR/baseline-rerun.stderr.log"

echo "$RUN_ID"   > "$OPT_SESSION_DIR/baseline-run-id"
echo "$RERUN_ID" > "$OPT_SESSION_DIR/baseline-rerun-id"

"${BENCH[@]}" --db "$STACKS_BENCH_DATA_DIR" --json \
  bench list --all --with-args --limit 100 \
  > "$OPT_SESSION_DIR/bench-list.json"
"${BENCH[@]}" --db "$STACKS_BENCH_DATA_DIR" --json \
  bench show --run-id "$RUN_ID" --profiler-hot 50 \
  > "$OPT_SESSION_DIR/baseline-profiler-hotspots.json"

# Sanity check: run id must actually be findable.
ACTUAL=$(jq -r '.data.run_id // empty' "$OPT_SESSION_DIR/baseline-bench-run.json")
if [ "$ACTUAL" != "$RUN_ID" ]; then
  echo "import-baseline: requested run_id $RUN_ID but bench show returned '$ACTUAL'." >&2
  echo "Check that run id exists in $STACKS_BENCH_DATA_DIR/appdata/stacks-bench.db." >&2
  exit 1
fi

echo "imported baseline-run-id   : $RUN_ID"
echo "imported baseline-rerun-id : $RERUN_ID"
if [ "$RUN_ID" = "$RERUN_ID" ]; then
  echo "WARNING: noise floor will be 0 (same id used for both)." >&2
fi
