#!/usr/bin/env bash
# Generate publish artifacts (PR or issue) for each shippable target in a
# session. Dispatches by delivery_mode read from optimization-targets.json:
#
#   normal_pr        — pr-writer.md → pr-title.txt + pr-body.md
#                      Gate: summary.experiments[target_id].status == "accepted"
#                      (the bench measured a real improvement above noise floor)
#   consensus_poc_pr — pr-writer.md → pr-title.txt + pr-body.md
#                      Gate: implementation.md exists (PoC tests passed; bench
#                      didn't run because the change is consensus-breaking)
#   consensus_issue  — issue-writer.md → issue-title.txt + issue-body.md
#                      Gate: consensus-issue.md exists (always — written by
#                      run-optimizers.sh as the no-op marker for this routing)
#
# Inputs (in SESSION_DIR):
#   summary.json
#   optimization-targets.json
#   experiments/<id>/{implementation.md|consensus-issue.md}
#
# Outputs (per shippable target):
#   experiments/<id>/{pr,issue}-writer-prompt.md
#   experiments/<id>/{pr,issue}-writer-events.jsonl
#   experiments/<id>/{pr,issue}-writer-stderr.log
#   experiments/<id>/{pr,issue}-writer-final-message.md
#   experiments/<id>/pr-title.txt + pr-body.md           (PR modes)
#   experiments/<id>/issue-title.txt + issue-body.md     (issue mode)
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"
assert_required_tools
assert_codex_compatible
assert_codex_writable

SUMMARY="$OPT_SESSION_DIR/summary.json"
TARGETS="$OPT_SESSION_DIR/optimization-targets.json"
[ -s "$TARGETS" ] || { echo "generate-pr-artifacts: missing optimization-targets.json" >&2; exit 2; }
# summary.json is REQUIRED only for normal_pr targets (where bench acceptance
# is the gate). consensus_poc_pr and consensus_issue targets route purely on
# delivery_mode + per-target marker files and don't need it. So a session
# composed entirely of consensus-routed targets is a valid input here, and
# the script must not exit at startup.
HAS_SUMMARY=0
[ -s "$SUMMARY" ] && HAS_SUMMARY=1

# Returns "ship" or "skip:<reason>" for the given target id, based on its
# delivery_mode and the per-mode gate.
target_publish_decision() {
  local TARGET_ID="$1"
  local OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  local DELIVERY_MODE
  DELIVERY_MODE=$(jq -r --arg id "$TARGET_ID" \
    '.targets[] | select(.id==$id) | .delivery_mode' "$TARGETS")

  case "$DELIVERY_MODE" in
    normal_pr)
      if [ "$HAS_SUMMARY" = "0" ]; then
        echo "skip:no-summary-file"
        return
      fi
      local STATUS
      STATUS=$(jq -r --arg id "$TARGET_ID" \
        '.experiments[] | select(.target_id==$id) | .status' "$SUMMARY")
      if [ "$STATUS" = "accepted" ]; then
        echo "ship:pr"
      else
        echo "skip:bench-status=${STATUS:-missing}"
      fi
      ;;
    consensus_poc_pr)
      if [ -s "$OUTPUT_DIR/implementation.md" ]; then
        echo "ship:pr"
      else
        echo "skip:no-implementation"
      fi
      ;;
    consensus_issue)
      if [ -s "$OUTPUT_DIR/consensus-issue.md" ]; then
        echo "ship:issue"
      else
        echo "skip:no-consensus-issue-marker"
      fi
      ;;
    *)
      echo "skip:unknown-delivery-mode=${DELIVERY_MODE:-missing}"
      ;;
  esac
}

