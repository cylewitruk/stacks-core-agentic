#!/usr/bin/env bash
# Quick demo-readiness check for the agentic framework environment.
# Verifies the key shell tools, Codex CLI shape, checkout paths, and trust hints.
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"

assert_required_tools
assert_codex_compatible
assert_codex_writable

if [ ! -d "$BASE/.git" ]; then
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

cat <<EOF
Preflight OK

FRAMEWORK_ROOT: $FRAMEWORK_ROOT
AGENTIC_ENV_FILE: $AGENTIC_ENV_FILE
BASE: $BASE
STACKS_BENCH_DATA_DIR: $STACKS_BENCH_DATA_DIR
CODEX_MODEL: ${CODEX_MODEL:-gpt-5.5}
CODEX_VERSION: $(codex --version 2>/dev/null || echo unknown)

Recommended Codex trust entries:
[projects."$FRAMEWORK_ROOT/repos/stacks-core"]
trust_level = "trusted"

[projects."$FRAMEWORK_ROOT/sessions"]
trust_level = "trusted"
EOF
