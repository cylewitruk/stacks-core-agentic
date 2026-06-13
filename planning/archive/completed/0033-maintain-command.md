# 0033: Maintain Command

- **id:** `0033-maintain-command`
- **status:** `shipped`
- **completed:** `2026-06-13`
- **iteration:** [v10: Maintain Command + PR Lifecycle Reconciliation](v10-maintain-and-pr-lifecycle.md)
- **source:** [assets/autonomous-roadmap.md](../../../assets/autonomous-roadmap.md)

## Problem

After a session publishes PRs or issues, sbagent needs to observe GitHub
lifecycle state so future autonomy can distinguish open, merged, closed, stale,
force-pushed, and branch-deleted artifacts.

## Shipped

- Added `sbagent maintain [--since <date>] [--dry-run] [--limit <N>]`.
- Extended `GhClient` with thin PR/issue state reads returning
  `GhStateRead<T>` plus rate-limit metadata.
- Added `MaintainReconciler`, which projects `maintain.jsonl`, skips terminal
  artifacts, derives lifecycle event transitions, suppresses duplicate stale /
  branch-deleted / force-push events, and defers work on query-limit or
  rate-limit-floor trips.
- Added `[maintain]` settings for `stale_after_days` and
  `secondary_rate_limit_floor_pct`.
- Extended `sbagent history show <session-id>` with a chronological
  "Maintenance events" section.

## Validation

- Reconciler tests cover first observation, merge transition, stale and
  branch-deleted duplicate suppression, force-push duplicate suppression,
  force-push reset of derived-state flags, query-limit deferral, and
  rate-limit-tail deferral.
- `tests/maintain_command.rs` covers the binary all-terminal no-op path without
  reading a publish token or querying GitHub.
- `tests/history_show.rs` covers maintenance-event rendering.
- `just lint --no-sccache` passed.
- `just test --summary --no-sccache` passed with `530/530`.

## Deviations

- Live GitHub validation is deferred: the next operator smoke should run
  `sbagent maintain --dry-run` against the bot-fork PRs from session
  `20260611-172955`, then run the real command if any event should be appended.
- Phase 5 stretch hardening was intentionally skipped.

## Design Note

The original design sketch used `events/maintenance/<utc-ts>.jsonl` and a
three-event model. v10 superseded that with a sibling `maintain.jsonl`, an
eight-kind typed event model, a split `GhClient` / `MaintainReconciler`
architecture, and idempotency via last-state diff rather than a persisted
backoff cache.