# Remove ALL publish artifacts for a target (both PR and issue shapes).
# Used by run_pr_writer / run_issue_writer (so a delivery_mode change between
# runs cannot leave stale cross-mode artifacts) AND on skip in the dispatch
# loop (so a target accepted in a prior run that becomes rejected/aborted in
# a rerun cannot leak stale artifacts through to publish-accepted.sh — that
# script publishes purely from artifact presence).
clear_publish_artifacts() {
  local TARGET_ID="$1"
  local OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  rm -f \
    "$OUTPUT_DIR/pr-title.txt" \
    "$OUTPUT_DIR/pr-body.md" \
    "$OUTPUT_DIR/pr-writer-prompt.md" \
    "$OUTPUT_DIR/pr-writer-events.jsonl" \
    "$OUTPUT_DIR/pr-writer-stderr.log" \
    "$OUTPUT_DIR/pr-writer-final-message.md" \
    "$OUTPUT_DIR/issue-title.txt" \
    "$OUTPUT_DIR/issue-body.md" \
    "$OUTPUT_DIR/issue-writer-prompt.md" \
    "$OUTPUT_DIR/issue-writer-events.jsonl" \
    "$OUTPUT_DIR/issue-writer-stderr.log" \
    "$OUTPUT_DIR/issue-writer-final-message.md"
}

mapfile -t TARGET_IDS < <(jq -r '.targets[].id' "$TARGETS")
if [ "${#TARGET_IDS[@]}" -eq 0 ]; then
  echo "No targets; publish-artifact generation is a no-op."
  exit 0
fi

run_pr_writer() {
  local TARGET_ID="$1"
  local OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  local WORKTREE_DIR="$WORKTREES/$TARGET_ID"
  local TARGET_JSON EXPERIMENT_JSON DELIVERY_MODE

  [ -s "$OUTPUT_DIR/implementation.md" ] || {
    echo "generate-pr-artifacts: missing implementation.md for $TARGET_ID" >&2
    return 1
  }

  TARGET_JSON=$(jq --arg id "$TARGET_ID" '.targets[] | select(.id==$id)' "$TARGETS")
  DELIVERY_MODE=$(jq -r '.delivery_mode' <<< "$TARGET_JSON")
  # For consensus_poc_pr the bench didn't run, so summary may not have an
  # experiment row. The summary file itself may also be absent if this
  # session contains only consensus-routed targets. Fall back to {} in both
  # cases so the prompt's envsubst still renders.
  #
  # NOTE on the jq idiom: `.experiments[] | select(...) // {}` is per-item
  # (the `//` binds to each item, not to the search result), so a non-match
  # against an N-experiment summary would emit N copies of {} instead of
  # one. Use `first(...) // {}` to apply the fallback to the search RESULT.
  if [ "$HAS_SUMMARY" = "1" ]; then
    EXPERIMENT_JSON=$(jq --arg id "$TARGET_ID" \
      'first(.experiments[] | select(.target_id==$id)) // {}' "$SUMMARY")
  else
    EXPERIMENT_JSON='{}'
  fi

  export TARGET_ID OUTPUT_DIR WORKTREE_DIR TARGET_JSON EXPERIMENT_JSON DELIVERY_MODE

  # Clear ALL publish artifacts (both PR and issue shapes) before writing new
  # PR ones. Without this, a target whose delivery_mode changed between runs
  # (e.g. consensus_poc_pr → consensus_issue) would leave stale cross-mode
  # artifacts behind that publish-accepted.sh might mistakenly act on.
  clear_publish_artifacts "$TARGET_ID"

  # shellcheck disable=SC2016
  envsubst '$OPT_SESSION_ID $TARGET_ID $OUTPUT_DIR $WORKTREE_DIR $TARGET_JSON $EXPERIMENT_JSON $DELIVERY_MODE' \
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
  # requires: ## Summary, ## What changed, ## Benchmark result, ## Validation
  # for both PR modes. consensus_poc_pr additionally requires
  # ## Consensus / HIP coordination so the consensus rationale is preserved
  # in the published PR body — operators reviewing draft PRs need the HIP
  # context inline rather than buried in implementation.md.
  local required_sections=('Summary' 'What changed' 'Benchmark result' 'Validation')
  if [ "$DELIVERY_MODE" = "consensus_poc_pr" ]; then
    required_sections+=('Consensus / HIP coordination')
  fi
  local missing_sections=()
  local section
  for section in "${required_sections[@]}"; do
    # Escape slashes (so `Consensus / HIP coordination` doesn't break the
    # regex delimiter) and normalize runs of whitespace into a tolerant
    # `[[:space:]]+`. Uses `sed -E` because BSD sed (macOS) does NOT treat
    # `\+` as the one-or-more metacharacter in BRE — it would match literal
    # `\+` and silently fail, requiring an exact whitespace match instead.
    local pattern
    pattern=$(printf '%s' "$section" | sed -E 's|/|\\/|g; s|[[:space:]]+|[[:space:]]+|g')
    grep -qiE "^##[[:space:]]+${pattern}[[:space:]]*$" "$OUTPUT_DIR/pr-body.md" \
      || missing_sections+=("$section")
  done
  if [ "${#missing_sections[@]}" -gt 0 ]; then
    echo "generate-pr-artifacts: pr-body.md for $TARGET_ID (delivery_mode=$DELIVERY_MODE) is missing required sections: ${missing_sections[*]}" >&2
    return 1
  fi
}

