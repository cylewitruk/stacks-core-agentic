#!/usr/bin/env bash
# Assemble optimization-targets.json from candidates.json + every analyses/<id>/analysis.json
# in the session dir. Drops rejected analyses, ranks accepted ones in candidate order.
# Usage: assemble-targets.sh [SESSION_DIR]
set -euo pipefail
SESSION_DIR="${1:-${OPT_SESSION_DIR:?need OPT_SESSION_DIR or arg}}"

CANDIDATES="$SESSION_DIR/candidates.json"
ANALYSES_DIR="$SESSION_DIR/analyses"

[ -s "$CANDIDATES" ]      || { echo "missing $CANDIDATES" >&2; exit 1; }
[ -d "$ANALYSES_DIR" ]    || { echo "missing $ANALYSES_DIR" >&2; exit 1; }

# Pull session-level fields from candidates.json so the assembled file is
# self-contained.
SESSION_ID=$(jq -r '.session_id' "$CANDIDATES")
BASELINE_RUN_ID=$(jq -r '.baseline_run_id' "$CANDIDATES")
BASELINE_RERUN_ID=$(jq -r '.baseline_rerun_id' "$CANDIDATES")
NOISE_FLOOR_PCT=$(jq -r '.noise_floor_pct' "$CANDIDATES")

# Glob every analysis.json. If no analyses exist (empty candidate set), emit
# an empty targets list instead of erroring.
shopt -s nullglob
ANALYSIS_FILES=( "$ANALYSES_DIR"/*/analysis.json )
shopt -u nullglob

if [ "${#ANALYSIS_FILES[@]}" -eq 0 ]; then
  jq -n \
    --arg session_id "$SESSION_ID" \
    --argjson baseline_run_id "$BASELINE_RUN_ID" \
    --argjson baseline_rerun_id "$BASELINE_RERUN_ID" \
    --argjson noise_floor_pct "$NOISE_FLOOR_PCT" \
    '{
      schema_version: 1,
      session_id: $session_id,
      baseline_run_id: $baseline_run_id,
      baseline_rerun_id: $baseline_rerun_id,
      noise_floor_pct: $noise_floor_pct,
      targets: []
    }'
  exit 0
fi

# slurp all analyses → keep only accepted → preserve candidate order from
# candidates.json → assign rank starting at 1, rename candidate_id → id.
jq -s \
  --slurpfile candidates "$CANDIDATES" \
  --arg session_id "$SESSION_ID" \
  --argjson baseline_run_id "$BASELINE_RUN_ID" \
  --argjson baseline_rerun_id "$BASELINE_RERUN_ID" \
  --argjson noise_floor_pct "$NOISE_FLOOR_PCT" \
  '
  ($candidates[0].candidates | map(.id)) as $order
  | (map(select(.status == "accepted"))
     | map({key: .candidate_id, value: .})
     | from_entries) as $by_id
  | {
      schema_version: 1,
      session_id: $session_id,
      baseline_run_id: $baseline_run_id,
      baseline_rerun_id: $baseline_rerun_id,
      noise_floor_pct: $noise_floor_pct,
      targets: (
        $order
        | map($by_id[.])
        | map(select(. != null))
        | to_entries
        | map(
            .value as $a
            | $a
            + { id: $a.candidate_id, rank: (.key + 1) }
            | del(.candidate_id, .status)
          )
      )
  }' \
  "${ANALYSIS_FILES[@]}"
