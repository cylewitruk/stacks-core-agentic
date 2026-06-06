# Completed: Stacks-Bench Targeted Replay

- **id:** `0001-stacks-bench-targeted-replay`
- **status:** `shipped`
- **completed:** `2026-05-11`
- **source:** `assets/autonomous-roadmap.md`

## Problem

`sbagent` needed targeted replay support in `stacks-bench` before it could
benchmark individual optimizer targets cheaply.

## Shipped

Upstream `stacks-bench` targeted replay support landed with multi-`--txid` and
`--block` support, then the operator submodule was pinned and smoke-tested.

## Follow-Up

This substrate was later superseded in `sbagent` by analyzer-defined
`verification_replay.invocations[]`, but the upstream CLI support remains
load-bearing.
