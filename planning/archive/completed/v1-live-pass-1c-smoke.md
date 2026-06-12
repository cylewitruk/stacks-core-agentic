# v1: Live Pass 1c Smoke

Goal: run one real end-to-end session through the current Pass 1c flow and
collect the first live evidence for prompt quality and artifact handoffs.

> **Status:** shipped.
>
> Completed against session `20260611-172955`: full session run, Phase 3.5
> rerun after narrow fixes, publish to the bot fork, archive to the operator
> repo, and `history show` / `history list` against the archived ledger.

## Items

<a id="0018-live-pass-1c-smoke"></a>

| Item | Role | Status |
| ---- | ---- | ------ |
| `0018-live-pass-1c-smoke` | primary validation slice | shipped |

## Why

Static review says the analyzer-defined invocation protocol, Phase 1.8/3
symmetry, Phase 3.5 verdict, finalize, and publish gates line up. The remaining
uncertainty is whether real agents produce good `verification_replay` and
`results-analysis.json` artifacts from live `bench-run.json` evidence.

## Deliverable

One reviewed session transcript and artifact set that either validates the
handoffs or produces concrete follow-up items.

## Phases

### Phase 1: Preflight And Baseline

**Goal:** Establish the strict archived baseline and prove the run starts from
clean operator/tool state.

**Scope:**

- Run session preflight.
- Build/archive the baseline binary.
- Run Phase 0b baseline with aliased rerun id.

**Status:**

- [x] Run completed
- [x] Artifacts reviewed

### Phase 2: Target Selection

**Goal:** Prove triage/analyzer/merge can produce v3 targets with analyzer-owned
invocations.

**Scope:**

- Triage.
- Analyzer fan-out.
- Merge.

**Status:**

- [x] Analyzer emits schema-valid v3 `verification_replay.invocations[]`
- [x] Merge preserves invocation IDs and expected-signal axes
- [x] Artifacts reviewed

### Phase 3: Baseline Calibration

**Goal:** Prove Phase 1.8 lowers analyzer-defined invocations into baseline
benchmark runs.

**Scope:**

- Run per-target baseline calibration.
- Inspect `verify/<target>/baseline-run-ids.json` and invocation-keyed
  `bench-run.json` files.

**Status:**

- [x] Invocation ID set matches each target
- [x] Run-id files validate
- [x] Artifacts reviewed

### Phase 4: Optimization And Verification

**Goal:** Prove optimizer outputs, candidate benches, and provenance gates align.

**Scope:**

- Optimizer fan-out.
- Phase 3 candidate benches.
- Coordinator provenance sidecar checks.

**Status:**

- [x] Optimizer reports validate
- [x] Candidate invocation IDs match baseline invocation IDs
- [x] Artifacts reviewed

### Phase 5: Results Analysis And Finalize

**Goal:** Prove Phase 3.5 verdicts drive final status instead of pooled-mean
fallback math.

**Scope:**

- Results-analyzer fan-out.
- Finalize.
- Publish dry-run or gated publish decision.

**Status:**

- [x] Results-analyzer emits context-valid `results-analysis.json`
- [x] Finalize sources status and `improvement_pct` from verdicts
- [x] Publish gate reasons match verdict and confidence floor
- [x] Artifacts reviewed

## Acceptance

- Every agent output validates against its schema.
- Phase 1.8 and Phase 3 invocation id sets match every bench-eligible target's
  `verification_replay.invocations[]`.
- Results-analyzer verdicts pass context, axis, and per-invocation run-id
  cross-checks.
- Finalize and publish decisions explain any non-shipped target without relying
  on stale pooled-mean logic.
- Prompt edits needed after the smoke are captured as a follow-up iteration.

## Non-Goals

- Broad prompt rewrites before seeing live failures.
- Publishing external PR copy unless the verdict and confidence floor justify it.

## Follow-Ups

- The smoke found and we fixed two narrow handoff bugs: missing
  triage-conversation-id should not fail validation, and
  `compare_spans_between_runs.sql` needed the production
  `profiler_span_summary.wall_time_us` column.
- The smoke validated v7's DB-evidence path: the MARF target archived as
  `mixed` / caveated rather than a binary accept/reject.
- Remaining prompt calibration from more sessions stays tracked by
  `0019-prompt-hardening-live-smoke`.
