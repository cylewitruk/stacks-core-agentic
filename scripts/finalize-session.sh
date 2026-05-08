#!/usr/bin/env bash
# Emit summary.json + summary.md for one optimization session.
#
# Per-target outcome dispatch (Phase 9c):
#   delivery_mode == consensus_issue   → status: "routed_to_issue"
#                                        (analyzer-only path; no optimizer ran)
#   delivery_mode == consensus_poc_pr  → if abort.md       → "aborted"
#                                        elif implementation.md → "poc_landed"
#                                                              (scoped tests
#                                                               passed; no
#                                                               benchmark)
#                                        else               → "aborted"
#   delivery_mode == normal_pr         → existing flow:
#                                          abort.md         → "aborted"
#                                          no run-ids       → "aborted"
#                                          else compare bench means:
#                                            improvement_pct >  noise_floor → "accepted"
#                                            improvement_pct < -noise_floor → "rejected" (regression)
#                                            else                            → "rejected" (within noise)
#
# Output schema: schemas/summary.schema.json (v2).
# Writes:
#   - JSON to stdout (the caller redirects to summary.json).
#   - summary.md alongside, as a derived human view.
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

# Lens dispositions propagate verbatim from optimization-targets.json. The
# field is REQUIRED on the merge schema (Phase 6+); fall back to empty array
# only as a defensive measure for older session data.
LENS_DISPOSITIONS=$(jq '.lens_dispositions // []' "$TARGETS")

