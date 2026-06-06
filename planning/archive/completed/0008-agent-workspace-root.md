# Completed: Agent Workspace Root

- **id:** `0008-agent-workspace-root`
- **status:** `shipped`
- **completed:** `2026-05-13`
- **source:** `assets/autonomous-roadmap.md`

## Problem

Per-target clones and build artifacts under the operator repo polluted durable
state and made disk use hard to reason about.

## Shipped

Added `layout.agent_workspace_root` so mutable agent scratch lives outside the
operator repo, with optimizer checkouts under a workspace-owned path.
