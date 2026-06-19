# 0043: History Report

- **id:** `0043-history-report`
- **status:** `shipped`
- **iteration:** [v17: History Report](v17-history-report.md)
- **completed:** 2026-06-19

## Shipped

v17 added `sbagent history report` as a read-only markdown digest over
`HistoryProjectionV1`:

- `ReportViewV1::build_v1(&HistoryProjectionV1, SinceCutoff) ->
  ReportViewV1` is the typed boundary between projection data and
  markdown rendering. Pure function; same inputs → same view.
- `render_markdown(&ReportViewV1) -> String` produces deterministic
  ASCII markdown — no env reads, no clock reads, no ANSI escapes.
- `sbagent history report [--since <YYYY-MM-DD | YYYY-Www>] [--out
  <path>]` prints to stdout by default; with `--out` it writes the
  file (creating parents) and keeps stdout empty.
- Sections: `Summary`, `Sessions`, optional `Pull requests`, optional
  `Issues`, optional `Mixed verdicts`. Empty optional sections are
  omitted entirely.
- Default cutoff is the Monday of the ISO week containing the most
  recent archived session; empty ledgers render the `no sessions
  archived yet` notice.
- Mixed verdicts (`reason_code` starting with `mixed:`) are surfaced
  both as a `(mixed N)` annotation on the Summary line and in a
  dedicated section, matching the classification rule `history show`
  uses for per-target status.
- `cli/history.rs` was split into `cli/history/{mod, list, show,
  report}.rs`; `parse_since` and ISO date helpers stay shared in
  `mod.rs` so list and report cannot drift on `--since` syntax.

## Validation

- `just lint --no-sccache`
- `just test --summary --no-sccache` — 612/612 passing.
- 20 unit tests in `cli::history::report::tests` (typed view + range
  selection + renderer byte-equality + section-omission discipline +
  mixed-verdict classification).
- 7 binary integration tests in `tests/history_report.rs` (stdout
  default, `--out` writes and creates parents, `--since` filters,
  invalid `--since` diagnostic, empty ledger notice, stdout +
  `--out` byte-equality against a single shared `EXPECTED_REPORT`
  fixture).
- `tests/history_list.rs` (6/6) and `tests/history_show.rs` (5/5)
  fixtures still pass — list / show output preserved.
- Audit boundary: `history report` reads only via
  `HistoryProjectionV1` (`sessions`, `latest_artifact_state`); no
  raw `sessions.jsonl` / `maintain.jsonl` access in the report
  module.

## Decisions

- **Dedup skips section omitted.** v12 dedup decisions live in
  per-session `optimization-targets.json` under `rejected_by_merge[]`
  on the archive branch; finalize processes only `MergedTarget`
  survivors, so `SessionRecord.targets[]` never carries
  dedup-skipped rows. Surfacing them is a follow-up (schema
  addition on `sessions.jsonl` OR archive-branch read).
- **Mixed verdicts are a subset of accepted**, not a fourth A/R/Ab
  bucket. They shipped (accepted) but the verdict carried a caveat.
- **Lifecycle rows require projection state**, never URL presence
  alone. Targets with PR/issue URLs that `maintain` has not observed
  do not appear in the lifecycle sections (though they still count
  in per-session `prs` / `issues` columns, matching `history list`).

## Follow-Up

Scheduled report generation + weekly `reports/<iso-week>.md` commits
remain deferred. Worth picking up only after operators confirm the
manual report shape is useful in practice. Surfacing dedup decisions
in the report is a separate follow-up; see the iteration doc for the
two candidate implementation paths.