# Build a {family_id → selection_lens} map from analyses/*/analysis.json so
# the summary.md 2D coverage table can locate each target's primary lens
# (= first contributor's lens) without re-reading per-target.
LENS_BY_FAMILY='{}'
shopt -s nullglob
for af in "$SESSION_DIR"/analyses/*/analysis.json; do
  fid=$(basename "$(dirname "$af")")
  lens=$(jq -r '.selection_lens // empty' "$af" 2>/dev/null || true)
  if [ -n "$lens" ]; then
    LENS_BY_FAMILY=$(jq --arg fid "$fid" --arg lens "$lens" \
      '. + {($fid): $lens}' <<< "$LENS_BY_FAMILY")
  fi
done
shopt -u nullglob

# Baseline = mean(baseline run, baseline rerun). Required only when at least
# one normal_pr target needs it; defer the strictness check until then so a
# session of consensus-only targets can finalize even if baseline metadata is
# stale or missing.
NEEDS_BASELINE=0
while IFS= read -r dm; do
  if [ "$dm" = "normal_pr" ]; then
    NEEDS_BASELINE=1; break
  fi
done < <(jq -r '.targets[].delivery_mode' "$TARGETS")

BASELINE_MEAN=""
if [ "$NEEDS_BASELINE" = "1" ]; then
  B1=$(total_duration_us "$BASELINE_RUN_ID")
  B2=$(total_duration_us "$BASELINE_RERUN_ID")
  if [ -z "$B1" ] || [ -z "$B2" ]; then
    echo "finalize-session: missing baseline summary (run_id=$BASELINE_RUN_ID, rerun_id=$BASELINE_RERUN_ID); required for normal_pr targets" >&2
    exit 1
  fi
  BASELINE_MEAN=$(printf '%s\n%s\n' "$B1" "$B2" | mean)
fi
NEG_NOISE=$(awk -v n="$NOISE_FLOOR_PCT" 'BEGIN { printf "%.6f", -n }')

# ---------------------------------------------------------------------------
# Per-experiment evaluation
# ---------------------------------------------------------------------------
EXPERIMENTS_JSON='[]'

while IFS=$'\t' read -r tid delivery_mode breakage_class; do
  EXP="$SESSION_DIR/experiments/$tid"

  # consensus_issue: optimizer was skipped entirely. The marker is the
  # routing receipt from run-optimizers.sh — its presence confirms the
  # routing happened. If it's missing, run-optimizers.sh either never ran
  # or failed before writing it, so we record `aborted` rather than falsely
  # claiming `routed_to_issue`.
  if [ "$delivery_mode" = "consensus_issue" ]; then
    if [ -s "$EXP/consensus-issue.md" ]; then
      EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --arg bc "$breakage_class" \
        '. + [
          {target_id: $tid, delivery_mode: $dm, status: "routed_to_issue"}
          + (if $bc == "" then {} else {breakage_class: $bc} end)
        ]' \
        <<< "$EXPERIMENTS_JSON")
    else
      EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --arg bc "$breakage_class" \
        '. + [
          {target_id: $tid, delivery_mode: $dm, status: "aborted",
           reason: "consensus-issue.md marker missing — run-optimizers.sh did not complete"}
          + (if $bc == "" then {} else {breakage_class: $bc} end)
        ]' \
        <<< "$EXPERIMENTS_JSON")
    fi
    continue
  fi

  # Aborted by subagent (any non-issue mode).
  if [ -f "$EXP/abort.md" ]; then
    REASON=$(head -c 4096 "$EXP/abort.md" | tr '\n' ' ' | awk '{$1=$1};1' | cut -c1-300)
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --arg bc "$breakage_class" --arg reason "$REASON" \
      '. + [
        {target_id: $tid, delivery_mode: $dm, status: "aborted", reason: $reason}
        + (if $bc == "" then {} else {breakage_class: $bc} end)
      ]' \
      <<< "$EXPERIMENTS_JSON")
    continue
  fi

  # consensus_poc_pr: implementation.md presence is the success gate (no
  # bench by design — the change is consensus-breaking). No implementation
  # AND no abort means the optimizer never produced output (build crash,
  # timeout, etc.) — record as aborted.
  if [ "$delivery_mode" = "consensus_poc_pr" ]; then
    if [ -s "$EXP/implementation.md" ]; then
      EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --arg bc "$breakage_class" \
        '. + [
          {target_id: $tid, delivery_mode: $dm, status: "poc_landed"}
          + (if $bc == "" then {} else {breakage_class: $bc} end)
        ]' \
        <<< "$EXPERIMENTS_JSON")
    else
      EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --arg bc "$breakage_class" \
        '. + [
          {target_id: $tid, delivery_mode: $dm, status: "aborted",
           reason: "no implementation.md emitted (PoC-mode optimizer produced no output)"}
          + (if $bc == "" then {} else {breakage_class: $bc} end)
        ]' \
        <<< "$EXPERIMENTS_JSON")
    fi
    continue
  fi

  # normal_pr: existing bench-eval flow.
  if [ ! -s "$EXP/run-ids" ]; then
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" \
      '. + [{target_id: $tid, delivery_mode: $dm, status: "aborted",
             reason: "no benchmark runs recorded"}]' \
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
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --argjson run_ids "$RUN_IDS_ARR" \
      '. + [{target_id: $tid, delivery_mode: $dm, status: "aborted",
             reason: "all benchmark runs missing summaries", run_ids: $run_ids}]' \
      <<< "$EXPERIMENTS_JSON")
    continue
  fi

  EXP_MEAN=$(printf '%s\n' "${EXP_DURATIONS[@]}" | mean)
  IMPROVEMENT_PCT=$(awk -v b="$BASELINE_MEAN" -v e="$EXP_MEAN" \
    'BEGIN { if (b == 0) printf "0"; else printf "%.4f", (b - e) / b * 100 }')

  if float_gt "$IMPROVEMENT_PCT" "$NOISE_FLOOR_PCT"; then
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --argjson run_ids "$RUN_IDS_ARR" \
      --argjson improvement "$IMPROVEMENT_PCT" \
      '. + [{target_id: $tid, delivery_mode: $dm, status: "accepted",
             run_ids: $run_ids, improvement_pct: $improvement}]' \
      <<< "$EXPERIMENTS_JSON")
  elif float_lt "$IMPROVEMENT_PCT" "$NEG_NOISE"; then
    REASON=$(awk -v p="$IMPROVEMENT_PCT" 'BEGIN { printf "regression: %.2f%% slower than baseline", -p }')
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --argjson run_ids "$RUN_IDS_ARR" \
      --argjson improvement "$IMPROVEMENT_PCT" --arg reason "$REASON" \
      '. + [{target_id: $tid, delivery_mode: $dm, status: "rejected",
             run_ids: $run_ids, improvement_pct: $improvement, reason: $reason}]' \
      <<< "$EXPERIMENTS_JSON")
  else
    REASON=$(awk -v p="$IMPROVEMENT_PCT" -v n="$NOISE_FLOOR_PCT" \
      'BEGIN { printf "within noise floor (%.2f%% improvement vs %.2f%% noise)", p, n }')
    EXPERIMENTS_JSON=$(jq --arg tid "$tid" --arg dm "$delivery_mode" --argjson run_ids "$RUN_IDS_ARR" \
      --argjson improvement "$IMPROVEMENT_PCT" --arg reason "$REASON" \
      '. + [{target_id: $tid, delivery_mode: $dm, status: "rejected",
             run_ids: $run_ids, improvement_pct: $improvement, reason: $reason}]' \
      <<< "$EXPERIMENTS_JSON")
  fi
done < <(jq -r '.targets[] | [.id, .delivery_mode, (.breakage_class // "")] | @tsv' "$TARGETS")

# ---------------------------------------------------------------------------
# Outcome counts (3×N matrix, schema-shaped)
# ---------------------------------------------------------------------------
OUTCOME_COUNTS=$(jq '
  {
    normal_pr: {
      accepted: ([.[] | select(.delivery_mode=="normal_pr"        and .status=="accepted")] | length),
      rejected: ([.[] | select(.delivery_mode=="normal_pr"        and .status=="rejected")] | length),
      aborted:  ([.[] | select(.delivery_mode=="normal_pr"        and .status=="aborted")]  | length)
    },
    consensus_poc_pr: {
      poc_landed: ([.[] | select(.delivery_mode=="consensus_poc_pr" and .status=="poc_landed")] | length),
      aborted:    ([.[] | select(.delivery_mode=="consensus_poc_pr" and .status=="aborted")]    | length)
    },
    consensus_issue: {
      routed_to_issue: ([.[] | select(.delivery_mode=="consensus_issue" and .status=="routed_to_issue")] | length),
      aborted:         ([.[] | select(.delivery_mode=="consensus_issue" and .status=="aborted")]         | length)
    }
  }' <<< "$EXPERIMENTS_JSON")

# ---------------------------------------------------------------------------
# next_targets_hint
# ---------------------------------------------------------------------------
NUM_TARGETS=$(jq 'length' <<< "$EXPERIMENTS_JSON")
NUM_ACCEPTED=$(jq '[.[] | select(.status=="accepted")] | length' <<< "$EXPERIMENTS_JSON")
NUM_POC=$(jq '[.[] | select(.status=="poc_landed")] | length' <<< "$EXPERIMENTS_JSON")
NUM_ISSUE=$(jq '[.[] | select(.status=="routed_to_issue")] | length' <<< "$EXPERIMENTS_JSON")
NUM_ABORTED=$(jq '[.[] | select(.status=="aborted")] | length' <<< "$EXPERIMENTS_JSON")
NUM_REGRESSIONS=$(jq '[.[] | select(.status=="rejected" and (.reason // "" | startswith("regression")))] | length' \
  <<< "$EXPERIMENTS_JSON")

HINT=""
if [ "$NUM_TARGETS" -eq 0 ]; then
  HINT="zero targets reached benchmarking; check analyses/*/analysis.json"
