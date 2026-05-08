#!/usr/bin/env bash
# Phase 2: optimizer fan-out. One Codex subagent per accepted target, each in
# its own git worktree. Implementation only — no benchmarks here.
#
# Inputs (in SESSION_DIR):
#   optimization-targets.json   (built by merge-analyses.sh; schema v2)
#
# Outputs:
#   experiments/<target-id>/
#     optimizer-prompt.md
#     subagent-events.jsonl
#     subagent-stderr.log
#     subagent-final-message.md
#     subagent-conversation-id
#     implementation.md OR abort.md   (the agent picks one)
#     side-observations.md             (optional)
#     nextest.log[.stderr.log]         (test output)
#   $WORKTREES/<target-id>/             (git worktree, with optional release binary)
#
# Knobs:
#   STACKS_BENCH_PARALLEL_AGENTS    cap on concurrent optimizers
#                                   (default: one per target)
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"
assert_required_tools
assert_codex_compatible
assert_codex_writable

TARGETS="$OPT_SESSION_DIR/optimization-targets.json"
if [ ! -s "$TARGETS" ]; then
  echo "run-optimizers: missing optimization-targets.json (run merge-analyses.sh first)" >&2
  exit 2
fi

# Sanity check: targets must be v2. A stale v1 file would have .targets[].id
# but lack the merge-phase fields the optimizer prompt uses, leading to a
# silently-mixed pipeline state.
v=$(jq -r '.schema_version // empty' "$TARGETS")
if [ "$v" != "2" ]; then
  echo "run-optimizers: optimization-targets.json schema_version=$v (expected 2; v1 targets are not supported in this pipeline — re-run merge-analyses.sh)" >&2
  exit 2
fi

mapfile -t TARGET_IDS < <(jq -r '.targets[].id' "$TARGETS")
if [ "${#TARGET_IDS[@]}" -eq 0 ]; then
  echo "No accepted targets; phase is a no-op."
  exit 0
fi

mkdir -p "$WORKTREES"

PARALLEL="${STACKS_BENCH_PARALLEL_AGENTS:-${#TARGET_IDS[@]}}"

