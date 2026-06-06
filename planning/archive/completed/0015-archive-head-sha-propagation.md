# Completed: Archive Head SHA Propagation

- **id:** `0015-archive-head-sha-propagation`
- **status:** `shipped`
- **completed:** `2026-05-21`

## Problem

`summary.json` carried target `head_sha`, but the archive ledger dropped it.

## Shipped

Archive now copies `summary.json.experiments[].head_sha` into
`SessionRecord.targets[]`.
