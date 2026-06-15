# 0028: Optimizer Memory

- **id:** `0028-optimizer-memory`
- **status:** `shipped`
- **priority:** `low`
- **iteration:** [v13: Cross-Session Optimizer Memory](v13-cross-session-optimizer-memory.md)
- **source:** [assets/autonomous-roadmap.md](../../../assets/autonomous-roadmap.md)

## Problem

Each session started cold. Even after v12 blocked exact duplicate signatures at
merge time, analyzer, merge, and optimizer agents still lacked compact context
about prior attempts, lifecycle state, and source drift.

## Shipped

v13 adds advisory cross-session optimizer memory:

- `results/optimizer-memory.json` is generated after triage identifies the
  current candidate families and before analyzer fan-out.
- The artifact is built from the durable operator ledgers:
  `sessions.jsonl` plus `maintain.jsonl`.
- Memory is compact by default: last 5 attempts per exact signature, last 3
  sibling signatures per family, plus omitted-row markers when prompt rendering
  truncates.
- Attempt rows carry archived status, delivery mode, reason code, PR / issue
  URL, latest maintain lifecycle state, head SHA, and `source_sha` when the
  archived session recorded one.
- Missing `source_sha` is treated as drift unknown, not as "same source."
- Analyzer, merge, and optimizer prompts receive memory as advisory context.
  v12 dedup remains the only deterministic hard-skip mechanism.
- The optimizer prompt explicitly forbids fuzzy similarity matching: exact
  signatures and same-family rows are context, not proof of equivalence.

During implementation the artifact write point moved from "after source
materialization" to "after triage." That is the first point where the current
candidate families are known, so it preserves compactness while keeping later
phases on one consistent memory snapshot.

## Validation

- `just lint --no-sccache`
- `just test --summary --no-sccache` — 571 tests passed.

Coverage added:

- open PR / issue lifecycle context;
- merged, stale, closed, rejected, failed, and aborted historical attempts;
- maintain lifecycle projection sorted by `observed_at`, not file order;
- unrelated families excluded from the compact artifact;
- per-signature and per-family compactness bounds;
- prompt contracts for advisory memory, v12 hard-skip ownership, and no fuzzy
  similarity matching.

## Notes

v13 deliberately does not add a memory append log or a unified event store.
Memory is derived from `sessions.jsonl` + `maintain.jsonl`; future work can
revisit `0030-event-log-skeleton` if more consumers make two-ledger projection
awkward.
