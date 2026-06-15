# v14: Local Session Systemd Schedule

Successor to [v13: Cross-Session Optimizer Memory](v13-cross-session-optimizer-memory.md).
v11 added the safety gates required before unattended sessions, while v10-v13
made maintain, dedup, and optimizer memory useful between sessions. v14 makes
the benchmark-session scheduler concrete for a dedicated host.

> **Status:** shipped — implementation, review, and local validation complete;
> live benchmark-server validation remains an operator follow-up.
>
> v14 deliberately schedules `sbagent session run` locally, not in
> GitHub-hosted CI. The target deployment is a benchmark server with chainstate,
> shadow storage, bench DB, PAT/config, and enough CPU/disk capacity already
> provisioned. GitHub Actions continues to own `sbagent maintain`.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0050-local-session-cron` | primary | shipped |

## Why

The autonomous loop now has the pieces that make unattended sessions plausible:

- v11 pause / cadence / queue / circuit-breaker gates block unsafe starts;
- v10 + v11 keep PR lifecycle state fresh through `maintain`;
- v12 prevents exact duplicate fix signatures from re-entering optimizer
  fan-out;
- v13 surfaces prior outcomes to analyzer / merge / optimizer prompts as
  advisory memory.

The remaining gap is operational: a server needs a small, auditable,
drop-in schedule that runs
`sbagent session run --publish-accepted-prs --archive` under the same operator
config a human would use, without relying on GitHub-hosted CI.

## Scope

In scope:

- Systemd-first operator templates:
  - `sbagent-session.service`;
  - `sbagent-session.timer`;
  - sample environment file documenting `SBAGENT_CONFIG`,
    `SBAGENT_OPERATOR_DIR`, and optional range / publish / archive args.
- No wrapper script in v14. The service should call `sbagent` directly; if
  quoting, lock checks, or environment setup prove too awkward during
  implementation, file a follow-up rather than widening this iteration.
- Schedule shape:
  - conservative `OnCalendar` default;
  - timer comments documenting `OnUnitInactiveSec` as the alternative for
    "N hours after the last run completes" scheduling;
  - `Persistent=false` by default so missed windows do not queue a surprise
    catch-up benchmark after downtime;
  - one active session at a time.
- Safety integration:
  - v11 `.sbagent/pause` remains the source of truth;
  - v11 autonomy gates still run inside `sbagent session run`;
  - maintain remains allowed while paused;
  - session timer must not bypass publish/archive preflight or PAT wiring.
- Operator documentation:
  - install/copy paths for the templates;
  - `systemctl daemon-reload` / `enable --now` / `list-timers` commands;
  - manual dry-run / supervised first-run checklist before enabling;
  - disable/pause/recover workflow;
  - log inspection via `journalctl`.
- Static tests that parse the shipped templates and pin the important
  contracts.

Out of scope:

- GitHub Actions `session run`.
- Scheduling `sbagent maintain`; v11 already ships the operator workflow
  template for that.
- launchd / cron templates. Docs may mention them as equivalent substrates, but
  v14 ships systemd templates because the target is a server.
- New autonomy gates. v14 consumes v11 gates rather than inventing new policy.
- Auto-merge, PR mutation, or reviewer feedback ingestion.
- A unified event log (`0030`).

## Systemd Contract

The service should be boring and auditable:

- Run as a configured non-root benchmark user.
- Set `WorkingDirectory` to the operator repo.
- Load config from an environment file rather than hard-coding user paths in
  the unit.
- The environment example references secret paths, never secret values. For
  example, it may set `SBAGENT_CONFIG=/etc/sbagent/config.toml`, but it must
  not inline a PAT. Tokens stay in the path configured inside `config.toml`.
- Execute the installed `sbagent` binary with:

  ```text
  sbagent -c "$SBAGENT_CONFIG" session run --publish-accepted-prs --archive
  ```

- Let `sbagent` own semantic safety:
  - `.sbagent/pause`;
  - `max_open_agent_prs`;
  - `min_session_interval_hours`;
  - `zero_accepted_circuit_breaker`;
  - internal run / benchmark / test locks.
- Let systemd own scheduling and process supervision:
  - no overlapping service starts;
  - clear logs in journald;
  - optional host-level timeout chosen by the operator.

The template should not embed PAT values, GitHub tokens, or secrets. Secrets
stay in the operator's configured token file and existing `config.toml`.
When the service exits non-zero, systemd marks that invocation failed; the
timer remains enabled and fires on the next schedule. Operators should inspect
`journalctl` and create `.sbagent/pause` if they want to halt future starts
while investigating.

Hardening should be discoverable but not forced on day one. The service
template should include commented options such as `NoNewPrivileges=true`,
`ProtectSystem=strict`, `ProtectHome=true`, and `PrivateTmp=true`, with notes
explaining that `ReadWritePaths=` must cover the operator repo, bench DB,
shadow dir, chainstate paths that need write access, and token/config paths
when those hardening knobs are enabled.

## Phases

### Phase 1: Systemd Operator Templates

**Goal:** Ship copyable systemd units for a dedicated benchmark host.

**Scope:**

- Add templates under `assets/operator-templates/systemd/`:
  - `sbagent-session.service`;
  - `sbagent-session.timer`;
  - `sbagent-session.env.example`.
- Default cadence should be conservative (for example weekly or every several
  days), with comments showing how to tune.
- Unit should make the operator repo, config path, and binary path explicit.
- Service should not run as root by default; use a placeholder benchmark user.
- Timer should use systemd calendar syntax and avoid catch-up bursts.
- Include commented hardening directives with a short note about the
  `ReadWritePaths=` trade-off.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Template service invokes `sbagent session run`, not `maintain`.
- [x] Template includes `--publish-accepted-prs` and `--archive`.
- [x] Template does not embed secrets or PAT values.
- [x] Template is explicit about `User=`, `WorkingDirectory=`, config path, and
      binary path.
- [x] Timer has `Persistent=false` or an explicit documented equivalent.
- [x] Timer comments show `OnCalendar` and `OnUnitInactiveSec` as the two
      supported scheduling styles.
- [x] Hardening directives are present as commented options with a note
      explaining when to enable them.
- [x] Timer and service names are stable and operator-specific enough to avoid
      collision with maintain workflow naming.

**Tests:**

- Static fixture test over the template text.
- Run `systemd-analyze verify` against the service template when the host has
  systemd. Document the manual command for hosts where the test cannot run it.

### Phase 2: Documentation + Install Recipe

**Goal:** A benchmark-server operator can install, verify, enable, pause, and
debug the timer without reading source code.

**Scope:**

- Update `docs/operations.md` Local scheduled sessions section.
- Add a short install recipe:
  - copy templates to `/etc/systemd/system/` or user-systemd equivalent;
  - copy/edit env file;
  - run preflight commands;
  - run one supervised manual session;
  - enable timer;
  - inspect logs and timers.
- Include a clear "server prerequisites" checklist:
  - chainstate/source storage mounted;
  - shadow dir exists and is on the expected filesystem;
  - `sbagent` binary installed and current;
  - operator repo clean and writable;
  - token file readable by the service user;
  - `sbagent check --with-publish` passes.
- Document disable/pause paths:
  - `systemctl disable --now sbagent-session.timer`;
  - create `.sbagent/pause` for policy pause while maintain continues;
  - remove pause only after addressing the diagnostic.
- Document failure behavior: failed service invocations do not disable the
  timer; inspect logs, then pause explicitly if investigation should stop new
  sessions.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Docs state session scheduling belongs on a dedicated host, not CI.
- [x] Docs state maintain remains the GitHub Actions scheduled job.
- [x] Docs include copy/edit/enable commands for the systemd templates.
- [x] Docs include the supervised first-run checklist.
- [x] Docs explain pause vs disable.
- [x] Docs show `journalctl` and `systemctl list-timers` inspection commands.
- [x] Docs explain `systemd-analyze verify` for local template validation.
- [x] Docs explain failed service behavior and when to create `.sbagent/pause`.

**Tests:**

- Markdown lint.
- Static docs grep in the template test if useful.

### Phase 3: Safety + No-Overlap Contract Tests

**Goal:** Pin the scheduler contract so future edits do not accidentally bypass
the safety gates.

**Scope:**

- Add tests that inspect the service/timer templates and assert:
  - no `maintain` command in the session unit;
  - no token literal or environment variable containing token contents;
  - env example uses secret paths, not inline secret values;
  - service command keeps `--archive`;
  - service command keeps publish enabled;
  - timer does not permit catch-up bursts by default.
- Add docs/tests proving the timer does not replace v11 gates; it merely starts
  the same `session run` command a human would run.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Static test fails if `sbagent maintain` appears in the session service.
- [x] Static test fails if `--archive` is removed.
- [x] Static test fails if publish is disabled or omitted without an explicit
      documented reason.
- [x] Static test fails on obvious secret placeholders such as raw PAT values.
- [x] Static test fails if a wrapper script path appears in `ExecStart`.
- [x] Static test pins the `systemd-analyze verify` command documented for
      manual validation.

**Tests:**

- New `tests/local_session_systemd.rs` or equivalent.

### Phase 4: Operator Dry-Run Validation Notes

**Goal:** Capture the exact manual validation steps for the first server install.

**Scope:**

- Add a v14 validation section that remains open until the first real server
  install.
- Document expected commands:
  - `sbagent check --with-publish`;
  - `sbagent maintain --dry-run`;
  - one supervised `sbagent session run --publish-accepted-prs --archive`;
  - `systemctl start sbagent-session.service`;
  - `systemctl list-timers sbagent-session.timer`;
  - `journalctl -u sbagent-session.service`.
- Keep this phase docs-only if no server is available during implementation.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [x] Plan distinguishes code/template readiness from live server validation.
- [x] Live validation checkbox remains open until run on the benchmark server.
- [x] Docs say how to collect evidence for closing the live-validation box.
- [x] Evidence list includes:
  - `systemctl list-timers --all sbagent-session.timer`;
  - `journalctl -u sbagent-session.service --since=<date>` showing one
    successful service execution;
  - resulting session id plus `sbagent history list --limit 1` / `sessions.jsonl`
    row.

**Tests:**

- Markdown lint.

## Final Validation

- [x] `just lint --no-sccache`
- [x] `just test --summary --no-sccache`
- [x] Template fixture tests pass.
- [x] Docs describe a copyable systemd install path.
- [x] Live server validation is either completed or explicitly left open with
      the exact commands to run.

## Follow-Ups

- `0043-history-report` — after scheduled local sessions produce more ledger
  rows, weekly reports become more useful.
- `0030-event-log-skeleton` — reconsider if systemd scheduling plus maintain,
  dedup, and memory make two-ledger projections painful.
- launchd template — add only if macOS needs unattended benchmark scheduling.
- Dedicated session-budget settings — consider later if local scheduling needs
  weekly compute-hour or token-budget caps beyond v11's current gates.
