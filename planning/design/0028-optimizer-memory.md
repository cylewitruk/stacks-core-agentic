# Design: Cross-Session Optimizer Memory

- **id:** `0028-optimizer-memory`
- **status:** `shipped`
- **priority:** `low`
- **iteration:** [v13: Cross-Session Optimizer Memory](../archive/completed/v13-cross-session-optimizer-memory.md)
- **completed:** [0028 archive note](../archive/completed/0028-optimizer-memory.md)

## Problem

Each session starts cold. The system can rediscover targets or fixes that prior
sessions already rejected, deferred, or attempted.

## Existing Seed

`memory/analyzed-rejections.jsonl` records some analyzer-level rejections on
operator disk. It is not yet tracked in git or broadly surfaced to agents.

v10 and v12 added a stronger substrate than the original sketch expected:
`sessions.jsonl` records archived target outcomes, while `maintain.jsonl`
records PR / issue lifecycle state. v13 uses those two ledgers as the primary
memory source and keeps memory advisory; deterministic duplicate blocking stays
owned by v12's dedup filter.

## Possible Scope

- Promote useful memory into durable operator state.
- Surface prior analyzer rejections to triage/analyzer prompts.
- Surface prior optimizer attempts or known blockers to optimizer prompts.

## Trigger

Wait until enough real sessions exist that repeated work is observable.

## Acceptance

A new session avoids at least one previously rejected or already-attempted
target because memory was surfaced in the appropriate phase, without adding a
second hard-blocking gate beyond v12 dedup.

## Resolution

Shipped in v13 as a compact advisory artifact generated from `sessions.jsonl`
and `maintain.jsonl` after triage. Analyzer, merge, and optimizer prompts can
cite the memory, while v12 dedup remains the only deterministic hard gate.
