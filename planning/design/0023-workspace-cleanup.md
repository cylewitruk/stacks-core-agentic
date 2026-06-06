# Design: Workspace Cleanup

- **id:** `0023-workspace-cleanup`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0023-workspace-cleanup](../backlog.md#0023-workspace-cleanup)

## Problem

`agent_workspace_root` can grow quickly:

- Per-target optimizer clones can retain large build trees for the whole
  session.
- Old session workspaces persist indefinitely.
- Disk exhaustion currently surfaces as confusing clone/build failures.

## Scope

1. Drop or clean the previous target checkout before cloning the next target
   when optimizer parallelism is effectively serial.
2. Prune old session workspaces by age or archive status.
3. Add a preflight disk-space check with an actionable prune command.

## Constraints

- Do not delete active session workspaces.
- Do not remove durable session artifacts under the operator archive.
- Make destructive cleanup explicit unless it is limited to coordinator-owned
  scratch for the current session.

## Acceptance

- Peak disk use is bounded for serial optimizer runs.
- Operators can prune stale workspaces with one command.
- Session startup fails early when free space is obviously insufficient.
