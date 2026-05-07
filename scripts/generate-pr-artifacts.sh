#!/usr/bin/env bash
# Generate PR title/body artifacts for each accepted target in a session.
#
# Inputs (in SESSION_DIR):
#   summary.json
#   optimization-targets.json
#   experiments/<id>/implementation.md
#
# Outputs (per accepted target):
#   experiments/<id>/pr-writer-prompt.md
#   experiments/<id>/pr-writer-events.jsonl
#   experiments/<id>/pr-writer-stderr.log
#   experiments/<id>/pr-writer-final-message.md
#   experiments/<id>/pr-title.txt
#   experiments/<id>/pr-body.md
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"
assert_required_tools
assert_codex_compatible
assert_codex_writable

SUMMARY="$OPT_SESSION_DIR/summary.json"
TARGETS="$OPT_SESSION_DIR/optimization-targets.json"
[ -s "$SUMMARY" ] || { echo "generate-pr-artifacts: missing summary.json" >&2; exit 2; }
[ -s "$TARGETS" ] || { echo "generate-pr-artifacts: missing optimization-targets.json" >&2; exit 2; }

mapfile -t TARGET_IDS < <(jq -r '.experiments[] | select(.status=="accepted") | .target_id' "$SUMMARY")
if [ "${#TARGET_IDS[@]}" -eq 0 ]; then
  echo "No accepted targets; PR artifact generation is a no-op."
  exit 0
fi

run_pr_writer() {
  local TARGET_ID="$1"
  local OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  local WORKTREE_DIR="$WORKTREES/$TARGET_ID"
  local TARGET_JSON EXPERIMENT_JSON

  [ -s "$OUTPUT_DIR/implementation.md" ] || {
    echo "generate-pr-artifacts: missing implementation.md for $TARGET_ID" >&2
    return 1
  }

  TARGET_JSON=$(jq --arg id "$TARGET_ID" '.targets[] | select(.id==$id)' "$TARGETS")
  EXPERIMENT_JSON=$(jq --arg id "$TARGET_ID" '.experiments[] | select(.target_id==$id)' "$SUMMARY")

  export TARGET_ID OUTPUT_DIR WORKTREE_DIR TARGET_JSON EXPERIMENT_JSON

  rm -f \
    "$OUTPUT_DIR/pr-title.txt" \
    "$OUTPUT_DIR/pr-body.md" \
    "$OUTPUT_DIR/pr-writer-prompt.md" \
    "$OUTPUT_DIR/pr-writer-events.jsonl" \
    "$OUTPUT_DIR/pr-writer-stderr.log" \
    "$OUTPUT_DIR/pr-writer-final-message.md"

  # shellcheck disable=SC2016
  envsubst '$OPT_SESSION_ID $TARGET_ID $OUTPUT_DIR $WORKTREE_DIR $TARGET_JSON $EXPERIMENT_JSON' \
    < "$PROMPTS_DIR/pr-writer.md" \
    > "$OUTPUT_DIR/pr-writer-prompt.md"

  local -a CODEX_TOP_LEVEL_ARGS CODEX_EXEC_ARGS
  mapfile -t CODEX_TOP_LEVEL_ARGS < <(codex_top_level_args "${CODEX_MODEL:-gpt-5.5}")
  mapfile -t CODEX_EXEC_ARGS < <(codex_exec_args)

  run_with_timeout "${CODEX_EXEC_TIMEOUT_SEC:-3600}" \
    codex \
      "${CODEX_TOP_LEVEL_ARGS[@]}" \
      exec \
      --skip-git-repo-check \
      --cd "$OUTPUT_DIR" \
      --add-dir "$FRAMEWORK_ROOT" \
      --add-dir "$WORKTREE_DIR" \
      "${CODEX_EXEC_ARGS[@]}" \
      --output-last-message "$OUTPUT_DIR/pr-writer-final-message.md" \
      "$(cat "$OUTPUT_DIR/pr-writer-prompt.md")" \
    > "$OUTPUT_DIR/pr-writer-events.jsonl" \
    2> "$OUTPUT_DIR/pr-writer-stderr.log"

  [ -s "$OUTPUT_DIR/pr-title.txt" ] || {
    echo "generate-pr-artifacts: pr-title.txt missing for $TARGET_ID" >&2
    return 1
  }
  [ -s "$OUTPUT_DIR/pr-body.md" ] || {
    echo "generate-pr-artifacts: pr-body.md missing for $TARGET_ID" >&2
    return 1
  }

  # Validate that all required sections are present. The pr-writer prompt
  # requires: ## Summary, ## What changed, ## Benchmark result, ## Validation.
  local missing_sections=()
  for section in 'Summary' 'What changed' 'Benchmark result' 'Validation'; do
    grep -qiE "^##[[:space:]]+${section}[[:space:]]*$" "$OUTPUT_DIR/pr-body.md" \
      || missing_sections+=("$section")
  done
  if [ "${#missing_sections[@]}" -gt 0 ]; then
    echo "generate-pr-artifacts: pr-body.md for $TARGET_ID is missing required sections: ${missing_sections[*]}" >&2
    return 1
  fi
}

for TARGET_ID in "${TARGET_IDS[@]}"; do
  run_pr_writer "$TARGET_ID"
done

echo "Generated PR artifacts for ${#TARGET_IDS[@]} accepted target(s)."
