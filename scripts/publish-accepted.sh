#!/usr/bin/env bash
# Publish autonomous-run artifacts to GitHub. Dispatches per target based on
# delivery_mode read from optimization-targets.json:
#
#   normal_pr        — gh pr create with operator-configured labels.
#                      Gate: pr-title.txt + pr-body.md present (produced by
#                      generate-pr-artifacts.sh after bench acceptance).
#                      Honors PUBLISH_DRAFT_PRS for draft state.
#   consensus_poc_pr — gh pr create, ALWAYS as draft, with operator labels
#                      plus the safety set (consensus-change, needs-HIP,
#                      do-not-merge). Gate: pr-title.txt + pr-body.md
#                      present (produced by generate-pr-artifacts.sh after
#                      PoC tests passed; no benchmark gate).
#   consensus_issue  — gh issue create with consensus-change + needs-HIP
#                      labels. Gate: issue-title.txt + issue-body.md
#                      present (produced by generate-pr-artifacts.sh from
#                      the analyzer's consensus_writeup; no optimizer ran).
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

TARGETS="$OPT_SESSION_DIR/optimization-targets.json"
[ -s "$TARGETS" ] || { echo "publish-accepted: missing optimization-targets.json" >&2; exit 2; }

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

# Safety labels that ALWAYS apply to consensus-breaking PRs and issues.
# Hardcoded (not env-overridable) because their absence would let a
# consensus PR auto-merge on a misconfigured repo.
CONSENSUS_PR_LABELS="consensus-change,needs-HIP,do-not-merge"
CONSENSUS_ISSUE_LABELS="consensus-change,needs-HIP"

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

