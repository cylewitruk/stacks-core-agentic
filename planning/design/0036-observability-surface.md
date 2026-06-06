# Design: Observability Surface

- **id:** `0036-observability-surface`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0036-observability-surface](../backlog.md#0036-observability-surface)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Regular autonomous sessions need a compact status surface for humans.

## Design

Add `sbagent history report --format=markdown` with:

- sessions run;
- PRs opened/merged/closed;
- top fix signatures by attempts;
- token spend if tracked;
- time-to-merge distribution.

Optionally commit weekly reports to `reports/<iso-week>.md`.

## Acceptance

A report gives enough context to audit weekly autonomous behavior without
manually traversing event logs.
