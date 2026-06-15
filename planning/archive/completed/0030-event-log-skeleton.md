# 0030: Event Log Skeleton

- **id:** `0030-event-log-skeleton`
- **status:** `shipped`
- **priority:** `medium`
- **iteration:** [v15: Cross-Session Projection Facade](v15-cross-session-projection-facade.md)
- **source:** [assets/autonomous-roadmap.md](../../../assets/autonomous-roadmap.md)

## Problem

Closed-loop features were each deriving state from `sessions.jsonl` and
`maintain.jsonl` on their own. Dedup, optimizer memory, autonomy gates, history,
and maintain all needed overlapping lifecycle joins and observed-at ordering.
Letting each new feature repeat that projection would increase drift and make a
future unification more expensive.

## Shipped

v15 rescopes the original "event log skeleton" into a shared read-side
projection facade:

- `sessions.jsonl` and `maintain.jsonl` remain the durable source ledgers.
- `HistoryProjectionV1` is rebuildable from parsed `SessionRecord` and
  `MaintEvent` slices.
- The projection exposes neutral query methods:
  `attempts_for_signature`, `signatures_for_family`, and
  `latest_artifact_state`.
- The projection sorts maintain events by `observed_at`; file order is not
  semantic.
- `session/dedup.rs` now builds dedup policy from the shared projection.
- `session/optimizer_memory.rs` now builds optimizer-memory rows from the
  shared projection.
- Golden fixtures pin dedup decisions and optimizer-memory JSON output.

v15 deliberately does not introduce `events/`, SQLite, or ledger schema
changes. The old append-only event-log sketch remains deferred until the
existing ledgers stop being sufficient as durable state.

## Validation

- `just lint --no-sccache`
- `just test --summary --no-sccache` — 582 tests passed.

Coverage added:

- `HistoryProjectionV1` unit tests for empty inputs, observed-at ordering,
  newest-first grouping, and missing `source_sha`.
- Dedup golden fixtures for open PR, stale PR, force-push re-block, merged PR,
  repeated failures, and open issue.
- Optimizer-memory golden JSON fixture for family/signature/lifecycle context.

## Notes

`session/dedup.rs` and `session/optimizer_memory.rs` keep
`from_ledgers(...)`-style compatibility wrappers that mention `MaintEvent`.
Those wrappers delegate to projection-backed APIs; projection assembly no
longer lives in either consumer.

The projection API enables one ledger read and one projection build to serve
multiple migrated consumers, but v15 does not wire an orchestration-level cache.
During a chained `session run`, Phase 1.2 optimizer memory and Phase 1.7
merge/dedup still read/build independently. v16 can add shared orchestration
state if the extra read becomes worth removing.
