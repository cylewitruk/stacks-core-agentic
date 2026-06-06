# Completed: DB / Artifact Run-ID Consistency

- **id:** `0016-db-artifact-consistency`
- **status:** `shipped`
- **completed:** `2026-05-21`

## Problem

If the bench DB is wiped or misconfigured, run-id references in session
artifacts become dangling and can poison finalize/archive/publish audit data.

## Shipped

Added advisory DB-vs-artifact dangling run-id checks before finalize and
archive.
