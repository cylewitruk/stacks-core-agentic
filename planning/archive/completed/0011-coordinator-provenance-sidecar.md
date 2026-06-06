# Completed: Coordinator Provenance Sidecar

- **id:** `0011-coordinator-provenance-sidecar`
- **status:** `shipped`
- **completed:** `2026-05-21`

## Problem

Optimizer reports did not prove the commit base/head used for benchmarking, so
rebased or mismatched target branches could pass silently.

## Shipped

Added `coordinator-provenance.json` with `base_sha` and `head_sha`, resume
checks against the archived baseline source SHA, finalize propagation, and
schema bundling.
