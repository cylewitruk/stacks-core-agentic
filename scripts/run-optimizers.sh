#!/usr/bin/env bash
# Phase 2: optimizer fan-out. One Codex subagent per accepted target, each in
# its own git worktree. Implementation only — no benchmarks here.
#
# Inputs (in SESSION_DIR):
#   optimization-targets.json   (built by assemble-targets.sh)
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
  echo "run-optimizers: missing optimization-targets.json (run assemble-targets.sh first)" >&2
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
  # leave stale decision markers behind. In particular, both `abort.md` and
  # `implementation.md` must NOT coexist — bench-experiments.sh skips any
  # target with `abort.md`, so a stale abort would cause a successful retry
  # to be silently dropped. The agent re-emits whichever is relevant.
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

  git -C "$BASE" worktree add -B "$BRANCH" "$WT" feat/stacks-bench

  # Pass the full target object. The optimizer parses fields it needs
  # (hotspot, files, proposed_change, verification_plan, etc.) directly.
  local TARGET_JSON
  TARGET_JSON=$(jq --arg id "$TARGET_ID" \
    '.targets[] | select(.id==$id)' \
    "$OPT_SESSION_DIR/optimization-targets.json")

  export TARGET_ID WORKTREE_DIR="$WT" OUTPUT_DIR TARGET_JSON

  # envsubst's SHELL-FORMAT arg requires literal $VAR tokens; single quotes
  # prevent the shell from expanding them before envsubst sees them.
  # shellcheck disable=SC2016
  envsubst '$TARGET_ID $WORKTREE_DIR $OUTPUT_DIR $TEST_LOCK $TARGET_JSON $NON_TARGETS_PATH $OPTIMIZATION_TARGETS_SCHEMA_PATH' \
    < "$PROMPTS_DIR/optimizer.md" \
    > "$OUTPUT_DIR/optimizer-prompt.md"

  # codex CLI 0.128.0: --ask-for-approval and -m are TOP-LEVEL flags (before
  # `exec`). No --skip-git-repo-check needed here: cwd is a real git worktree.
  # --search dropped: optimizer reads + edits source, web search isn't useful.
  # Add the framework root plus the per-target output dir and test-lock dir so
  # the flow still works if sessions/data are moved outside the checkout root.
  run_with_timeout "${CODEX_EXEC_TIMEOUT_SEC:-3600}" \
    codex \
    -m "${CODEX_MODEL:-gpt-5.5}" \
    --ask-for-approval never \
    exec \
      --cd "$WT" \
      --add-dir "$FRAMEWORK_ROOT" \
      --add-dir "$OUTPUT_DIR" \
      --add-dir "$(dirname "$TEST_LOCK")" \
      --sandbox workspace-write --json \
      --output-last-message "$OUTPUT_DIR/subagent-final-message.md" \
      "$(cat "$OUTPUT_DIR/optimizer-prompt.md")" \
    > "$OUTPUT_DIR/subagent-events.jsonl" \
    2> "$OUTPUT_DIR/subagent-stderr.log"

  capture_codex_conversation_id "$OUTPUT_DIR/subagent-events.jsonl" \
    > "$OUTPUT_DIR/subagent-conversation-id"
}
export -f run_optimizer capture_codex_conversation_id run_with_timeout
export WORKTREES OPT_SESSION_DIR BASE TEST_LOCK

# xargs -P fans out, preserving streaming logs per subagent.
printf '%s\n' "${TARGET_IDS[@]}" \
  | xargs -n1 -P "$PARALLEL" -I{} bash -c 'run_optimizer "$@"' _ {}

# Tally landed / aborted.
LANDED=0; ABORTED=0
for tid in "${TARGET_IDS[@]}"; do
  if [ -f "$OPT_SESSION_DIR/experiments/$tid/abort.md" ]; then
    ABORTED=$((ABORTED + 1))
  elif [ -f "$OPT_SESSION_DIR/experiments/$tid/implementation.md" ]; then
    LANDED=$((LANDED + 1))
  fi
done
echo "optimizers: ${LANDED} landed, ${ABORTED} aborted (of ${#TARGET_IDS[@]} targets)"
