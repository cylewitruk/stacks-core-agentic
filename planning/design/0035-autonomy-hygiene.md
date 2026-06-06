# Design: Autonomy Hygiene

- **id:** `0035-autonomy-hygiene`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0035-autonomy-hygiene](../backlog.md#0035-autonomy-hygiene)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Unattended runs need hard safety controls before cron is enabled.

## Design

- `.sbagent/pause` blocks new sessions.
- Rate limits:
  - `max_open_agent_prs`
  - `min_session_interval_hours`
  - `max_total_bench_hours_per_week`
- Circuit breaker: pause automatically after K zero-accepted sessions.
- Event versioning enforcement.
- Idempotency keys for GitHub API calls.
- Signed bot commits.

## Acceptance

The scheduler fails closed when the operator queue, cadence, budget, or repeated
failure thresholds are exceeded.
