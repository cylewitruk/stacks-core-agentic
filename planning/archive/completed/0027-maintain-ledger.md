# 0027: Maintain Ledger

- **id:** `0027-maintain-ledger`
- **status:** `shipped`
- **completed:** `2026-06-13`
- **iteration:** [v10: Maintain Command + PR Lifecycle Reconciliation](v10-maintain-and-pr-lifecycle.md)

## Problem

Closed-loop operation needs durable post-publish lifecycle observations without
rewriting immutable session archive branches.

## Shipped

- Added `maintain.jsonl` as an append-only sibling ledger to `sessions.jsonl`.
- Added the typed `MaintEvent` model with v1 schema versioning and eight event
  kinds: PR open, merged, closed-unmerged, stale, force-pushed,
  branch-deleted, issue open, and issue closed.
- Added a lossy `maintain.jsonl` reader and append helper following the v6
  ledger-reader pattern.
- Generated and bundled `maintain-event.schema.json`.

## Validation

- `models::maintain_event` tests cover schema validation and JSON round-trip.
- `session::maintain_ledger` tests cover missing-file handling, append/read
  round-trip, lossy reads, and invalid-event rejection.
- `just lint --no-sccache` passed.
- `just test --summary --no-sccache` passed with `530/530`.

## Follow-Ups

- `0030-event-log-skeleton` should be reconsidered once multiple consumers read
  both `sessions.jsonl` and `maintain.jsonl`.
