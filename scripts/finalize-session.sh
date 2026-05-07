#!/usr/bin/env bash
# Emit summary.json for one optimization session.
#
# For each target in optimization-targets.json:
#   - if experiments/<id>/abort.md exists  → status: "aborted"
#   - elif no run-ids                       → status: "aborted" (no benchmark data)
#   - else
#       compare mean(exp run total_duration_us) vs mean(baseline + baseline_rerun)
#       improvement_pct = (baseline_mean - exp_mean) / baseline_mean * 100
#       if improvement_pct >  noise_floor_pct → "accepted"
#       elif improvement_pct < -noise_floor_pct → "rejected" (regression)
#       else                                    → "rejected" (within noise)
#
# Output schema: schemas/summary.schema.json
# Writes:
#   - JSON to stdout (the caller redirects to summary.json)
#   - summary.md alongside, as a derived human view
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"
SESSION_DIR="$OPT_SESSION_DIR"

TARGETS="$SESSION_DIR/optimization-targets.json"
[ -s "$TARGETS" ] || { echo "missing $TARGETS" >&2; exit 1; }

# Prefer the prebuilt binary so finalize doesn't re-link via cargo.
if [ -x "$BASE/target/release/stacks-bench" ]; then
  BENCH_BIN=( "$BASE/target/release/stacks-bench" )
else
  BENCH_BIN=( cargo stacks-bench )
fi

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Emit `total_duration_us` for a given run_id, or empty string if missing.
total_duration_us() {
  local run_id="$1"
  "${BENCH_BIN[@]}" --db "$STACKS_BENCH_DATA_DIR" --json \
      bench show --run-id "$run_id" 2>/dev/null \
    | jq -r '.data.summary.total_duration_us // empty'
}

# Arithmetic mean of stdin lines. Empty input → "0".
mean() {
  awk 'NF { s += $1; n++ } END { if (n>0) printf "%.6f", s/n; else printf "0" }'
}

# Compare two floats with awk; usage: float_gt A B  → returns 0 if A > B.
float_gt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a > b) }'; }
float_lt() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a < b) }'; }

# ---------------------------------------------------------------------------
# Session-level fields
# ---------------------------------------------------------------------------
SESSION_ID=$(jq -r '.session_id'        "$TARGETS")
BASELINE_RUN_ID=$(jq -r '.baseline_run_id'   "$TARGETS")
BASELINE_RERUN_ID=$(jq -r '.baseline_rerun_id' "$TARGETS")
NOISE_FLOOR_PCT=$(jq -r '.noise_floor_pct'   "$TARGETS")

# Baseline = mean(baseline run, baseline rerun). If either is missing we
# can't compute improvements reliably; bail loudly so the operator notices.
B1=$(total_duration_us "$BASELINE_RUN_ID")
B2=$(total_duration_us "$BASELINE_RERUN_ID")
if [ -z "$B1" ] || [ -z "$B2" ]; then
  echo "finalize-session: missing baseline summary (run_id=$BASELINE_RUN_ID, rerun_id=$BASELINE_RERUN_ID)" >&2
  exit 1
fi
BASELINE_MEAN=$(printf '%s\n%s\n' "$B1" "$B2" | mean)
NEG_NOISE=$(awk -v n="$NOISE_FLOOR_PCT" 'BEGIN { printf "%.6f", -n }')

# ---------------------------------------------------------------------------
# Per-experiment evaluation
# ---------------------------------------------------------------------------
EXPERIMENTS_JSON='[]'

while IFS= read -r tid; do
  EXP="$SESSION_DIR/experiments/$tid"

  # Aborted by subagent.
  if [ -f "$EXP/abort.md" ]; then
    REASON=$(head -c 4096 "$EXP/abort.md" | tr '\n' ' ' | awk '{$1=$1};1' | cut -c1-300)
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg reason "$REASON" \
      '. + [{target_id: $tid, status: "aborted", reason: $reason}]' \
      <<< "$EXPERIMENTS_JSON")
    continue
  fi

  # No run ids recorded (binary missing, build failed, benchmarks crashed, etc.).
  if [ ! -s "$EXP/run-ids" ]; then
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" \
      '. + [{target_id: $tid, status: "aborted", reason: "no benchmark runs recorded"}]' \
      <<< "$EXPERIMENTS_JSON")
    continue
  fi

  # Collect (run_id, total_duration_us) pairs.
  RUN_IDS_ARR='[]'
  EXP_DURATIONS=()
  while IFS= read -r rid; do
    [ -z "$rid" ] && continue
    RUN_IDS_ARR=$(jq --argjson r "$rid" '. + [$r]' <<< "$RUN_IDS_ARR")
    val=$(total_duration_us "$rid")
    [ -n "$val" ] && EXP_DURATIONS+=("$val")
  done < "$EXP/run-ids"

  if [ "${#EXP_DURATIONS[@]}" -eq 0 ]; then
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --argjson run_ids "$RUN_IDS_ARR" \
      '. + [{target_id: $tid, status: "aborted", reason: "all benchmark runs missing summaries", run_ids: $run_ids}]' \
      <<< "$EXPERIMENTS_JSON")
    continue
  fi

  EXP_MEAN=$(printf '%s\n' "${EXP_DURATIONS[@]}" | mean)
  IMPROVEMENT_PCT=$(awk -v b="$BASELINE_MEAN" -v e="$EXP_MEAN" \
    'BEGIN { if (b == 0) printf "0"; else printf "%.4f", (b - e) / b * 100 }')

  if float_gt "$IMPROVEMENT_PCT" "$NOISE_FLOOR_PCT"; then
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --argjson run_ids "$RUN_IDS_ARR" \
      --argjson improvement "$IMPROVEMENT_PCT" \
      '. + [{target_id: $tid, status: "accepted", run_ids: $run_ids, improvement_pct: $improvement}]' \
      <<< "$EXPERIMENTS_JSON")
  elif float_lt "$IMPROVEMENT_PCT" "$NEG_NOISE"; then
    REASON=$(awk -v p="$IMPROVEMENT_PCT" 'BEGIN { printf "regression: %.2f%% slower than baseline", -p }')
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --argjson run_ids "$RUN_IDS_ARR" \
      --argjson improvement "$IMPROVEMENT_PCT" --arg reason "$REASON" \
      '. + [{target_id: $tid, status: "rejected", run_ids: $run_ids, improvement_pct: $improvement, reason: $reason}]' \
      <<< "$EXPERIMENTS_JSON")
  else
    REASON=$(awk -v p="$IMPROVEMENT_PCT" -v n="$NOISE_FLOOR_PCT" \
      'BEGIN { printf "within noise floor (%.2f%% improvement vs %.2f%% noise)", p, n }')
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --argjson run_ids "$RUN_IDS_ARR" \
      --argjson improvement "$IMPROVEMENT_PCT" --arg reason "$REASON" \
      '. + [{target_id: $tid, status: "rejected", run_ids: $run_ids, improvement_pct: $improvement, reason: $reason}]' \
      <<< "$EXPERIMENTS_JSON")
  fi
