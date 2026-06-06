# Planning System

This directory is the canonical repo-local planning system for `sbagent`.

## Where To Look

- [iterations/](iterations/) — selected deliverables currently planned or in
  progress.
- [backlog.md](backlog.md) — implementation items not currently assigned to an
  iteration.
- [design/](design/) — optional detailed plans for backlog/iteration items.
- [archive/completed/](archive/completed/) — shipped work summaries.
- [archive/rejected/](archive/rejected/) — ideas we deliberately declined.
- [archive/superseded/](archive/superseded/) — historical plans kept for
  archaeology, not current instructions.
- [decisions/](decisions/) — durable architecture decisions.

## Lifecycle

```text
backlog -> iteration -> archive/completed
        \-> archive/rejected
        \-> archive/superseded
```

## Item IDs

Implementation items use stable numeric IDs in likely historical order:

```text
NNNN-short-slug
```

Use the same ID everywhere an item appears:

- backlog metadata while unscheduled: `id: 0021-preflight-v2`
- iteration item list once selected: `0021-preflight-v2`
- optional design doc: `planning/design/0021-preflight-v2.md`
- completed archive: `planning/archive/completed/0021-preflight-v2.md`
- rejected archive: `planning/archive/rejected/0021-preflight-v2.md`
- superseded archive: `planning/archive/superseded/0021-preflight-v2.md`

Iterations are different: they group one or more items into a deliverable and
use their own `vN-*` names.

An active item should live in one place: `backlog.md` while unscheduled, or an
iteration doc once selected. Do not duplicate a full item in both places; use a
short pointer only when it helps navigation.

Statuses are execution state; location is still determined by the file:
backlog items live in `backlog.md`, active selected work lives in
`iterations/`, and terminal work lives in `archive/`.

- `backlog` — captured but unscheduled.
- `candidate` — plausible near-term work, still in backlog.
- `planned` — selected inside an iteration.
- `in_progress` — actively being implemented inside an iteration.
- `blocked` — cannot move without a decision or external input.
- `shipped` — implemented and archived.
- `superseded` — replaced by a newer plan or design.
- `rejected` — intentionally not pursued.

## Item Template

Use this shape for backlog entries:

```md
### Short Title

- **id:** `NNNN-stable-kebab-id`
- **status:** `candidate`
- **priority:** `medium`
- **source:** optional link to decision, issue, session, or archive note
- **design:** optional link to a detailed design doc

**Problem:** What is wrong or missing?

**Scope:** What this item would change.

**Acceptance:** Observable checks that make it done.

**Deferred / non-goals:** What this item explicitly leaves out.
```

Keep planning docs concise. Move old rationale to a decision note or archive
note once a decision is stable.

## Iterations

See [iterations/README.md](iterations/README.md) for the deliverable template
and ownership rules.

## Decisions

Architecture decisions can feed several items and usually outlive the item that
triggered them. Keep them in [decisions/](decisions/) rather than archiving
them with a single implementation item.

## Archiving Items

When an item leaves backlog or an iteration:

1. Create one archive file named with the same item ID, e.g.
   `planning/archive/completed/0028-optimizer-memory.md`.
2. Start with the item metadata and a concise problem/scope summary.
3. Include the matching design doc contents when one existed and they remain
   useful for archaeology.
4. Record what shipped, validation evidence, notable deviations, and follow-up
   items when applicable.
5. Remove or mark superseded the live design doc so future agents do not treat
   it as current.
