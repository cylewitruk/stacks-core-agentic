# Completed: Workspace Cleanup

- **id:** `0023-workspace-cleanup`
- **status:** `shipped`
- **completed:** `2026-06-12`
- **priority:** `low`
- **iteration:** [v2: Cleanup And Workspace Hygiene](v2-cleanup-and-workspace-hygiene.md)

## Problem

`agent_workspace_root` can grow quickly:

- Per-target optimizer clones can retain large build trees for the whole
  session.
- Old session workspaces persist indefinitely.
- Disk exhaustion currently surfaces as confusing clone/build failures.

## Shipped

- `sbagent session bench clean` removes both Phase 1.8 verify artifacts and
  Phase 3 candidate bench artifacts, while preserving optimizer-owned outputs.
- Prompt lint validates explicitly marked schema example fences.
- The existing per-worktree cargo-clean reclamation path is locked in by tests
  and documented.
- `sbagent workspace prune` can dry-run or prune archived/aged session
  workspaces with `.run.pid` liveness protection.
- Session preflight warns or fails on low free disk depending on
  `preflight.min_free_gib`.

## Original Scope

1. Lock in the existing per-worktree `cargo clean` reclamation that already
   runs between binary copy and bench invocations
   ([bench_experiments.rs:161-167](../../crates/stacks-bench-agent/src/session/bench_experiments.rs#L161-L167)):
   add regression-fencing tests and operations-docs coverage so the contract
   cannot drift unnoticed. Do NOT remove the per-target checkout itself —
   Phase 5 publish needs the worktree
   ([publish.rs:264-267](../../crates/stacks-bench-agent/src/session/publish.rs#L264-L267),
   [publish.rs:983-989](../../crates/stacks-bench-agent/src/session/publish.rs#L983-L989)).
2. Prune old session workspaces by age + archive-ledger status via a new
   `sbagent workspace prune` command. Use the durable signals that exist
   today (`sessions.jsonl` for terminal state, a best-effort `.run.pid` file
   for live-session refusal) — no new on-disk session-state file.
3. Add a preflight disk-space check with an actionable prune command, opt-in
   via `preflight.min_free_gib` (default `None`/warn-only until live
   operation validates a sane floor).

## Constraints

- Do not delete active session workspaces.
- Do not remove durable session artifacts under the operator archive.
- Do not remove per-target optimizer checkouts before Phase 5 publish.
- Make destructive cleanup explicit unless it is limited to coordinator-owned
  scratch for the current session.

## Acceptance

- The per-target `cargo clean` reclamation contract is locked in by tests and
  named in the operations docs alongside its `--skip-cargo-clean` escape
  hatch.
- Operators can prune stale workspaces with one command, and that command
  refuses to touch a session whose `.run.pid` matches a live process.
- Session startup warns (or fails, when `preflight.min_free_gib` is set) when
  free space is obviously insufficient, with the exact prune invocation in
  the error body.

## Validation

- v2 unit/integration tests cover clean, prompt lint, cargo-clean, prune, and
  preflight behavior.
- Smoke session `20260611-172955` published successfully after the default
  cargo-clean path.
- `sbagent workspace prune --dry-run --archived-only` listed the archived smoke
  session as prunable without deleting it.
