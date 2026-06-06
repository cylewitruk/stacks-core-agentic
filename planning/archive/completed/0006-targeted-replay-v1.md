# Completed: Targeted Replay V1

- **id:** `0006-targeted-replay-v1`
- **status:** `shipped`
- **completed:** `2026-05-12`
- **source:** `assets/autonomous-roadmap.md`

## Problem

Phase 3 full-range verification was too slow for per-target optimizer work.

## Shipped

Added early `verification_replay` support with txid/block replay recipes and
Phase 3 targeted benchmark branching.

## Deviations / Supersession

This shape was later superseded by Pass 1c's analyzer-defined
`verification_replay.invocations[]` protocol.
