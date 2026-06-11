# v7: Evidence-Backed Verification

Successor to [v6: Observability Surface](v6-observability-surface.md).
The core workflow is wired end-to-end, but the analyzer does not leave a
structured query trail and the results-analyzer prompt overweights
`bench-run.json`, which is only a coarse run envelope. v7 makes the
analyzer's baseline evidence replayable and makes Phase 3.5 judge
mechanism movement from the benchmark DB, not from pooled run summaries.

> **Status:** planned.
>
> Goal: harden the Pass 1c judgment loop before live smoke. This is prompt +
> schema + query-catalog work; no changes to benchmark execution or publish
> routing.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0038-prompt-example-concretization` | supporting prompt-lint cleanup | planned |
| `0044-evidence-backed-verification` | primary | planned |

## Why

`bench-run.json` is useful but boring. It records run success, run id,
interrupt status, and coarse totals such as execution / commit / setup time.
It does not contain the profiler span, block, transaction, or Clarity-cost
detail needed to answer the important question:

> Did the optimizer move the mechanism the analyzer predicted?

Today the analyzer can describe its reasoning in prose, but it does not
emit a structured list of DB queries, parameters, output paths, or extracted
signals. The results-analyzer therefore has to infer what to compare after
the fact and is currently prompted as if `bench-run.json` were the primary
evidence. Even before a live smoke, that is a structural verification gap:
the verification agent is under-instrumented for the judgment it must make.

## Scope

In scope:

- Add typed analyzer evidence provenance: query file, parameters, output
  path, key observation, and which verification invocations it supports.
- Carry that provenance through merge into the optimization-targets artifact
  so the results-analyzer receives the same evidence trail the analyzer used.
- Treat the optimizer as a transparent consumer: it reads
  `optimization-targets.json`, but it does not mutate, forward, or re-emit
  analyzer evidence provenance. No optimizer-report shape change is planned.
- Add paired baseline-vs-candidate SQL queries for the results-analyzer,
  keyed by `baseline_run_id` and `candidate_run_id`.
- Rewrite `results-analyzer.md` so `bench-run.json` is the run envelope and
  coarse signal, while the benchmark DB is the primary mechanism evidence.
- Keep every query run by agents logged in typed output: analyzer evidence
  queries in `analysis.json`, results-analyzer follow-up queries in
  `results-analysis.json.db_queries[]`.

Out of scope:

- Changing how Phase 1.8 or Phase 3 runs benchmarks.
- Adding new `stacks-bench` output formats.
- Replacing the query catalog with a Rust query runner. Agents may still use
  `sqlite3` and write CSVs under their output directories.
- Live smoke itself. v7 should make the smoke more meaningful; `0019` remains
  the live prompt-hardening item.

## Phases

### Phase 1: Evidence Provenance Model

**Goal:** `analysis.json` can say exactly which DB evidence the analyzer used
and what signal it extracted.

**Scope:**

- Add a typed model, likely in `models/common.rs`, for analyzer evidence
  queries. Candidate shape:

  ```rust
  pub struct EvidenceQuery {
      pub purpose: String,
      pub sql_path: PathBuf,
      pub params: BTreeMap<String, String>,
      pub output_path: String,
      pub key_observation: String,
      pub supports_invocations: Vec<String>,
  }
  ```

- Add `evidence_queries: Vec<EvidenceQuery>` to
  `AnalyzerTarget`. Require it for `delivery_mode == normal_pr` targets;
  `consensus_poc_pr` and `consensus_issue` stay exempt because they do not
  reach Phase 1.8 / Phase 3 / Phase 3.5.
- Validate:
  - non-empty `purpose`, `sql_path`, `output_path`, `key_observation`;
  - `sql_path` is a stable logical path of the form `queries/<name>.sql`,
    relative to the seeded/bundled query catalog, not to the agent sandbox
    cwd or the operator repo;
  - `sql_path` exists in the bundled query registry at model-validation time
    so hallucinated query names fail before Phase 1.8 runs;
  - `supports_invocations[]` references invocation ids present in
    `verification_replay.invocations[]`;
  - `delivery_mode == normal_pr` targets carry at least one evidence query.
- Bump `analysis.schema.json` and `optimization-targets.schema.json` from v3
  to v4. These are in-session artifacts, so v7 uses a clean cutover rather
  than adding legacy readers.
- Regenerate canonical schemas and bundled mirrors with the existing schema
  export flow. The merge artifact must preserve the evidence trail.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Analyzer output without evidence queries on a `normal_pr` target
      fails validation with a clear message.
- [ ] `supports_invocations[]` referencing an unknown invocation id fails
      validation.
- [ ] A hallucinated `sql_path` outside `queries/` or absent from the bundled
      query catalog fails validation.
- [ ] Merge preserves evidence queries from analyzer targets into merged
      targets without dropping or inventing provenance.
- [ ] Regenerated schemas and bundled mirrors are in sync.

**Tests:**

- Model validation tests in
  [models/analyze.rs](../../crates/stacks-bench-agent/src/models/analyze.rs)
  and merge/round-trip tests where appropriate.

### Phase 2: Analyzer Prompt Contract

**Goal:** The analyzer writes replayable evidence, not just prose.

**Scope:**

- Update [analyzer.md](../../crates/stacks-bench-agent/templates/analyzer.md)
  to require `evidence_queries[]` for each `normal_pr` target.
- Tell the analyzer to write query outputs under
  `analysis/<family-id>/queries/` and reference those paths from
  `analysis.json`.
- Make `key_observation` numeric and specific enough for Phase 3.5 to
  compare. Examples:
  - "baseline span p95 self-wall = 18.4ms across 9/10 samples";
  - "runtime is near-binding at 72% of block budget for contract X";
  - "commit phase is 61% of total run wall-clock".
  This is a prompt contract, not a schema-level numeric parser for v7; if
  live smoke shows fuzzy observations, split it into typed value/unit fields
  in a follow-up.
- Concretize any affected analyzer JSON examples so prompt schema lint
  can cover the new fields. This phase is expected to close
  `0038-prompt-example-concretization`.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] `sbagent prompt lint` passes.
- [ ] Marked analyzer output examples validate against
      `analysis.schema.json`.
- [ ] Prompt text tells the analyzer to log every query it relies on, not
      only "large traces".
- [ ] `0038-prompt-example-concretization` acceptance is satisfied: analyzer
      examples no longer use schema-invalid placeholder enum values and the
      marked examples pass schema-example lint.

**Tests:**

- Prompt lint / schema-example lint coverage.

### Phase 3: Paired Comparison Queries

**Goal:** Results-analyzer agents can compare baseline and candidate evidence
with one query instead of manually diffing two CSVs.

**Scope:**

- Add paired SQL files to `queries/`, for example:
  - `compare_run_summary.sql`;
  - `compare_spans_between_runs.sql`;
  - `compare_block_timing_between_runs.sql`;
  - optional Clarity-cost comparison if the current schema supports it cleanly.
- Each query accepts `:baseline_run_id` and `:candidate_run_id`; span-focused
  queries also accept `:span_id` or a span-name filter.
- Query output should include baseline value, candidate value, absolute delta,
  and percent delta with the same sign convention as `ResultsAnalysis`:
  positive means candidate faster / cheaper.
- Update `queries/README.md` with a results-analyzer section that explains
  which paired query to use for each `expected_signal.axis`.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Each new query has parameter documentation and a runnable `sqlite3`
      example.
- [ ] Minimum validation floor: every new SQL file is run through `sqlite3`
      in CI or a checked-in test harness. Prefer a small migration-backed
      fixture DB; if that is too expensive, record an explicit syntax probe
      command and distinguish syntax errors from expected missing-table errors.
- [ ] `queries/README.md` no longer describes the catalog as triage-only; it
      names analyzer and results-analyzer usage.

**Tests:**

- Prefer a lightweight SQLite fixture test if practical. Otherwise add a
  checked-in syntax-probe test / script that runs every new SQL file through
  `sqlite3` and distinguishes parser errors from expected missing-table
  errors.

### Phase 4: Results-Analyzer Prompt Rewrite

**Goal:** Phase 3.5 judges mechanism movement from DB-backed per-invocation
evidence.

**Scope:**

- Rewrite [results-analyzer.md](../../crates/stacks-bench-agent/templates/results-analyzer.md):
  - `bench-run.json` is the run envelope and coarse directional signal;
  - run ids + benchmark DB are the primary mechanism evidence;
  - analyzer `evidence_queries[]` are the baseline trail to replay or compare;
  - paired comparison queries are preferred over manual CSV diffing.
- Replace text that says `bench-run.json` has rich profile data.
- Require the agent to:
  - verify every baseline/candidate run succeeded from `bench-run.json`;
  - run paired DB comparisons for the spans/axes named by analyzer evidence;
  - log those queries in `results-analysis.json.db_queries[]`;
  - base `matches_expected_signal` on DB-backed mechanism evidence, using
    coarse run totals only as supporting context.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Prompt no longer calls `bench-run.json` primary evidence.
- [ ] Prompt explicitly maps each `EvidenceQuery.supports_invocations[]` entry
      to the result-analyzer's paired comparison work.
- [ ] Prompt still requires `results-analysis.json` to satisfy the existing
      schema and to log every DB query in `db_queries[]`.
- [ ] `sbagent prompt lint` passes.

**Tests:**

- Prompt lint. If examples are added, schema-example lint should validate them.

### Phase 5: Handoff Fixture

**Goal:** Prove the analyzer -> merge -> results-analyzer handoff in-process
before live smoke.

**Scope:**

- Add a small fixture analysis carrying `evidence_queries[]`.
- Assert merge preserves it into `optimization-targets.json`.
- Render a results-analyzer prompt for that target and assert it contains:
  - the evidence query trail;
  - baseline/candidate run-id paths;
  - paired-query instructions;
  - no "bench-run.json is primary evidence" wording.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] A fixture analyzer output with evidence provenance survives merge.
- [ ] Rendered results-analyzer prompt contains enough structured context to
      replay analyzer evidence against candidate runs.
- [ ] Existing Pass 1c fixture tests still pass after schema bumps.
- [ ] Schema-parity fixtures and bundled schema mirrors are regenerated after
      the v4 schema bumps from Phase 1.

**Tests:**

- Merge fixture tests and prompt-render tests under the existing test files,
  or a focused new integration test if that keeps setup cleaner.

## Final Validation

- [ ] `just lint --no-sccache`.
- [ ] `just test --summary --no-sccache`.
- [ ] `sbagent prompt lint` covers the updated analyzer/results-analyzer
      templates.
- [ ] Manual review of one rendered analyzer prompt and one rendered
      results-analyzer prompt confirms the evidence hierarchy:
      DB-backed mechanism evidence first, `bench-run.json` as envelope.

Live / operator (not blocking v7 ship):

- [ ] Next live smoke (`0019`) verifies that the analyzer emits useful
      evidence queries and the results-analyzer uses them without operator
      correction.

## Follow-Ups

- `0019-prompt-hardening-live-smoke` - run after v7; patch prompt text based
  on real agent behavior.
- Possible future item: add a Rust helper for executing catalog queries and
  writing CSVs if agents repeatedly make shell/SQLite mistakes.
