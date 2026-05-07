#!/usr/bin/env bash
# Publish draft PRs for accepted targets in a session.
#
# Intended to run as a dedicated publisher user that can read a protected
# GitHub token file but does not run Codex.
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"

require_command jq
require_command git
require_command gh

SUMMARY="$OPT_SESSION_DIR/summary.json"
[ -s "$SUMMARY" ] || { echo "publish-accepted: missing summary.json" >&2; exit 2; }

if [ "${PUBLISH_ACCEPTED_PRS:-0}" != "1" ]; then
  echo "publish-accepted: disabled (set PUBLISH_ACCEPTED_PRS=1 to enable)" >&2
  exit 0
fi

# Internal defaults must match scripts/env.example. If .env was not sourced
# (e.g. an invocation outside the coordinator), these are the values that
# apply — keep them safe-by-default.
TOKEN_FILE="${PUBLISH_TOKEN_FILE:-/var/lib/stacks-core-agentic/gh_token}"
[ -r "$TOKEN_FILE" ] || { echo "publish-accepted: token file not readable: $TOKEN_FILE" >&2; exit 1; }
GH_TOKEN="$(tr -d '\r\n' < "$TOKEN_FILE")"
export GH_TOKEN

PUBLISH_REMOTE="${PUBLISH_REMOTE:-origin}"
PUBLISH_BASE_REPO="${PUBLISH_BASE_REPO:-cylewitruk/stacks-core}"
PUBLISH_BASE_BRANCH="${PUBLISH_BASE_BRANCH:-feat/stacks-bench}"
PUBLISH_DRAFT_PRS="${PUBLISH_DRAFT_PRS:-1}"
PUBLISH_PR_LABELS="${PUBLISH_PR_LABELS:-}"
PUBLISH_BRANCH_PREFIX="${PUBLISH_BRANCH_PREFIX:-agentic}"

derive_head_owner() {
  local remote_url
  remote_url=$(git -C "$BASE" remote get-url "$PUBLISH_REMOTE")
  case "$remote_url" in
    git@github.com:*)
      remote_url=${remote_url#git@github.com:}
      remote_url=${remote_url%%/*}
      printf '%s\n' "$remote_url"
      ;;
    https://github.com/*)
      remote_url=${remote_url#https://github.com/}
      remote_url=${remote_url%%/*}
      printf '%s\n' "$remote_url"
      ;;
    *)
      return 1
      ;;
  esac
}

HEAD_OWNER="${PUBLISH_HEAD_OWNER:-}"
if [ -z "$HEAD_OWNER" ]; then
  HEAD_OWNER="$(derive_head_owner)" || {
    echo "publish-accepted: unable to derive head owner from remote '$PUBLISH_REMOTE'" >&2
    exit 1
  }
fi

mapfile -t TARGET_IDS < <(jq -r '.experiments[] | select(.status=="accepted") | .target_id' "$SUMMARY")
if [ "${#TARGET_IDS[@]}" -eq 0 ]; then
  echo "publish-accepted: no accepted targets; nothing to publish."
  exit 0
fi

for TARGET_ID in "${TARGET_IDS[@]}"; do
  WT="$WORKTREES/$TARGET_ID"
  OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  TITLE_FILE="$OUTPUT_DIR/pr-title.txt"
  BODY_FILE="$OUTPUT_DIR/pr-body.md"

  [ -d "$WT" ] || { echo "publish-accepted: missing worktree for $TARGET_ID" >&2; continue; }
  [ -s "$TITLE_FILE" ] || { echo "publish-accepted: missing pr-title.txt for $TARGET_ID" >&2; continue; }
  [ -s "$BODY_FILE" ] || { echo "publish-accepted: missing pr-body.md for $TARGET_ID" >&2; continue; }

  BRANCH="${PUBLISH_BRANCH_PREFIX}/${OPT_SESSION_ID}/${TARGET_ID}"
  TITLE="$(head -n 1 "$TITLE_FILE" | tr -d '\r')"
  COMMIT_MSG="${TITLE:-perf: optimize $TARGET_ID}"

  # Idempotency: if a PR already exists for this target's branch, skip ALL
  # git mutations (no `switch -C` clobber, no force-push). This makes Phase 5
  # safely re-runnable.
  EXISTING_COUNT=$(gh pr list \
    --repo "$PUBLISH_BASE_REPO" \
    --state all \
    --search "head:${HEAD_OWNER}:${BRANCH} base:${PUBLISH_BASE_BRANCH}" \
    --json number \
    --jq 'length')

  if [ "$EXISTING_COUNT" -gt 0 ]; then
    echo "publish-accepted: PR already exists for $TARGET_ID ($BRANCH); skipping git ops."
    continue
  fi

  git -C "$WT" switch -C "$BRANCH"

  # Stage ONLY modifications to tracked files. `-u` skips untracked content,
  # so stray `.codex/` dirs, build droppings, and other artifacts do not get
  # swept into the published commit. Anything truly new (e.g. a new module)
  # must be added by the operator before re-running.
  git -C "$WT" add -u

  if ! git -C "$WT" diff --cached --quiet; then
    {
      echo "publish-accepted: staged files for $TARGET_ID:"
      git -C "$WT" diff --cached --name-only | sed 's/^/  /'
    } >&2
    git -C "$WT" commit -m "$COMMIT_MSG"
  fi

  git -C "$WT" push -u "$PUBLISH_REMOTE" "$BRANCH"

  DRAFT_FLAG=()
  if [ "$PUBLISH_DRAFT_PRS" = "1" ]; then
    DRAFT_FLAG=(--draft)
  fi

  LABEL_ARGS=()
  if [ -n "$PUBLISH_PR_LABELS" ]; then
    IFS=',' read -ra _labels <<< "$PUBLISH_PR_LABELS"
    for _label in "${_labels[@]}"; do
      # Trim leading/trailing whitespace.
      _label="${_label#"${_label%%[![:space:]]*}"}"
      _label="${_label%"${_label##*[![:space:]]}"}"
      [ -n "$_label" ] && LABEL_ARGS+=(--label "$_label")
    done
  fi

  gh pr create \
    --repo "$PUBLISH_BASE_REPO" \
    --base "$PUBLISH_BASE_BRANCH" \
    --head "${HEAD_OWNER}:${BRANCH}" \
    "${DRAFT_FLAG[@]}" \
    "${LABEL_ARGS[@]}" \
    --title "$TITLE" \
    --body-file "$BODY_FILE"

  echo "publish-accepted: created PR for $TARGET_ID ($BRANCH)"
done
