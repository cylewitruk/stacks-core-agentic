# 0031: Triage / Merge Dedup Filter

- **id:** `0031-triage-merge-dedup-filter`
- **status:** `shipped`
- **priority:** `low`
- **iteration:** [v12: Cross-Session Dedup Filter](v12-cross-session-dedup-filter.md)
- **source:** [assets/autonomous-roadmap.md](../../../assets/autonomous-roadmap.md)

## Problem

Without cross-session memory, sbagent could repeatedly send the same structural
fix through analysis, merge, optimization, bench, and publish, even when a prior
PR or issue for that exact fix signature was still open, already merged, or had
failed repeatedly.

## Shipped

v12 adds deterministic exact-signature dedup at merge time:

- `session/dedup.rs` builds a read-only projection from `sessions.jsonl` plus
  `maintain.jsonl`.
- Archived `TargetRecord.id` is treated as the canonical historical
  `fix_signature` key.
- The merge coordinator filters blocked analyzer targets before rendering the
  LLM merge prompt.
- Deduped targets are recorded as `rejected_by_merge` rows with a closed
  `dedup:` reason:
  - `dedup:open-pr`
  - `dedup:open-issue`
  - `dedup:merged`
  - `dedup:repeated-failure`
- The validator requires coordinator-computed dedup decisions to appear exactly
  once and rejects invented or unknown `dedup:` reasons.
- `optimization-targets.json` remains the authoritative dedup record;
  `merge/final-message.md` is an operator summary.

The `dedup:open-issue` reason was added during implementation so consensus
issues are represented honestly instead of being folded into PR terminology.

## Validation

- `just lint --no-sccache`
- `just test --summary --no-sccache` — 560 tests passed.

Coverage added:

- open PR blocks;
- open issue blocks;
- merged PR blocks;
- stale PR does not hard-block;
- closed-unmerged and archived rejected / failed / aborted targets count toward
  the lifetime repeated-failure threshold;
- exact signature matching only;
- maintain events are projected by `observed_at`, not file order;
- force-push after stale re-blocks as open;
- dedup rows cannot be missing, invented, duplicated into `targets[]`, or carry
  unknown `dedup:` categories.

## Notes

Repeated-failure counts are lifetime counts in v12. A future recency window or
manual override can be added if operators need to re-attempt old signatures
after substantial upstream changes.

v12 deliberately does not introduce the deferred unified event log
(`0030-event-log-skeleton`). The existing `sessions.jsonl` + `maintain.jsonl`
substrate is sufficient for exact-signature dedup.
