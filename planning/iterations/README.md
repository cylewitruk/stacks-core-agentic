# Iterations

Concrete deliverable specs live here. An iteration may contain one or more
backlog items, but it should have one validation story and one clear archive
destination. This directory is the current-priority surface.

Use `vN-short-name.md` naming. Iterations are deliverables, not item IDs; they
should reference the numbered items they implement.

When an iteration ships, archive each completed item under its own
`NNNN-slug.md` file in `planning/archive/completed/`. Leave the iteration file
only if it remains useful as a validation recipe; otherwise move it to
`planning/archive/superseded/`.

## Iteration Template

```md
# vN: Deliverable Name

Successor to optional previous iteration. One-paragraph goal.

> **Status:** planned | in_progress | shipped | superseded
>
> Short current-state note, including what shipped, what remains, and where
> follow-up work moved.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `NNNN-item-id` | primary | planned |

## Why

The problem this iteration solves.

## Scope

What changes in this deliverable.

## Phases

### Phase 1: Name

**Goal:** Concrete objective.

**Scope:**

- Work item.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests, if applicable
- [ ] Reviewed
- [ ] Validated, meaning the acceptance checks below were run

**Acceptance & Validation:**

- [ ] Acceptance criterion, plus how it should be validated.

**Tests:**

- [test_name](../../crates/stacks-bench-agent/tests/some_test_file.rs)
- Manual/smoke check if no automated test applies.

**Notes:** Optional; omit if none.

## Final Validation

Observable checks for the whole iteration.

## Follow-Ups

New item IDs or backlog entries produced by this iteration.
```