elif [ "$NUM_ACCEPTED" -eq 0 ] && [ "$NUM_POC" -eq 0 ] && [ "$NUM_ISSUE" -eq 0 ]; then
  if [ "$NUM_ABORTED" -eq "$NUM_TARGETS" ]; then
    HINT="all experiments aborted before benchmarking; review experiments/*/abort.md and subagent-stderr.log"
  elif [ "$NUM_REGRESSIONS" -gt 0 ]; then
    HINT="rejected: $NUM_REGRESSIONS regression(s); rest within noise. Try smaller-scope changes or tighter targets."
  else
    HINT="all rejected within noise floor; try wider profiler view (--profiler-hot 100+) or different block range"
  fi
else
  HINT="$NUM_ACCEPTED PR(s) + $NUM_POC PoC PR(s) + $NUM_ISSUE issue(s) of $NUM_TARGETS target(s); review and re-run rejected/aborted with refined analyses"
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
  --argjson outcome_counts   "$OUTCOME_COUNTS" \
  --argjson lens_dispositions "$LENS_DISPOSITIONS" \
  --arg next_targets_hint    "$HINT" \
  '{
    schema_version: 2,
    session_id: $session_id,
    baseline_run_id: $baseline_run_id,
    baseline_rerun_id: $baseline_rerun_id,
    noise_floor_pct: $noise_floor_pct,
    experiments: $experiments,
    outcome_counts: $outcome_counts,
    lens_dispositions: $lens_dispositions,
    next_targets_hint: $next_targets_hint
  }')

printf '%s\n' "$SUMMARY_JSON"

# ---------------------------------------------------------------------------
# Emit summary.md (derived view) alongside
# ---------------------------------------------------------------------------

