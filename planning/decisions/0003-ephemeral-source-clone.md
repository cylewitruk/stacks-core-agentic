# Decision 0003: Per-Session Ephemeral Source Clone

- **status:** draft
- **date:** 2026-06

## Proposed Decision

Replace the shared operator `repos/stacks-core` submodule with a per-session
source checkout under `agent_workspace_root`, backed by a shared bare cache.

## Rationale

The shared submodule creates drift modes: stale SHA, detached HEAD vs branch-ref
divergence, and cross-session interference.

## Open Questions

- Exact `source.json` shape.
- Migration path for existing operator repos.
- Interaction with `session baseline import`.
