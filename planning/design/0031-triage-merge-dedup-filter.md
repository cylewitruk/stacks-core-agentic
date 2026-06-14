# Design: Triage / Merge Dedup Filter

- **id:** `0031-triage-merge-dedup-filter`
- **status:** `planned`
- **priority:** `low`
- **iteration:** [v12: Cross-Session Dedup Filter](../iterations/v12-cross-session-dedup-filter.md)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Without cross-session memory, sbagent can repeatedly send the same structural
fix through analysis and optimization.

## Design

At merge start, read a dedup projection from `sessions.jsonl` plus
`maintain.jsonl` and drop analyzer targets whose exact `fix_signature` is
already:

- open in a non-stale PR;
- merged upstream;
- tried unsuccessfully at least `dedup_failure_threshold` times.

v12 implements this without the deferred unified event log: the coordinator
precomputes dedup decisions, removes blocked targets from the merge prompt
input, and appends deterministic `rejected_by_merge` rows with a stable
`dedup:` reason before validation and write. The closed reasons are
`dedup:open-pr`, `dedup:merged`, and `dedup:repeated-failure`. Stale open PRs
are retained as context but do not hard-block.

`TargetRecord.id` in `sessions.jsonl` is the archived fix signature; v12 treats
it as the same key as analyzer `target.fix_signature`. The policy applies to all
delivery modes, including consensus PoC PRs and consensus issues.

## Acceptance

Every analyzer target appears either in merged output or `rejected_by_merge`.
Deduped targets carry a closed `dedup:` historical reason and never reach
optimizer fan-out.
