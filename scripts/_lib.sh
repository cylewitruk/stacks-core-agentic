# shellcheck shell=bash
# Shared helpers for the bench-agent phase scripts. Source from each phase:
#
#   source "$(dirname "$0")/_lib.sh"
#   init_session "$@"
#
# Runtime paths are derived from FRAMEWORK_ROOT, which defaults to the parent
# directory of this scripts/ folder. The framework can therefore be checked out
# anywhere; `/work` is only a deployment convention used in some docs/examples.
#
# `init_session` parses SESSION_DIR from $1 (or $OPT_SESSION_DIR fallback),
# derives OPT_SESSION_ID + WORKTREES, exports them, and ensures the session
# dir exists. After it returns, every phase script has the same view of:
#
#   SESSION_DIR / OPT_SESSION_DIR  — per-session results dir (positional arg)
#   OPT_SESSION_ID                 — the session id (parent dir name)
#   WORKTREES                      — sibling worktrees dir
#   FRAMEWORK_ROOT                 — repository checkout root
#   PROMPTS_DIR, SCHEMAS_DIR,      — derived from FRAMEWORK_ROOT
#   SCRIPTS_DIR, NON_TARGETS_PATH,
#   STACKS_BENCH_DATA_DIR, BASE,   — from .env (or defaults)
#   BENCH_LOCK, TEST_LOCK,
#   STACKS_BENCH_NETWORK,
#   STACKS_BENCH_START_AT,
#   STACKS_BENCH_COUNT, ...

# Resolve the framework checkout root from this file's location unless the
# caller explicitly overrides it.
FRAMEWORK_ROOT="${FRAMEWORK_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
SCRIPTS_DIR="${SCRIPTS_DIR:-$FRAMEWORK_ROOT/scripts}"
AGENTIC_ENV_FILE="${AGENTIC_ENV_FILE:-$FRAMEWORK_ROOT/.env}"

# Load env once; idempotent (set -a is benign on re-source).
# The env file lives next to the framework checkout by default, so shellcheck
# cannot follow it statically.
# shellcheck source=/dev/null
if [ -f "$AGENTIC_ENV_FILE" ]; then
  set -a; source "$AGENTIC_ENV_FILE"; set +a
fi

PROMPTS_DIR="${PROMPTS_DIR:-$FRAMEWORK_ROOT/prompts}"
SCHEMAS_DIR="${SCHEMAS_DIR:-$FRAMEWORK_ROOT/schemas}"
NON_TARGETS_PATH="${NON_TARGETS_PATH:-$PROMPTS_DIR/non-targets.md}"
CANDIDATES_SCHEMA_PATH="${CANDIDATES_SCHEMA_PATH:-$SCHEMAS_DIR/candidates.schema.json}"
ANALYSIS_SCHEMA_PATH="${ANALYSIS_SCHEMA_PATH:-$SCHEMAS_DIR/analysis.schema.json}"
OPTIMIZATION_TARGETS_SCHEMA_PATH="${OPTIMIZATION_TARGETS_SCHEMA_PATH:-$SCHEMAS_DIR/optimization-targets.schema.json}"
SUMMARY_SCHEMA_PATH="${SUMMARY_SCHEMA_PATH:-$SCHEMAS_DIR/summary.schema.json}"

export FRAMEWORK_ROOT SCRIPTS_DIR AGENTIC_ENV_FILE
export PROMPTS_DIR SCHEMAS_DIR NON_TARGETS_PATH
export CANDIDATES_SCHEMA_PATH ANALYSIS_SCHEMA_PATH
export OPTIMIZATION_TARGETS_SCHEMA_PATH SUMMARY_SCHEMA_PATH

init_session() {
  SESSION_DIR="${1:-${OPT_SESSION_DIR:-}}"
  if [ -z "$SESSION_DIR" ]; then
    echo "usage: $0 SESSION_DIR  (or set OPT_SESSION_DIR)" >&2
    return 2
  fi
  mkdir -p "$SESSION_DIR" "$STACKS_BENCH_DATA_DIR"
  # SESSION_ID is the parent dir name: <sessions-root>/<id>/results
  OPT_SESSION_ID=$(basename "$(dirname "$SESSION_DIR")")
  WORKTREES="$(dirname "$SESSION_DIR")/worktrees"
  OPT_SESSION_DIR="$SESSION_DIR"
  export OPT_SESSION_ID OPT_SESSION_DIR WORKTREES
}

