# 0051: Projection Migration Completion

- **id:** `0051-projection-migration-completion`
- **status:** `shipped`
- **iteration:** [v16: Projection Migration Completion](v16-projection-migration-completion.md)
- **completed:** 2026-06-15

## Shipped

v16 completed the v15 projection migration boundary:

- v11 autonomy gates now consume `HistoryProjectionV1` for open PR counts,
  cadence checks, and zero-accepted-session circuit-breaker state.
- `sbagent history show <id>` now consumes `HistoryProjectionV1` for session
  lookup and maintenance-event rendering.
- Chained `session run` builds one shared read-side projection and passes it
  immutably into Phase 1.2 optimizer memory and Phase 1.7 merge/dedup.
- Standalone commands still build their own projection once per invocation.
- `history list` intentionally remains on the simple `sessions.jsonl` reader
  because it does not join against `maintain.jsonl`.

## Validation

- `just lint --no-sccache`
- `just test --summary --no-sccache` — 585/585 passing
- Audit boundary: direct maintain-ledger reads remain only in the shared
  projection and maintain command/reconciler surfaces.

## Follow-Up

`0043-history-report` is now the natural v17 candidate. It should consume
`HistoryProjectionV1` rather than adding another raw ledger pass.
