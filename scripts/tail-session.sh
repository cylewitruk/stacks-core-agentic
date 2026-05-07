#!/usr/bin/env bash
# Multiplex every JSONL/stderr stream produced by a bench-agent session.
# Usage: tail-session.sh [SESSION_DIR]
# Defaults to $OPT_SESSION_DIR if no arg is given.
set -euo pipefail
SESSION_DIR="${1:-${OPT_SESSION_DIR:?need OPT_SESSION_DIR or arg}}"

# Tail everything that exists; -F handles files appearing later (analyzer
# and subagent dirs are created on the fly during phases 1.5 and 2).
exec tail -F \
  "$SESSION_DIR/triage-stderr.log" \
  "$SESSION_DIR/triage-events.jsonl" \
  "$SESSION_DIR/analyses"/*/analyzer-stderr.log \
  "$SESSION_DIR/analyses"/*/analyzer-events.jsonl \
  "$SESSION_DIR/experiments"/*/subagent-stderr.log \
  "$SESSION_DIR/experiments"/*/subagent-events.jsonl \
  "$SESSION_DIR/experiments"/*/cargo-build.stderr.log \
  "$SESSION_DIR/experiments"/*/run-*/bench-run.stderr.log \
  2>/dev/null
