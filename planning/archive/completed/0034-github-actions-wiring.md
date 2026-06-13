# 0034: GitHub Actions Wiring

- **id:** `0034-github-actions-wiring`
- **status:** `shipped`
- **completed:** `2026-06-13`
- **iteration:** [v11: Autonomy Safety + Scheduled Maintain](v11-autonomy-safety-and-maintain-schedule.md)
- **source:** [assets/autonomous-roadmap.md](../../../assets/autonomous-roadmap.md)

## Problem

The autonomous loop needs scheduled PR lifecycle reconciliation, but benchmark
sessions cannot run on GitHub-hosted CI because they need a dedicated host with
chainstate, disk, and runtime capacity.

## Shipped

- Added an operator workflow template at
  `assets/operator-templates/.github/workflows/sbagent-maintain.yml`.
- The template runs `sbagent maintain` only; it never invokes
  `sbagent session run`.
- Triggers:
  - `workflow_dispatch`;
  - pushes that touch `sessions.jsonl`;
  - conservative daily cron.
- Added the shared `sbagent-autonomy` concurrency group.
- Added a bot-actor loop guard so bot-authored maintain commits do not
  recursively trigger more work.
- Added a loud no-op path when `SBAGENT_CONFIG_TOML` or
  `STACKS_BENCH_BOT_PAT` is missing, so operators can copy the template before
  configuring secrets.
- Documented required operator secrets and minimum PAT permissions.

## Validation

- `tests/maintain_workflow.rs` pins the workflow-template shape:
  triggers, permissions, concurrency, bot-actor guard, secret gate, token-file
  path, package install command, and absence of `sbagent session run`.
- `docs/operations.md` explains copying the template into the operator repo and
  configuring secrets.
- `just lint --no-sccache` passed.
- `just test --summary --no-sccache` passed with `543/543`.

## Follow-Ups

- Live workflow dispatch remains operator-side validation: copy the template
  into the operator repo, configure secrets, and run `workflow_dispatch`.
- Scheduled benchmark sessions remain local-cron / dedicated-host work tracked
  by `0050-local-session-cron`.
