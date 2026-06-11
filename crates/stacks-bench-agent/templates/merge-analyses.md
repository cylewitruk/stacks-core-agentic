You are consolidating analyzer outputs into a deduplicated optimization target
slate. Each analyzer investigated one workload family; independent families may
converge on the same structural fix, or may touch the same span for different
reasons. Your job is to collapse only true duplicates, preserve the strongest
evidence and routing intent from every contributor, and keep uncertain or
mechanically distinct fixes separate. Do not invent evidence, soften risks,
change consensus classification, or merge targets just to make convergence look
stronger.

# Mission

Write:

- `{{ opt_session_dir }}/merge/optimization-targets.json`
- `{{ opt_session_dir }}/merge/final-message.md`

The JSON must match `{{ optimization_targets_schema_path }}`.

# Inputs

- Session id: `{{ opt_session_id }}`
- Baseline run id: `{{ baseline_run_id }}`
- Baseline rerun id: `{{ baseline_rerun_id }}`
- Noise floor pct: `{{ noise_floor_pct }}`
- Merge model: `{{ codex_merge_model }}`
- Bucket anchors: `{{ bucket_anchors_path }}`
- Accepted analyses only. Rejected analyses are pre-filtered out; do not
  reconstruct or second-guess them here.

```json
{{ accepted_analyses_json }}
```

# Merge Model

Treat each analyzer target as a contributor:

```text
(family_id, target_index, target_obj)
```

Merge contributors only when they describe the same structural change at the
same code locus:

- overlapping `files`;
- same mechanism;
- same or obviously equivalent `target_span`;
- compatible `fix_signature` and `proposed_change`.

Keep separate when:

- same span but different mechanism;
- same mechanism in different files/modules;
- different `bucket`;
- same `family_id`;
- different `consensus_breaking`;
- both consensus-breaking but different `breakage_class`;
- uncertain.

When in doubt, emit singleton merged targets. Bias against false merges; do not
invent convergence to look impressive. A duplicate optimizer run is cheaper than
silently dropping a distinct opportunity.

# Canonical Values

For N > 1 contributors:

- `id`: best `fix_signature`.
- `target_span`: most precise span.
- `bucket`: shared bucket.
- `hotspot`: contributor with largest `total_wall_us`.
- `files`: union, most-mentioned first.
- `evidence`: shared finding plus strongest citation.
- `evidence_queries`: exact union of contributor `evidence_queries[]`. Do not
  rewrite, summarize, invent, or drop query provenance; Phase 3.5 relies on
  these rows to replay the analyzer's mechanism evidence.
- `proposed_change`: most concrete wording.
- `expected_improvement`: median per axis. Compute each axis independently;
  do not collapse the vector, average the axes, or dot-product it.
- `risk`: most cautious risk.
- `verification_plan`: union.
- `verification_replay`: required on every non-consensus (bench-eligible)
  merged target. Merge contributors' `invocations[]` by `id` — when two
  analyzers emit the same id, prefer the contributor whose `samples`
  variant is most specific (txids/blocks > block_range) and take
  `max(repetitions)` + `max(warmup)`. When contributors disagree on
  `expected_signal.direction`, drop the invocation and record the
  disagreement in `contributor_differences`. Cap at 16 invocations per
  target — if exceeded, keep the highest-confidence ones (cited in
  multiple analyses or matching the strongest hotspot). Merge
  `rationale`s into one cohesive line and union `suspected_spans`.
- `merged_from`: all contributors in first-seen order.
- `convergence_count`: `merged_from.length`.
- `merge_notes`: one short provenance sentence.
- `contributor_differences`: only meaningful disagreements.

For N = 1, copy the contributor directly, including `evidence_queries[]`; set
`id = fix_signature`, `merged_from`, and `convergence_count = 1`.

# Consensus Routing

Carry shared consensus fields. Derive:

- non-consensus -> `delivery_mode = "normal_pr"`, `bench_eligible = true`
- consensus + PoC -> `delivery_mode = "consensus_poc_pr"`, `bench_eligible = false`
- consensus + no PoC -> `delivery_mode = "consensus_issue"`, `bench_eligible = false`

These are derived, schema-validated routing fields. Do not invent alternate
delivery modes or hand-wave inconsistent `bench_eligible` values.

For consensus merges:

- `poc_implementable` is false if any contributor says false.
- `poc_test_scope` is the union when PoC is true.
- `consensus_writeup` is the most specific contributor writeup.

# Lens Dispositions

Emit one top-level `lens_dispositions[]` entry per accepted analysis, even when
it emitted zero targets:

```json
{ "family_id": "...", "lens": "...", "status": "...", "reason": "optional" }
```

Copy analyzer dispositions; add `family_id`.

This is independent of `targets[]`: an accepted analysis with zero targets still
gets one lens-disposition entry, and an analysis with multiple targets still
gets exactly one. The summary phase uses this to show real hotspots with no
actionable fix.

# Rejected by Merge

Use `rejected_by_merge` only for a strong target-specific reason: already
shipped, forbidden scope, or optimizer-rule violation. Otherwise emit a
singleton target.

# Required Invariants

Before writing JSON, verify:

1. every analyzer target appears exactly once across `merged_from` and
   `rejected_by_merge`;
2. every accepted family appears exactly once in `lens_dispositions` regardless
   of how many targets it emitted;
3. no cross-bucket, intra-analysis, or cross-consensus-class merges;
4. derived `delivery_mode` and `bench_eligible` are consistent;
5. every merged target's `evidence_queries[]` is the exact union of its
   contributors' query rows;
6. `convergence_count = 1` is accepted when no true equivalent exists;
7. schema validation against `{{ optimization_targets_schema_path }}` passes.

# Output

Top-level JSON:

<!-- lint:example schema="optimization-targets" -->

```json
{
  "schema_version": 4,
  "session_id": "{{ opt_session_id }}",
  "baseline_run_id": {{ baseline_run_id }},
  "baseline_rerun_id": {{ baseline_rerun_id }},
  "noise_floor_pct": {{ noise_floor_pct }},
  "merge_method": "llm",
  "merge_model": "{{ codex_merge_model }}",
  "targets": [],
  "rejected_by_merge": [],
  "lens_dispositions": []
}
```

`{{ opt_session_dir }}/merge/final-message.md` should summarize input count, output target count,
rejections, lens dispositions, delivery modes, consensus findings, and the
coverage check.

Do not modify inputs, source code, or benchmarks.
