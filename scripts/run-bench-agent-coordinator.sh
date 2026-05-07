#!/usr/bin/env bash
# Bench-agent pipeline orchestrator. Calls each phase script in order.
#
# Each phase reads its inputs from files in SESSION_DIR and writes outputs
# back. You can run any phase script directly for a controlled walkthrough;
# this script just chains them.
#
# Phase 0a: run-baseline.sh        (or import-baseline.sh — see env vars below)
# Phase 1:  run-triage.sh          → candidates.json
# Phase 1.5: run-analyzers.sh      → analyses/<id>/analysis.json
# Phase 1.6: assemble-targets.sh   → optimization-targets.json
# Phase 2:  run-optimizers.sh      → experiments/<id>/{implementation,abort}.md
# Phase 3:  bench-experiments.sh   → experiments/<id>/run-N/bench-run.json
# Phase 4:  finalize-session.sh    → summary.{json,md}
#
# Env vars:
#   IMPORT_BASELINE_RUN_ID    if set, import this existing run id instead of
#                             running a fresh baseline benchmark.
#   IMPORT_BASELINE_RERUN_ID  optional; passed to import-baseline.sh.
#   STACKS_BENCH_PARALLEL_ANALYZERS, STACKS_BENCH_PARALLEL_AGENTS  (see phase scripts)
#   SKIP_CARGO_CLEAN          (see bench-experiments.sh)
set -euo pipefail
# Resolve our own directory (symlink-safe) so we can find _lib.sh and the
# sibling phase scripts even when invoked via a symlink in $PATH.
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./_lib.sh disable=SC1091
source "$S/_lib.sh"

# Allow caller to pass SESSION_DIR explicitly; otherwise mint a fresh one.
if [ "$#" -ge 1 ] && [ -n "$1" ]; then
  init_session "$1"
else
  OPT_SESSION_ID="$(date +%Y%m%d-%H%M%S)"
  init_session "$OPT_SESSIONS_ROOT/$OPT_SESSION_ID/results"
fi

# OPT_SESSION_DIR is assigned + exported by `init_session` above (defined in
# _lib.sh). The assertion below makes that contract explicit and also silences
# SC2153 (shellcheck doesn't follow the source).
: "${OPT_SESSION_DIR:?init_session must assign OPT_SESSION_DIR}"

# Phase 0
if [ -n "${IMPORT_BASELINE_RUN_ID:-}" ]; then
  "$S/import-baseline.sh" "$OPT_SESSION_DIR" \
    "$IMPORT_BASELINE_RUN_ID" "${IMPORT_BASELINE_RERUN_ID:-}"
else
  "$S/run-baseline.sh" "$OPT_SESSION_DIR"
fi

# Phase 1 → 1.5 → 1.6
"$S/run-triage.sh"        "$OPT_SESSION_DIR"
"$S/run-analyzers.sh"     "$OPT_SESSION_DIR"
"$S/assemble-targets.sh"  "$OPT_SESSION_DIR" \
  > "$OPT_SESSION_DIR/optimization-targets.json"

# Phase 2 → 3 → 4
"$S/run-optimizers.sh"    "$OPT_SESSION_DIR"
"$S/bench-experiments.sh" "$OPT_SESSION_DIR"
"$S/finalize-session.sh"  "$OPT_SESSION_DIR" \
  > "$OPT_SESSION_DIR/summary.json"

echo
echo "session: $OPT_SESSION_DIR"
echo "summary: $OPT_SESSION_DIR/summary.md"
