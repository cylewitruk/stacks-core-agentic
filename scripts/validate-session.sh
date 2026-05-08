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

# Schema-version sanity:
#   candidates.json + optimization-targets.json   → v2 (Phase 9 family-first)
#   summary.json                                  → v2 (Phase 9c delivery_mode)
schema_warnings=()
if [ -s "$SESSION_DIR/candidates.json" ]; then
  v=$(jq -r '.schema_version // empty' "$SESSION_DIR/candidates.json" 2>/dev/null)
  [ "$v" = "2" ] || schema_warnings+=("candidates.json schema_version=$v (expected 2)")
fi
if [ -s "$SESSION_DIR/optimization-targets.json" ]; then
  v=$(jq -r '.schema_version // empty' "$SESSION_DIR/optimization-targets.json" 2>/dev/null)
  [ "$v" = "2" ] || schema_warnings+=("optimization-targets.json schema_version=$v (expected 2)")
fi
if [ -s "$SESSION_DIR/summary.json" ]; then
  v=$(jq -r '.schema_version // empty' "$SESSION_DIR/summary.json" 2>/dev/null)
  [ "$v" = "2" ] || schema_warnings+=("summary.json schema_version=$v (expected 2)")
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

# Phase 2/3: every merged target must have one of three terminal markers,
# determined by its delivery_mode:
#   normal_pr / consensus_poc_pr  → implementation.md OR abort.md
#   consensus_issue               → consensus-issue.md (run-optimizers.sh
#                                   skipped the optimizer; the analyzer's
#                                   writeup is the artifact)
# Targets are produced by the merge phase, so their ids are the canonical
# fix_signatures.
if [ -s "$SESSION_DIR/optimization-targets.json" ]; then
  while IFS=$'\t' read -r tid dm; do
    EXP="$SESSION_DIR/experiments/$tid"
    case "$dm" in
      consensus_issue)
        [ -s "$EXP/consensus-issue.md" ] \
          || missing+=("experiments/$tid/consensus-issue.md (consensus_issue routing marker)")
        ;;
      *)
        if [ ! -s "$EXP/implementation.md" ] && [ ! -s "$EXP/abort.md" ]; then
          missing+=("experiments/$tid/{implementation.md|abort.md}")
        fi
        ;;
    esac
  done < <(jq -r '.targets[] | [.id, .delivery_mode] | @tsv' "$SESSION_DIR/optimization-targets.json")
fi

# Coverage invariant (Phase 6+): every analyzer-emitted target identified by
# (family_id, target_index) must appear in exactly one of:
#   - a merged target's merged_from
#   - rejected_by_merge
# The merge script enforces this at write time; checking again here catches
# manual-edit drift. ALSO checks that every accepted family_id appears once
# in lens_dispositions[] (Phase 6 propagation invariant).
if [ -s "$SESSION_DIR/optimization-targets.json" ] && [ -d "$SESSION_DIR/analyses" ]; then
  shopt -s nullglob
  ANALYSIS_FILES=( "$SESSION_DIR"/analyses/*/analysis.json )
  shopt -u nullglob
  if [ "${#ANALYSIS_FILES[@]}" -gt 0 ]; then
    accepted_ids=$(jq -s 'map(select(.status == "accepted") | .family_id)' "${ANALYSIS_FILES[@]}")
    accepted_targets=$(jq -s '
      [ .[] | select(.status == "accepted")
            | .family_id as $fid
            | (.targets // []) | to_entries
            | map({family_id: $fid, target_index: .key})
      ] | flatten' "${ANALYSIS_FILES[@]}")

    if ! jq --argjson accepted "$accepted_targets" -e '
        ([.targets[].merged_from[]]
         + [(.rejected_by_merge // [])[] | {family_id, target_index}]
        ) as $accounted
        | (($accounted | sort_by(.family_id, .target_index))
           == ($accepted | sort_by(.family_id, .target_index)))
          and (($accounted | length) == ($accounted | unique | length))
      ' "$SESSION_DIR/optimization-targets.json" >/dev/null 2>&1; then
      missing+=("optimization-targets.json target coverage invariant (every (family_id, target_index) from accepted analyses must appear once across merged_from / rejected_by_merge)")
    fi

    if ! jq --argjson accepted "$accepted_ids" -e '
        [(.lens_dispositions // [])[].family_id] as $present
        | (($present | sort) == ($accepted | sort))
          and (($present | length) == ($present | unique | length))
      ' "$SESSION_DIR/optimization-targets.json" >/dev/null 2>&1; then
      missing+=("optimization-targets.json lens_dispositions coverage (every accepted family_id must appear once in lens_dispositions[])")
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
