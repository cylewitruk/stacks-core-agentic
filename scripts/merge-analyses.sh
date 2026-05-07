#!/usr/bin/env bash
# Phase 1.7: merge accepted family analyses into optimization-targets.json
# via an LLM consolidation pass.
#
# Reads:
#   $OPT_SESSION_DIR/candidates.json                (for session-level fields)
#   $OPT_SESSION_DIR/analyses/*/analysis.json       (one per family)
#
# Writes:
#   $OPT_SESSION_DIR/merge-prompt.md                (rendered LLM prompt)
#   $OPT_SESSION_DIR/merge-events.jsonl             (Codex JSONL events)
#   $OPT_SESSION_DIR/merge-stderr.log
#   $OPT_SESSION_DIR/merge-final-message.md         (audit summary)
#   $OPT_SESSION_DIR/merge-conversation-id
#   $OPT_SESSION_DIR/optimization-targets.json      (the contract — schema v2)
#
# The LLM merge is the only path. If the call fails or its output fails
# validation, the script exits non-zero — Phase 1.7 must succeed for the
# pipeline to proceed. If you see frequent failures, the right fix is to
# investigate the prompt / model / inputs, not to silently degrade.
#
# Special case: zero accepted analyses → emits a valid empty targets list and
# exits 0 without invoking Codex.
#
# Knobs:
#   CODEX_MERGE_MODEL          model id for the LLM merge call
#                              (default: gpt-5.3-codex-spark)
#   CODEX_EXEC_TIMEOUT_SEC     per-codex-exec timeout
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"
assert_required_tools

CANDIDATES="$OPT_SESSION_DIR/candidates.json"
ANALYSES_DIR="$OPT_SESSION_DIR/analyses"
TARGETS_OUT="$OPT_SESSION_DIR/optimization-targets.json"
MSG_OUT="$OPT_SESSION_DIR/merge-final-message.md"

[ -s "$CANDIDATES" ] || { echo "merge-analyses: missing candidates.json" >&2; exit 2; }
[ -d "$ANALYSES_DIR" ] || { echo "merge-analyses: missing analyses/ dir" >&2; exit 2; }

# ---------------------------------------------------------------------------
# Pull session-level fields from candidates.json so the assembled targets file
# is self-contained.
# ---------------------------------------------------------------------------
SESSION_ID=$(jq -r '.session_id' "$CANDIDATES")
BASELINE_RUN_ID=$(jq -r '.baseline_run_id' "$CANDIDATES")
BASELINE_RERUN_ID=$(jq -r '.baseline_rerun_id' "$CANDIDATES")
NOISE_FLOOR_PCT=$(jq -r '.noise_floor_pct' "$CANDIDATES")

