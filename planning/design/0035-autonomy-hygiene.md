# Design: Autonomy Hygiene

- **id:** `0035-autonomy-hygiene`
- **status:** `shipped`
- **priority:** `medium`
- **iteration:** [v11: Autonomy Safety + Scheduled Maintain](../archive/completed/v11-autonomy-safety-and-maintain-schedule.md)
- **completed:** [0035 archive note](../archive/completed/0035-autonomy-hygiene.md)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Unattended runs need hard safety controls before cron is enabled.

## Design

- `.sbagent/pause` blocks new sessions while allowing `sbagent maintain`.
- Rate limits:
  - `max_open_agent_prs`
  - `min_session_interval_hours`
- Circuit breaker: pause automatically after K zero-accepted sessions.
  Fewer than K completed archived sessions do not trip the breaker, and
  sessions blocked by safety gates do not count toward K.
- Clear operator diagnostics for every blocked run.

Deferred from the older broad sketch:

- `max_total_bench_hours_per_week`
- Event versioning enforcement.
- Idempotency keys for GitHub API calls.
- Signed bot commits.

## Acceptance

Session start fails closed when the operator pause file, PR queue, cadence, or
repeated zero-accepted-session thresholds are exceeded.
