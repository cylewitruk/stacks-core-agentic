# Design: Triage Anchor Benchmarks

- **id:** `0037-triage-anchor-benchmarks`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0037-triage-anchor-benchmarks](../backlog.md#0037-triage-anchor-benchmarks)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

If benchmark signal remains noisy, every downstream phase may need to compare
against the exact same baseline recipe/cache regime.

## Design

Triage runs targeted anchor benchmarks for promoted representatives and records
their run ids in `candidates.json`. Analyzer, optimizer, and verification reuse
those anchors instead of each running a fresh baseline.

## Trade-Off

This moves benchmark cost earlier and can add hours to triage. Defer until live
sessions prove the current Pass 1c protocol is too noisy.

## Acceptance

Downstream phases can compare against the same anchor recipe with no additional
baseline bench run.
