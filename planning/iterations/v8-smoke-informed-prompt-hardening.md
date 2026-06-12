# v8: Smoke-Informed Prompt Hardening

Successor to [v7: Evidence-Backed Verification](../archive/completed/v7-evidence-backed-verification.md).
Use smoke session `20260611-172955` to tighten result-judgment and PR-writing
prompts before the autonomous maintenance substrate lands.

> **Status:** planned.
>
> v8 is intentionally small: apply the live-smoke lessons that are still fresh,
> without changing schemas, handoff artifacts, or analyzer estimate generation.
> The next autonomy arc is v9 (`0033-maintain-command` + `0027-maintain-ledger`)
> and v10 (`0034-github-actions-wiring` + `0035-autonomy-hygiene`).

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0019-prompt-hardening-live-smoke` | primary | planned |

## Why

The first live smoke validated the v7 evidence-backed verification path, but it
also surfaced prompt calibration issues:

- `mixed` verdicts need to read as shippable-with-caveats, not as clean accepts
  or soft rejections.
- Clean, direction-matching wins should not be demoted solely because the
  analyzer's prior magnitude estimate was low.
- PR bodies need consistent vocabulary for expected-signal matches and caveats.
- The results analyzer should preserve the v7 hierarchy: DB evidence is the
  primary mechanism evidence; `bench-run.json` is the run envelope and coarse
  directional context.

## Scope

- Update prompt text and prompt-facing examples only.
- Use hybrid calibration in `results-analyzer.md`: one concrete MARF
  mixed-verdict anchor plus compact rubric rules for clean accepts / rejects.
- Adopt confidence policy B: clean direction-match + mechanism evidence can earn
  high confidence even when the original magnitude estimate was off; the
  estimate gap becomes a caveat or analyzer-feedback signal, not an automatic
  confidence demotion.
- Update `pr-writer.md` so mixed verdicts are externally legible as
  shippable-with-caveats.

## Non-Goals

- No schema changes.
- No `AnalyzerTarget` / `ResultsAnalysis` model changes.
- No analyzer estimate-generation calibration. That is tracked separately as
  [`0047-analyzer-estimate-calibration`](../backlog.md#0047-analyzer-estimate-calibration).
- No history-renderer changes; `mixed` already surfaces in `history show`.
- No maintain / GitHub reconciliation work; that belongs in v9.

## Phases

### Phase 1: Results-Analyzer Verdict Calibration

**Goal:** Teach the results analyzer to preserve v7's evidence hierarchy while
making better confidence calls on clean wins and mixed outcomes.

**Scope:**

- Add one compact MARF anchor from smoke session `20260611-172955` as the
  concrete mixed-verdict example.
- Add rubric rules for:
  - clean direction-match + acceptable samples + material movement;
  - mixed direction-match where magnitude / distribution is suspect;
  - clean rejection when additional DB evidence shows no mechanism movement.
- Preserve the additional-query guidance: up to ten extra targeted queries
  before an observation-level justification is required.
- State that estimate gaps are caveats or analyzer-feedback signals, not
  automatic confidence demotions.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests, if applicable
- [ ] Reviewed
- [ ] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [ ] Prompt explicitly states DB evidence is primary and `bench-run.json` is
      envelope/coarse directional context.
- [ ] Prompt includes one mixed-verdict MARF anchor, not three full smoke
      examples.
- [ ] Prompt allows high confidence for clean, direction-matching wins even when
      the original estimate was too small.
- [ ] Prompt tells the agent to record estimate gaps as caveats / feedback
      rather than using them as automatic demotion.
- [ ] Existing prompt-lint and results-analyzer prompt tests pass.

**Tests:**

- `crates/stacks-bench-agent/src/prompts.rs` prompt substring tests, adjusted or
  extended as needed.

### Phase 2: PR Body Verdict Clarity

**Goal:** Make externally visible PR bodies match the verdict semantics a human
reviewer needs.

**Scope:**

- Update `pr-writer.md` to treat `mixed` as shippable-with-caveats.
- Keep caveats easy to find without making a mixed PR read like a rejection.
- Standardize expected-signal vocabulary so smoke outputs do not drift between
  `yes`, `no`, `true`, and prose for the same concept.
- Ensure clean accepts remain concise and do not inherit mixed-verdict caution
  language.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests, if applicable
- [ ] Reviewed
- [ ] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [ ] `mixed` PR guidance says "shippable with caveats" or equivalent.
- [ ] PR guidance gives one consistent representation for expected-signal
      match state.
- [ ] PR-writer prompt examples / lint still pass.
- [ ] Existing publish-render tests continue to pass.

**Tests:**

- Prompt lint.
- Existing publish / prompt tests that cover PR-writer rendering.

### Phase 3: Prompt Sync And Smoke Fixture Check

**Goal:** Verify the prompt changes are reflected in both bundled and synced
operator-facing prompt surfaces.

**Scope:**

- Run prompt lint against bundled templates.
- Run or update targeted prompt tests for results-analyzer / PR-writer text.
- Sync `.sbagent/prompts/` mirrors if the workflow requires it before the next
  operator smoke.
- Do not run another benchmark smoke in this iteration; the next live session is
  the validation witness.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests, if applicable
- [ ] Reviewed
- [ ] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [ ] `just lint --no-sccache` passes.
- [ ] Targeted prompt tests pass.
- [ ] Any synced operator prompt mirrors are byte-aligned with the bundled
      templates when expected.

**Tests:**

- `just lint --no-sccache`
- Targeted `just test --summary <prompt-test-filter>` if a narrow filter is
  available.

## Final Validation

- [ ] `0019-prompt-hardening-live-smoke` acceptance is represented in prompt
      prose and tests.
- [ ] No schema files changed.
- [ ] No Rust models changed.
- [ ] `0047-analyzer-estimate-calibration` remains in backlog as the analyzer
      side of magnitude-estimate calibration.
- [ ] Next smoke can compare PR body consistency and confidence calibration
      against session `20260611-172955`.

## Follow-Ups

- `0047-analyzer-estimate-calibration` — calibrate analyzer expected-signal
  magnitude estimates once more session history exists.
- v9 candidate: `0033-maintain-command` + `0027-maintain-ledger`.
- v10 candidate: `0034-github-actions-wiring` + `0035-autonomy-hygiene`.
