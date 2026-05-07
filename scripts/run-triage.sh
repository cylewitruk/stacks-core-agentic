#!/usr/bin/env bash
# Phase 1: triage agent. Reads baseline-* artifacts; emits candidates.json.
#
# Inputs (in SESSION_DIR):
#   baseline-run-id, baseline-rerun-id     (from Phase 0)
#   baseline-profiler-hotspots.json
#   bench-list.json
#
# Outputs (in SESSION_DIR):
#   triage-prompt.md            (rendered prompt)
#   triage-events.jsonl
#   triage-stderr.log
#   triage-final-message.md
#   triage-conversation-id
#   candidates.json             (the agent's primary output)
#   candidates.md               (human view, derived)
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"
assert_required_tools
assert_codex_compatible
assert_codex_writable

BASELINE_RUN_ID="$(cat "$OPT_SESSION_DIR/baseline-run-id")"
BASELINE_RERUN_ID="$(cat "$OPT_SESSION_DIR/baseline-rerun-id")"
export BASELINE_RUN_ID BASELINE_RERUN_ID

PRECOMPUTED_NOISE_FLOOR_PCT=""
if [ -f "$OPT_SESSION_DIR/baseline-noise-floor-pct" ]; then
  PRECOMPUTED_NOISE_FLOOR_PCT="$(cat "$OPT_SESSION_DIR/baseline-noise-floor-pct")"
fi
export PRECOMPUTED_NOISE_FLOOR_PCT

# envsubst's SHELL-FORMAT arg is a literal list of $VAR tokens; single quotes
# are required so the shell doesn't expand them before envsubst sees them.
# shellcheck disable=SC2016
envsubst '$OPT_SESSION_ID $OPT_SESSION_DIR $STACKS_BENCH_DATA_DIR $BASE $BASELINE_RUN_ID $BASELINE_RERUN_ID $PRECOMPUTED_NOISE_FLOOR_PCT $NON_TARGETS_PATH $CANDIDATES_SCHEMA_PATH $QUERIES_DIR' \
  < "$PROMPTS_DIR/triage.md" \
  > "$OPT_SESSION_DIR/triage-prompt.md"

# codex CLI 0.128.0: --ask-for-approval and -m are TOP-LEVEL flags (before
# `exec`). --skip-git-repo-check is needed because the cwd here is a session
# results dir, not a git repo. --search dropped: triage doesn't need web
# search and removing it cuts a network dependency / source of nondeterminism.
# Add the framework root plus any env-overridden data/code roots explicitly so
# the prompt can still read them even if the operator moves them elsewhere.
mapfile -t CODEX_SANDBOX_ARGS < <(codex_sandbox_args)
mapfile -t CODEX_EXEC_ARGS < <(codex_exec_args)

run_with_timeout "${CODEX_EXEC_TIMEOUT_SEC:-3600}" \
  codex \
    -m "${CODEX_MODEL:-gpt-5.5}" \
    "${CODEX_SANDBOX_ARGS[@]}" \
  exec \
    --skip-git-repo-check \
    --cd "$OPT_SESSION_DIR" \
    --add-dir "$FRAMEWORK_ROOT" \
    --add-dir "$STACKS_BENCH_DATA_DIR" \
    --add-dir "$BASE" \
    "${CODEX_EXEC_ARGS[@]}" \
    --output-last-message "$OPT_SESSION_DIR/triage-final-message.md" \
    "$(cat "$OPT_SESSION_DIR/triage-prompt.md")" \
  > "$OPT_SESSION_DIR/triage-events.jsonl" \
  2> "$OPT_SESSION_DIR/triage-stderr.log"

capture_codex_conversation_id "$OPT_SESSION_DIR/triage-events.jsonl" \
  > "$OPT_SESSION_DIR/triage-conversation-id"

if [ ! -s "$OPT_SESSION_DIR/candidates.json" ]; then
  echo "Triage did not emit candidates.json. See triage-final-message.md." >&2
  exit 2
fi

N=$(jq '.candidates | length' "$OPT_SESSION_DIR/candidates.json")
echo "candidates: $N"
if [ "$N" -eq 0 ]; then
  echo "Triage returned zero candidates. Downstream phases will no-op." >&2
fi
