# Design: Per-Session Commit And Push

- **id:** `0032-per-session-commit-push`
- **status:** `backlog`
- **priority:** `low`
- **backlog:** [0032-per-session-commit-push](../backlog.md#0032-per-session-commit-push)
- **source:** [assets/autonomous-roadmap.md](../../assets/autonomous-roadmap.md)

## Problem

Manual sessions should leave a durable operator-git audit commit.

## Design

At the end of `sbagent session run`, commit:

- `events/<session-id>.jsonl`
- summary artifacts such as `summary.md`, `targets.md`, `summary.json`

Avoid committing raw scratch trees. Skip commit if no events were emitted.

## Acceptance

Operator git history contains one concise session commit with a summary message
including accepted/rejected/aborted counts and duration.
