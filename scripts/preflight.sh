#!/usr/bin/env bash
# Quick demo-readiness check for the agentic framework environment.
# Verifies the key shell tools, Codex CLI shape, checkout paths, and trust hints.
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"

assert_required_tools
assert_codex_compatible
assert_codex_writable

if ! git -C "$BASE" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "BASE is not a git checkout: $BASE" >&2
  exit 1
fi

if [ ! -d "$FRAMEWORK_ROOT/repos/stacks-core" ]; then
  echo "missing stacks-core submodule checkout: $FRAMEWORK_ROOT/repos/stacks-core" >&2
  exit 1
fi

if [ ! -d "$STACKS_BENCH_DATA_DIR" ]; then
  echo "missing stacks-bench data dir: $STACKS_BENCH_DATA_DIR" >&2
  exit 1
fi

# Publish-mode checks: only run when Phase 5 is enabled in the env.
# These verify the publisher user, sudoers wiring, token, and gh availability
# from the agent user's perspective — without ever reading the token itself.
if [ "${PUBLISH_ACCEPTED_PRS:-0}" = "1" ]; then
  PUBLISH_SUDO_USER="${PUBLISH_SUDO_USER:-publisher}"
  PUBLISH_TOKEN_FILE="${PUBLISH_TOKEN_FILE:-/var/lib/stacks-core-agentic/gh_token}"
  PUBLISH_REMOTE="${PUBLISH_REMOTE:-origin}"
  PUBLISH_BASE_REPO="${PUBLISH_BASE_REPO:-cylewitruk/stacks-core}"

  publish_fail=0
  publish_check() {
    local label="$1"; shift
    if "$@" >/dev/null 2>&1; then
      printf '  OK    %s\n' "$label"
    else
      printf '  FAIL  %s\n' "$label" >&2
      publish_fail=1
    fi
  }

  echo "Publish-mode checks (PUBLISH_ACCEPTED_PRS=1):"
  publish_check "publisher user '$PUBLISH_SUDO_USER' exists" \
    id -u "$PUBLISH_SUDO_USER"
  publish_check "NOPASSWD sudo to '$PUBLISH_SUDO_USER' works" \
    sudo -nH -u "$PUBLISH_SUDO_USER" true
  publish_check "publisher can read non-empty token file" \
    sudo -nH -u "$PUBLISH_SUDO_USER" test -s "$PUBLISH_TOKEN_FILE"
  publish_check "publisher has gh CLI available" \
    sudo -nH -u "$PUBLISH_SUDO_USER" sh -c 'command -v gh'
  # Phase 5 sources $AGENTIC_ENV_FILE and runs $FRAMEWORK_ROOT/scripts/publish-accepted.sh
  # as the publisher. If either is not readable / executable for the publisher,
  # publish-accepted.sh dies before any of the per-target logic runs — preflight
  # should catch that here.
  publish_check "publisher can read AGENTIC_ENV_FILE" \
    sudo -nH -u "$PUBLISH_SUDO_USER" test -r "$AGENTIC_ENV_FILE"
  publish_check "publisher can execute publish-accepted.sh" \
    sudo -nH -u "$PUBLISH_SUDO_USER" test -x "$FRAMEWORK_ROOT/scripts/publish-accepted.sh"
  # Run the BASE git query AS THE PUBLISHER so it catches safe.directory or
  # repo-readability issues that would otherwise bite Phase 5. publish-accepted.sh
  # does the same get-url call from the publisher's process.
  publish_check "publisher can resolve remote '$PUBLISH_REMOTE' in BASE" \
    sudo -nH -u "$PUBLISH_SUDO_USER" git -C "$BASE" remote get-url "$PUBLISH_REMOTE"

  case "$PUBLISH_BASE_REPO" in
    */*) printf '  OK    PUBLISH_BASE_REPO looks valid: %s\n' "$PUBLISH_BASE_REPO" ;;
    *)
      printf '  FAIL  PUBLISH_BASE_REPO is not owner/repo: %s\n' "$PUBLISH_BASE_REPO" >&2
      publish_fail=1
      ;;
  esac

  if [ "$publish_fail" -ne 0 ]; then
    echo "Publish-mode preflight FAILED. Re-run scripts/setup-publisher.sh as root, then place the GitHub PAT into $PUBLISH_TOKEN_FILE." >&2
    exit 1
  fi
fi

cat <<EOF
Preflight OK

FRAMEWORK_ROOT: $FRAMEWORK_ROOT
AGENTIC_ENV_FILE: $AGENTIC_ENV_FILE
BASE: $BASE
STACKS_BENCH_DATA_DIR: $STACKS_BENCH_DATA_DIR
CODEX_MODEL: ${CODEX_MODEL:-gpt-5.5}
CODEX_VERSION: $(codex --version 2>/dev/null || echo unknown)
PUBLISH_ACCEPTED_PRS: ${PUBLISH_ACCEPTED_PRS:-0}

Recommended Codex trust entries:
[projects."$FRAMEWORK_ROOT/repos/stacks-core"]
trust_level = "trusted"

[projects."$FRAMEWORK_ROOT/sessions"]
trust_level = "trusted"
EOF
