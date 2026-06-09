# Design: Per-Session Ephemeral Source Clone

- **id:** `0022-ephemeral-source-clone`
- **status:** `planned`
- **priority:** `medium`
- **iteration:** [v3-ephemeral-source-clone](../iterations/v3-ephemeral-source-clone.md)
- **decision:** [Decision 0003](../decisions/0003-ephemeral-source-clone.md)

## Problem

The operator repo currently shares one `repos/stacks-core` submodule pinned on
operator `main`. That creates several drift modes:

- Submodule SHA staleness.
- Detached HEAD vs local branch-ref divergence.
- Cross-session interference when one session bumps source state for another.
- Implicit provenance: a session id does not identify the source SHA without
  correlating operator git history.

## Proposed Shape

Drop the operator `repos/<base>` submodule. At session start, materialize a
source checkout under `agent_workspace_root` from a shared bare object cache.

```text
<operator_dir>/
  sessions/<id>/results/...
  sessions/<id>/source.json          # durable provenance
  # no repos/<base> submodule on main

<agent_workspace_root>/
  cache/
    <base>.git/                      # shared bare cache
  sessions/<id>/
    repos/<base>/                    # session source checkout
    optimizers/<target>/             # per-target clones/reference checkouts
```

## Source Provenance

Write `source.json` into the durable session artifact tree:

```json
{
  "url": "...",
  "branch": "...",
  "sha": "...",
  "fetched_at": "..."
}
```

The same source fields should flow into `summary.json` and `SessionRecord` once
the model migration is in scope.

## Drift Modes Eliminated

- Each session resolves and pins its own source SHA at start.
- No operator-edited local branch ref can drift from detached HEAD.
- Parallel sessions can use different source SHAs without stomping each other.

## Trade-Offs

- Disk: naïve clones are too large; use a bare cache plus local/reference clones.
- `sbagent init` becomes simpler: no `.gitmodules` or submodule bootstrap.
- Migration must remove the existing submodule from operator repos.
- Auth surface should reuse existing PAT-via-extraheader helpers and URL
  validation.

## Open Questions

- Exact `source.json` schema and whether it starts as v1.
- How `session baseline import` handles source SHA mismatch.
- Whether the bare cache is single-operator-only in v1.

## Acceptance

- Running a session does not require or mutate an operator submodule.
- Archived artifacts identify source URL, branch, SHA, and fetch time.
- Per-target optimizer checkouts start from the session-pinned source.
