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
# Collect analyses.
# ---------------------------------------------------------------------------
shopt -s nullglob
ANALYSIS_FILES=( "$ANALYSES_DIR"/*/analysis.json )
shopt -u nullglob

# Validate each analysis.json against analysis.schema.json BEFORE constructing
# ACCEPTED_ANALYSES_JSON. Without this, a stale or malformed input (missing
# `bucket`, wrong status enum, etc.) only surfaces later as a downstream
# coverage / cross-bucket error with a misleading attribution. Failing here
# points the operator at the offending file directly.
validate_analysis_file() {
  local file="$1"

  if python3 -c 'import jsonschema' >/dev/null 2>&1; then
    if python3 - "$file" "$ANALYSIS_SCHEMA_PATH" <<'PY' 2>&1
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
      return 1
    fi
  fi

  # Structural fallback: covers the fields the merge phase consumes from
  # accepted analyses, plus the minimal contract for rejected analyses. Does
  # NOT enforce kebab-case patterns or additionalProperties:false. If you
  # need full coverage, install `jsonschema`.
  jq -e '
    def lens_ok: (. == "tx_latency") or (. == "tenure_throughput") or (. == "commit_time");
    def bucket_ok: (. == "block_processing") or (. == "block_commit");
    def risk_ok: (. == "low") or (. == "medium") or (. == "high");
    def disposition_ok:
      (.lens | lens_ok)
      and ((.status == "addressed") or (.status == "not_actionable"))
      and (if .status == "not_actionable" then (.reason | type == "string") else true end);
    def improvement_vector_ok:
      (. | type == "object")
      and (.tx_latency        | type == "number")
      and (.tenure_throughput | type == "number")
      and (.commit_time       | type == "number");
    def breakage_class_ok:
      (. == "clarity_cost_weight")
      or (. == "clarity_vm_behavior")
      or (. == "mining_flow")
      or (. == "block_validation")
      or (. == "marf_layout")
      or (. == "on_chain_format");
    def consensus_fields_ok:
      (.consensus_breaking | type == "boolean")
      and (if .consensus_breaking then
             (.breakage_class    | breakage_class_ok)
             and (.poc_implementable | type == "boolean")
             and (.consensus_writeup | type == "string")
             and (if .poc_implementable then
                    (.poc_test_scope | type == "array")
                    and (.poc_test_scope | length >= 1)
                  else
                    # poc_test_scope forbidden when poc_implementable == false.
                    (has("poc_test_scope") | not)
                  end)
             # block_validation is not exercised by stacks-bench; PoC mode is impossible.
             and (if .breakage_class == "block_validation"
                  then .poc_implementable == false
                  else true
                  end)
           else
             # When consensus_breaking == false, NO consensus-only fields may appear.
             (has("breakage_class")    | not)
             and (has("poc_implementable") | not)
             and (has("poc_test_scope") | not)
             and (has("consensus_writeup") | not)
           end);
    def target_ok:
      (.target_span | type == "string")
      and (.bucket | bucket_ok)
      and (.fix_signature | type == "string")
      and (.hotspot | type == "object")
      and (.hotspot
           | has("span") and has("self_wall_us") and has("total_wall_us")
             and has("calls") and has("location"))
      and (.files | type == "array")
      and (.evidence | type == "string")
      and (.proposed_change | type == "string")
      and (.expected_improvement | improvement_vector_ok)
      and (.risk | risk_ok)
      and (.verification_plan | type == "string")
      and consensus_fields_ok;

    if .status == "accepted" then
      (.schema_version == 2)
      and (.family_id        | type == "string")
      and (.selection_lens   | lens_ok)
      and (.lens_disposition | type == "object")
      and (.lens_disposition | disposition_ok)
      # lens_disposition.lens MUST equal the analysis-level selection_lens.
      and (.lens_disposition.lens == .selection_lens)
      and (.targets          | type == "array")
      and ((.targets | length) == 0 or all(.targets[]; target_ok))
      # Empty targets[] is valid only when lens_disposition.status is not_actionable.
      and (if (.targets | length) == 0
             then .lens_disposition.status == "not_actionable"
             else true
           end)
    elif .status == "rejected" then
      (.schema_version == 2)
      and (.family_id  | type == "string")
      and (.reason     | type == "string")
    else
      false
    end
  ' "$file" >/dev/null 2>&1
}

for af in "${ANALYSIS_FILES[@]}"; do
  if ! validate_analysis_file "$af"; then
    echo "merge-analyses: input analysis is invalid: $af" >&2
    echo "  Re-run run-analyzers.sh, or delete the offending file if it is stale." >&2
    exit 1
  fi
done

if [ "${#ANALYSIS_FILES[@]}" -eq 0 ]; then
  ACCEPTED_ANALYSES_JSON='[]'
else
  ACCEPTED_ANALYSES_JSON=$(jq -s 'map(select(.status == "accepted"))' "${ANALYSIS_FILES[@]}")
fi

ACCEPTED_COUNT=$(jq 'length' <<< "$ACCEPTED_ANALYSES_JSON")
ACCEPTED_FAMILY_IDS=$(jq '[.[].family_id]' <<< "$ACCEPTED_ANALYSES_JSON")

# Flat list of every analyzer-emitted target as a {family_id, target_index}
# pair. This is the canonical reference set for the target-level coverage
# invariant — every entry must appear in EXACTLY ONE of (a merged target's
# merged_from, OR rejected_by_merge). Analyses with zero targets contribute
# zero entries here (they're still covered by lens_dispositions[]).
ACCEPTED_TARGETS_REF=$(jq '
  [ .[] | .family_id as $fid
        | (.targets // []) | to_entries | map({family_id: $fid, target_index: .key})
  ] | flatten
' <<< "$ACCEPTED_ANALYSES_JSON")

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
    and (.lens_dispositions  | type == "array")
    and (.lens_dispositions | all(
          (.family_id | type == "string")
          and ((.lens == "tx_latency") or (.lens == "tenure_throughput") or (.lens == "commit_time"))
          and ((.status == "addressed") or (.status == "not_actionable"))
          and (if .status == "not_actionable" then (.reason | type == "string") else true end)
        ))
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: top-level schema check failed (structural)" >&2; return 1; }

  jq -e '
    def breakage_class_ok:
      (. == "clarity_cost_weight")
      or (. == "clarity_vm_behavior")
      or (. == "mining_flow")
      or (. == "block_validation")
      or (. == "marf_layout")
      or (. == "on_chain_format");

    .targets | all(
      has("id") and has("merged_from") and has("convergence_count")
      and has("target_span") and has("bucket") and has("hotspot") and has("files")
      and has("evidence") and has("proposed_change")
      and has("expected_improvement") and has("risk")
      and has("verification_plan")
      and has("consensus_breaking") and has("delivery_mode") and has("bench_eligible")
      and ((.bucket == "block_processing") or (.bucket == "block_commit"))
      and (.hotspot
           | has("span") and has("self_wall_us") and has("total_wall_us")
             and has("calls") and has("location"))
      and (.expected_improvement | type == "object")
      and (.expected_improvement.tx_latency        | type == "number")
      and (.expected_improvement.tenure_throughput | type == "number")
      and (.expected_improvement.commit_time       | type == "number")
      and (.merged_from        | type == "array")
      and (.merged_from        | length >= 1)
      and (.merged_from | all(
            (.family_id | type == "string")
            and (.target_index | type == "number")
            and (.target_index >= 0)
          ))
      and (.convergence_count == (.merged_from | length))
      and ((.risk == "low") or (.risk == "medium") or (.risk == "high"))
      and (.consensus_breaking | type == "boolean")
      and ((.delivery_mode == "normal_pr") or (.delivery_mode == "consensus_poc_pr") or (.delivery_mode == "consensus_issue"))
      and (.bench_eligible | type == "boolean")
      and (if .consensus_breaking then
             (.breakage_class | breakage_class_ok)
             and (.poc_implementable | type == "boolean")
             and (.consensus_writeup | type == "string")
             and (if .poc_implementable then
                    (.poc_test_scope | type == "array")
                    and (.poc_test_scope | length >= 1)
                  else
                    # poc_test_scope forbidden when poc_implementable == false.
                    (has("poc_test_scope") | not)
                  end)
             # block_validation is not exercised by the mining harness; PoC mode is impossible.
             and (if .breakage_class == "block_validation"
                  then .poc_implementable == false
                  else true
                  end)
           else
             # When consensus_breaking == false, NO consensus-only fields may appear.
             (has("breakage_class")    | not)
             and (has("poc_implementable") | not)
             and (has("poc_test_scope") | not)
             and (has("consensus_writeup") | not)
           end)
    )
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: per-target schema check failed (structural)" >&2; return 1; }

  # Validate rejected_by_merge entries individually, when present.
  jq -e '
    (.rejected_by_merge // []) | all(
      (.family_id    | type == "string")
      and (.target_index | type == "number")
      and (.target_index >= 0)
      and (.reason   | type == "string")
    )
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: rejected_by_merge schema check failed (structural)" >&2; return 1; }
}

# Returns 0 if every analyzer-emitted target — identified by its
# (family_id, target_index) pair — appears in exactly one of (a merged
# target's merged_from, rejected_by_merge). This is the target-level coverage
# contract under multi-target analyses (Phase 6+). Analyses with zero targets
# contribute zero entries to either side and are covered separately by
# coverage_invariant_lens_dispositions().
coverage_invariant_targets() {
  local file="$1"
  local accepted_targets="$2"  # JSON array of {family_id, target_index}

  jq --argjson accepted "$accepted_targets" -e '
    ([.targets[].merged_from[]]
     + [(.rejected_by_merge // [])[] | {family_id, target_index}]
    ) as $accounted
    | (($accounted | sort_by(.family_id, .target_index))
       == ($accepted | sort_by(.family_id, .target_index)))
      and (($accounted | length) == ($accounted | unique | length))
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: target coverage invariant failed (every (family_id, target_index) from inputs must appear in exactly one of: a merged target's merged_from, OR rejected_by_merge)" >&2; return 1; }
}

# Returns 0 if every accepted family_id appears exactly once in
# lens_dispositions[]. This is independent of the targets coverage invariant
# — an analysis with zero targets still gets one lens_dispositions entry.
coverage_invariant_lens_dispositions() {
  local file="$1"
  local accepted_ids="$2"  # JSON array of family_id strings

  jq --argjson accepted "$accepted_ids" -e '
    [(.lens_dispositions // [])[].family_id] as $present
    | (($present | sort) == ($accepted | sort))
      and (($present | length) == ($present | unique | length))
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: lens_dispositions coverage failed (every accepted family_id must appear exactly once in lens_dispositions[])" >&2; return 1; }
}

# Returns 0 if no merged target merges across buckets. For each merged target,
# every (family_id, target_index) reference in merged_from must point at an
# analyzer-emitted target whose bucket equals the merged target's bucket.
# Looks up bucket via the analyses JSON.
no_cross_bucket_merges() {
  local file="$1"
  local analyses_json="$2"

  jq --argjson analyses "$analyses_json" -e '
    # Build a map keyed by "family_id::target_index" -> bucket.
    ([
      $analyses[] | .family_id as $fid
                  | (.targets // []) | to_entries[]
                  | { key: ($fid + "::" + (.key|tostring)),
                      value: .value.bucket }
    ] | from_entries) as $bucket_by_ref
    | .targets | all(
        (.merged_from
         | map($bucket_by_ref[.family_id + "::" + (.target_index|tostring)])
         | unique) == [.bucket]
      )
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: cross-bucket merge detected (a merged target's bucket must equal the bucket of every (family_id, target_index) referenced in merged_from; cross-bucket collapse is forbidden by the merge prompt)" >&2; return 1; }
}

# Returns 0 if no merged target collapses two contributors from the same
# analysis. Each merged target's merged_from must have unique family_id values
# across its entries — two targets emitted by the same analyzer represent
# intentionally distinct findings and must remain separate after merge.
no_intra_analysis_merges() {
  local file="$1"

  jq -e '
    .targets | all(
      (.merged_from | map(.family_id) | length)
        == (.merged_from | map(.family_id) | unique | length)
    )
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: intra-analysis merge detected (a merged target's merged_from must not reference two targets from the same family_id; those are intentionally distinct findings)" >&2; return 1; }
}

# Returns 0 if no merged target collapses contributors with different
# consensus classifications. Every (family_id, target_index) referenced in a
# merged target's merged_from must point at an analyzer-emitted target with
# the same `consensus_breaking` value, and (when consensus_breaking == true)
# the same `breakage_class`, AND the merged target itself must agree.
no_cross_consensus_merges() {
  local file="$1"
  local analyses_json="$2"

  jq --argjson analyses "$analyses_json" -e '
    # Build a map keyed by "family_id::target_index" -> {consensus_breaking, breakage_class}.
    ([
      $analyses[] | .family_id as $fid
                  | (.targets // []) | to_entries[]
                  | { key: ($fid + "::" + (.key|tostring)),
                      value: { consensus_breaking: .value.consensus_breaking,
                               breakage_class: (.value.breakage_class // null) } }
    ] | from_entries) as $consensus_by_ref
    | .targets | all(
        ({ consensus_breaking: .consensus_breaking,
           breakage_class: (.breakage_class // null) } as $merged_sig
         | .merged_from
         | map($consensus_by_ref[.family_id + "::" + (.target_index|tostring)])
         | unique
         | . == [$merged_sig])
      )
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: cross-consensus merge detected (a merged target's consensus_breaking + breakage_class must equal those of every contributor in merged_from; cross-class collapse is forbidden by the merge prompt)" >&2; return 1; }
}

# Returns 0 if every merged target's delivery_mode and bench_eligible are
# correctly derived from consensus_breaking + poc_implementable, per the
# table:
#   consensus_breaking=false                          → normal_pr,        bench_eligible=true
#   consensus_breaking=true && poc_implementable=true → consensus_poc_pr, bench_eligible=false
#   consensus_breaking=true && poc_implementable=false→ consensus_issue,  bench_eligible=false
# The schema's allOf if/then chain enforces this when JSON Schema validation
# runs; this guard provides the same enforcement when the structural
# fallback is in use.
delivery_mode_correctly_derived() {
  local file="$1"

  jq -e '
    .targets | all(
      if .consensus_breaking == false then
        (.delivery_mode == "normal_pr") and (.bench_eligible == true)
      elif (.consensus_breaking == true) and (.poc_implementable == true) then
        (.delivery_mode == "consensus_poc_pr") and (.bench_eligible == false)
      elif (.consensus_breaking == true) and (.poc_implementable == false) then
        (.delivery_mode == "consensus_issue") and (.bench_eligible == false)
      else
        false
      end
    )
  ' "$file" >/dev/null 2>&1 \
    || { echo "merge-analyses: delivery_mode / bench_eligible incorrectly derived (must match the table: false→normal_pr/true; true+true→consensus_poc_pr/false; true+false→consensus_issue/false)" >&2; return 1; }
}

validate_targets_json() {
  local file="$1"
  local accepted_ids="$2"
  local accepted_targets="$3"
  local accepted_analyses="$4"
  schema_validate "$file" || return 1
  coverage_invariant_targets "$file" "$accepted_targets" || return 1
  coverage_invariant_lens_dispositions "$file" "$accepted_ids" || return 1
  no_cross_bucket_merges "$file" "$accepted_analyses" || return 1
  no_intra_analysis_merges "$file" || return 1
  no_cross_consensus_merges "$file" "$accepted_analyses" || return 1
  delivery_mode_correctly_derived "$file" || return 1
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
      targets: [],
      lens_dispositions: []
    }' > "$TARGETS_OUT"

  cat <<'EOF' > "$MSG_OUT"
# Merge phase: no-op

No accepted analyses; emitted empty targets list and empty lens_dispositions.
Coverage check trivially satisfied (0 inputs, 0 outputs, 0 rejected).
EOF

  validate_targets_json "$TARGETS_OUT" "$ACCEPTED_FAMILY_IDS" "$ACCEPTED_TARGETS_REF" "$ACCEPTED_ANALYSES_JSON" \
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
export OPTIMIZATION_TARGETS_SCHEMA_PATH BUCKET_ANCHORS_PATH
export ACCEPTED_ANALYSES_JSON
export CODEX_MERGE_MODEL

# shellcheck disable=SC2016
envsubst '$OPT_SESSION_ID $OPT_SESSION_DIR $BASELINE_RUN_ID $BASELINE_RERUN_ID $NOISE_FLOOR_PCT $OPTIMIZATION_TARGETS_SCHEMA_PATH $BUCKET_ANCHORS_PATH $CODEX_MERGE_MODEL $ACCEPTED_ANALYSES_JSON' \
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

if ! validate_targets_json "$TARGETS_OUT" "$ACCEPTED_FAMILY_IDS" "$ACCEPTED_TARGETS_REF" "$ACCEPTED_ANALYSES_JSON"; then
  echo "merge-analyses: LLM merge output failed validation. Inspect $TARGETS_OUT and $MSG_OUT, then re-run." >&2
  exit 1
fi

echo "merge-analyses: LLM merge succeeded ($ACCEPTED_COUNT inputs → $(jq '.targets | length' "$TARGETS_OUT") target(s); model=${CODEX_MERGE_MODEL})."
