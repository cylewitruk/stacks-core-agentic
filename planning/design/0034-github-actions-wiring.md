# Design: GitHub Actions Wiring

- **id:** `0034-github-actions-wiring`
- **status:** `shipped`
- **priority:** `low`
- **iteration:** [v11: Autonomy Safety + Scheduled Maintain](../archive/completed/v11-autonomy-safety-and-maintain-schedule.md)
- **completed:** [0034 archive note](../archive/completed/0034-github-actions-wiring.md)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

The closed loop needs scheduled maintenance and, eventually, scheduled
benchmark sessions. GitHub-hosted CI is appropriate for `sbagent maintain`, but
not for `sbagent session run`: benchmark sessions need a dedicated host with
chainstate, disk, and runtime capacity.

## Design

v11 implements the maintain-only slice as an operator-repo workflow template:

- `assets/operator-templates/.github/workflows/sbagent-maintain.yml`: copied by
  operators into `<operator>/.github/workflows/sbagent-maintain.yml`.
- Daily maintenance cron plus `workflow_dispatch`.
- Push trigger for `sessions.jsonl` so newly archived sessions are reconciled
  promptly.
- Shared concurrency group reserved for future autonomy jobs.
- Bot git identity configured early.
- Job-level loop guard: `if: github.actor != 'stacks-bench-bot'`.
- Missing required secrets produce an informational no-op so the copied
  workflow can land before repo secrets are configured.
- Minimum PAT permissions: Contents: write and Pull requests: read. No
  merge, close, comment, or label permissions.
- Cron cadence starts conservative and is tuned to active PR volume and
  GitHub rate-limit budget.
- No `sbagent-session.yml` yet. Scheduled benchmark sessions move to a
  dedicated-host local cron recipe / follow-up item.

## Acceptance

Scheduled maintain runs at configured cadence, serializes through one
concurrency group, and cannot start benchmark work.
