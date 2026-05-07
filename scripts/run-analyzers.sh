#!/usr/bin/env bash
# Phase 1.5: analyzer fan-out. One Codex subagent per candidate, in parallel.
#
# Inputs (in SESSION_DIR):
#   candidates.json
#
# Outputs (in SESSION_DIR/analyses/<candidate-id>/):
#   analyzer-prompt.md
#   analyzer-events.jsonl
#   analyzer-stderr.log
#   analyzer-final-message.md
#   analyzer-conversation-id
#   analysis.json    (status: accepted | rejected)
#   analysis.md
#
# Knobs:
#   STACKS_BENCH_PARALLEL_ANALYZERS  cap on concurrent analyzers
#                                    (default: one per candidate)
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

mapfile -t CANDIDATE_IDS < <(jq -r '.candidates[].id' "$OPT_SESSION_DIR/candidates.json")
if [ "${#CANDIDATE_IDS[@]}" -eq 0 ]; then
  echo "No candidates to analyze; phase is a no-op."
  exit 0
fi

ANALYSES_DIR="$OPT_SESSION_DIR/analyses"
mkdir -p "$ANALYSES_DIR"

PARALLEL="${STACKS_BENCH_PARALLEL_ANALYZERS:-${#CANDIDATE_IDS[@]}}"

run_analyzer() {
  local CAND_ID="$1"
  local OUT="$ANALYSES_DIR/$CAND_ID"
  mkdir -p "$OUT"

  local CANDIDATE_JSON
  CANDIDATE_JSON=$(jq --arg id "$CAND_ID" \
    '.candidates[] | select(.id==$id)' \
    "$OPT_SESSION_DIR/candidates.json")

  export CAND_ID OUTPUT_DIR="$OUT" CANDIDATE_JSON

  # envsubst's SHELL-FORMAT arg requires literal $VAR tokens; single quotes
  # prevent the shell from expanding them before envsubst sees them.
  # shellcheck disable=SC2016
  envsubst '$CAND_ID $OUTPUT_DIR $BASE $CANDIDATE_JSON $NON_TARGETS_PATH $ANALYSIS_SCHEMA_PATH' \
    < "$PROMPTS_DIR/analyzer.md" \
    > "$OUT/analyzer-prompt.md"

  # codex CLI 0.128.0: --ask-for-approval and -m are TOP-LEVEL flags (before
  # `exec`). --skip-git-repo-check is needed because the cwd here is the
  # analysis output dir, not a git repo. --search dropped: analysis is a
  # codebase-reading task, not a web-search task. The analyzer's cwd is its
  # own output dir (writable); $BASE is added for read access to the codebase
  # and the prompt forbids modifying it.
  run_with_timeout "${CODEX_EXEC_TIMEOUT_SEC:-3600}" \
    codex \
    -m "${CODEX_MODEL:-gpt-5.5}" \
    --ask-for-approval never \
    exec \
      --skip-git-repo-check \
      --cd "$OUT" --add-dir "$FRAMEWORK_ROOT" --add-dir "$BASE" \
      --sandbox workspace-write --json \
      --output-last-message "$OUT/analyzer-final-message.md" \
      "$(cat "$OUT/analyzer-prompt.md")" \
    > "$OUT/analyzer-events.jsonl" \
    2> "$OUT/analyzer-stderr.log"

  capture_codex_conversation_id "$OUT/analyzer-events.jsonl" \
    > "$OUT/analyzer-conversation-id"
}
export -f run_analyzer capture_codex_conversation_id run_with_timeout
export ANALYSES_DIR OPT_SESSION_DIR BASE

# xargs -P fans out, preserving streaming logs per subagent.
printf '%s\n' "${CANDIDATE_IDS[@]}" \
  | xargs -n1 -P "$PARALLEL" -I{} bash -c 'run_analyzer "$@"' _ {}

# Tally accepted / rejected.
ACCEPTED=$(find "$ANALYSES_DIR" -name analysis.json \
  -exec jq -r 'select(.status=="accepted") | .candidate_id' {} \; 2>/dev/null | wc -l | tr -d ' ')
REJECTED=$(find "$ANALYSES_DIR" -name analysis.json \
  -exec jq -r 'select(.status=="rejected") | .candidate_id' {} \; 2>/dev/null | wc -l | tr -d ' ')
echo "analyses: ${ACCEPTED} accepted, ${REJECTED} rejected (of ${#CANDIDATE_IDS[@]} candidates)"
