# 0050: Local Session Cron

- **id:** `0050-local-session-cron`
- **status:** `shipped`
- **priority:** `medium`
- **iteration:** [v14: Local Session Systemd Schedule](v14-local-session-systemd-schedule.md)
- **source:** [assets/autonomous-roadmap.md](../../../assets/autonomous-roadmap.md)

## Problem

`sbagent session run` needs a dedicated benchmark host with chainstate mounts,
shadow storage, bench DB, PAT/config, and enough CPU/disk capacity. GitHub-hosted
CI can run `sbagent maintain`, but it cannot run full benchmark sessions.

## Shipped

v14 adds copyable systemd templates for local scheduled sessions:

- `assets/operator-templates/systemd/sbagent-session.service`
- `assets/operator-templates/systemd/sbagent-session.timer`
- `assets/operator-templates/systemd/sbagent-session.env.example`

The service calls `sbagent` directly with
`session run --publish-accepted-prs --archive`; there is no wrapper script. The
timer defaults to conservative calendar scheduling with `Persistent=false`, and
documents `OnUnitInactiveSec` as the alternative for delay-after-completion
schedules.

The templates keep secrets out of unit files. The environment example points to
secret paths, while PAT values remain in the operator's configured token file.
Commented hardening directives document the `ReadWritePaths=` caveat for
operator repo, bench DB, chainstate, config, and token paths.

`docs/operations.md` now includes the install, verify, enable, pause, disable,
and journald inspection flow for a dedicated benchmark server.

## Validation

- `cargo test -p stacks-bench-agent --test local_session_systemd`
- `just lint --no-sccache`
- `just test --summary --no-sccache` — 576 tests passed.

Coverage added:

- static checks that the service runs `session run`, not `maintain`;
- `--publish-accepted-prs` and `--archive` pinned in the unit;
- anti-secret checks across service, timer, and env example;
- `Persistent=false`, `OnCalendar`, and `OnUnitInactiveSec` pinned;
- operations docs pinned for `systemd-analyze verify`, `list-timers`,
  `journalctl`, pause, and failed-service behavior.

## Notes

Live benchmark-server validation remains open by design. The implementation
ships the templates and docs; the operator follow-up is to copy them to the
dedicated server, run `systemd-analyze verify`, start one supervised service
run, inspect `journalctl`, confirm `systemctl list-timers --all
sbagent-session.timer`, and record the resulting session id.
