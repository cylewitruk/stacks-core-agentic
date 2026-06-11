# Design: Preflight V2

- **id:** `0021-preflight-v2`
- **status:** `planned` (likely → `superseded` after v6's prologue audit)
- **priority:** `medium`
- **iteration:** [v6: Observability Surface](../iterations/v6-observability-surface.md) — prologue rescope. Two remaining checks are expected to be subsumed by v3's per-session fetch + `source.json` SHA pinning. If the audit confirms, this design moves to `archive/superseded/`.

## Problem

Session-start preflight v1 catches installed-binary drift, load-bearing prompt
drift, and local submodule reachability. Two drift classes remain:

- Local branch-ref divergence from detached-HEAD bumps.
- Local-only submodule reachability that misses unfetched upstream state.

## Scope

Add focused checks unless the per-session ephemeral source clone lands first and
removes the shared-submodule drift class entirely.

## Remaining Checks

1. **Branch-ref divergence detection.**
   Compare the checked-out source SHA against the local branch ref that
   per-target clones will use. This caught a prior failure where a HEAD bump did
   not update the branch ref, so optimizer clones started from stale code.

2. **Network-fetch variant of submodule reachability.**
   Today's check is local-only. Add an authenticated, non-interactive freshness
   check that catches "operator forgot to fetch + bump" before expensive
   session phases.

## Constraints

- Avoid git prompts. Use the existing no-prompt/auth helpers.
- Keep failures actionable: include the exact sync/fetch/remediation command.
- If `ephemeral-source-clone` ships first, delete or shrink this design.

## Acceptance

- A stale local branch ref fails before Phase 0a.
- An unfetched upstream base fails before Phase 0a when network preflight is
  enabled.
- `--skip-preflight` remains the explicit escape hatch.
