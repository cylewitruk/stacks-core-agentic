# 0019: Prompt Hardening From Live Smoke

- **id:** `0019-prompt-hardening-live-smoke`
- **status:** `shipped`
- **completed:** `2026-06-12`
- **iteration:** [v8: Smoke-Informed Prompt Hardening](v8-smoke-informed-prompt-hardening.md)

## Problem

Smoke session `20260611-172955` validated the end-to-end v7 evidence-backed
verification path but surfaced concrete prompt-calibration gaps:

- PR body vocabulary drifted across three real PRs (`yes` / `no` / `true` for
  the same `matches_expected_signal` boolean).
- `mixed` verdicts rendered as `accepted` in the per-target table without
  clear shippable-with-caveats framing, risking reviewer confusion between
  clean accepts and caveated mixed verdicts.
- Confidence calibration was conservative on clean direction-matching wins
  whose measured magnitude overshot the analyzer's estimate band (e.g.
  rollback-wrapper-at-block-read-cache landed `+27.22%` against a `+6% ± 4%`
  estimate and was returned at `medium` because of the magnitude gap).
- The results-analyzer's per-invocation `matches_expected_signal` rule
  defaulted to `false` for any direction-match outside tolerance, which
  contradicted the new high-confidence-for-clean-overshoots policy.

## Shipped

- `results-analyzer.md` carries the smoke-session MARF calibration anchor
  (`+1.004%` measured against `8.0% ± 5.0%` expectation; per-block range
  `+6.696% → -2.806%`; mechanism span `calculate_node_hashes` `+5.174%`
  exclusive wall) as the canonical mixed/medium example — one anchor instead
  of three.
- Confidence rubric tightened: clean direction-match with mechanism evidence
  earns `high` even on favorable magnitude overshoots; estimate gaps go in
  `caveats`, not auto-confidence-demotions. Dedicated "Estimate gaps are
  caveats, not confidence demotions" paragraph documents the policy.
- Per-invocation `matches_expected_signal` rule rewritten with five
  explicit cases (mismatch, in-band, favorable overshoot, below-band,
  per-invocation contradiction) so the rule harmonizes with the new
  confidence policy. The MARF-style mixed case is explicitly named as
  the canonical false-default.
- `pr-writer.md` pins per-invocation table vocabulary: render
  `matches_expected_signal` as the literal string `yes` (true) or `no`
  (false), never `true`/`false`, never prose. Column order and column
  names also pinned to prevent future drift.
- `pr-writer.md` adds explicit verdict framing: `accepted` reads as a
  clean win; `mixed` is framed as "shippable with caveats" not
  "near-rejection", with a required one-sentence mixed-verdict callout in
  the Summary section.

## Validation

- `crates/stacks-bench-agent/src/prompts.rs` carries two new substring tests
  that pin the v8 contract:
  - `results_analyzer_prompt_carries_v8_calibration_anchor_and_estimate_gap_policy`
  - `pr_writer_prompt_pins_vocabulary_and_mixed_verdict_framing`
- Existing v7 substring test
  `results_analyzer_prompt_uses_db_evidence_hierarchy` continues to pass
  (DB-primary hierarchy preserved).
- `just lint --no-sccache` clean; `just test --summary --no-sccache` →
  511/511.
- Operator-disk mirror `.sbagent/prompts/{results-analyzer,pr-writer}.md`
  byte-identical to the bundled templates.

## Follow-Ups

- Live-smoke validation against the v8 prompts is the natural successor
  checkpoint. Compare PR body consistency and confidence calibration
  against session `20260611-172955` in the next live run.
- `0047-analyzer-estimate-calibration` (backlog) — analyzer-side magnitude
  estimate calibration is explicitly out of v8 scope; tracked separately.
