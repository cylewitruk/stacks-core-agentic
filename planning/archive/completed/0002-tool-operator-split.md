# Completed: Tool / Operator Split

- **id:** `0002-tool-operator-split`
- **status:** `shipped`
- **completed:** `2026-05-12`
- **source:** `assets/autonomous-roadmap.md`

## Problem

Long-lived autonomous state, schedules, events, and bot credentials did not
belong in the tool repository.

## Shipped

The project split into:

- Tool repo: `stacks-bench-agent`.
- Operator repo: `stacks-bench-agentic-operator`.

The tool remains versioned code and bundled defaults; the operator owns runtime
state, config, scheduling, and audit history.
