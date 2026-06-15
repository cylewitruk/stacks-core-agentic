# v17: History Report

Successor to
[v16: Projection Migration Completion](../archive/completed/v16-projection-migration-completion.md).
v16 made `HistoryProjectionV1` the shared read-side API for autonomy and
history detail. v17 uses that substrate to finally ship the deferred
`0043-history-report` command without adding another raw-ledger projection.

> **Status:** planned.
>
> v17 is a read-only reporting iteration. It should not mutate GitHub, append
> ledgers, schedule workflows, or change session / maintain schemas.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0043-history-report` | primary | planned |

## Why

Operators now have:

- `history list` for compact session scanning;
- `history show` for per-session detail;
- `maintain.jsonl` lifecycle events from v10/v11;
- dedup decisions from v12;
- optimizer memory from v13;
- the shared read-side projection from v15/v16.

The missing surface is a single markdown report that can be saved, committed,
or pasted into a weekly review. v17 should make the operator's autonomous loop
legible without requiring `jq` over `sessions.jsonl` and `maintain.jsonl`.

## Source-Of-Truth Invariants

- `HistoryProjectionV1` is the only cross-session read API for v17 report
  data.
- `sessions.jsonl` and `maintain.jsonl` remain the durable ledgers.
- The report is a rendered view, not a new state source.
- No schema files change.
- `history list` and `history show` output should remain unchanged unless a
  helper extraction is strictly behavior-preserving.
- The report is markdown-only in v17.

## Scope

In scope:

- Add `sbagent history report [--since <ref>] [--out <path>]`.
- Split the history CLI module before adding the report surface:
  `cli/history/{mod.rs,list.rs,show.rs,report.rs}`. Shared helpers such as
  `parse_since` live in `cli/history/mod.rs`.
- Read report data from `HistoryProjectionV1`, not raw ledgers.
- Support `--since` values compatible with `history list` where practical:
  - `YYYY-MM-DD`;
  - ISO week id `YYYY-Www`.
- Default `--since` to the most recent ISO week with archived sessions.
- Add `ReportViewV1::build_v1(projection: &HistoryProjectionV1, since:
  SinceCutoff) -> ReportViewV1` as the typed boundary between projection data
  and markdown rendering.
- Render markdown sections:
  - summary rollup;
  - per-session table;
  - PR lifecycle rollup and PR links;
  - issue lifecycle rollup and issue links;
  - dedup skips / repeated-signature notes only if Phase 1 proves they are
    visible through archived projection data.
- Write to stdout by default.
- With `--out <path>`, create parent directories, write the markdown file, and
  keep stdout empty.
- Add byte-equality fixture tests for stdout and `--out`.

Out of scope:

- Scheduled report generation.
- Committing reports under `reports/`.
- HTML, TUI, JSON, or CSV output.
- New maintain events or GitHub API calls.
- New projection methods solely for speculative future use.
- Token/cost reporting unless already present in the projection.
- Named-phase vocabulary cleanup (`0025`) beyond report-local wording.
- Reading per-session archive branch artifacts such as
  `optimization-targets.json`. v17 consumes the operator ledgers through
  `HistoryProjectionV1` only.

## Report Contract

The default report should be deterministic, ASCII markdown:

```text
# sbagent history report: <range>

## Summary
Total sessions: N (succeeded: A, failed: B, aborted: C)
Targets: accepted A / rejected R / aborted Ab
Total wall-clock: ...

## Sessions
| id | started_at | status | targets | wall-clock | prs | issues |

## Pull requests
...

## Issues
...

