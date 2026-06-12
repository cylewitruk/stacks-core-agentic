# Completed: Evidence-Backed Verification

- **id:** `0044-evidence-backed-verification`
- **status:** `shipped`
- **completed:** `2026-06-12`
- **iteration:** [v7: Evidence-Backed Verification](v7-evidence-backed-verification.md)

## Problem

`bench-run.json` is only a coarse run envelope. The analyzer did not emit a
structured query trail, so the results analyzer had to infer what DB evidence
to compare after optimization.

## Shipped

- Added typed analyzer evidence provenance and carried it through merge into
  optimization targets.
- Added bundled paired baseline-vs-candidate SQL queries.
- Updated the results-analyzer prompt to treat the benchmark DB as the primary
  mechanism evidence and `bench-run.json` as the envelope.
- Kept all results-analyzer follow-up queries in `db_queries[]`.

## Validation

- `just lint --no-sccache`, `just test --summary --no-sccache`, and prompt
  lint passed during v7 review.
- Smoke session `20260611-172955` validated the flow on real artifacts. The
  MARF target archived as `mixed` after DB evidence showed the mechanism moved
  in the expected direction but below the expected magnitude band.