run_issue_writer() {
  local TARGET_ID="$1"
  local OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  local TARGET_JSON

  [ -s "$OUTPUT_DIR/consensus-issue.md" ] || {
    echo "generate-pr-artifacts: missing consensus-issue.md for $TARGET_ID" >&2
    return 1
  }

  TARGET_JSON=$(jq --arg id "$TARGET_ID" '.targets[] | select(.id==$id)' "$TARGETS")

  export TARGET_ID OUTPUT_DIR TARGET_JSON

  # Clear ALL publish artifacts before writing new issue ones. See
  # clear_publish_artifacts comment for the cross-mode rationale.
  clear_publish_artifacts "$TARGET_ID"

  # shellcheck disable=SC2016
  envsubst '$OPT_SESSION_ID $TARGET_ID $OUTPUT_DIR $TARGET_JSON' \
    < "$PROMPTS_DIR/issue-writer.md" \
    > "$OUTPUT_DIR/issue-writer-prompt.md"

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
      "${CODEX_EXEC_ARGS[@]}" \
      --output-last-message "$OUTPUT_DIR/issue-writer-final-message.md" \
      "$(cat "$OUTPUT_DIR/issue-writer-prompt.md")" \
    > "$OUTPUT_DIR/issue-writer-events.jsonl" \
    2> "$OUTPUT_DIR/issue-writer-stderr.log"

  [ -s "$OUTPUT_DIR/issue-title.txt" ] || {
    echo "generate-pr-artifacts: issue-title.txt missing for $TARGET_ID" >&2
    return 1
  }
  [ -s "$OUTPUT_DIR/issue-body.md" ] || {
    echo "generate-pr-artifacts: issue-body.md missing for $TARGET_ID" >&2
    return 1
  }

  # Validate the issue-writer's required sections per prompts/issue-writer.md.
  local missing_sections=()
  for section in 'Summary' 'Breakage class' 'Proposed change' 'Expected impact' 'HIP / coordination concerns' 'Why an issue, not a PR' 'Reference: target id'; do
    grep -qiE "^##[[:space:]]+${section}[[:space:]]*$" "$OUTPUT_DIR/issue-body.md" \
      || missing_sections+=("$section")
  done
  if [ "${#missing_sections[@]}" -gt 0 ]; then
    echo "generate-pr-artifacts: issue-body.md for $TARGET_ID is missing required sections: ${missing_sections[*]}" >&2
    return 1
  fi
}

PR_COUNT=0; ISSUE_COUNT=0; SKIP_COUNT=0
for TARGET_ID in "${TARGET_IDS[@]}"; do
  DECISION=$(target_publish_decision "$TARGET_ID")
  case "$DECISION" in
    ship:pr)
      run_pr_writer "$TARGET_ID"
      PR_COUNT=$((PR_COUNT + 1))
      ;;
    ship:issue)
      run_issue_writer "$TARGET_ID"
      ISSUE_COUNT=$((ISSUE_COUNT + 1))
      ;;
    skip:*)
      echo "skip $TARGET_ID: ${DECISION#skip:}"
      clear_publish_artifacts "$TARGET_ID"
      SKIP_COUNT=$((SKIP_COUNT + 1))
      ;;
  esac
done

echo "Generated artifacts for ${PR_COUNT} PR target(s), ${ISSUE_COUNT} issue target(s); ${SKIP_COUNT} skipped (publish artifacts cleared)."
