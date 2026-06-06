# Design: Event Log Skeleton

- **id:** `0030-event-log-skeleton`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0030-event-log-skeleton](../backlog.md#0030-event-log-skeleton)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Closed-loop operation needs durable state across sessions without treating
SQLite as the source of truth.

## Design

- Append JSONL events to `events/<session-id>.jsonl`.
- Every event carries `event_version: 1`.
- Rebuild a gitignored SQLite projection at
  `<layout.sessions_root>/.cache/history.db` by replaying events.
- Add `sbagent history show [--format=markdown|tsv]`.

## Initial Event Types

- Session: `session_started`, `session_finalized`.
- Triage: `candidate_proposed`, `candidate_skipped_by_dedup`.
- Analysis: `analysis_accepted`, `analysis_rejected`.
- Merge: `target_merged`, `target_rejected_by_merge`.
- Optimize: `attempt_started`, `attempt_kept`, `attempt_reverted`,
  `attempt_crashed`.
- Bench: `bench_completed`, `bench_failed`.
- Finalize: `experiment_accepted`, `experiment_rejected`,
  `experiment_aborted`.
- Publish: `pr_opened`, `pr_opened_failed`, `issue_opened`.

## Acceptance

History replay produces indexed rows by `(fix_signature, session_id)` with
current PR state, source/head SHAs, improvement, and latest timestamp.
