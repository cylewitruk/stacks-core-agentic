# Completed: Flag Symmetry

- **id:** `0012-flag-symmetry`
- **status:** `shipped`
- **completed:** `2026-05-21`

## Problem

Baseline calibration used rich profiler flags while candidate benches were
lean, biasing measured improvements.

## Shipped

Dropped lean candidate flags so baseline and candidate use the same rich profile
shape. Pass 1c carries this invariant per invocation via `profiler`.
