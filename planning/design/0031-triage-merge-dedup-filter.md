# Design: Triage / Merge Dedup Filter

- **id:** `0031-triage-merge-dedup-filter`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0031-triage-merge-dedup-filter](../backlog.md#0031-triage-merge-dedup-filter)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Without cross-session memory, sbagent can repeatedly send the same structural
fix through analysis and optimization.

## Design

At merge start, read the history projection and drop targets whose
`fix_signature` is already:

- open in a non-stale PR;
- merged upstream;
- tried unsuccessfully at least `dedup_failure_threshold` times.

Emit `candidate_skipped_by_dedup` events for every drop.

## Acceptance

Every analyzer target appears either in merged output, rejected-by-merge, or a
dedup skip event with the historical reason.