## Dedup skips (only when archived projection data exposes rows)
...
```

Formatting rules:

- Use GitHub-flavored markdown tables only where they stay readable.
- Use stable ordering:
  - sessions newest first;
  - lifecycle links grouped by latest state, then URL;
  - dedup skips grouped by reason, then target id.
- Omit optional sections entirely when they have no rows. Do not render empty
  placeholder tables for pull requests, issues, or dedup skips.
- Use `-` for absent values.
- Do not emit ANSI color or Unicode box drawing.
- Markdown output is byte-identical whether stdout is a TTY or a pipe.
- Keep report prose concise; this is an operator digest, not a narrative
  retrospective.

## Phases

### Phase 1: History Module Split + Projection Report View

**Goal:** Add a small typed report view over `HistoryProjectionV1` without
teaching the renderer about ledger internals.

**Scope:**

- Split `cli/history.rs` into:
  - `cli/history/mod.rs` for command dispatch and shared helpers;
  - `cli/history/list.rs`;
  - `cli/history/show.rs`;
  - `cli/history/report.rs`.
- Move the existing `--since` parser into `cli/history/mod.rs` and reuse it for
  list and report. Do not add a second date parser.
- Define `ReportViewV1::build_v1(projection: &HistoryProjectionV1, since:
  SinceCutoff) -> ReportViewV1`.
- Define a read-only `ReportViewV1` containing:
  - selected sessions;
  - target outcome counts;
  - total wall-clock seconds;
  - PR / issue URL rows with latest lifecycle state;
  - dedup skip rows only if archived projection data exposes `dedup:` reason
    rows.
- Implement `--since` range selection.
- Implement default range selection: most recent ISO week with archived
  sessions.
- At the start of implementation, verify whether v12 dedup rows are visible
  through `HistoryProjectionV1` (for example through
  `ProjectedAttemptV1.reason_code` / `TargetRecord.reason_code`). If they are
  not visible, omit the `Dedup skips` section in v17 and record a follow-up for
  a future archive/projection surface. Do not read archive-branch
  `optimization-targets.json` to recover them.
- Add projection helpers only if report rendering truly needs them now; any
  helper must be consumer-neutral and tested in `history_projection.rs`.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Report view consumes `HistoryProjectionV1`; it does not read
      `sessions.jsonl` or `maintain.jsonl` directly.
- [ ] `cli/history.rs` is split into `mod.rs`, `list.rs`, `show.rs`, and
      `report.rs` with behavior-preserving list/show fixture coverage.
- [ ] `ReportViewV1::build_v1` is the renderer-facing typed boundary.
- [ ] `--since` parsing is shared with `history list`; no duplicate date parser
      exists.
- [ ] Phase 1 verifies whether archived projection data exposes `dedup:` rows.
      If not, v17 omits `Dedup skips` and records the follow-up explicitly.
- [ ] Default range is the latest ISO week containing archived sessions.
- [ ] `--since YYYY-MM-DD` and `--since YYYY-Www` filter sessions
      deterministically.
- [ ] Empty ledgers produce a friendly "no sessions archived yet" report or
      message with exit 0.
- [ ] Any new projection method is typed, consumer-neutral, and unit-tested.

**Tests:**

- Unit tests for range selection.
- Unit tests for report-view rollups against fixture projection data.

### Phase 2: Markdown Renderer

**Goal:** Render a deterministic, compact markdown report suitable for stdout,
commits, and copy/paste.

**Scope:**

- Render the required sections:
  - `Summary`;
  - `Sessions`;
  - `Pull requests`, when rows exist;
  - `Issues`, when rows exist;
  - `Dedup skips`, only when Phase 1 proves archived projection data exposes
    rows.
- Include session count, target outcome rollup, total wall-clock, PR/issue
  lifecycle counts, and links.
- Summary includes failed and aborted sessions, not only successful sessions:
  `Total sessions: N (succeeded: A, failed: B, aborted: C)`.
- The per-session table uses the fixed column set
  `id | started_at | status | targets | wall-clock | prs | issues` and sorts
  sessions newest first.
- Make mixed verdicts visible where target status/reason data carries them.
- Keep output pure ASCII.
- Add checked-in expected markdown fixtures.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Markdown fixture test asserts byte-for-byte equality.
- [ ] Sections render in the documented order.
- [ ] Empty optional sections are omitted; fixtures cover both populated and
      empty PR/issue cases, plus the dedup-present or dedup-omitted path
      selected in Phase 1.
- [ ] The summary counts succeeded, failed, and aborted sessions.
- [ ] The per-session table uses the documented columns and newest-first order.
- [ ] PR and issue lifecycle state comes from projection state, not from URL
      presence alone.
- [ ] Dedup skips render from archived dedup reasons only, or the section is
      omitted when Phase 1 proves those rows are not archived.
- [ ] Output is pure ASCII and contains no ANSI escapes.
- [ ] Output is byte-identical with and without a TTY.
- [ ] Missing optional fields render as `-`.

**Tests:**

- Renderer unit test or integration fixture under
  `crates/stacks-bench-agent/tests/fixtures/history-report/`.

### Phase 3: CLI Wiring + `--out`

**Goal:** Expose the report as `sbagent history report` with scriptable stdout
and file-writing behavior.

**Scope:**

- Add `HistoryCommand::Report`.
- Add args:
  - `--since <ref>`;
  - `--out <path>`.
- Reuse the shared history `parse_since` helper.
- Default to stdout when `--out` is absent.
- When `--out` is present:
  - create parent directories;
  - write the markdown file;
  - keep stdout empty;
  - preserve stderr for warnings only.
- Reuse skipped-line warning behavior from projection reads.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] `sbagent history report` prints markdown to stdout.
- [ ] `sbagent history report --out reports/<iso-week>.md` writes the file and
      prints nothing to stdout.
- [ ] `--since` changes the selected sessions in integration tests.
- [ ] Missing output parent directories are created.
- [ ] Invalid `--since` fails with a clear diagnostic.

**Tests:**

- Binary integration tests, mirroring `history list` / `history show` style.

### Phase 4: Docs + Planning Handoff

**Goal:** Document the report command and leave the next automation step clear.

**Scope:**

- Update operator docs with a short `history report` section.
- Update `assets/autonomous-roadmap.md` to mark Layer 3D shipped.
- Do not add a workflow/timer for report generation in v17.
- Note the natural follow-up: scheduled report generation or report commits
  only after operators confirm the manual report shape is useful.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Docs show stdout and `--out` examples.
- [ ] Docs state the report is read-only.
- [ ] Roadmap no longer describes `history report` as unimplemented after v17.
- [ ] Follow-up notes defer scheduled report commits.

**Tests:**

- `just lint --no-sccache`
- `just test --summary --no-sccache`

## Final Validation

- [ ] `just lint --no-sccache`
- [ ] `just test --summary --no-sccache`
- [ ] Test count is at least the v16 baseline of 585 unless redundant coverage
      is explicitly named and replaced.
- [ ] No schema files changed.
- [ ] `history list` and `history show` fixture output remains unchanged.
- [ ] `history report` never reads raw ledgers directly outside
      `HistoryProjectionV1`.
- [ ] `--out` integration test proves stdout is empty.

## Follow-Ups

- Scheduled / committed reports can be a later iteration if the manual report
  is useful in practice.
- `0025-named-phases` remains a good opportunistic prose pass if report wording
  makes phase numbering feel too noisy.
- Live re-smoke on the systemd host can use the report as the digest artifact.
