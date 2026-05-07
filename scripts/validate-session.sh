#!/usr/bin/env bash
# Validate that an optimization session has produced every required artifact.
# Exits 0 ("OK") on success; 1 with a printed missing-list on failure.
# Usage: validate-session.sh [SESSION_DIR]
set -euo pipefail
SESSION_DIR="${1:-${OPT_SESSION_DIR:?}}"

required=(
  # Phase 0: shell-owned baseline
  baseline-bench-run.json
  baseline-rerun.json
  bench-list.json
  baseline-profiler-hotspots.json
  baseline-run-id
  baseline-rerun-id

  # Phase 1: triage
  candidates.json
  triage-conversation-id

  # Phase 1.6: assembled targets
  optimization-targets.json

  # Phase 4: summary
  summary.json
)
missing=()
for f in "${required[@]}"; do
  [ -s "$SESSION_DIR/$f" ] || missing+=("$f")
done

# Phase 1.5: every candidate must have an analysis.json (accepted or rejected).
if [ -s "$SESSION_DIR/candidates.json" ]; then
  while IFS= read -r cid; do
    A="$SESSION_DIR/analyses/$cid/analysis.json"
    [ -s "$A" ] || missing+=("analyses/$cid/analysis.json")
  done < <(jq -r '.candidates[].id' "$SESSION_DIR/candidates.json")
fi

# Phase 2/3: every accepted target must have either implementation.md
# (success path) or abort.md (clean exit).
if [ -s "$SESSION_DIR/optimization-targets.json" ]; then
  while IFS= read -r tid; do
    EXP="$SESSION_DIR/experiments/$tid"
    if [ ! -s "$EXP/implementation.md" ] && [ ! -s "$EXP/abort.md" ]; then
      missing+=("experiments/$tid/{implementation.md|abort.md}")
    fi
  done < <(jq -r '.targets[].id' "$SESSION_DIR/optimization-targets.json")
fi

if [ "${#missing[@]}" -gt 0 ]; then
  printf 'MISSING:\n'; printf '  %s\n' "${missing[@]}"
  exit 1
fi
echo "OK"
