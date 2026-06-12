# Completed: Migration Leftovers

- **id:** `0020-migration-leftovers`
- **status:** `shipped`
- **completed:** `2026-06-12`
- **iteration:** [v2: Cleanup And Workspace Hygiene](v2-cleanup-and-workspace-hygiene.md)

## Problem

The Pass 1c artifact tree grew new verify/analyze surfaces, and a few cleanup /
lint paths lagged behind the new layout.

## Shipped

- `sbagent session bench clean` now removes Phase 1.8 `verify/` artifacts as
  well as Phase 3 candidate bench artifacts.
- Prompt lint validates explicitly opted-in output JSON examples against the
  bundled schemas after rendering.
- Phase clean/documentation coverage was updated for the Pass 1c artifact tree.

## Validation

- `bench_clean` tests cover verify cleanup, idempotence, corrupt target-file
  error behavior, and optimizer-artifact preservation.
- Prompt-lint tests cover schema mismatch, unknown schema, dangling marker,
  unparseable JSON, and unmarked-fence skip behavior.
- v2 live validation completed during smoke session `20260611-172955`.
