# shellcheck shell=bash
# Shared helpers for the bench-agent phase scripts. Source from each phase:
#
#   source "$(dirname "$0")/_lib.sh"
#   init_session "$@"
#
# `init_session` parses SESSION_DIR from $1 (or $OPT_SESSION_DIR fallback),
# derives OPT_SESSION_ID + WORKTREES, exports them, and ensures the session
# dir exists. After it returns, every phase script has the same view of:
#
#   SESSION_DIR / OPT_SESSION_DIR  — per-session results dir (positional arg)
#   OPT_SESSION_ID                 — the session id (parent dir name)
#   WORKTREES                      — sibling worktrees dir
#   STACKS_BENCH_DATA_DIR, BASE,   — from /work/.env
#   BENCH_LOCK, TEST_LOCK,
#   STACKS_BENCH_NETWORK,
#   STACKS_BENCH_START_AT,
#   STACKS_BENCH_COUNT, ...

# Load env once; idempotent (set -a is benign on re-source).
# /work/.env lives on the deploy VM, not in the repo, so shellcheck can't
# follow it statically.
# shellcheck source=/dev/null
if [ -f /work/.env ]; then
  set -a; source /work/.env; set +a
fi

init_session() {
  SESSION_DIR="${1:-${OPT_SESSION_DIR:-}}"
  if [ -z "$SESSION_DIR" ]; then
    echo "usage: $0 SESSION_DIR  (or set OPT_SESSION_DIR)" >&2
    return 2
  fi
  mkdir -p "$SESSION_DIR" "$STACKS_BENCH_DATA_DIR"
  # SESSION_ID is the parent dir name: /work/sessions/<id>/results
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
