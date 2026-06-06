# Design: Maintain Command

- **id:** `0033-maintain-command`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0033-maintain-command](../backlog.md#0033-maintain-command)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Closed-loop operation needs to observe PR lifecycle state after a session opens
PRs or issues.

## Design

Add `sbagent maintain`:

- Read history projection.
- Query GitHub for open/failed PR rows.
- Emit maintenance events:
  - `pr_merged`
  - `pr_closed_unmerged`
  - `pr_stale`
- Append to `events/maintenance/<utc-ts>.jsonl`.
- Commit and push the maintenance event file.

## Acceptance

GitHub lifecycle changes update the event log without modifying source code.
