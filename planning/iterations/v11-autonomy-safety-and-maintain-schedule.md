# v11: Autonomy Safety + Scheduled Maintain

Successor to [v10: Maintain Command + PR Lifecycle Reconciliation](../archive/completed/v10-maintain-and-pr-lifecycle.md).
v10 gave the bot a post-publish lifecycle ledger. v11 adds the brakes needed
before any unattended loop runs, then schedules the lowest-risk loop:
`sbagent maintain`.

> **Status:** planned.
>
> v11 is deliberately not "scheduled benchmark sessions in CI." Benchmark
> sessions require a dedicated machine with chainstate, disk, and runtime
> capacity. v11 prepares safety gates for local scheduled sessions, but only
> automates maintain in GitHub Actions.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0035-autonomy-hygiene` | primary | planned |
| `0034-github-actions-wiring` | maintain-only scheduling | planned |

## Why

The smoke session proved the manual loop can run from triage through publish,
archive, and maintain. The next risk is not technical reach; it is unattended
execution. Before anything runs on a schedule, sbagent needs clear brakes:

- an operator pause switch;
- queue/cadence limits before expensive sessions start;
- a circuit breaker when sessions repeatedly produce no accepted work;
- a scheduled maintain path that can keep lifecycle state fresh without running
  benchmarks or opening new PRs.

`sbagent maintain` is the right first scheduled job because it is read-only on
GitHub and only appends lifecycle observations to `maintain.jsonl`. Scheduled
benchmark sessions remain a local-cron / dedicated-runner problem, not a
GitHub-hosted CI problem.

## Scope

In scope:

- `.sbagent/pause` blocks `sbagent session run`.
- `sbagent maintain` remains allowed while paused.
- New safety settings:

  ```toml
  [autonomy]
  max_open_agent_prs = 10
  min_session_interval_hours = 144
  zero_accepted_circuit_breaker = 3
  ```

- Session-start preflight checks:
  - open bot PR count must be below `max_open_agent_prs`;
  - most recent archived session must be older than
    `min_session_interval_hours`;
  - the last N sessions must not all have zero accepted targets, where N is
    `zero_accepted_circuit_breaker`.
- Circuit breaker writes `.sbagent/pause` with a short diagnostic when it trips.
- Operator-facing diagnostics explain the block reason and the reset path.
- GitHub Actions workflow for `sbagent maintain` only:
  - shared concurrency group name reserved for future local/remote scheduling;
  - loop guard uses a job-level actor check so commits pushed by
    `stacks-bench-bot` do not recursively trigger work;
  - bot identity and PAT wiring documented with minimum permissions:
    Contents: write and Pull requests: read;
  - manual `workflow_dispatch` plus conservative cron.
- Docs for local benchmark scheduling as a future operator recipe, not an
  implemented workflow.

Out of scope:

- GitHub Actions `session run`; this is infeasible for the current benchmark
  workload and chainstate requirements.
- Local cron implementation for `session run`; tracked as a follow-up.
- Auto-merge, auto-close, or PR mutation behavior.
- Optimizer memory / dedup behavior (`0028`, `0031`).
- Weekly history reports (`0043`).
- Signed commits, idempotency keys, and bench-hour weekly budgeting from the
  older hygiene sketch. Keep those as later hardening if scheduled operation
  proves useful.

## Phases

### Phase 1: Safety Settings + Pause Gate

**Goal:** A human operator can stop new benchmark sessions with one committed
file, while maintain still runs.

**Scope:**

- Add `[autonomy]` settings and defaults.
- Add a pause-file helper that resolves `<operator>/.sbagent/pause`.
- Add a session-start check before expensive session work starts.
- `sbagent maintain` does not consult the pause gate.
- The diagnostic names the pause file path and says remove it to resume.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] With `.sbagent/pause` present, `sbagent session run` fails before
      materializing source or running baseline.
- [ ] The failure message names `.sbagent/pause` and the unpause action.
- [ ] `sbagent maintain --dry-run` still runs with `.sbagent/pause` present.
- [ ] A combined fixture proves `session run` is blocked while `maintain`
      remains allowed under the same paused operator tree.
- [ ] Default settings parse from `assets/example.config.toml`.

**Tests:**

- In-process preflight/unit tests for pause detection.
- CLI fixture test for `maintain --dry-run` while paused.

### Phase 2: Queue + Cadence Gates

**Goal:** A scheduled local session cannot run when the review queue is full or
when the previous session is too recent.

**Scope:**

- Add a pre-session gate that reads `sessions.jsonl` and `maintain.jsonl`.
- Count open agent PRs from archived targets plus maintain projection:
  - `pr_open` with no terminal follow-up counts as open;
  - `pr_merged` / `pr_closed_unmerged` do not count as open.
- Compare the latest archived session's `started_at` / `finished_at` against
  `min_session_interval_hours`.
- Keep the gate conservative: malformed ledger lines produce a warning through
  the existing lossy reader path but do not silently bypass hard limits.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] If open bot PR count is at or above `max_open_agent_prs`, session start
      fails before expensive work.
- [ ] Terminal maintain events remove PRs from the open count.
- [ ] If the most recent session is younger than
      `min_session_interval_hours`, session start fails with the timestamp and
      configured threshold.
- [ ] If both gates fail, diagnostics list both reasons.

**Tests:**

- Fixture-ledger tests for open-count projection.
- Fixture-ledger tests for cadence threshold.

### Phase 3: Zero-Accepted Circuit Breaker

**Goal:** Repeated unproductive sessions automatically pause future scheduled
sessions until a human looks.

**Scope:**

- Inspect the last N archived sessions by `started_at`, where N is
  `zero_accepted_circuit_breaker`.
- A session counts as zero-accepted when it completed without any accepted or
  mixed/accepted target.
- Only completed sessions count toward the breaker. Sessions that were blocked
  by pause, PR queue, cadence, preflight, or other gates did not try and must
  not count as "unproductive."
- If fewer than N completed archived sessions exist, the breaker does not trip.
- If all N are zero-accepted, write `.sbagent/pause` with:
  - UTC timestamp;
  - threshold;
  - session ids that tripped it;
  - reset instructions.
- If the pause file already exists, preserve it and report the existing pause.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] N consecutive zero-accepted sessions create `.sbagent/pause`.
- [ ] Fewer than N completed archived sessions do not trip the breaker.
- [ ] Gated or aborted-before-run sessions do not count toward N.
- [ ] A successful accepted session in the window prevents the breaker.
- [ ] Existing pause file is not overwritten.
- [ ] The generated pause file is concise and operator-readable.

**Tests:**

- Fixture-ledger tests for breaker trip / no-trip.
- File-content assertion for generated pause message.

### Phase 4: Scheduled Maintain Workflow

**Goal:** GitHub can keep PR lifecycle state fresh without running benchmarks.

**Scope:**

- Add `.github/workflows/sbagent-maintain.yml`.
- Workflow triggers:
  - `workflow_dispatch`;
  - conservative cron, e.g. daily.
- Shared concurrency group, e.g. `sbagent-autonomy`.
- Loop guard uses `if: github.actor != 'stacks-bench-bot'` at the job level so
  bot-authored bundle/maintain commits do not recursively trigger work.
- Installs or uses the pinned sbagent binary path according to current project
  convention.
- Runs `sbagent maintain` with the operator config.
- Documents required secrets and expected no-op output. Minimum token
  permissions: Contents: write for the maintain.jsonl commit/push and Pull
  requests: read for PR-state queries. No merge, close, comment, or label
  permissions.
- Notes that cron cadence is tunable based on active PR volume; secondary
  GitHub rate-limit budget is the ceiling.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Workflow YAML is syntactically valid and has `workflow_dispatch`.
- [ ] Workflow uses the shared concurrency group.
- [ ] Workflow cannot trigger a benchmark session.
- [ ] Workflow job has the pinned `github.actor != 'stacks-bench-bot'` guard.
- [ ] Docs explain required PAT/config setup.
- [ ] Docs list the minimum PAT permissions and explicitly avoid
      merge/close/comment scopes.
- [ ] Manual operator validation records one successful workflow dispatch or
      a documented reason it cannot run in the current repo.

**Tests:**

- Static YAML test or fixture parser for trigger/concurrency/command shape.
- Manual workflow-dispatch validation if repository secrets are configured.

### Phase 5: Local Session Cron Recipe

**Goal:** Capture the practical path for scheduled benchmark sessions without
pretending GitHub-hosted CI can run them.

**Scope:**

- Add docs for running `sbagent session run` from a dedicated benchmark host:
  - local cron or launchd timer;
  - required chainstate/data mounts;
  - config path;
  - existing `bench.lock` / `test.lock` lockfiles under
    `<framework>/data/run/` as the serialization primitive;
  - expected interaction with `.sbagent/pause`.
- No code that schedules sessions.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Docs clearly state GitHub-hosted CI is not the current session-run
      substrate.
- [ ] Recipe shows how to run a no-op dry-check / preflight before enabling
      local cron.
- [ ] Recipe points at the safety gates from Phases 1-3.
- [ ] Recipe says local cron must not start a session while `bench.lock` or
      `test.lock` is held.

**Tests:**

- Documentation review only.

## Final Validation

- [ ] `just lint --no-sccache` clean.
- [ ] `just test --summary --no-sccache` clean.
- [ ] `sbagent maintain --dry-run` still works when `.sbagent/pause` exists.
- [ ] `sbagent session run` is blocked by pause, PR-queue, cadence, and circuit
      breaker fixture cases before expensive work starts.
- [ ] `sbagent-maintain.yml` can be manually dispatched or is documented as
      pending repo-secret setup.

## Follow-Ups

- New item: local scheduled session runner / dedicated-host cron hardening.
- `0031-triage-merge-dedup-filter` — consume `maintain.jsonl` once more
  lifecycle data exists.
- `0043-history-report` — better after maintain workflow produces real
  lifecycle rows.
- `0030-event-log-skeleton` — reconsider once more consumers depend on both
  `sessions.jsonl` and `maintain.jsonl`.
