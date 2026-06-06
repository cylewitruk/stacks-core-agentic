# Completed: Prompt Lint And Sync

- **id:** `0005-prompt-lint-sync`
- **status:** `shipped`
- **completed:** `2026-05-12`
- **source:** `assets/autonomous-roadmap.md`

## Problem

Disk-loaded prompts need a runtime drift check and a way to restore bundled
defaults.

## Shipped

Added `sbagent prompt lint` and `sbagent prompt sync --force`. Lint renders
every template against field-complete synthetic prompt structs.
