# v1: Live Pass 1c Smoke

Goal: run one real end-to-end session through the current Pass 1c flow and
collect the first live evidence for prompt quality and artifact handoffs.

> **Status:** blocked (awaiting NVMe-with-chainstate availability).
>
> This iteration validates the system as implemented; no engineering
> work remains to start it. v2, v3, and v4 are all code-complete and
> route their live-validation bullets through this same smoke. Prompt
> hardening and other follow-up work should become new numbered
> backlog items unless it is tiny and directly blocks the smoke.

## Items

<a id="0018-live-pass-1c-smoke"></a>

| Item | Role | Status |
| ---- | ---- | ------ |
| `0018-live-pass-1c-smoke` | primary validation slice | blocked |

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

- [ ] Run completed
- [ ] Artifacts reviewed

### Phase 2: Target Selection

**Goal:** Prove triage/analyzer/merge can produce v3 targets with analyzer-owned
invocations.

**Scope:**

- Triage.
- Analyzer fan-out.
- Merge.

**Status:**

- [ ] Analyzer emits schema-valid v3 `verification_replay.invocations[]`
- [ ] Merge preserves invocation IDs and expected-signal axes
- [ ] Artifacts reviewed

### Phase 3: Baseline Calibration

**Goal:** Prove Phase 1.8 lowers analyzer-defined invocations into baseline
benchmark runs.

**Scope:**

- Run per-target baseline calibration.
- Inspect `verify/<target>/baseline-run-ids.json` and invocation-keyed
  `bench-run.json` files.

**Status:**

- [ ] Invocation ID set matches each target
- [ ] Run-id files validate
- [ ] Artifacts reviewed

### Phase 4: Optimization And Verification

**Goal:** Prove optimizer outputs, candidate benches, and provenance gates align.

**Scope:**

- Optimizer fan-out.
- Phase 3 candidate benches.
- Coordinator provenance sidecar checks.

**Status:**

- [ ] Optimizer reports validate
- [ ] Candidate invocation IDs match baseline invocation IDs
- [ ] Artifacts reviewed

### Phase 5: Results Analysis And Finalize

**Goal:** Prove Phase 3.5 verdicts drive final status instead of pooled-mean
fallback math.

**Scope:**

- Results-analyzer fan-out.
- Finalize.
- Publish dry-run or gated publish decision.

**Status:**

- [ ] Results-analyzer emits context-valid `results-analysis.json`
- [ ] Finalize sources status and `improvement_pct` from verdicts
- [ ] Publish gate reasons match verdict and confidence floor
- [ ] Artifacts reviewed

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

- `0019-prompt-hardening-live-smoke` if the smoke identifies prompt-quality
  issues.
