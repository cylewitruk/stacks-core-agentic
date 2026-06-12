# Completed: Phase Timing

- **id:** `0026-phase-timing`
- **status:** `shipped`
- **completed:** `2026-06-11`
- **iteration:** [v5: Archive Metadata](../../iterations/v5-archive-metadata.md)

## Problem

`SessionRecord.phase_durations_secs` existed but archived as an empty map, so
operators had to reconstruct session timing from logs.

## Shipped

- Added `models::timings::Timings` (`schema_version: 1`) and bundled
  `timings.schema.json`.
- Added `session::phase_timing::PhaseTimingsRecorder`.
- `cli/session/run.rs` records wall-clock durations after each full-pipeline
  phase and rewrites `<session>/results/timings.json` atomically after each
  successful phase.
- Archive reads `timings.json` into
  `SessionRecord.phase_durations_secs`; missing files remain legacy-compatible
  and archive as `{}`.

## Validation

- `tests/phase_timing.rs` covers recorder round-trip and crash-partial behavior.
- `tests/archive.rs::archive_populates_phase_durations_secs_from_timings_json`
  covers archive ingestion.
- `just lint --no-sccache` and focused phase-timing tests passed during review.

## Live Validation

Smoke session `20260611-172955` archived real `phase_durations_secs`, and
`history show` rendered the phase-duration bars from the operator ledger.
