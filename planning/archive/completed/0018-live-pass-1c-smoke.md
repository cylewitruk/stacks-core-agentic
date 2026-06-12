# Completed: Live Pass 1c Smoke

- **id:** `0018-live-pass-1c-smoke`
- **status:** `shipped`
- **completed:** `2026-06-12`
- **iteration:** [v1: Live Pass 1c Smoke](v1-live-pass-1c-smoke.md)

## Problem

Pass 1c had strong static coverage, but the full triage -> analyzer -> merge
-> baseline calibration -> optimize -> verification bench -> results analyzer
-> finalize -> publish -> archive flow still needed one real operator session.

## Shipped

Ran smoke session `20260611-172955` end-to-end on the bot operator config.
The session produced three normal PR targets, published three draft PRs to the
bot fork, archived the session bulk to `session/20260611-172955`, appended the
operator `sessions.jsonl` ledger row, and rendered through `history show`.

## Validation

- `sbagent session validate --session-id 20260611-172955` returned `OK`.
- Phase 3.5 rerun after narrow fixes produced context-valid verdicts.
- Publish opened PRs 1, 2, and 3 on `stacks-bench-bot/stacks-core`.
- Archive appended `ledger_appended=true` and pushed the session branch.

## Follow-Ups

- SQL column drift and the optional triage conversation-id validator mismatch
  were fixed immediately.
- Additional prompt calibration remains under
  [`0019-prompt-hardening-live-smoke`](../../iterations/v8-smoke-informed-prompt-hardening.md).
