#!/usr/bin/env bash
# Bench-agent pipeline orchestrator. Calls each phase script in order.
#
# Each phase reads its inputs from files in SESSION_DIR and writes outputs
# back. You can run any phase script directly for a controlled walkthrough;
# this script just chains them.
#
# Phase 0a: run-baseline.sh        (or import-baseline.sh — see env vars below)
# Phase 1:  run-triage.sh          → candidates.json (family-shaped, schema v2)
# Phase 1.5: run-analyzers.sh      → analyses/<family-id>/analysis.json
# Phase 1.7: merge-analyses.sh     → optimization-targets.json (LLM consolidation)
# Phase 2:  run-optimizers.sh      → experiments/<target-id>/{implementation,abort}.md
# Phase 3:  bench-experiments.sh   → experiments/<target-id>/run-N/bench-run.json
# Phase 4:  finalize-session.sh    → summary.{json,md}
# Phase 5:  generate-pr-artifacts.sh + publish-accepted.sh (optional)
#
# Env vars:
#   IMPORT_BASELINE_RUN_ID    if set, import this existing run id instead of
#                             running a fresh baseline benchmark.
#   IMPORT_BASELINE_RERUN_ID  optional; passed to import-baseline.sh.
#   STACKS_BENCH_PARALLEL_ANALYZERS, STACKS_BENCH_PARALLEL_AGENTS  (see phase scripts)
#   CODEX_MERGE_MODEL         model id for the Phase 1.7 merge call
#                             (see scripts/merge-analyses.sh; default: gpt-5.3-codex-spark)
#   SKIP_CARGO_CLEAN          (see bench-experiments.sh)
#   PUBLISH_ACCEPTED_PRS      (see Phase 5)
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

# Phase 1 → 1.5 → 1.7
# (assemble-targets.sh from v1 has been removed; the LLM merge in Phase 1.7
# replaces it. The merge script writes optimization-targets.json directly.)
"$S/run-triage.sh"        "$OPT_SESSION_DIR"
"$S/run-analyzers.sh"     "$OPT_SESSION_DIR"
"$S/merge-analyses.sh"    "$OPT_SESSION_DIR"

# Phase 2 → 3 → 4
"$S/run-optimizers.sh"    "$OPT_SESSION_DIR"
"$S/bench-experiments.sh" "$OPT_SESSION_DIR"
"$S/finalize-session.sh"  "$OPT_SESSION_DIR" \
  > "$OPT_SESSION_DIR/summary.json"

# Optional Phase 5: generate and publish draft PRs for accepted improvements.
# `sudo -H` resets HOME to the publisher's home directory so $HOME-relative
# paths in publish-accepted.sh resolve to the publisher's filesystem rather
# than the invoking agent user's. Sudoers must allow the agent user to run
# this script as the publisher with NOPASSWD — see scripts/setup-publisher.sh.
if [ "${PUBLISH_ACCEPTED_PRS:-0}" = "1" ]; then
  "$S/generate-pr-artifacts.sh" "$OPT_SESSION_DIR"
  sudo -H -u "${PUBLISH_SUDO_USER:-publisher}" \
    "$S/publish-accepted.sh" "$OPT_SESSION_DIR"
fi

echo
echo "session: $OPT_SESSION_DIR"
echo "summary: $OPT_SESSION_DIR/summary.md"
