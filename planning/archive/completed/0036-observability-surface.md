# Completed: Observability Surface

- **id:** `0036-observability-surface`
- **status:** `shipped`
- **completed:** `2026-06-12`
- **priority:** `low`
- **iteration:** [v6: Observability Surface](v6-observability-surface.md)
- **source:** [assets/autonomous-roadmap.md](../../../assets/autonomous-roadmap.md)

## Problem

Regular autonomous sessions need a compact status surface for humans.

## Shipped

- Added a typed lossy `sessions.jsonl` reader that accepts supported ledger
  schema versions and returns skipped malformed lines in-band.
- Added `sbagent history list` for recent-session summaries.
- Added `sbagent history show <id>` for per-session phase timings, target
  outcome details, PR/issue URLs, and bench wall-clock totals.
- Closed `0021-preflight-v2` as superseded by v3's per-session source clone.

## Deferred Design

Add `sbagent history report --format=markdown` with:

- sessions run;
- PRs opened/merged/closed;
- top fix signatures by attempts;
- token spend if tracked;
- time-to-merge distribution.

Optionally commit weekly reports to `reports/<iso-week>.md`.

This report slice is now tracked separately as
[`0043-history-report`](0043-history-report.md), best picked up
after `0033-maintain-command` adds GitHub PR lifecycle state.

## Validation

- Fixture tests cover `history list`, `history show`, ASCII output, filters,
  missing ledgers, and mixed-version ledger reads.
- Live smoke session `20260611-172955` archived into the operator ledger and
  rendered through both `history list` and `history show`.
