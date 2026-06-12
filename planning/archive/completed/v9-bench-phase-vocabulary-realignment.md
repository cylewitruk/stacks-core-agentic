# v9: Bench Phase Vocabulary Realignment

Successor to [v8: Smoke-Informed Prompt Hardening](v8-smoke-informed-prompt-hardening.md).
Align operator-facing prose around three benchmark-stage terms before the next
live smoke.

> **Status:** shipped.
>
> Tier 1 terminology pass completed: prompts, docs, query catalog prose, and
> rendered summary/targets prose all use one term per benchmark stage. Legacy
> Rust symbols, schemas, CLI commands, artifact paths, and SQL parameter names
> were preserved per the Tier 1 contract; the legacy-name boundary is softened
> by a vocabulary note at the top of [`session/baseline.rs`](../../crates/stacks-bench-agent/src/session/baseline.rs)
> and by explicit `(legacy field name)` annotations where the new prose
> references legacy template variables. `just lint --no-sccache` clean and
> `just test --results -p stacks-bench-agent prompt` → 23/23. Tier 2 (schema
> field renames, Rust symbol renames) and Tier 3 (CLI / artifact path / SQL
> parameter renames) explicitly deferred.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0049-bench-phase-naming-realignment` | primary | shipped |

## Why

The codebase historically used "baseline" for several related but distinct
concepts. That is tolerable in legacy symbols, but confusing in prompts and
operator docs. The next live smoke should speak with one vocabulary:

- **Discovery pass** — Phase 0 session-wide range scan that feeds triage.
- **Target calibration baseline** — Phase 1.8 per-target unchanged-source run
  paired with a candidate invocation.
- **Verification bench** — Phase 3 optimized-candidate run for the same
  invocation.

## Scope

- Rewrite prompt prose in the bundled templates.
- Rewrite docs and query-catalog prose where "baseline" is ambiguous.
- Update rendered-summary prose and explanatory comments that guide operators or
  future maintainers.
- Add one explicit comment near layout/model code explaining that legacy
  artifact/API names still use `baseline`.

## Non-Goals

- No Rust symbol/module/function renames.
- No schema field renames.
- No CLI command renames (`sbagent session baseline ...` remains).
- No artifact path renames (`baseline/`, `verify/.../baseline-run-ids.json`
  remain).
- No SQL parameter renames (`:baseline_run_id` remains).

## Phases

### Phase 1: Prompt Vocabulary

**Goal:** Ensure agents use the three-term vocabulary in prompt prose while
preserving legacy JSON/path names.

**Scope:**

- Triage prompt: Phase 0 data becomes the discovery pass.
- Analyzer / merge prompts: discovery-pass evidence remains distinct from
  target calibration baselines and verification benches.
- Results-analyzer prompt: paired comparisons are target calibration baseline vs
  verification bench.
- PR/issue writer prompts: externally visible prose uses the new terms.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests, if applicable
- [x] Reviewed
- [x] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [x] Prompt prose does not use bare "baseline" when it means the discovery
      pass or a target calibration baseline.
- [x] Legacy fields/paths remain visible where needed and are annotated rather
      than renamed.
- [x] Prompt lint passes.

**Tests:**

- `just test --results -p stacks-bench-agent prompt` — 23/23 passed.

### Phase 2: Docs And Query Catalog Vocabulary

**Goal:** Make operator docs explain the three benchmark stages up front and use
the terms consistently.

**Scope:**

- `docs/architecture.md`, `docs/workflow.md`, `docs/operations.md`,
  `docs/configuration.md`, `docs/setup.md`, `docs/git-topology.md`,
  `docs/session-archive.md`, `docs/publishing.md`, `README.md`.
- `queries/README.md`, including explicit notes that
  `:baseline_run_id` means target calibration baseline run id in paired
  comparison queries.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests, if applicable
- [x] Reviewed
- [x] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [x] Docs introduce the three-term vocabulary.
- [x] Query README distinguishes discovery-pass queries from paired
      calibration-baseline vs verification-bench queries.
- [x] Legacy CLI/path names are preserved but explained.

**Tests:**

- Targeted grep sweep for ambiguous prose; remaining exact phrase hits are
  schema-facing model doc comments intentionally left unchanged to avoid
  generated-schema drift.

### Phase 3: Comments, Rendered Prose, And Guardrails

**Goal:** Clean up explanatory comments and rendered human-readable text without
changing contracts.

**Scope:**

- Module/doc comments in session orchestration and prompt context structs.
- Human-readable rendered summaries such as `candidates.md` labels.
- One guardrail comment near layout/model code documenting legacy names.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests, if applicable
- [x] Reviewed
- [x] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [x] Comments explain legacy names instead of introducing new symbol names.
- [x] `candidates.md` / summary prose labels Phase 0 data as discovery-pass
      data.
- [x] No schema, CLI, artifact path, SQL parameter, or Rust symbol rename lands.

**Tests:**

- `git diff --stat` confirms a prose/comment/prompt/docs-only pass plus
  `planning/` bookkeeping. Stable legacy symbols/paths remain.
- Targeted `rg` checks for remaining bare "baseline" prose.

## Final Validation

- [x] Prompt lint passes.
- [x] Targeted grep sweep reviewed.
- [x] No generated schemas changed.
- [x] No CLI/API/artifact path renames.
- [ ] Next smoke can verify PR bodies, summaries, and history output use the
      new vocabulary.

Validation run:

- `just test --results -p stacks-bench-agent prompt` — 23/23 passed.
- `just lint --no-sccache` — clean.

## Follow-Ups

- Tier 2/3 renames remain deferred until a forcing function exists.
- v10 candidate: `0033-maintain-command` + `0027-maintain-ledger`.
