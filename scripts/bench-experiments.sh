#!/usr/bin/env bash
# Phase 3: build the optimizer worktrees' release binaries, copy them out,
# then run two benchmarks per non-aborted experiment, serialized under
# BENCH_LOCK so no two benchmarks contend for the host.
#
# Inputs (in SESSION_DIR):
#   optimization-targets.json
#   experiments/<id>/{implementation.md|abort.md}
#   $WORKTREES/<id>/                       (the built worktree)
#
# Outputs (in SESSION_DIR):
#   experiments/<id>/cargo-build.log[.stderr.log]
#   experiments/<id>/cargo-clean.log[.stderr.log]   (unless SKIP_CARGO_CLEAN=1)
#   experiments/<id>/bin/stacks-bench               (copied from worktree)
#   experiments/<id>/run-{1,2}/bench-run.json[.stderr.log]
#   experiments/<id>/run-ids                         (one numeric id per line)
#
# Knobs:
#   SKIP_CARGO_CLEAN=1              don't `cargo clean` the worktree after build
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"

TARGETS="$OPT_SESSION_DIR/optimization-targets.json"
if [ ! -s "$TARGETS" ]; then
  echo "bench-experiments: missing optimization-targets.json" >&2
  exit 2
fi

mapfile -t TARGET_IDS < <(jq -r '.targets[].id' "$TARGETS")
if [ "${#TARGET_IDS[@]}" -eq 0 ]; then
  echo "No targets to benchmark; phase is a no-op."
  exit 0
fi

COMMON_RANGE_ARGS=(
  --source "$SOURCE_DIR"
  --network "$STACKS_BENCH_NETWORK"
  --start-at "$STACKS_BENCH_START_AT"
  --count   "$STACKS_BENCH_COUNT"
)

# Build + copy binaries (parallel-safe; each worktree has its own ./target).
for TARGET_ID in "${TARGET_IDS[@]}"; do
  WT="$WORKTREES/$TARGET_ID"
  OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"

  # Consensus-breaking targets are skipped — the bench harness encodes
  # current-epoch consensus and would either crash or produce meaningless
  # numbers under a consensus change. Read bench_eligible directly from the
  # merged target so the skip is auditable and routed in one place.
  BENCH_ELIGIBLE=$(jq -r --arg id "$TARGET_ID" \
    '.targets[] | select(.id==$id) | .bench_eligible' "$TARGETS")
  DELIVERY_MODE=$(jq -r --arg id "$TARGET_ID" \
    '.targets[] | select(.id==$id) | .delivery_mode' "$TARGETS")
  if [ "$BENCH_ELIGIBLE" != "true" ]; then
    echo "skip $TARGET_ID: not bench_eligible (delivery_mode=$DELIVERY_MODE)"
    continue
  fi

  if [ -f "$OUTPUT_DIR/abort.md" ]; then
    echo "skip $TARGET_ID: aborted"
    continue
  fi
  if [ -f "$OUTPUT_DIR/consensus-issue.md" ]; then
    # Belt-and-suspenders: a consensus_issue target should already be filtered
    # by bench_eligible above, but if both markers ever coexist (e.g. mid-
    # migration), the consensus-issue marker takes precedence.
    echo "skip $TARGET_ID: consensus-issue (no optimizer ran)"
    continue
  fi

  ( cd "$WT" && cargo build --release -p stacks-bench ) \
    > "$OUTPUT_DIR/cargo-build.log" \
    2> "$OUTPUT_DIR/cargo-build.stderr.log"

  BIN_DIR="$OUTPUT_DIR/bin"; mkdir -p "$BIN_DIR"
  cp "$WT/target/release/stacks-bench" "$BIN_DIR/stacks-bench"
  chmod +x "$BIN_DIR/stacks-bench"

  if [ "${SKIP_CARGO_CLEAN:-0}" != "1" ]; then
    ( cd "$WT" && cargo clean ) \
      > "$OUTPUT_DIR/cargo-clean.log" \
      2> "$OUTPUT_DIR/cargo-clean.stderr.log" || true
  fi
done

# Run benchmarks serialized under BENCH_LOCK (one experiment at a time on the host).
for TARGET_ID in "${TARGET_IDS[@]}"; do
  OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  BENCH_ELIGIBLE=$(jq -r --arg id "$TARGET_ID" \
    '.targets[] | select(.id==$id) | .bench_eligible' "$TARGETS")
  [ "$BENCH_ELIGIBLE" = "true" ] || continue
  [ -f "$OUTPUT_DIR/abort.md" ] && continue
  [ -f "$OUTPUT_DIR/consensus-issue.md" ] && continue
  [ -x "$OUTPUT_DIR/bin/stacks-bench" ] || { echo "no binary for $TARGET_ID, skipping" >&2; continue; }

  # Idempotency: truncate run-ids before re-populating. The bench-run.json
  # files are overwritten by `>` below, so without this truncate a re-run
  # of this phase would leave stale ids in the file (alongside fresh ones)
  # and finalize-session.sh would average across both. The visible JSON
  # files only show the most recent two runs, so the file/data drift would
  # be invisible and the summary would be wrong.
  : > "$OUTPUT_DIR/run-ids"

  for N in 1 2; do
    OUT="$OUTPUT_DIR/run-$N"; mkdir -p "$OUT"
    flock "$BENCH_LOCK" \
      "$OUTPUT_DIR/bin/stacks-bench" \
        --db "$STACKS_BENCH_DATA_DIR" --json \
        bench run "${COMMON_RANGE_ARGS[@]}" \
        --name "$TARGET_ID-run-$N" \
        > "$OUT/bench-run.json" \
        2> "$OUT/bench-run.stderr.log"
    extract_run_id "$OUT/bench-run.json" >> "$OUTPUT_DIR/run-ids"
  done
done