# ---------------------------------------------------------------------------
# Collect accepted analyses.
# ---------------------------------------------------------------------------
shopt -s nullglob
ANALYSIS_FILES=( "$ANALYSES_DIR"/*/analysis.json )
shopt -u nullglob

if [ "${#ANALYSIS_FILES[@]}" -eq 0 ]; then
  ACCEPTED_ANALYSES_JSON='[]'
else
  ACCEPTED_ANALYSES_JSON=$(jq -s 'map(select(.status == "accepted"))' "${ANALYSIS_FILES[@]}")
fi

ACCEPTED_COUNT=$(jq 'length' <<< "$ACCEPTED_ANALYSES_JSON")
ACCEPTED_FAMILY_IDS=$(jq '[.[].family_id]' <<< "$ACCEPTED_ANALYSES_JSON")

# ---------------------------------------------------------------------------
# Schema validation. Prefers real JSON Schema validation via python +
# jsonschema (covers patterns, enums, additionalProperties, etc). Falls back
# to a structural jq check that covers the most-likely LLM mistakes
# (missing fields, wrong types, wrong enums). The fallback is not 100%
# schema-equivalent — it does NOT enforce the kebab-case `id` pattern,
# additionalProperties:false, or const fields. If you need full coverage,
# install `jsonschema`:
#
#   pip install --user jsonschema   # or: pipx install jsonschema
# ---------------------------------------------------------------------------
schema_validate() {
  local file="$1"

  if python3 -c 'import jsonschema' >/dev/null 2>&1; then
    if python3 - "$file" "$OPTIMIZATION_TARGETS_SCHEMA_PATH" <<'PY' 2>&1
import json, sys
from jsonschema import Draft202012Validator
with open(sys.argv[1]) as f: doc = json.load(f)
with open(sys.argv[2]) as f: schema = json.load(f)
errs = list(Draft202012Validator(schema).iter_errors(doc))
if errs:
    for e in errs:
        path = "/".join(map(str, e.absolute_path)) or "<root>"
        sys.stderr.write(f"schema: {path}: {e.message}\n")
    sys.exit(1)
PY
    then
      return 0
    else
      echo "merge-analyses: schema validation failed (real JSON Schema)" >&2
      return 1
    fi
  fi

  # Structural fallback (jq).
  jq -e '
    .schema_version == 2
    and (.session_id         | type == "string")
    and (.baseline_run_id    | type == "number")
    and (.baseline_rerun_id  | type == "number")
    and (.noise_floor_pct    | type == "number")
    and (.merge_method == "llm")
    and (.merge_model        | type == "string")
    and (.targets            | type == "array")
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: top-level schema check failed (structural)" >&2; return 1; }

  jq -e '
    .targets | all(
      has("id") and has("merged_from") and has("convergence_count")
      and has("target_span") and has("hotspot") and has("files")
      and has("evidence") and has("proposed_change")
      and has("expected_improvement_pct") and has("risk")
      and has("verification_plan")
      and (.hotspot
           | has("span") and has("self_wall_us") and has("total_wall_us")
             and has("calls") and has("location"))
      and (.merged_from        | type == "array")
      and (.merged_from        | length >= 1)
      and (.convergence_count == (.merged_from | length))
      and ((.risk == "low") or (.risk == "medium") or (.risk == "high"))
    )
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: per-target schema check failed (structural)" >&2; return 1; }
}

# Returns 0 if every family_id in $2 (JSON array) is accounted for in exactly
# one of (a target's merged_from, rejected_by_merge.family_id). This is the
# merge-specific contract — independent of schema.
coverage_invariant() {
  local file="$1"
  local accepted_ids="$2"

  jq --argjson accepted "$accepted_ids" -e '
    ([.targets[].merged_from[]] + [(.rejected_by_merge // [])[].family_id]) as $accounted
    | (($accounted | sort) == ($accepted | sort))
      and (($accounted | length) == ($accounted | unique | length))
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: coverage invariant failed (every accepted family_id must appear in exactly one of: a target's merged_from, OR rejected_by_merge)" >&2; return 1; }
}

validate_targets_json() {
  local file="$1"
  local accepted_ids="$2"
  schema_validate "$file" || return 1
  coverage_invariant "$file" "$accepted_ids" || return 1
}

# ---------------------------------------------------------------------------
# Empty-input shortcut: nothing to merge.
# ---------------------------------------------------------------------------
if [ "$ACCEPTED_COUNT" -eq 0 ]; then
  jq -n \
    --arg session_id        "$SESSION_ID" \
    --argjson baseline_run_id   "$BASELINE_RUN_ID" \
    --argjson baseline_rerun_id "$BASELINE_RERUN_ID" \
    --argjson noise_floor_pct   "$NOISE_FLOOR_PCT" \
    '{
      schema_version: 2,
      session_id: $session_id,
      baseline_run_id: $baseline_run_id,
      baseline_rerun_id: $baseline_rerun_id,
      noise_floor_pct: $noise_floor_pct,
      merge_method: "llm",
      merge_model: "",
      targets: []
    }' > "$TARGETS_OUT"

  cat <<'EOF' > "$MSG_OUT"
# Merge phase: no-op

No accepted analyses; emitted empty targets list. Coverage check trivially
satisfied (0 inputs, 0 outputs, 0 rejected).
EOF

  validate_targets_json "$TARGETS_OUT" "$ACCEPTED_FAMILY_IDS" \
    || { echo "merge-analyses: empty-input output failed validation (this should not happen)" >&2; exit 1; }

  echo "merge-analyses: 0 accepted analyses; emitted empty targets list."
  exit 0
fi

# ---------------------------------------------------------------------------
# LLM merge. Sole path; failure is fatal.
# ---------------------------------------------------------------------------
assert_codex_compatible
assert_codex_writable

CODEX_MERGE_MODEL="${CODEX_MERGE_MODEL:-gpt-5.3-codex-spark}"

export OPT_SESSION_ID OPT_SESSION_DIR
export BASELINE_RUN_ID BASELINE_RERUN_ID NOISE_FLOOR_PCT
export OPTIMIZATION_TARGETS_SCHEMA_PATH
export ACCEPTED_ANALYSES_JSON
export CODEX_MERGE_MODEL

# shellcheck disable=SC2016
envsubst '$OPT_SESSION_ID $OPT_SESSION_DIR $BASELINE_RUN_ID $BASELINE_RERUN_ID $NOISE_FLOOR_PCT $OPTIMIZATION_TARGETS_SCHEMA_PATH $CODEX_MERGE_MODEL $ACCEPTED_ANALYSES_JSON' \
  < "$PROMPTS_DIR/merge-analyses.md" \
  > "$OPT_SESSION_DIR/merge-prompt.md"

mapfile -t CODEX_TOP_LEVEL_ARGS < <(codex_top_level_args "$CODEX_MERGE_MODEL" "${CODEX_MERGE_REASONING_EFFORT:-${CODEX_REASONING_EFFORT:-}}")
mapfile -t CODEX_EXEC_ARGS    < <(codex_exec_args)

# Clear stale SCRATCH artifacts only. Canonical outputs (TARGETS_OUT, MSG_OUT)
# are NOT pre-deleted, so a prior valid result is preserved if the LLM call
# fails — but to ensure we don't false-succeed using stale prior output, we
# stamp a freshness marker before invoking codex and require both canonical
# outputs to be newer than the marker on success.
rm -f "$OPT_SESSION_DIR/merge-events.jsonl" \
      "$OPT_SESSION_DIR/merge-stderr.log" \
      "$OPT_SESSION_DIR/merge-conversation-id"

FRESHNESS_MARKER=$(mktemp)
trap 'rm -f "$FRESHNESS_MARKER"' EXIT

if ! run_with_timeout "${CODEX_EXEC_TIMEOUT_SEC:-3600}" \
     codex \
       "${CODEX_TOP_LEVEL_ARGS[@]}" \
     exec \
       --skip-git-repo-check \
       --cd "$OPT_SESSION_DIR" \
       --add-dir "$FRAMEWORK_ROOT" \
       "${CODEX_EXEC_ARGS[@]}" \
       --output-last-message "$MSG_OUT" \
       "$(cat "$OPT_SESSION_DIR/merge-prompt.md")" \
       > "$OPT_SESSION_DIR/merge-events.jsonl" \
       2> "$OPT_SESSION_DIR/merge-stderr.log"; then
  echo "merge-analyses: codex exec returned non-zero (timeout or runtime error). See merge-stderr.log for details." >&2
  exit 1
fi

capture_codex_conversation_id "$OPT_SESSION_DIR/merge-events.jsonl" \
  > "$OPT_SESSION_DIR/merge-conversation-id" || true

# Freshness checks: codex returned 0, but did it actually write the artifacts
# this invocation requires? A stale prior file would otherwise pass the
# existence + schema checks below and produce a false success.
[ -s "$TARGETS_OUT" ] \
  || { echo "merge-analyses: optimization-targets.json missing or empty" >&2; exit 1; }
[ "$TARGETS_OUT" -nt "$FRESHNESS_MARKER" ] \
  || { echo "merge-analyses: codex exited 0 but did not refresh optimization-targets.json (stale prior file detected)" >&2; exit 1; }

[ -s "$MSG_OUT" ] \
  || { echo "merge-analyses: merge-final-message.md missing or empty (the prompt requires the agent to write an audit summary)" >&2; exit 1; }
[ "$MSG_OUT" -nt "$FRESHNESS_MARKER" ] \
  || { echo "merge-analyses: codex exited 0 but did not refresh merge-final-message.md (stale prior file detected)" >&2; exit 1; }

if ! validate_targets_json "$TARGETS_OUT" "$ACCEPTED_FAMILY_IDS"; then
  echo "merge-analyses: LLM merge output failed validation. Inspect $TARGETS_OUT and $MSG_OUT, then re-run." >&2
  exit 1
fi

echo "merge-analyses: LLM merge succeeded ($ACCEPTED_COUNT inputs → $(jq '.targets | length' "$TARGETS_OUT") target(s); model=${CODEX_MERGE_MODEL})."