# Build a `--label` arg array from a comma-separated string. Trims whitespace
# around each label. Out-arg is read by reference (caller must declare the
# array name).
build_label_args() {
  local _csv="$1"
  local _out_name="$2"
  declare -n _out="$_out_name"
  _out=()
  if [ -n "$_csv" ]; then
    IFS=',' read -ra _labels <<< "$_csv"
    for _label in "${_labels[@]}"; do
      _label="${_label#"${_label%%[![:space:]]*}"}"
      _label="${_label%"${_label##*[![:space:]]}"}"
      [ -n "$_label" ] && _out+=(--label "$_label")
    done
  fi
}

publish_pr() {
  local TARGET_ID="$1"
  local DELIVERY_MODE="$2"   # normal_pr | consensus_poc_pr
  local WT="$WORKTREES/$TARGET_ID"
  local OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  local TITLE_FILE="$OUTPUT_DIR/pr-title.txt"
  local BODY_FILE="$OUTPUT_DIR/pr-body.md"

  [ -d "$WT" ] || { echo "publish-accepted: missing worktree for $TARGET_ID" >&2; return 1; }
  [ -s "$TITLE_FILE" ] || { echo "publish-accepted: missing pr-title.txt for $TARGET_ID" >&2; return 1; }
  [ -s "$BODY_FILE" ] || { echo "publish-accepted: missing pr-body.md for $TARGET_ID" >&2; return 1; }

  local BRANCH TITLE COMMIT_MSG EXISTING_COUNT
  BRANCH="${PUBLISH_BRANCH_PREFIX}/${OPT_SESSION_ID}/${TARGET_ID}"
  TITLE="$(head -n 1 "$TITLE_FILE" | tr -d '\r')"
  COMMIT_MSG="${TITLE:-perf: optimize $TARGET_ID}"

  EXISTING_COUNT=$(gh pr list \
    --repo "$PUBLISH_BASE_REPO" \
    --state all \
    --search "head:${HEAD_OWNER}:${BRANCH} base:${PUBLISH_BASE_BRANCH}" \
    --json number \
    --jq 'length')

  if [ "$EXISTING_COUNT" -gt 0 ]; then
    echo "publish-accepted: PR already exists for $TARGET_ID ($BRANCH); skipping git ops."
    return 0
  fi

  git -C "$WT" switch -C "$BRANCH"

  # Stage ONLY modifications to tracked files. `-u` skips untracked content,
  # so stray `.codex/` dirs, build droppings, and other artifacts do not get
  # swept into the published commit.
  git -C "$WT" add -u

  if ! git -C "$WT" diff --cached --quiet; then
    {
      echo "publish-accepted: staged files for $TARGET_ID:"
      git -C "$WT" diff --cached --name-only | sed 's/^/  /'
    } >&2
    git -C "$WT" commit -m "$COMMIT_MSG"
  fi

  git -C "$WT" push -u "$PUBLISH_REMOTE" "$BRANCH"

  # Draft state: consensus_poc_pr is ALWAYS draft (merging would be a
  # disaster); normal_pr respects the operator preference.
  local -a DRAFT_FLAG=()
  if [ "$DELIVERY_MODE" = "consensus_poc_pr" ] || [ "$PUBLISH_DRAFT_PRS" = "1" ]; then
    DRAFT_FLAG=(--draft)
  fi

  # Labels: operator-configured + (for consensus_poc_pr) the safety set.
  local LABELS_CSV="$PUBLISH_PR_LABELS"
  if [ "$DELIVERY_MODE" = "consensus_poc_pr" ]; then
    if [ -n "$LABELS_CSV" ]; then
      LABELS_CSV="${LABELS_CSV},${CONSENSUS_PR_LABELS}"
    else
      LABELS_CSV="$CONSENSUS_PR_LABELS"
    fi
  fi
  local -a LABEL_ARGS=()
  build_label_args "$LABELS_CSV" LABEL_ARGS

  gh pr create \
    --repo "$PUBLISH_BASE_REPO" \
    --base "$PUBLISH_BASE_BRANCH" \
    --head "${HEAD_OWNER}:${BRANCH}" \
    "${DRAFT_FLAG[@]}" \
    "${LABEL_ARGS[@]}" \
    --title "$TITLE" \
    --body-file "$BODY_FILE"

  echo "publish-accepted: created PR for $TARGET_ID ($BRANCH; mode=$DELIVERY_MODE)"
}

publish_issue() {
  local TARGET_ID="$1"
  local OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
  local TITLE_FILE="$OUTPUT_DIR/issue-title.txt"
  local BODY_FILE="$OUTPUT_DIR/issue-body.md"

  [ -s "$TITLE_FILE" ] || { echo "publish-accepted: missing issue-title.txt for $TARGET_ID" >&2; return 1; }
  [ -s "$BODY_FILE" ] || { echo "publish-accepted: missing issue-body.md for $TARGET_ID" >&2; return 1; }

  local TITLE TRACE_TAG EXISTING_COUNT
  TITLE="$(head -n 1 "$TITLE_FILE" | tr -d '\r')"
  # Issues don't have a branch to dedupe against. Use a hidden trace-tag
  # token in the body — `<!-- agentic:<session>:<target> -->` — and search
  # for it via `gh issue list --search`. This keeps idempotency tied to the
  # session+target tuple rather than the title (which the LLM might rephrase
  # on a retry).
  TRACE_TAG="agentic-${OPT_SESSION_ID}-${TARGET_ID}"

  EXISTING_COUNT=$(gh issue list \
    --repo "$PUBLISH_BASE_REPO" \
    --state all \
    --search "in:body \"$TRACE_TAG\"" \
    --json number \
    --jq 'length')

  if [ "$EXISTING_COUNT" -gt 0 ]; then
    echo "publish-accepted: issue already exists for $TARGET_ID ($TRACE_TAG); skipping."
    return 0
  fi

  # Append the trace tag to the body (idempotent — the body file is
  # regenerated each run, so we re-append every invocation but the tag is
  # what `gh issue list --search` looks for, so duplicates are caught above
  # before this point).
  local TMP_BODY
  TMP_BODY=$(mktemp)
  trap 'rm -f "$TMP_BODY"' RETURN
  {
    cat "$BODY_FILE"
    printf '\n\n<!-- %s -->\n' "$TRACE_TAG"
  } > "$TMP_BODY"

  local -a LABEL_ARGS=()
  build_label_args "$CONSENSUS_ISSUE_LABELS" LABEL_ARGS

  gh issue create \
    --repo "$PUBLISH_BASE_REPO" \
    "${LABEL_ARGS[@]}" \
    --title "$TITLE" \
    --body-file "$TMP_BODY"

  echo "publish-accepted: created issue for $TARGET_ID ($TRACE_TAG)"
}

PR_COUNT=0; ISSUE_COUNT=0; SKIP_COUNT=0
mapfile -t TARGET_IDS < <(jq -r '.targets[].id' "$TARGETS")
if [ "${#TARGET_IDS[@]}" -eq 0 ]; then
  echo "publish-accepted: no targets; nothing to publish."
  exit 0
fi

for TARGET_ID in "${TARGET_IDS[@]}"; do
  DELIVERY_MODE=$(jq -r --arg id "$TARGET_ID" \
    '.targets[] | select(.id==$id) | .delivery_mode' "$TARGETS")
  OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"

  case "$DELIVERY_MODE" in
    normal_pr|consensus_poc_pr)
      if [ ! -s "$OUTPUT_DIR/pr-title.txt" ] || [ ! -s "$OUTPUT_DIR/pr-body.md" ]; then
        echo "skip $TARGET_ID: pr artifacts not generated (mode=$DELIVERY_MODE)"
        SKIP_COUNT=$((SKIP_COUNT + 1))
        continue
      fi
      publish_pr "$TARGET_ID" "$DELIVERY_MODE" || {
        echo "publish-accepted: failed to publish PR for $TARGET_ID" >&2
        SKIP_COUNT=$((SKIP_COUNT + 1))
        continue
      }
      PR_COUNT=$((PR_COUNT + 1))
      ;;
    consensus_issue)
      if [ ! -s "$OUTPUT_DIR/issue-title.txt" ] || [ ! -s "$OUTPUT_DIR/issue-body.md" ]; then
        echo "skip $TARGET_ID: issue artifacts not generated (mode=$DELIVERY_MODE)"
        SKIP_COUNT=$((SKIP_COUNT + 1))
        continue
      fi
      publish_issue "$TARGET_ID" || {
        echo "publish-accepted: failed to publish issue for $TARGET_ID" >&2
        SKIP_COUNT=$((SKIP_COUNT + 1))
        continue
      }
      ISSUE_COUNT=$((ISSUE_COUNT + 1))
      ;;
    *)
      echo "skip $TARGET_ID: unknown delivery_mode=${DELIVERY_MODE:-missing}"
      SKIP_COUNT=$((SKIP_COUNT + 1))
      ;;
  esac
done

echo "publish-accepted: ${PR_COUNT} PR(s), ${ISSUE_COUNT} issue(s); ${SKIP_COUNT} skipped (of ${#TARGET_IDS[@]} targets)."