done < <(jq -r '.targets[].id' "$TARGETS")

# ---------------------------------------------------------------------------
# next_targets_hint
# ---------------------------------------------------------------------------
NUM_TARGETS=$(jq 'length' <<< "$EXPERIMENTS_JSON")
NUM_ACCEPTED=$(jq '[.[] | select(.status=="accepted")] | length' <<< "$EXPERIMENTS_JSON")
NUM_ABORTED=$(jq '[.[] | select(.status=="aborted")] | length' <<< "$EXPERIMENTS_JSON")
NUM_REGRESSIONS=$(jq '[.[] | select(.status=="rejected" and (.reason // "" | startswith("regression")))] | length' \
  <<< "$EXPERIMENTS_JSON")

HINT=""
if [ "$NUM_TARGETS" -eq 0 ]; then
  HINT="zero targets reached benchmarking; check analyses/*/analysis.json"
elif [ "$NUM_ACCEPTED" -eq 0 ]; then
  if [ "$NUM_ABORTED" -eq "$NUM_TARGETS" ]; then
    HINT="all experiments aborted before benchmarking; review experiments/*/abort.md and subagent-stderr.log"
  elif [ "$NUM_REGRESSIONS" -gt 0 ]; then
    HINT="rejected: $NUM_REGRESSIONS regression(s); rest within noise. Try smaller-scope changes or tighter targets."
  else
    HINT="all rejected within noise floor; try wider profiler view (--profiler-hot 100+) or different block range"
  fi
else
  HINT="$NUM_ACCEPTED of $NUM_TARGETS accepted; consider re-running rejected/aborted targets with refined analysis"
fi

# ---------------------------------------------------------------------------
# Emit summary.json (stdout)
# ---------------------------------------------------------------------------
SUMMARY_JSON=$(jq -n \
  --arg session_id           "$SESSION_ID" \
  --argjson baseline_run_id  "$BASELINE_RUN_ID" \
  --argjson baseline_rerun_id "$BASELINE_RERUN_ID" \
  --argjson noise_floor_pct  "$NOISE_FLOOR_PCT" \
  --argjson experiments      "$EXPERIMENTS_JSON" \
  --arg next_targets_hint    "$HINT" \
  '{
    schema_version: 1,
    session_id: $session_id,
    baseline_run_id: $baseline_run_id,
    baseline_rerun_id: $baseline_rerun_id,
    noise_floor_pct: $noise_floor_pct,
    experiments: $experiments,
    next_targets_hint: $next_targets_hint
  }')

printf '%s\n' "$SUMMARY_JSON"

# ---------------------------------------------------------------------------
# Emit summary.md (derived view) alongside
# ---------------------------------------------------------------------------
{
  cat <<EOF
# Session $SESSION_ID

- Baseline run id: $BASELINE_RUN_ID
- Baseline rerun id: $BASELINE_RERUN_ID
- Noise floor: ${NOISE_FLOOR_PCT}%

## Experiments

| Target | Status | Improvement | Run ids | Notes |
| ------ | ------ | ----------- | ------- | ----- |
EOF
  jq -r '.experiments[] |
    [
      .target_id,
      .status,
      ((.improvement_pct // null) | if . == null then "—" else ((. * 100 | floor) / 100 | tostring) + "%" end),
      ((.run_ids // []) | map(tostring) | join(", ") | if . == "" then "—" else . end),
      (.reason // "")
    ] | "| " + join(" | ") + " |"' <<< "$SUMMARY_JSON"
  cat <<EOF

## Next steps

$HINT
EOF
} > "$SESSION_DIR/summary.md"