# 2D coverage matrix: bucket × selection_lens. Built from optimization-targets
# (bucket per merged target) joined to the analyses dir (selection_lens per
# first contributor). Produces a row per bucket and a count per lens cell;
# missing combinations render as "-" in the markdown table.
COVERAGE_JSON=$(jq --argjson lens_by_fid "$LENS_BY_FAMILY" '
  reduce .targets[] as $t ({};
    (
      if ($t.merged_from | length) > 0 then
        ($lens_by_fid[$t.merged_from[0].family_id] // "unknown")
      else
        "unknown"
      end
    ) as $lens
    | .[$t.bucket] = ((.[$t.bucket] // {}) | .[$lens] = ((.[$t.bucket][$lens] // 0) + 1))
  )' "$TARGETS")

# Render the count for a (bucket, lens) cell, or "-" if absent.
cell() {
  local matrix="$1" bucket="$2" lens="$3"
  local val
  val=$(jq -r --arg b "$bucket" --arg l "$lens" \
    '.[$b][$l] // empty' <<< "$matrix")
  printf '%s' "${val:--}"
}

{
  cat <<EOF
# Session $SESSION_ID

- Baseline run id: $BASELINE_RUN_ID
- Baseline rerun id: $BASELINE_RERUN_ID
- Noise floor: ${NOISE_FLOOR_PCT}%

## Outcomes

| Delivery mode      | Counts                                                          |
| ------------------ | --------------------------------------------------------------- |
| normal_pr          | $(jq -r '.normal_pr | "accepted=\(.accepted), rejected=\(.rejected), aborted=\(.aborted)"' <<< "$OUTCOME_COUNTS") |
| consensus_poc_pr   | $(jq -r '.consensus_poc_pr | "poc_landed=\(.poc_landed), aborted=\(.aborted)"' <<< "$OUTCOME_COUNTS") |
| consensus_issue    | $(jq -r '.consensus_issue  | "routed_to_issue=\(.routed_to_issue), aborted=\(.aborted)"' <<< "$OUTCOME_COUNTS") |

## Coverage matrix (bucket × selection_lens)

|                  | tx_latency | tenure_throughput | commit_time |
| ---------------- | ---------- | ----------------- | ----------- |
| block_processing | $(cell "$COVERAGE_JSON" "block_processing" "tx_latency") | $(cell "$COVERAGE_JSON" "block_processing" "tenure_throughput") | $(cell "$COVERAGE_JSON" "block_processing" "commit_time") |
| block_commit     | $(cell "$COVERAGE_JSON" "block_commit" "tx_latency") | $(cell "$COVERAGE_JSON" "block_commit" "tenure_throughput") | $(cell "$COVERAGE_JSON" "block_commit" "commit_time") |

> Cell counts use each merged target's primary lens (= first contributor's
> selection_lens). Targets with cross-lens convergence are counted once; see
> optimization-targets.json contributor_differences for cross-lens cases.

## Experiments

| Target | Delivery mode | Status | Improvement | Run ids | Notes |
| ------ | ------------- | ------ | ----------- | ------- | ----- |
EOF
  jq -r '.experiments[] |
    [
      .target_id,
      .delivery_mode,
      .status,
      ((.improvement_pct // null) | if . == null then "—" else ((. * 100 | floor) / 100 | tostring) + "%" end),
      ((.run_ids // []) | map(tostring) | join(", ") | if . == "" then "—" else . end),
      ((.reason // "") +
        (if .breakage_class then (if (.reason // "") == "" then "" else "; " end) + "breakage_class=\(.breakage_class)" else "" end))
    ] | "| " + join(" | ") + " |"' <<< "$SUMMARY_JSON"

  # "Real hotspot, no fix found" — pulled from lens_dispositions where
  # status == not_actionable. Print only when at least one such entry exists,
  # otherwise the section is silently omitted.
  N_NOT_ACTIONABLE=$(jq '[.[] | select(.status == "not_actionable")] | length' <<< "$LENS_DISPOSITIONS")
  if [ "$N_NOT_ACTIONABLE" -gt 0 ]; then
    cat <<EOF

## Real hotspots without an actionable fix

The analyzer drilled into the families below, confirmed the signal at code
level, and could not find a structural handle. The reasons reflect code-level
constraints (consensus rules, inherent CPU cost, already-cached paths). These
are first-class artifacts — surface them to whoever decides what to optimize
next.

| Family | Lens | Reason |
| ------ | ---- | ------ |
EOF
    jq -r '.[] | select(.status == "not_actionable") |
      "| \(.family_id) | \(.lens) | \(.reason // "(no reason)") |"' \
      <<< "$LENS_DISPOSITIONS"
  fi

  cat <<EOF

## Next steps

$HINT
EOF
} > "$SESSION_DIR/summary.md"
