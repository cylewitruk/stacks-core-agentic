# Design: GitHub Actions Wiring

- **id:** `0034-github-actions-wiring`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0034-github-actions-wiring](../backlog.md#0034-github-actions-wiring)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

The closed loop needs scheduled session and maintenance runs once safety controls
exist.

## Design

- `sbagent-session.yml`: weekly session cron.
- `sbagent-maintain.yml`: daily maintenance cron.
- Shared concurrency group so maintain/session cannot race.
- Bot git identity configured early.
- Pause-file check before session runs.
- Loop guard so bot commits do not recursively trigger sessions.

## Acceptance

Scheduled workflows run at configured cadence, serialize through one concurrency
group, and skip when paused.
