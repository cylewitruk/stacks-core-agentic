# Decision 0003: Per-Session Ephemeral Source Clone

- **status:** accepted
- **date:** 2026-06
- **related items:** `0022-ephemeral-source-clone`

## Decision

Replace the shared operator `repos/stacks-core` submodule with a per-session
source checkout under `agent_workspace_root`, backed by a shared bare object
cache. Each session resolves and pins its own source URL + branch + SHA at
start, materializes a working checkout, and writes a `source.json` provenance
record into the session bulk that flows through into `summary.json` and
`SessionRecord` at archive time.

## Rationale

The shared submodule creates four drift modes, three of which are now
empirically confirmed:

1. **SHA staleness.** The submodule is pinned to whatever was committed to
   operator `main`. Operators who `git submodule update --remote` without
   committing see different state across sessions in the same operator-main
   shape.
2. **Detached-HEAD vs branch-ref divergence.** Per-target optimizer clones
   fork via `--branch <base_branch>` (a ref); Phase 0a samples
   `git rev-parse HEAD` (a SHA). Manual `git checkout` inside `<base>`
   between sessions can desync ref and HEAD invisibly.
3. **Cross-session interference.** Phase 0a's `cargo build --release -p
   stacks-bench` writes into `<base>/target/`, the shared submodule
   filesystem. Confirmed during the v2 documentation pass (Codex review of
   `docs/git-topology.md`). Two sessions running back-to-back share a
   `target/` cache; concurrent sessions cannot share `<base>` safely at all.
4. **Implicit provenance.** A `session/<id>` archive branch records the
   source SHA only in `baseline/manifest.json`. Confirming what
   submodule pointer was current requires consulting operator git history
   at session-time, which is fragile after subsequent operator commits.

A per-session ephemeral clone eliminates all four. Disk economy is preserved
via a shared bare cache (`<workspace>/cache/<base>.git/`); per-session
working trees clone with `--reference --local` against this cache, so each
session pays for refs + working tree but not for object storage.

## Consequences

- `<operator>/repos/<base>/` submodule is removed. `.gitmodules` shrinks
  to empty (or is deleted).
- `sbagent init` no longer runs `git submodule add` and becomes simpler.
- Per-session `source.json` becomes part of the durable evidence bundle —
  flows through to `summary.json` and `SessionRecord`.
- Per-target optimizer clones (§3 of `docs/git-topology.md`) fork from the
  session-pinned source checkout, not from a shared submodule.
- Parallel session execution becomes safe at the source-state layer
  (concurrency now bounded only by the bench lock + chainstate access).
- Migration: existing operator repos need a one-shot script to remove the
  submodule and seed the bare cache from its current contents.

## Open Questions

Deferred to the implementation iteration; do not block acceptance:

- Exact `source.json` schema and whether it starts at v1.
- How `session baseline import` reconciles a recorded source SHA against
  the current bare-cache state.
- Whether the bare cache is single-operator-only in v1 (multi-operator
  sharing implies a coordination contract).
