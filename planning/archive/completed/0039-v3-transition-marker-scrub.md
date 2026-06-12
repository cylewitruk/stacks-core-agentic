# 0039: v3 Transition Marker Scrub

- **id:** `0039-v3-transition-marker-scrub`
- **status:** `shipped`
- **priority:** `low`
- **iteration:** [v4-v3-polish-and-bot-fork-seed](v4-v3-polish-and-bot-fork-seed.md)

## Shipped

Removed stale `v3 Phase`, `pre-v3`, and `post-cutover` transition comments
from active source files, replacing them with current invariants where useful.

## Validation

- `rg '(v3 Phase|pre-v3|post-cutover)' crates/stacks-bench-agent/src/`
  returned only documented exceptions.
- `just lint --no-sccache` passed during v4 Phase 1.
