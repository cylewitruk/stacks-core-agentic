# 0035: Autonomy Hygiene

- **id:** `0035-autonomy-hygiene`
- **status:** `shipped`
- **completed:** `2026-06-13`
- **iteration:** [v11: Autonomy Safety + Scheduled Maintain](v11-autonomy-safety-and-maintain-schedule.md)
- **source:** [assets/autonomous-roadmap.md](../../../assets/autonomous-roadmap.md)

## Problem

Before unattended sessions can run, sbagent needs brakes that stop expensive
benchmark work when the operator has paused the bot, the PR queue is full,
sessions are too frequent, or recent sessions repeatedly produced no accepted
work.

## Shipped

- Added `[autonomy]` settings:
  - `max_open_agent_prs`;
  - `min_session_interval_hours`;
  - `zero_accepted_circuit_breaker`.
- Added session-start autonomy preflight gates for:
  - `<operator>/.sbagent/pause`;
  - open bot PR queue size, projected from `sessions.jsonl` plus
    `maintain.jsonl`;
  - minimum interval since the most recent archived session;
  - zero-accepted circuit breaker over the last N completed sessions.
- Split autonomy checks into report-only and enforcing modes:
  - `sbagent check` and other read-only preflight callers report the breaker
    without writing files;
  - full `sbagent session run` can write `.sbagent/pause` when the breaker
    trips.
- Confirmed `sbagent maintain` does not consult the pause gate and remains
  allowed while paused.
- Documented local dedicated-host scheduling with `benchmark.lock`,
  `test.lock`, and the safety gates.

## Validation

- Unit tests cover pause detection, open-PR projection, terminal maintain event
  subtraction, cadence blocking, combined queue/cadence diagnostics, breaker
  cold-start behavior, aborted-session exclusion, pause-file writing, accepted
  sessions preventing the breaker, and report-only no-write behavior.
- `tests/maintain_command.rs` covers `sbagent maintain --dry-run` while
  `.sbagent/pause` exists.
- `assets/example.config.toml` includes documented `[autonomy]` defaults.
- `just lint --no-sccache` passed.
- `just test --summary --no-sccache` passed with `543/543`.
