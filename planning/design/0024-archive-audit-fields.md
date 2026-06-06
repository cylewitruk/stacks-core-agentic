# Design: Archive Ledger Audit Fields

- **id:** `0024-archive-audit-fields`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0024-archive-audit-fields](../backlog.md#0024-archive-audit-fields)

## Problem

The archive ledger has partial per-target audit data. `head_sha` now flows from
summary into `SessionRecord.targets[]`, but `pr_url` and useful bench wall-clock
totals remain absent or placeholder-like.

## Scope

- Capture PR URLs after publish succeeds.
- Add per-target bench wall-clock totals or links to the invocation-level data.
- Keep the write-once session branch contract intact.

## Constraints

- Do not mutate archived session branches after write-once archival.
- If data is only known after publish, append it through main-ledger state rather
  than rewriting session artifacts.
- Keep `SessionRecord` validation strict enough that placeholder zeros do not
  look like measured data.

## Acceptance

- Archived target records include `head_sha`, `pr_url` when published, and
  meaningful benchmark timing metadata.
- Missing publish feedback remains explicit, not silently null-looking success.