# Pull a numeric run_id from a `cargo stacks-bench --json bench {run,rerun}`
# envelope. Falls back to MAX(id) FROM benchmark_run if the JSON shape is
# missing the field (e.g. interrupted run that still inserted a row).
extract_run_id() {
  local json_path="$1"; local id
  id=$(jq -r '.data.run_id // empty' "$json_path" 2>/dev/null || true)
  if [ -z "$id" ] || [ "$id" = "null" ]; then
    id=$(sqlite3 "$STACKS_BENCH_DATA_DIR/appdata/stacks-bench.db" \
           "SELECT MAX(id) FROM benchmark_run;")
  fi
  echo "$id"
}

# Pull the Codex conversation id out of a JSONL events stream.
capture_codex_conversation_id() {
  jq -r 'select(.conversation_id // .session_id) | (.conversation_id // .session_id)' \
    "$1" | head -1
}

# Resolve the prebuilt stacks-bench binary, falling back to `cargo stacks-bench`
# if it's missing. Echoes the command tokens (one per line) suitable for `mapfile`.
resolve_bench_bin() {
  if [ -x "$BASE/target/release/stacks-bench" ]; then
    printf '%s\n' "$BASE/target/release/stacks-bench"
  else
    printf '%s\n' "cargo" "stacks-bench"
  fi
}

require_command() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    echo "missing required command: $name" >&2
    return 1
  fi
}

assert_required_tools() {
  require_command jq
  require_command sqlite3
  require_command envsubst
  require_command flock
  require_command cargo
  require_command codex
}

assert_codex_compatible() {
  local help_text version_text

  help_text=$(codex --help 2>/dev/null || true)
  if [ -z "$help_text" ]; then
    echo "failed to run 'codex --help'" >&2
    return 1
  fi

  case "$help_text" in
    *"--ask-for-approval"* ) ;;
    * )
      echo "codex CLI does not advertise --ask-for-approval; current scripts expect it" >&2
      return 1
      ;;
  esac

  case "$help_text" in
    *"--model"* ) ;;
    * )
      echo "codex CLI does not advertise --model/-m; current scripts expect it" >&2
      return 1
      ;;
  esac

  version_text=$(codex --version 2>/dev/null || true)
  if [ -n "${EXPECTED_CODEX_VERSION:-}" ] && [ "$version_text" != "$EXPECTED_CODEX_VERSION" ]; then
    echo "warning: codex version mismatch; expected '$EXPECTED_CODEX_VERSION', got '$version_text'" >&2
  fi
}

# Fail fast if Codex's config/session dir isn't writable by the current user.
# This typically happens when codex was first launched as root (e.g. via sudo)
# and chowned ~/.codex to root, after which unprivileged invocations can't
# write session files. The Codex CLI's own error for this is cryptic; this
# helper catches it before the prompt is even rendered.
assert_codex_writable() {
  local d="$HOME/.codex"
  if [ ! -d "$d" ]; then
    cat >&2 <<EOF
Codex config dir missing: $d
Run \`codex\` once interactively to initialize it, then re-run this script.
EOF
    return 1
  fi
  if [ ! -w "$d" ] || { [ -d "$d/sessions" ] && [ ! -w "$d/sessions" ]; }; then
    cat >&2 <<EOF
Codex session dir not writable by \$USER ($USER): $d
Fix:
  sudo chown -R "\$USER" "$d"
EOF
    return 1
  fi
}

run_with_timeout() {
  local seconds="$1"
  shift

  if [ -n "$seconds" ] && [ "$seconds" -gt 0 ] 2>/dev/null; then
    if command -v timeout >/dev/null 2>&1; then
      timeout --foreground "${seconds}s" "$@"
    else
      "$@"
    fi
  else
    "$@"
  fi
}

codex_sandbox_args() {
  if [ "${CODEX_DANGEROUS_BYPASS_SANDBOX:-0}" = "1" ]; then
    printf '%s\n' "--dangerously-bypass-approvals-and-sandbox"
  else
    printf '%s\n' "--ask-for-approval" "never"
  fi
}

codex_exec_args() {
  if [ "${CODEX_DANGEROUS_BYPASS_SANDBOX:-0}" = "1" ]; then
    printf '%s\n' "--json"
  else
    printf '%s\n' "--sandbox" "workspace-write" "--json"
  fi
}
