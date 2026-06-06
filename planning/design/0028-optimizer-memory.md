# Design: Cross-Session Optimizer Memory

- **id:** `0028-optimizer-memory`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0028-optimizer-memory](../backlog.md#0028-optimizer-memory)

## Problem

Each session starts cold. The system can rediscover targets or fixes that prior
sessions already rejected, deferred, or attempted.

## Existing Seed

`memory/analyzed-rejections.jsonl` records some analyzer-level rejections on
operator disk. It is not yet tracked in git or broadly surfaced to agents.

## Possible Scope

- Promote useful memory into durable operator state.
- Surface prior analyzer rejections to triage/analyzer prompts.
- Surface prior optimizer attempts or known blockers to optimizer prompts.

## Trigger

Wait until enough real sessions exist that repeated work is observable.

## Acceptance

A new session avoids at least one previously rejected or already-attempted
target because memory was surfaced in the appropriate phase.