run_optimizer() {
  local TARGET_ID="$1"
  local BRANCH="agent/$TARGET_ID"
  local WT="$WORKTREES/$TARGET_ID"
  local OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  mkdir -p "$OUTPUT_DIR"

  # Read delivery_mode and the optimizer-relevant consensus fields from the
  # merged target. Three branches:
  #   normal_pr         → existing flow (run optimizer, full nextest suite).
  #   consensus_poc_pr  → run optimizer in PoC mode; nextest is filtered to
  #                       poc_test_scope only (the full suite would fail by
  #                       definition under a consensus change). Optimizer
  #                       still produces implementation.md; downstream
  #                       artifacts get PoC labels.
  #   consensus_issue   → SKIP the optimizer entirely. Write a
  #                       `consensus-issue.md` marker so downstream phases
  #                       (bench, generate-pr-artifacts, publish) route this
  #                       target to the issue path. The analyzer's
  #                       consensus_writeup is the artifact.
  local TARGET_JSON DELIVERY_MODE POC_TEST_SCOPE_EXPR
  TARGET_JSON=$(jq --arg id "$TARGET_ID" \
    '.targets[] | select(.id==$id)' \
    "$OPT_SESSION_DIR/optimization-targets.json")
  DELIVERY_MODE=$(jq -r '.delivery_mode' <<< "$TARGET_JSON")

  # consensus_issue path: drop a marker, skip everything else. Idempotency:
  # remove any optimizer-side artifacts from prior runs first so a retry
  # cleanly reflects the new routing.
  if [ "$DELIVERY_MODE" = "consensus_issue" ]; then
    rm -f \
      "$OUTPUT_DIR/abort.md" \
      "$OUTPUT_DIR/implementation.md" \
      "$OUTPUT_DIR/side-observations.md" \
      "$OUTPUT_DIR/nextest.log" \
      "$OUTPUT_DIR/nextest.stderr.log" \
      "$OUTPUT_DIR/subagent-events.jsonl" \
      "$OUTPUT_DIR/subagent-stderr.log" \
      "$OUTPUT_DIR/subagent-final-message.md" \
      "$OUTPUT_DIR/subagent-conversation-id" \
      "$OUTPUT_DIR/optimizer-prompt.md"
    {
      echo "# Consensus issue: optimizer skipped"
      echo
      echo "delivery_mode = consensus_issue"
      echo
      echo "This target proposes a consensus-breaking change that the"
      echo "analyzer determined is not PoC-implementable (poc_implementable"
      echo "= false). The optimizer phase is intentionally skipped: the"
      echo "analyzer's consensus_writeup is the shipping artifact."
      echo
      echo "Downstream phases:"
      echo "  - bench-experiments.sh skips this target (bench_eligible=false)."
      echo "  - generate-pr-artifacts.sh routes this target to the issue writer."
      echo "  - publish-accepted.sh creates a GitHub issue rather than a PR."
    } > "$OUTPUT_DIR/consensus-issue.md"
    return 0
  fi

  # Idempotency, part 1: tear down the worktree if it survives from a prior
  # run. `-B "$BRANCH"` already force-recreates the branch; the worktree
  # itself needs an explicit cleanup. `git worktree remove --force` handles
  # uncommitted edits; if git refuses (orphaned dir with no worktree entry),
  # fall back to rm.
  if [ -d "$WT" ]; then
    git -C "$BASE" worktree remove --force "$WT" 2>/dev/null || rm -rf "$WT"
  fi
  # Always prune before re-adding — covers the inverse case too: a prior
  # cleanup removed the dir but left stale worktree metadata behind, which
  # would otherwise make `worktree add` fail.
  git -C "$BASE" worktree prune 2>/dev/null || true

  # Idempotency, part 2: clear per-target agent outputs so a retry doesn't
  # leave stale decision markers behind. In particular, neither `abort.md`,
  # `implementation.md`, nor `consensus-issue.md` should coexist —
  # bench-experiments.sh and generate-pr-artifacts.sh route on these markers.
  rm -f \
    "$OUTPUT_DIR/abort.md" \
    "$OUTPUT_DIR/implementation.md" \
    "$OUTPUT_DIR/consensus-issue.md" \
    "$OUTPUT_DIR/side-observations.md" \
    "$OUTPUT_DIR/nextest.log" \
    "$OUTPUT_DIR/nextest.stderr.log" \
    "$OUTPUT_DIR/subagent-events.jsonl" \
    "$OUTPUT_DIR/subagent-stderr.log" \
    "$OUTPUT_DIR/subagent-final-message.md" \
    "$OUTPUT_DIR/subagent-conversation-id" \
    "$OUTPUT_DIR/optimizer-prompt.md"

  git -C "$BASE" worktree add -B "$BRANCH" "$WT" feat/stacks-bench

  # Build POC_TEST_SCOPE_EXPR for consensus_poc_pr targets — joining the
  # analyzer's poc_test_scope strings with ` | ` produces a single nextest
  # filter expression. For non-PoC targets, this stays empty and the prompt
  # treats it as the full-suite signal. Empty string for normal_pr by design.
  if [ "$DELIVERY_MODE" = "consensus_poc_pr" ]; then
    POC_TEST_SCOPE_EXPR=$(jq -r '.poc_test_scope | join(" | ")' <<< "$TARGET_JSON")
  else
    POC_TEST_SCOPE_EXPR=""
  fi

  export TARGET_ID WORKTREE_DIR="$WT" OUTPUT_DIR TARGET_JSON \
         DELIVERY_MODE POC_TEST_SCOPE_EXPR

  # envsubst's SHELL-FORMAT arg requires literal $VAR tokens; single quotes
  # prevent the shell from expanding them before envsubst sees them.
  # shellcheck disable=SC2016
  envsubst '$TARGET_ID $WORKTREE_DIR $OUTPUT_DIR $TEST_LOCK $TARGET_JSON $NON_TARGETS_PATH $OPTIMIZATION_TARGETS_SCHEMA_PATH $DELIVERY_MODE $POC_TEST_SCOPE_EXPR' \
    < "$PROMPTS_DIR/optimizer.md" \
    > "$OUTPUT_DIR/optimizer-prompt.md"

  # codex CLI 0.128.0: --ask-for-approval and -m are TOP-LEVEL flags (before
  # `exec`). No --skip-git-repo-check needed here: cwd is a real git worktree.
  # --search dropped: optimizer reads + edits source, web search isn't useful.
  # Add the framework root plus the per-target output dir and test-lock dir so
  # the flow still works if sessions/data are moved outside the checkout root.
  local -a CODEX_TOP_LEVEL_ARGS CODEX_EXEC_ARGS
  mapfile -t CODEX_TOP_LEVEL_ARGS < <(codex_top_level_args "${CODEX_MODEL:-gpt-5.5}")
  mapfile -t CODEX_EXEC_ARGS < <(codex_exec_args)

  run_with_timeout "${CODEX_EXEC_TIMEOUT_SEC:-3600}" \
    codex \
    "${CODEX_TOP_LEVEL_ARGS[@]}" \
    exec \
      --cd "$WT" \
      --add-dir "$FRAMEWORK_ROOT" \
      --add-dir "$OUTPUT_DIR" \
      --add-dir "$(dirname "$TEST_LOCK")" \
      "${CODEX_EXEC_ARGS[@]}" \
      --output-last-message "$OUTPUT_DIR/subagent-final-message.md" \
      "$(cat "$OUTPUT_DIR/optimizer-prompt.md")" \
    > "$OUTPUT_DIR/subagent-events.jsonl" \
    2> "$OUTPUT_DIR/subagent-stderr.log"

  capture_codex_conversation_id "$OUTPUT_DIR/subagent-events.jsonl" \
    > "$OUTPUT_DIR/subagent-conversation-id"
}
export -f run_optimizer capture_codex_conversation_id run_with_timeout
export -f codex_top_level_args codex_exec_args
export WORKTREES OPT_SESSION_DIR BASE TEST_LOCK

# xargs -P fans out, preserving streaming logs per subagent.
printf '%s\n' "${TARGET_IDS[@]}" \
  | xargs -P "$PARALLEL" -I{} bash -c 'run_optimizer "$@"' _ {}

# Tally landed / aborted / consensus-issue (skipped).
LANDED=0; ABORTED=0; ISSUE_SKIPPED=0
for tid in "${TARGET_IDS[@]}"; do
  if [ -f "$OPT_SESSION_DIR/experiments/$tid/consensus-issue.md" ]; then
    ISSUE_SKIPPED=$((ISSUE_SKIPPED + 1))
  elif [ -f "$OPT_SESSION_DIR/experiments/$tid/abort.md" ]; then
    ABORTED=$((ABORTED + 1))
  elif [ -f "$OPT_SESSION_DIR/experiments/$tid/implementation.md" ]; then
    LANDED=$((LANDED + 1))
  fi
done
echo "optimizers: ${LANDED} landed, ${ABORTED} aborted, ${ISSUE_SKIPPED} routed to issue (of ${#TARGET_IDS[@]} targets)"
