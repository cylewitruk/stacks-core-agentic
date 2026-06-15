# Design: Event Log Skeleton

- **id:** `0030-event-log-skeleton`
- **status:** `shipped`
- **priority:** `medium`
- **iteration:** [v15: Cross-Session Projection Facade](../archive/completed/v15-cross-session-projection-facade.md)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Closed-loop operation needs one shared way to derive cross-session state.
Several shipped consumers now project `sessions.jsonl` plus `maintain.jsonl`
independently, which increases drift risk as more autonomous-loop features
land.

## Current Design

v15 rescopes 0030 from "new event log first" to "shared projection facade
first."

The durable source of truth remains:

- `sessions.jsonl` — archived session / target ledger;
- `maintain.jsonl` — PR / issue lifecycle ledger.

The new projection is read-only and rebuildable from those ledgers. It is not a
third ledger, not a SQLite cache, and not a schema migration.

Initial implementation lives in `session/history_projection.rs` and should use
versioned in-memory types such as `HistoryProjectionV1` plus
`from_ledgers_v1(...)`. The first migrated consumers are v12 dedup and v13
optimizer memory.

## Deferred Original Sketch

The original design proposed:

- append JSONL events to `events/<session-id>.jsonl`;
- `event_version: 1` on every event;
- disposable SQLite projection at `<layout.sessions_root>/.cache/history.db`;
- `history show [--format=markdown|tsv]` over the projection.

That may still be useful later, but it is no longer the next implementation
step. The current ledgers are sufficient as durable state, and the immediate
problem is duplicated read-side projection logic.

## Initial Event Types

Deferred with the original event-log sketch:

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

v15 acceptance:

- shared read-side projection is versioned from day one;
- `sessions.jsonl` + `maintain.jsonl` remain source ledgers;
- dedup decisions are unchanged after migration;
- optimizer-memory JSON is unchanged after migration;
- event-log / SQLite work remains explicitly deferred.
