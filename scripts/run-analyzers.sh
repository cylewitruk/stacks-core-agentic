#!/usr/bin/env bash
# Phase 1.5: analyzer fan-out. One Codex subagent per family candidate, in
# parallel. Each analyzer receives the family payload from candidates.json
# (kind, representative_ids, suspected_spans hints, global_materiality) and
# commits the target_span + fix_signature for its family.
#
# Inputs (in SESSION_DIR):
#   candidates.json                  (v2 — family-shaped)
#   span_recurrence.csv (optional)   (cached by triage; analyzer can grep it)
#
# Outputs (in SESSION_DIR/analyses/<family-id>/):
#   analyzer-prompt.md
#   analyzer-events.jsonl
#   analyzer-stderr.log
#   analyzer-final-message.md
#   analyzer-conversation-id
#   analysis.json    (schema v2; status: accepted | rejected)
#   analysis.md
#
# Knobs:
#   STACKS_BENCH_PARALLEL_ANALYZERS  cap on concurrent analyzers
#                                    (default: one per family)
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"
assert_required_tools
assert_codex_compatible
assert_codex_writable

if [ ! -s "$OPT_SESSION_DIR/candidates.json" ]; then
  echo "run-analyzers: missing candidates.json (run run-triage.sh first)" >&2
  exit 2
fi

# Sanity check: candidates must be v2.
v=$(jq -r '.schema_version // empty' "$OPT_SESSION_DIR/candidates.json")
if [ "$v" != "2" ]; then
  echo "run-analyzers: candidates.json schema_version=$v (expected 2; v1 candidates are not supported in this pipeline)" >&2
  exit 2
fi

# Pull session-level fields the analyzer prompt needs.
BASELINE_RUN_ID=$(jq -r '.baseline_run_id' "$OPT_SESSION_DIR/candidates.json")
export BASELINE_RUN_ID

mapfile -t FAMILY_IDS < <(jq -r '.candidates[].id' "$OPT_SESSION_DIR/candidates.json")
if [ "${#FAMILY_IDS[@]}" -eq 0 ]; then
  echo "No families to analyze; phase is a no-op."
  exit 0
fi

ANALYSES_DIR="$OPT_SESSION_DIR/analyses"
mkdir -p "$ANALYSES_DIR"

PARALLEL="${STACKS_BENCH_PARALLEL_ANALYZERS:-${#FAMILY_IDS[@]}}"

run_analyzer() {
  local FAMILY_ID="$1"
  local OUT="$ANALYSES_DIR/$FAMILY_ID"
  mkdir -p "$OUT"

  # Slice the single family object out of candidates.json so the analyzer
  # never has to scan the full session-level file.
  local FAMILY_JSON
  FAMILY_JSON=$(jq --arg id "$FAMILY_ID" \
    '.candidates[] | select(.id==$id)' \
    "$OPT_SESSION_DIR/candidates.json")

  export FAMILY_ID OUTPUT_DIR="$OUT" FAMILY_JSON

  # envsubst's SHELL-FORMAT arg requires literal $VAR tokens; single quotes
  # prevent the shell from expanding them before envsubst sees them.
  # shellcheck disable=SC2016
  envsubst '$FAMILY_ID $OUTPUT_DIR $OPT_SESSION_DIR $BASE $FAMILY_JSON $STACKS_BENCH_DATA_DIR $QUERIES_DIR $BASELINE_RUN_ID $NON_TARGETS_PATH $ANALYSIS_SCHEMA_PATH' \
    < "$PROMPTS_DIR/analyzer.md" \
    > "$OUT/analyzer-prompt.md"

  # codex CLI 0.128.0: --ask-for-approval and -m are TOP-LEVEL flags. The
  # analyzer's cwd is its own output dir (writable); $BASE is added so it can
  # read source code (the prompt forbids modifying it). $FRAMEWORK_ROOT covers
  # the queries dir + the session dir for span_recurrence.csv reads.
  # $STACKS_BENCH_DATA_DIR is added so the analyzer can run trace queries.
  local -a CODEX_TOP_LEVEL_ARGS CODEX_EXEC_ARGS
  mapfile -t CODEX_TOP_LEVEL_ARGS < <(codex_top_level_args "${CODEX_MODEL:-gpt-5.5}")
  mapfile -t CODEX_EXEC_ARGS < <(codex_exec_args)

  run_with_timeout "${CODEX_EXEC_TIMEOUT_SEC:-3600}" \
    codex \
    "${CODEX_TOP_LEVEL_ARGS[@]}" \
    exec \
      --skip-git-repo-check \
      --cd "$OUT" \
      --add-dir "$FRAMEWORK_ROOT" \
      --add-dir "$BASE" \
      --add-dir "$STACKS_BENCH_DATA_DIR" \
      "${CODEX_EXEC_ARGS[@]}" \
      --output-last-message "$OUT/analyzer-final-message.md" \
      "$(cat "$OUT/analyzer-prompt.md")" \
    > "$OUT/analyzer-events.jsonl" \
    2> "$OUT/analyzer-stderr.log"

  capture_codex_conversation_id "$OUT/analyzer-events.jsonl" \
    > "$OUT/analyzer-conversation-id"
}
export -f run_analyzer capture_codex_conversation_id run_with_timeout
export -f codex_top_level_args codex_exec_args
export ANALYSES_DIR OPT_SESSION_DIR BASE STACKS_BENCH_DATA_DIR QUERIES_DIR BASELINE_RUN_ID

# xargs -P fans out, preserving streaming logs per subagent.
printf '%s\n' "${FAMILY_IDS[@]}" \
  | xargs -n1 -P "$PARALLEL" -I{} bash -c 'run_analyzer "$@"' _ {}

# Tally accepted / rejected. Field name is family_id in v2.
ACCEPTED=$(find "$ANALYSES_DIR" -name analysis.json \
  -exec jq -r 'select(.status=="accepted") | .family_id' {} \; 2>/dev/null | wc -l | tr -d ' ')
REJECTED=$(find "$ANALYSES_DIR" -name analysis.json \
  -exec jq -r 'select(.status=="rejected") | .family_id' {} \; 2>/dev/null | wc -l | tr -d ' ')
echo "analyses: ${ACCEPTED} accepted, ${REJECTED} rejected (of ${#FAMILY_IDS[@]} families)"
