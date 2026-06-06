# Completed: Preflight V1

- **id:** `0013-preflight-v1`
- **status:** `shipped`
- **completed:** `2026-05-21`

## Problem

Operator/tool drift repeatedly wasted session time after expensive phases had
already started.

## Shipped

Added session-start preflight checks for installed-binary drift, load-bearing
prompt drift, and submodule reachability. Wired into session commands and
`sbagent check`, with `--skip-preflight` as the explicit escape hatch.
