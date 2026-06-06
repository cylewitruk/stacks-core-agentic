# Completed: Disk-First Prompt Templates

- **id:** `0004-disk-first-prompts`
- **status:** `shipped`
- **completed:** `2026-05-12`
- **source:** `assets/autonomous-roadmap.md`

## Problem

Compile-time prompt templates made operator tuning awkward and drift hard to
inspect.

## Shipped

Prompt templates moved to disk under the operator's configured prompt override
directory, seeded from bundled defaults and rendered with MiniJinja strict mode.
Reference context docs moved into the same operator-tunable surface.
