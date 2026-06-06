# Completed: Submodule Migration To Operator

- **id:** `0003-submodule-migration`
- **status:** `shipped`
- **completed:** `2026-05-12`
- **source:** `assets/autonomous-roadmap.md`

## Problem

The target `stacks-core` checkout originally lived in the tool repo, coupling
tool development to operator runtime state.

## Shipped

`repos/stacks-core` moved to the operator repo. Tool settings made the base
checkout lazy via `Layout::require_base()`, so prompt lint/sync can run without
a target checkout.
