# v8: Smoke-Informed Prompt Hardening

Successor to [v7: Evidence-Backed Verification](v7-evidence-backed-verification.md).
Use smoke session `20260611-172955` to tighten result-judgment and PR-writing
prompts before the autonomous maintenance substrate lands.

> **Status:** shipped.
>
> All three phases implemented + reviewed. `just lint` clean and
> `just test --summary --no-sccache` returns 511/511 (including two new v8
> contract tests). Codex review folded in a per-invocation
> `matches_expected_signal` rule update so the rule harmonizes with the new
> high-confidence policy: favorable overshoot with clean mechanism evidence
> matches, MARF-style per-invocation contradiction does not. Live-smoke
> validation against the v8 prompts is the natural successor checkpoint
> (open bullet under Final Validation).
>
> The next autonomy arc is v9 (`0033-maintain-command` + `0027-maintain-ledger`)
> and v10 (`0034-github-actions-wiring` + `0035-autonomy-hygiene`).

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0019-prompt-hardening-live-smoke` | primary | shipped |

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

- [x] Core implementation
- [x] Unit/integration tests, if applicable
- [x] Reviewed
- [x] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [x] Prompt explicitly states DB evidence is primary and `bench-run.json` is
      envelope/coarse directional context. (Existing v7 phrasing preserved
      verbatim — the v7 substring test
      `results_analyzer_prompt_uses_db_evidence_hierarchy` still passes.)
- [x] Prompt includes one mixed-verdict MARF anchor, not three full smoke
      examples. New "Calibration anchor — MARF deferred-seal mixed/medium"
      paragraph (cited to smoke session `20260611-172955`) names the
      `+1.004%` headline against an `8.0% ± 5.0%` expectation, the per-block
      range `+6.696% → -2.806%`, and the mechanism span
      `calculate_node_hashes` `+5.174%` exclusive wall.
- [x] Prompt allows high confidence for clean, direction-matching wins even when
      the original estimate was too small. (The `high` bullet now states
      explicitly: *"A magnitude overshoot is a clean win — do not demote to
      `medium` solely because the analyzer's `estimate_pct` was low."*)
- [x] Prompt tells the agent to record estimate gaps as caveats / feedback
      rather than using them as automatic demotion. (Dedicated
      `**Estimate gaps are caveats, not confidence demotions.**` paragraph
      following the confidence rubric.)
- [x] Existing prompt-lint and results-analyzer prompt tests pass.
      (`just test --results -p stacks-bench-agent prompt` → 23/23.)

**Tests:**

- [src/prompts.rs](../../crates/stacks-bench-agent/src/prompts.rs) — new
  `results_analyzer_prompt_carries_v8_calibration_anchor_and_estimate_gap_policy`
  test pins the anchor + policy substrings. Existing
  `results_analyzer_prompt_uses_db_evidence_hierarchy` test untouched and
  still passing.

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

- [x] Core implementation
- [x] Unit/integration tests, if applicable
- [x] Reviewed
- [x] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [x] `mixed` PR guidance says "shippable with caveats" or equivalent.
      (New `**Verdict framing.**` block under the `normal_pr` requirements:
      *"`verdict = "mixed"` (shippable with caveats): the change IS shippable
      — frame it that way, not as a near-rejection."* Plus explicit
      anti-pattern: don't downgrade title to `wip:` / `rfc:`, keep `perf:`.)
- [x] PR guidance gives one consistent representation for expected-signal
      match state. (Per-invocation table contract now pinned:
      `matches_expected_signal` renders as the literal string `yes` (true)
      or `no` (false) — never `true`/`false`, never prose. Column names and
      order also pinned.)
- [x] PR-writer prompt examples / lint still pass. (`bundled_templates_lint_clean`
      + new `pr_writer_prompt_pins_vocabulary_and_mixed_verdict_framing`
      tests both pass.)
- [x] Existing publish-render tests continue to pass. (Full `just test
      --summary --no-sccache` → 511/511.)

**Tests:**

- [src/prompts.rs](../../crates/stacks-bench-agent/src/prompts.rs) — new
  `pr_writer_prompt_pins_vocabulary_and_mixed_verdict_framing` test asserts
  the canonical `yes`/`no` rendering and the mixed-verdict "shippable with
  caveats" framing. Includes anti-pattern guard against `\`true\` or \`false\``
  drift.

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

- [x] Core implementation
- [x] Unit/integration tests, if applicable
- [x] Reviewed
- [x] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [x] `just lint --no-sccache` passes.
- [x] Targeted prompt tests pass. (`just test --results -p stacks-bench-agent
      prompt` → 23/23.)
- [x] Any synced operator prompt mirrors are byte-aligned with the bundled
      templates when expected. (`.sbagent/prompts/results-analyzer.md` and
      `.sbagent/prompts/pr-writer.md` are byte-identical to the bundled
      templates; verified via `diff`.)

**Tests:**

- `just lint --no-sccache` → clean.
- `just test --summary --no-sccache` → 511/511.
- `just test --results -p stacks-bench-agent prompt` → 23/23 (including the
  two new v8 contract tests).

## Final Validation

- [x] `0019-prompt-hardening-live-smoke` acceptance is represented in prompt
      prose and tests.
- [x] No schema files changed. (Verified — `git status` shows only template
      + prompts.rs + planning doc changes; no `schemas/` or `crates/.../models/`
      files modified.)
- [x] No Rust models changed.
- [x] `0047-analyzer-estimate-calibration` remains in backlog as the analyzer
      side of magnitude-estimate calibration.
- [ ] Next smoke can compare PR body consistency and confidence calibration
      against session `20260611-172955`. (Open until the next live smoke
      runs against the v8 prompts — natural successor checkpoint.)

## Follow-Ups

- `0047-analyzer-estimate-calibration` — calibrate analyzer expected-signal
  magnitude estimates once more session history exists.
- v9 candidate: `0033-maintain-command` + `0027-maintain-ledger`.
- v10 candidate: `0034-github-actions-wiring` + `0035-autonomy-hygiene`.
