# Completed: Sync Refreshes Tunables By Default

- **id:** `0014-sync-refresh-by-default`
- **status:** `shipped`
- **completed:** `2026-05-21`

## Problem

Prompt/context drift could leave operator templates behind the tool's typed
contracts.

## Shipped

`sbagent sync` now refreshes schemas, queries, prompts, and context by default.
`--keep-tunables` preserves operator edits.
