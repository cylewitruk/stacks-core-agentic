# v17: History Report

Successor to
[v16: Projection Migration Completion](v16-projection-migration-completion.md).
v16 made `HistoryProjectionV1` the shared read-side API for autonomy and
history detail. v17 uses that substrate to finally ship the deferred
`0043-history-report` command without adding another raw-ledger projection.

> **Status:** shipped 2026-06-19.
>
> v17 is a read-only reporting iteration. It should not mutate GitHub, append
> ledgers, schedule workflows, or change session / maintain schemas.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0043-history-report` | primary | shipped |

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

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Report view consumes `HistoryProjectionV1`; it does not read
      `sessions.jsonl` or `maintain.jsonl` directly.
- [x] `cli/history.rs` is split into `mod.rs`, `list.rs`, `show.rs`, and
      `report.rs` with behavior-preserving list/show fixture coverage.
      (Both `tests/history_list.rs` (6/6) and `tests/history_show.rs`
      (5/5) pass against the split.)
- [x] `ReportViewV1::build_v1(&HistoryProjectionV1, SinceCutoff) ->
      ReportViewV1` is the renderer-facing typed boundary; pure
      function of its inputs.
- [x] `--since` parsing is shared with `history list` via
      `cli/history/mod.rs::parse_since`; no duplicate date parser
      exists. `report::parse_explicit_cutoff` wraps it to yield
      `SinceCutoff` with `CutoffSource::Explicit`.
- [x] Phase 1 verified that archived projection data does NOT expose
      `dedup:` rows. v12 dedup decisions live in per-session
      `optimization-targets.json` under `rejected_by_merge[]` in the
      archive branch; finalize processes only `MergedTarget`
      survivors, so `SessionRecord.targets[]` never carries
      dedup-skipped rows. **v17 omits the `Dedup skips` section.**
      Follow-up recorded for a future iteration that either: (a)
      surfaces dedup counts via a `sessions.jsonl` schema addition,
      or (b) reads `optimization-targets.json` from archive branches
      (heavyweight).
- [x] Default range is the latest ISO week containing archived
      sessions (Monday of that week). Empty ledger yields a
      `0000-00-00` epoch sentinel with `CutoffSource::EmptyLedger`.
- [x] `--since YYYY-MM-DD` and `--since YYYY-Www` filter sessions
      deterministically via `parse_explicit_cutoff`.
- [x] Empty ledger / empty selection produces a "no sessions archived
      yet" message at the renderer (Phase 2) or CLI (Phase 3); Phase 1
      surfaces this via `CutoffSource::EmptyLedger` + zero-row view.
- [x] No new projection methods added in Phase 1. ReportViewV1 derives
      everything from the existing `sessions()`,
      `latest_artifact_state(url)` API surface. (Discipline:
      consumer-neutral, typed-return-only.)

**Tests:**

- Unit tests in `cli::history::report::tests`:
  - `default_cutoff_empty_projection_renders_epoch_sentinel`
  - `default_cutoff_picks_monday_of_iso_week_containing_latest_session`
  - `build_v1_selects_sessions_on_or_after_cutoff_newest_first`
  - `build_v1_rolls_up_target_outcomes_with_failed_folded_into_aborted`
  - `build_v1_lifecycle_rows_require_projection_state_not_just_url_presence`
  - `build_v1_lifecycle_rows_sort_by_state_then_url`
  - `parse_explicit_cutoff_supports_iso_date_and_iso_week`
  - `monday_of_iso_week_containing_returns_monday`
  - `iso_week_monday_helper_is_consistent_with_default_cutoff`
- Existing `cli::history::tests` (8 tests for `parse_since`, `is_iso_date`,
  `iso_week_monday`, `days_from_civil`, `weekday_from_serial`, and
  `format_wall_clock`) preserved verbatim through the split.
- `tests/history_list.rs` (6/6) + `tests/history_show.rs` (5/5)
  byte-equality fixtures still pass — list and show behavior preserved.
- Workspace test count after Phase 1: **594/594** (up from v16 baseline
  of 585; +9 from new `report.rs` unit tests).

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

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Markdown fixture test asserts byte-for-byte equality
      (`render_markdown_full_view_matches_byte_equality_fixture` in
      `cli::history::report::tests`).
- [x] Sections render in the documented order: `Summary`, `Sessions`,
      `Pull requests` (when present), `Issues` (when present). Dedup
      section never renders — see Phase 1 acceptance.
- [x] Empty optional sections are omitted. Tests cover both populated
      cases (`render_markdown_full_view_matches_byte_equality_fixture`),
      PR-only / no-issues
      (`render_markdown_omits_issues_section_when_only_prs_present`),
      and zero-lifecycle (`render_markdown_omits_pr_section_when_no_pr_lifecycle_rows`),
      plus the dedup-omitted path
      (`render_markdown_never_renders_dedup_section`).
- [x] The summary line `Total sessions: N (succeeded: A, failed: B,
      aborted: C)` counts all three buckets.
- [x] The per-session table uses the
      `id | started_at | status | targets | wall-clock | prs | issues`
      column set and renders sessions newest-first by `started_at`
      (proven by the byte-equality fixture).
- [x] PR and issue lifecycle state comes from
      `HistoryProjectionV1::latest_artifact_state(url)`, not URL
      presence. Carried over from the Phase 1 `build_v1` contract; the
      renderer only emits rows the view already filtered down to
      observed URLs.
- [x] Dedup skips section is omitted per Phase 1
      (`render_markdown_never_renders_dedup_section` pins this as a
      regression test).
- [x] Output is pure ASCII (no byte ≥ 0x80) and contains no ANSI
      escape codes (asserted by the byte-equality fixture).
- [x] `render_markdown` is a pure function of `&ReportViewV1`; output
      is byte-identical regardless of TTY (the renderer never reads
      env / stdout state).
- [x] Header labels distinguish default vs. explicit cutoffs:
      - explicit → `since 2026-06-01`;
      - default → `since 2026-06-15 (default: latest ISO week)`;
      - empty ledger → `empty ledger`.
- [x] Empty selection (cutoff excludes all sessions OR empty ledger)
      yields title + `no sessions archived yet`, matching `history list`.
- [x] Mixed verdicts are surfaced (Phase 2 follow-up after Codex
      review). Classification rule matches `history show`: target
      status is `Accepted` AND `reason_code` starts with `mixed:`.
      Surface in two places:
      - Summary line annotation `Targets: accepted N (mixed M) /
        rejected R / aborted Ab` when M > 0; omitted when M = 0.
      - Optional `## Mixed verdicts` section listing `session | target
        | reason` rows, sorted by `(session_id, target_id)`.
      `mixed` is a SUBSET of `accepted` — it does not change A/R/Ab
      bucket counts. Implementation lives in
      [`TargetOutcomeCounts::mixed`](../../crates/stacks-bench-agent/src/cli/history/report.rs) +
      [`MixedVerdictRow`](../../crates/stacks-bench-agent/src/cli/history/report.rs) +
      [`ReportViewV1::mixed_verdicts`](../../crates/stacks-bench-agent/src/cli/history/report.rs).

**Tests:**

- 11 new unit tests in `cli::history::report::tests`:
  - `render_markdown_empty_ledger_renders_title_and_empty_message`
  - `render_markdown_explicit_cutoff_with_no_matches_still_emits_empty_message`
  - `render_markdown_header_labels_default_cutoff_distinctly`
  - `render_markdown_full_view_matches_byte_equality_fixture` (the
    section-order + ASCII + no-ANSI byte-equality test for the
    no-mixed common case)
  - `render_markdown_omits_pr_section_when_no_pr_lifecycle_rows`
  - `render_markdown_omits_issues_section_when_only_prs_present`
  - `render_markdown_never_renders_dedup_section`
  - `build_v1_classifies_accepted_with_mixed_reason_as_mixed_subset`
  - `build_v1_does_not_classify_non_mixed_reason_codes_as_mixed`
  - `render_markdown_surfaces_mixed_verdicts_in_summary_and_section`
    (byte-equality fixture for the mixed case)
  - `render_markdown_omits_mixed_section_when_no_mixed_verdicts`
- Workspace test count after Phase 2: **605/605** (+11 over Phase 1's
  594).
- Phase 1 fixtures (`tests/history_list.rs` 6/6, `tests/history_show.rs`
  5/5) still pass — no regression on list/show.

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

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `sbagent history report` prints markdown to stdout
      (`history_report_stdout_default_prints_markdown_digest`).
- [x] `sbagent history report --out <path>` writes the file and
      prints nothing to stdout
      (`history_report_out_writes_file_and_keeps_stdout_empty`).
- [x] `--since` changes the selected sessions; ISO date and ISO
      week id produce the same selection
      (`history_report_since_filters_sessions`).
- [x] Missing output parent directories are created — the test
      writes to `reports/2026-W23/digest.md` under a fresh tempdir
      and asserts the file exists.
- [x] Invalid `--since` fails non-zero with a diagnostic that
      includes the recognized formats (`YYYY-MM-DD`, `YYYY-Www`)
      (`history_report_invalid_since_exits_nonzero_with_diagnostic`).
- [x] Empty ledger renders the `empty ledger` title + `no sessions
      archived yet` notice
      (`history_report_empty_ledger_renders_no_sessions_notice`).
- [x] CLI reuses `super::parse_since` via `parse_explicit_cutoff`;
      no duplicate date parser. Skipped-line warnings on
      `sessions.jsonl` and `maintain.jsonl` route through the same
      projection-read path `history show` uses.
- [x] Byte-equality fixtures pin the CLI layer for both output
      paths. A single `EXPECTED_REPORT` constant drives both
      `history_report_stdout_matches_byte_equality_fixture` (asserts
      `stdout == EXPECTED_REPORT`) and
      `history_report_out_file_matches_byte_equality_fixture_and_stdout_is_empty`
      (asserts `read_to_string(--out) == EXPECTED_REPORT` AND
      stdout empty), so the two output paths cannot drift apart.
      Renderer-layer byte equality is pinned at the unit-test
      layer (Phase 2), CLI-layer byte equality is pinned here.

**Tests:**

- 7 new binary integration tests in
  `crates/stacks-bench-agent/tests/history_report.rs`:
  - `history_report_stdout_default_prints_markdown_digest`
  - `history_report_out_writes_file_and_keeps_stdout_empty`
  - `history_report_since_filters_sessions`
  - `history_report_invalid_since_exits_nonzero_with_diagnostic`
  - `history_report_stdout_matches_byte_equality_fixture`
  - `history_report_out_file_matches_byte_equality_fixture_and_stdout_is_empty`
  - `history_report_empty_ledger_renders_no_sessions_notice`
- Workspace test count after Phase 3: **612/612** (+7 over Phase 2's
  605).
- Phase 1/2 fixtures (`tests/history_list.rs` 6/6,
  `tests/history_show.rs` 5/5, `cli::history::report::tests` 20/20)
  still pass.

### Phase 4: Docs + Planning Handoff

**Goal:** Document the report command and leave the next automation step clear.

**Scope:**

- Update operator docs with a short `history report` section.
- Update `assets/autonomous-roadmap.md` to mark Layer 3D shipped.
- Do not add a workflow/timer for report generation in v17.
- Note the natural follow-up: scheduled report generation or report commits
  only after operators confirm the manual report shape is useful.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Docs show stdout and `--out` examples. Added a
      `History reports: sbagent history report` section to
      [`docs/operations.md`](../../docs/operations.md), placed right
      after `Workspace hygiene` (the closest sibling operator-CLI
      section). Examples cover default cutoff, explicit
      `--since` with `YYYY-MM-DD`, `--since` with `YYYY-Www`, and
      `--out reports/<id>.md`.
- [x] Docs state the report is read-only. The new operations.md
      section opens with **Read-only** in bold and spells out that the
      command consumes `HistoryProjectionV1` (no GH mutations, no
      ledger appends, no workflow scheduling). It also points at
      `history show` / `history list` for the per-session and
      leaderboard views.
- [x] Roadmap no longer describes `history report` as unimplemented
      after v17. Layer 3D in
      [`assets/autonomous-roadmap.md`](../../assets/autonomous-roadmap.md)
      flipped from `[~]` Planned to `[x]` Shipped with the actual
      surface (`--since`, `--out`; no `--format=markdown` flag, since
      markdown is the only output). The
      `Recommended start sequence` line about Layer 3D being "last (or
      skip until needed)" was updated to note it shipped in v17.
- [x] Follow-up notes defer scheduled report commits. Layer 3D's
      bullet list closes with "Follow-up (deferred): scheduled
      report generation + weekly `reports/<iso-week>.md` commits.
      Hold until operators confirm the manual report shape is useful
      in practice."

**Tests:**

- `just lint --no-sccache` — clean.
- `just test --summary --no-sccache` — **612/612 pass**.
- No code change in Phase 4; doc-only iteration on top of the Phase 3
  CLI.

## Final Validation

- [x] `just lint --no-sccache` — clean.
- [x] `just test --summary --no-sccache` — **612/612 pass**.
- [x] Test count is at least the v16 baseline of 585. Final count
      is 612 (+27 over baseline; +20 in `cli::history::report::tests`
      + 7 binary integration tests in `tests/history_report.rs`).
- [x] No schema files changed.
- [x] `history list` and `history show` fixture output remains
      unchanged. `tests/history_list.rs` (6/6) and
      `tests/history_show.rs` (5/5) pass against the post-split,
      post-renderer, post-CLI-wiring tree.
- [x] `history report` never reads raw ledgers directly outside
      `HistoryProjectionV1`. Both `cli::history::report::run_report`
      and `ReportViewV1::build_v1` consume
      `history_projection::read_operator_projection_v1` /
      `HistoryProjectionV1::sessions` /
      `HistoryProjectionV1::latest_artifact_state` only.
- [x] `--out` integration test proves stdout is empty
      (`history_report_out_writes_file_and_keeps_stdout_empty` and
      `history_report_out_file_matches_byte_equality_fixture_and_stdout_is_empty`).

## Follow-Ups

- Scheduled / committed reports can be a later iteration if the manual report
  is useful in practice.
- `0025-named-phases` remains a good opportunistic prose pass if report wording
  makes phase numbering feel too noisy.
- Live re-smoke on the systemd host can use the report as the digest artifact.
