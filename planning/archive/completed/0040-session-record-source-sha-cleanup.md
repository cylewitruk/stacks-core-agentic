# 0040: SessionRecord Source SHA Cleanup

- **id:** `0040-session-record-source-sha-cleanup`
- **status:** `shipped`
- **priority:** `low`
- **iteration:** [v4-v3-polish-and-bot-fork-seed](v4-v3-polish-and-bot-fork-seed.md)

## Shipped

Removed the dead `stacks_core_base_sha` field from `SessionRecord` and bumped
the ledger schema to v3 while preserving read compatibility for v1/v2 records.

## Validation

- v1, v2, and v3 ledger read-compat tests pass.
- Generated schema mirrors were refreshed during v4 Phase 2.
