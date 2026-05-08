#!/usr/bin/env bash
# Phase 1: triage agent. Reads baseline-* artifacts; emits candidates.json.
#
# Inputs (in SESSION_DIR):
#   baseline-run-id, baseline-rerun-id     (from Phase 0)
#   baseline-profiler-hotspots.json
#   bench-list.json
#
# Outputs (in SESSION_DIR):
#   triage-prompt.md            (rendered prompt)
#   triage-events.jsonl
#   triage-stderr.log
#   triage-final-message.md
#   triage-conversation-id
#   candidates.json             (the agent's primary output)
#   candidates.md               (human view, derived)
set -euo pipefail
# shellcheck source=./_lib.sh disable=SC1091
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_lib.sh"
init_session "$@"
assert_required_tools
assert_codex_compatible
assert_codex_writable

BASELINE_RUN_ID="$(cat "$OPT_SESSION_DIR/baseline-run-id")"
BASELINE_RERUN_ID="$(cat "$OPT_SESSION_DIR/baseline-rerun-id")"
export BASELINE_RUN_ID BASELINE_RERUN_ID

PRECOMPUTED_NOISE_FLOOR_PCT=""
if [ -f "$OPT_SESSION_DIR/baseline-noise-floor-pct" ]; then
  PRECOMPUTED_NOISE_FLOOR_PCT="$(cat "$OPT_SESSION_DIR/baseline-noise-floor-pct")"
fi
export PRECOMPUTED_NOISE_FLOOR_PCT

# envsubst's SHELL-FORMAT arg is a literal list of $VAR tokens; single quotes
# are required so the shell doesn't expand them before envsubst sees them.
# shellcheck disable=SC2016
envsubst '$OPT_SESSION_ID $OPT_SESSION_DIR $STACKS_BENCH_DATA_DIR $BASE $BASELINE_RUN_ID $BASELINE_RERUN_ID $PRECOMPUTED_NOISE_FLOOR_PCT $NON_TARGETS_PATH $BUCKET_ANCHORS_PATH $CANDIDATES_SCHEMA_PATH $QUERIES_DIR $STACKS_BENCH_AXIS_WEIGHTS' \
  < "$PROMPTS_DIR/triage.md" \
  > "$OPT_SESSION_DIR/triage-prompt.md"

# codex CLI 0.128.0: --ask-for-approval and -m are TOP-LEVEL flags (before
# `exec`). --skip-git-repo-check is needed because the cwd here is a session
# results dir, not a git repo. --search dropped: triage doesn't need web
# search and removing it cuts a network dependency / source of nondeterminism.
# Add the framework root plus any env-overridden data/code roots explicitly so
# the prompt can still read them even if the operator moves them elsewhere.
mapfile -t CODEX_TOP_LEVEL_ARGS < <(codex_top_level_args "${CODEX_MODEL:-gpt-5.5}")
mapfile -t CODEX_EXEC_ARGS < <(codex_exec_args)

run_with_timeout "${CODEX_EXEC_TIMEOUT_SEC:-3600}" \
  codex \
    "${CODEX_TOP_LEVEL_ARGS[@]}" \
  exec \
    --skip-git-repo-check \
    --cd "$OPT_SESSION_DIR" \
    --add-dir "$FRAMEWORK_ROOT" \
    --add-dir "$STACKS_BENCH_DATA_DIR" \
    --add-dir "$BASE" \
    "${CODEX_EXEC_ARGS[@]}" \
    --output-last-message "$OPT_SESSION_DIR/triage-final-message.md" \
    "$(cat "$OPT_SESSION_DIR/triage-prompt.md")" \
  > "$OPT_SESSION_DIR/triage-events.jsonl" \
  2> "$OPT_SESSION_DIR/triage-stderr.log"

capture_codex_conversation_id "$OPT_SESSION_DIR/triage-events.jsonl" \
  > "$OPT_SESSION_DIR/triage-conversation-id"

if [ ! -s "$OPT_SESSION_DIR/candidates.json" ]; then
  echo "Triage did not emit candidates.json. See triage-final-message.md." >&2
  exit 2
fi

# Validate candidates.json against the canonical schema before downstream
# phases consume it. selection_lens (Phase 5+), bucket hints, and the family
# kind discriminator all matter to the analyzer fanout — a malformed triage
# artifact otherwise stalls or produces confusing analyzer behavior. Mirrors
# the input-validation pattern in scripts/merge-analyses.sh.
validate_candidates_file() {
  local file="$1"

  if python3 -c 'import jsonschema' >/dev/null 2>&1; then
    if python3 - "$file" "$CANDIDATES_SCHEMA_PATH" <<'PY' 2>&1
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

  # Structural fallback: covers the fields downstream phases consume. Does
  # NOT enforce kebab-case `id` patterns or additionalProperties:false. If
  # you need full coverage, install `jsonschema`.
  jq -e '
    def lens_ok: (. == "tx_latency") or (. == "tenure_throughput") or (. == "commit_time");
    def kind_ok: (. == "tx_family") or (. == "block_family") or (. == "contract_family");
    def bucket_ok: (. == "block_processing") or (. == "block_commit");

    (.schema_version == 2)
    and (.session_id        | type == "string")
    and (.baseline_run_id   | type == "number")
    and (.baseline_rerun_id | type == "number")
    and (.noise_floor_pct   | type == "number")
    and (.candidates        | type == "array")
    and (.candidates | all(
          (.id   | type == "string")
          and (.kind | kind_ok)
          and (.selection_lens | lens_ok)
          and (.representative_ids | type == "object")
          and (.rationale | type == "string")
          and (if has("bucket") then (.bucket | bucket_ok) else true end)
        ))
  ' "$file" >/dev/null 2>&1
}

if ! validate_candidates_file "$OPT_SESSION_DIR/candidates.json"; then
  echo "run-triage: candidates.json failed schema validation. Inspect $OPT_SESSION_DIR/candidates.json and triage-final-message.md, then re-run." >&2
  exit 1
fi

N=$(jq '.candidates | length' "$OPT_SESSION_DIR/candidates.json")
echo "candidates: $N"
if [ "$N" -eq 0 ]; then
  echo "Triage returned zero candidates. Downstream phases will no-op." >&2
fi
