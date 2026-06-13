# Design: GitHub Actions Wiring

- **id:** `0034-github-actions-wiring`
- **status:** `planned`
- **priority:** `low`
- **iteration:** [v11: Autonomy Safety + Scheduled Maintain](../iterations/v11-autonomy-safety-and-maintain-schedule.md)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

The closed loop needs scheduled maintenance and, eventually, scheduled
benchmark sessions. GitHub-hosted CI is appropriate for `sbagent maintain`, but
not for `sbagent session run`: benchmark sessions need a dedicated host with
chainstate, disk, and runtime capacity.

## Design

v11 implements the maintain-only slice:

- `sbagent-maintain.yml`: daily maintenance cron plus `workflow_dispatch`.
- Shared concurrency group reserved for future autonomy jobs.
- Bot git identity configured early.
- Job-level loop guard: `if: github.actor != 'stacks-bench-bot'`.
- Minimum PAT permissions: Contents: write and Pull requests: read. No
  merge, close, comment, or label permissions.
- Cron cadence starts conservative and is tuned to active PR volume and
  GitHub rate-limit budget.
- No `sbagent-session.yml` yet. Scheduled benchmark sessions move to a
  dedicated-host local cron recipe / follow-up item.

## Acceptance

Scheduled maintain runs at configured cadence, serializes through one
concurrency group, and cannot start benchmark work.
