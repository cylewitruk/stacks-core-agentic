#!/usr/bin/env bash
# Validate that an optimization session has produced every required artifact
# under the v2 (family-first) pipeline shape.
#
# Pipeline phases this script knows about:
#   0   shell    baseline + noise floor
#   1   triage   candidates.json (family-shaped)
#   1.5 analyzer analyses/<family-id>/analysis.json (one per candidate)
#   1.7 merge    optimization-targets.json + merge-final-message.md
#   2/3 optimizer experiments/<target-id>/{implementation.md|abort.md}
#   4   shell    summary.json
#
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
  triage-final-message.md

  # Phase 1.7: merge (LLM consolidation pass)
  optimization-targets.json
  merge-final-message.md

  # Phase 4: summary
  summary.json
)
missing=()
for f in "${required[@]}"; do
  [ -s "$SESSION_DIR/$f" ] || missing+=("$f")
done

# Schema-version sanity: candidates.json and optimization-targets.json must be v2.
schema_warnings=()
if [ -s "$SESSION_DIR/candidates.json" ]; then
  v=$(jq -r '.schema_version // empty' "$SESSION_DIR/candidates.json" 2>/dev/null)
  [ "$v" = "2" ] || schema_warnings+=("candidates.json schema_version=$v (expected 2)")
fi
if [ -s "$SESSION_DIR/optimization-targets.json" ]; then
  v=$(jq -r '.schema_version // empty' "$SESSION_DIR/optimization-targets.json" 2>/dev/null)
  [ "$v" = "2" ] || schema_warnings+=("optimization-targets.json schema_version=$v (expected 2)")
fi

# Phase 1.5: every candidate family must have an analysis.json (accepted or rejected).
# Family ids come from candidates.json's `.candidates[].id`. The analyzer writes its
# output keyed by family_id (matches `id` of the corresponding candidate).
if [ -s "$SESSION_DIR/candidates.json" ]; then
  while IFS= read -r fid; do
    A="$SESSION_DIR/analyses/$fid/analysis.json"
    [ -s "$A" ] || missing+=("analyses/$fid/analysis.json")
  done < <(jq -r '.candidates[].id' "$SESSION_DIR/candidates.json")
fi

# Phase 2/3: every merged target must have either implementation.md (success path)
# or abort.md (clean exit). Targets are produced by the merge phase, so their ids
# are the canonical fix_signatures.
if [ -s "$SESSION_DIR/optimization-targets.json" ]; then
  while IFS= read -r tid; do
    EXP="$SESSION_DIR/experiments/$tid"
    if [ ! -s "$EXP/implementation.md" ] && [ ! -s "$EXP/abort.md" ]; then
      missing+=("experiments/$tid/{implementation.md|abort.md}")
    fi
  done < <(jq -r '.targets[].id' "$SESSION_DIR/optimization-targets.json")
fi

# Coverage invariant: every accepted analysis's family_id must appear in exactly
# one of (a target's merged_from, rejected_by_merge.family_id). The merge script
# enforces this at write time, but checking again here catches manual-edit drift.
if [ -s "$SESSION_DIR/optimization-targets.json" ] && [ -d "$SESSION_DIR/analyses" ]; then
  shopt -s nullglob
  ANALYSIS_FILES=( "$SESSION_DIR"/analyses/*/analysis.json )
  shopt -u nullglob
  if [ "${#ANALYSIS_FILES[@]}" -gt 0 ]; then
    accepted_ids=$(jq -s 'map(select(.status == "accepted") | .family_id)' "${ANALYSIS_FILES[@]}")
    if ! jq --argjson accepted "$accepted_ids" -e '
        ([.targets[].merged_from[]] + [(.rejected_by_merge // [])[].family_id]) as $accounted
        | (($accounted | sort) == ($accepted | sort))
          and (($accounted | length) == ($accounted | unique | length))
      ' "$SESSION_DIR/optimization-targets.json" >/dev/null 2>&1; then
      missing+=("optimization-targets.json coverage invariant (every accepted analysis must appear in exactly one target's merged_from or rejected_by_merge)")
    fi
  fi
fi

# Surface schema warnings even when nothing else is missing.
if [ "${#schema_warnings[@]}" -gt 0 ]; then
  printf 'SCHEMA WARNINGS:\n'
  printf '  %s\n' "${schema_warnings[@]}"
fi

if [ "${#missing[@]}" -gt 0 ]; then
  printf 'MISSING:\n'; printf '  %s\n' "${missing[@]}"
  exit 1
fi
echo "OK"
