# Completed: Archive Ledger Audit Fields

- **id:** `0024-archive-audit-fields`
- **status:** `shipped`
- **completed:** `2026-06-11`
- **iteration:** [v5: Archive Metadata](../../iterations/v5-archive-metadata.md)

## Problem

The archive ledger had partial per-target audit data. `head_sha` already flowed
from summary into `SessionRecord.targets[]`, but PR / issue URLs and useful
bench wall-clock totals were absent or placeholder-like.

## Shipped

- Added `models::publish_feedback::PublishFeedback` (`schema_version: 1`) and
  bundled `publish-feedback.schema.json`.
- `GhClient::create_pr` and `create_issue` now return the opened GitHub URL.
- Phase 5 writes
  `<session>/results/optimize/<target>/publish-feedback.json` after each
  successful PR / issue creation.
- Archive reads each target's publish-feedback sidecar into
  `TargetRecord.pr_url` / `issue_url`.
- Archive aggregates `TargetBench.baseline_total_us` and
  `candidate_total_us` from per-invocation `bench-run.json` files for targets
  with `verification_replay`.

## Validation

- Publish-side tests cover PR and issue sidecar creation with fake GitHub URLs.
- Archive tests cover PR / issue URL ingestion, missing-sidecar fallback, and
  bench wall-clock aggregation.
- `PublishFeedback` validates that exactly one of `pr_url` / `issue_url` is set
  and that `opened_at` is non-empty.

## Deviations From Design

The original design expected bench totals to be lifted through `summary.json`.
The implementation aggregates directly in archive because the required data
already lives in per-invocation `bench-run.json` files, avoiding a duplicate
summary hop.

## Follow-Up

Live end-to-end validation remains part of
[v1: Live Pass 1c Smoke](../../iterations/v1-live-pass-1c-smoke.md).

## Original Design

### Problem

The archive ledger has partial per-target audit data. `head_sha` now flows from
summary into `SessionRecord.targets[]`, but `pr_url` and useful bench wall-clock
totals remain absent or placeholder-like.

### Scope

- Capture PR URLs after publish succeeds.
- Add per-target bench wall-clock totals or links to the invocation-level data.
- Keep the write-once session branch contract intact.

### Constraints

- Do not mutate archived session branches after write-once archival.
- If data is only known after publish, append it through main-ledger state rather
  than rewriting session artifacts.
- Keep `SessionRecord` validation strict enough that placeholder zeros do not
  look like measured data.

### Acceptance

- Archived target records include `head_sha`, `pr_url` when published, and
  meaningful benchmark timing metadata.
- Missing publish feedback remains explicit, not silently null-looking success.
