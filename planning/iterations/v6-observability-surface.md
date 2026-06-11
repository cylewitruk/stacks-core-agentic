# v6: Observability Surface

Successor to [v5: Archive Metadata](v5-archive-metadata.md). v5
made `sessions.jsonl` carry the data operators actually want — per-phase
durations, PR/issue URLs, bench wall-clock totals. v6 makes that
data easy to consume: `sbagent history list` for the leaderboard,
`sbagent history show <id>` for per-session detail. Plus a small
prologue to close out the now-superseded `0021-preflight-v2`
planning debt before the next iteration uses preflight as a hook.

> **Status:** planned.
>
> Scoped tight on purpose — v5 already shipped the data; v6 just
> reads it back. No new artifacts, no new sidecars. The Stretch
> phase (weekly report) is genuinely optional and may move to a
> follow-up.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0021-preflight-v2` | prologue (rescope/close) | shipped (superseded by v3) |
| `0036-observability-surface` | primary | planned |

## Why

v5's three landed fields are dark unless someone parses
`sessions.jsonl` by hand. Operators currently can't answer:

- "Which sessions opened PRs this week?"
  → `pr_url` lives in `sessions.jsonl`, but no command renders it.
- "Where did session X spend its time?"
  → `phase_durations_secs` is populated but only via `jq`.
- "Which target reached bench fastest?"
  → `TargetBench.candidate_total_us` is populated for targets with
  `verification_replay`, but invisible.

Plus one piece of planning hygiene that v3 made obsolete but never
closed:

- **`0021-preflight-v2`** explicitly said "If `ephemeral-source-clone`
  ships first, delete or shrink this design" (see the now-archived
  design at
  [archive/superseded/0021-preflight-v2.md](../archive/superseded/0021-preflight-v2.md)).
  v3 shipped the ephemeral source clone. Both remaining checks
  (branch-ref divergence, network-fetch freshness) are obsoleted:
  per-session fetch + `source.json` SHA pinning catches the same
  drift classes that motivated the design. Closing it now removes
  stale planning debt before any v6-or-later preflight work
  inadvertently builds against a superseded spec.

## Scope

In scope:

- Execute `0021`'s documented kill switch: close as `superseded`
  with a short rationale referencing v3's per-session fetch +
  source.json contract. If the rescope reveals one or two checks
  that ARE still real (unlikely), shrink the design instead of
  closing it.
- New typed reader for `sessions.jsonl` exposing
  `read_all(&Path) -> Result<LedgerReadReport>`, where
  `LedgerReadReport { records, skipped }` returns unparseable
  lines in-band rather than tanking the whole read. CLI warning
  emission for `skipped` is the consumer's job (Phases 3/4); the
  reader itself never touches stderr. Phase 2 pins the contract;
  see that section for the full type signature.
- New `sbagent history list` subcommand: tabular per-session
  summary on stdout. One row per session with id, started_at,
  status, target outcome counts, total wall-clock from
  `phase_durations_secs.values().sum()`, opened-PR count.
- New `sbagent history show <session-id>` subcommand: full
  per-session detail. Per-phase durations rendered as an ASCII bar
  chart sized to terminal width, per-target table with status +
  `pr_url`/`issue_url` + bench totals.
- **(Stretch)** `sbagent history report [--out <path>]` — markdown
  report aggregating recent sessions; defaults to last 7 sessions
  or `--since <iso-week>`. Skip if budget runs short; can move to
  a follow-up.

Out of scope:

- Cross-session aggregation back into the ledger (e.g.
  `top_fix_signatures_by_attempts` — needs event projection, which
  is `0030-event-log-skeleton`'s territory).
- GitHub-side PR state reconciliation (open / merged / closed) —
  that's `0033-maintain-command`.
- Token spend tracking (no producer yet).
- Time-to-merge distribution (needs cross-session PR-state
  resolution per above).
- Web dashboard / TUI. Terminal-friendly stdout only.

## Phases

### Phase 1: `0021-preflight-v2` Prologue Rescope

**Goal:** `0021` lands at one of three terminal states with the
backlog + design updated to match: `superseded` (most likely
outcome), `shipped` (if the design's remaining checks are
quietly already implemented somewhere), or `backlog` with a
shrunken design (if one or two checks survive the v3 audit).

**Scope:**

- Audit the two checks `0021`'s design names against what v3 +
  v5's preflight stack actually does — both
  [cli/preflight.rs](../../crates/stacks-bench-agent/src/cli/preflight.rs)
  and [session/preflight.rs](../../crates/stacks-bench-agent/src/session/preflight.rs).
- **Branch-ref divergence**: v3's per-session
  fetch + `source.json` SHA pinning means every session start
  resolves a fresh SHA from the upstream branch. The drift mode
  `0021` named ("HEAD bump didn't update the branch ref") was
  specific to the operator submodule, which is gone. Expected
  verdict: superseded.
- **Network-fetch freshness**: v3's `ensure_cache` runs
  `git fetch <url> <branch>` at session start, with a write-once
  lock. The drift mode `0021` named ("operator forgot to fetch +
  bump") is impossible by construction. Expected verdict:
  superseded.
- Update [planning/backlog.md](../backlog.md) to remove `0021`'s
  entry (or shrink it). Move the design doc to
  [planning/archive/superseded/0021-preflight-v2.md](../archive/superseded/0021-preflight-v2.md)
  with a one-line "superseded by v3 per-session source clone"
  header note.

**Status:**

- [x] Core implementation
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `0021-preflight-v2` no longer appears in `backlog.md`'s
      candidate-items list. Verdict: superseded (no shrunken
      design needed — both checks structurally obsoleted by v3).
- [x] Design doc moved to
      `planning/archive/superseded/0021-preflight-v2.md` with
      rationale naming `ensure_cache`'s explicit-refspec fetch
      and `source.json.sha` pinning as the v3 mechanisms that
      subsume branch-ref divergence and network-fetch freshness.

**Tests:** No new tests; planning doc edits only.

**Outcome:** Verdict was superseded (the expected outcome). The
audit confirmed v3's per-session fetch + `source.json.sha` pinning
makes both remaining checks structurally impossible, not merely
unnecessary. `ensure_cache` force-updates the bare cache's
`refs/heads/<branch>` via explicit refspec on every session start;
`clone_session_checkout` clones from that just-refreshed ref; the
resolved SHA pins durably into `source.json`. No operator-side
branch ref to drift, no operator-side fetch step to forget. See
[archive/superseded/0021-preflight-v2.md](../archive/superseded/0021-preflight-v2.md)
for the full audit note.

### Phase 2: Typed Ledger Reader

**Goal:** A single source-of-truth reader for `sessions.jsonl` that
the new CLI commands (and future tools like `maintain`) all share.

**Scope:**

- New module `session/ledger_reader.rs` exposing a single
  reader function:

  ```rust
  pub fn read_all(path: &Path) -> Result<LedgerReadReport>;

  pub struct LedgerReadReport {
      pub records: Vec<SessionRecord>,
      pub skipped: Vec<SkippedLine>,
  }
  pub struct SkippedLine {
      pub line_number: usize,  // 1-indexed
      pub error: String,        // anyhow error chain rendered
  }
  ```

  **Default behavior is lossy**: unparseable lines land in
  `skipped` rather than failing the whole read, so a single
  corrupted line doesn't tank `history list` against a long
  ledger. Callers that want strict-parse semantics check
  `report.skipped.is_empty()` after the call.
- Parse path reuses
  [`SessionRecord::from_ledger_line`](../../crates/stacks-bench-agent/src/models/session_record.rs)
  for v1/v2/v3 read-compat (v4 Phase 2's pattern flows through
  unchanged).
- Skipped-line warning emission is the **CLI command's**
  responsibility, not the reader's: `history list` / `history show`
  call `eprintln!` once per skipped line after reading, so
  library consumers (future `maintain`, batch tooling) can stay
  quiet. The reader function does NOT touch stderr.
- Helper to derive total session wall-clock from a record:
  `session_total_secs(record: &SessionRecord) -> f64` —
  `record.phase_durations_secs.values().sum::<f64>()`. Tiny
  pure-function helper but it gives every CLI command and future
  tool one shared definition.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [x] `read_all` against a fixture ledger with 5 valid records
      returns `LedgerReadReport { records: [5 records], skipped: [] }`.
- [x] `read_all` against a mixed fixture (3 valid + 2 malformed
      lines) returns `records.len() == 3` AND `skipped.len() == 2`
      with each `SkippedLine.line_number` matching the actual
      file position. No `Err` — the reader itself never tanks
      on parse failures.
- [x] `SkippedLine.error` carries the underlying parse error
      message (not just an empty string).
- [x] `read_all` against a ledger carrying v1, v2, AND v3 records
      reads all three into `records` transparently (regression
      coverage for the schema-version compat pattern v4
      established). `skipped` stays empty.
- [x] Reader emits NO stderr output on its own — verified by
      the reader's signature and module-level contract; skipped
      lines are returned in-band via `LedgerReadReport.skipped`,
      never written to stderr. The CLI will `eprintln!` them in
      Phases 3 / 4.
- [x] `session_total_secs` is the deterministic sum of the
      `phase_durations_secs` values.
- [x] Bonus: blank/whitespace-only lines are silently tolerated
      (don't show up as `skipped`), so a trailing newline or
      hand-edited spacing doesn't trigger a spurious CLI warning.
- [x] Bonus: missing file returns `Ok(empty report)` rather
      than `Err`, matching the existing `ledger_contains_id`
      precedent and giving `history list` a clean
      "no sessions archived yet" path.

**Tests:**

- [tests/ledger_reader.rs](../../crates/stacks-bench-agent/tests/ledger_reader.rs)
  (new) — fixtures + the three round-trip cases above.

### Phase 3: `sbagent history list`

**Goal:** Operators get a one-line-per-session leaderboard view
on stdout without writing `jq` queries.

**Scope:**

- New `sbagent history` top-level subcommand under
  [cli/](../../crates/stacks-bench-agent/src/cli/) with one
  sub-subcommand for now: `list`.
- Args:
  - `--limit <N>` — max rows to print, default 20, most recent
    first (by `started_at` descending).
  - `--since <iso-week-or-date>` — filter to sessions started on
    or after the given week / date. Optional.
- Output: a fixed-column table on stdout. **ASCII-only by default**
  so fixture tests stay scriptable; no glyphs / emoji in the
  base table. Columns: `id` (full), `started_at` (ISO date, no
  time), `status` (`succeeded` / `failed` / `aborted` as
  rendered by `SessionStatus`), `targets` (`A/R/Ab` —
  slash-separated accepted / rejected / aborted counts, e.g.
  `3/1/0`), `wall-clock` (`mm:ss` or `hh:mm:ss` from
  `session_total_secs`), `prs` (count of `targets[].pr_url` set),
  `issues` (count of `targets[].issue_url` set).
- Optional color (status column only — green on `succeeded`,
  red on `failed`, yellow on `aborted`) when stdout is a TTY and
  `NO_COLOR` is unset. Color escape codes never appear in
  non-TTY output, so `sbagent history list | grep ...` stays
  clean and fixture-test assertions don't need to strip ANSI.
- Empty ledger: print "no sessions archived yet" and exit 0.
- Reads `<operator>/sessions.jsonl` via `Layout::operator_repo_root`.
  Errors loudly if the operator repo isn't configured (matches
  every other operator-facing command's expectation).

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] `sbagent history list` against a 3-session fixture prints
      a 3-row table with the canonical column set; total wall-clock
      matches the sum of `phase_durations_secs` per session.
- [ ] Output is pure ASCII — no glyphs, no emoji, no ANSI escape
      codes when stdout is piped (fixture tests assert byte-for-byte
      equality against a checked-in expected output).
- [ ] `--limit 2` truncates to the 2 most-recent sessions.
- [ ] `--since 2026-W23` filters by ISO week (or `2026-06-01` by
      date — accept both).
- [ ] Empty ledger prints "no sessions archived yet" and exits 0
      (not an error).

**Tests:**

- [tests/history_list.rs](../../crates/stacks-bench-agent/tests/history_list.rs)
  (new) — `CARGO_BIN_EXE_sbagent` invocations against a fixture
  operator dir with a hand-rolled `sessions.jsonl`.

### Phase 4: `sbagent history show <session-id>`

**Goal:** Operators inspecting one session see the per-phase
breakdown + per-target detail in a single command.

**Scope:**

- New `sbagent history show <session-id>` sub-subcommand.
- Output (stdout): **ASCII-only by default** — same scriptable
  contract as `history list`. No glyphs, no Unicode box-drawing,
  no ANSI escape codes when stdout is piped.
- Three sections, each preceded by a one-line header:

  1. **Header**: session id, `started_at -> finished_at`, status,
     any `failure_phase` + `failure_reason`.
  2. **Phase durations**: bar chart, one row per phase,
     proportional to `phase_durations_secs`. Bars rendered with
     plain ASCII (`"#".repeat(n)`), no Unicode block characters.
     Bar width sized to `min(terminal_width, 60)`. Sub-second
     phases render as `< 1s` with no bar.
  3. **Targets**: one row per target. Columns: `id`, `status`,
     `delivery_mode`, `improvement_pct` (when bench ran), bench
     wall-clock total (`mm:ss`), URL (`pr_url` or `issue_url`,
     or `-` ASCII hyphen if neither set).
- Looking up a session by id: linear scan via the ledger reader
  (ledgers stay small in operator practice). If the id isn't
  found, exit 1 with a clear "no such session id in sessions.jsonl"
  message and suggest `sbagent history list` to see what IS
  archived.
- Optional color (status column only — green on `succeeded`,
  red on `failed`, yellow on `aborted`) when stdout is a TTY
  and `NO_COLOR` is unset. Same TTY/NO_COLOR contract as Phase 3.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] `sbagent history show <known-id>` against the fixture
      session prints the three sections in order with correct
      values.
- [ ] Output is pure ASCII — no glyphs, no Unicode box-drawing,
      no ANSI escape codes when stdout is piped (fixture test
      asserts byte-for-byte equality against a checked-in
      expected output, same shape as Phase 3's test).
- [ ] An unknown session id exits 1 with the documented error
      message.
- [ ] `NO_COLOR=1 sbagent history show <id>` suppresses color
      escape codes even on a TTY.
- [ ] Phase durations bar chart proportional to the values: a
      session with `optimize=1200, baseline=295` shows the
      optimize bar roughly 4x the baseline bar (assert by hash-
      character count, not by visual eyeball).

**Tests:**

- Extend `tests/history_list.rs` (or new `tests/history_show.rs`)
  with the show flow + NO_COLOR assertion + unknown-id
  rejection.

**Notes:** Phase 4 is where the rendering library choice matters.
The standard Rust pick for terminal tables is `comfy-table` or
`tabled`. If using either, configure the ASCII-only preset (e.g.
`comfy-table::presets::ASCII_FULL`) so the output stays in the
scriptable contract — no Unicode box-drawing in the default
table style. Bars are simply `"#".repeat(n)`; no crate needed.

### Phase 5 (Stretch): `sbagent history report`

**Goal:** A markdown report aggregating recent sessions, suitable
for committing to `<operator>/reports/<iso-week>.md`.

**Scope:**

- New `sbagent history report [--since <ref>] [--out <path>]`
  sub-subcommand. Default `--since` is "the most recent ISO week
  with archived sessions"; default `--out` is stdout.
- Markdown sections:
  - **Summary**: session count, target outcome rollup, total
    wall-clock spent across the period.
  - **Per-session table**: same columns as `history list` but
    rendered as a markdown table.
  - **PRs opened**: bulleted list of `pr_url`s grouped by session.
  - **Issues opened**: bulleted list of `issue_url`s.
- No GitHub-side reconciliation (open/merged/closed status) —
  that's `0033-maintain-command`. The report renders WHAT the
  archived ledger knows, nothing more.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Default invocation produces a markdown document with the
      five sections above against a fixture ledger.
- [ ] `--out reports/2026-W24.md` writes to disk; stdout stays
      empty.

**Tests:**

- Extend the history test file with a `report` flow.

**Notes:** Phase 5 is the explicitly-stretchable phase. Skip it if
the v6 implementation budget gets tight — a markdown report is
just `history show` repeated and joined, so the deferred work is
small and can roll into v7 cleanly.

## Final Validation

In-process / unit:

- [x] `0021-preflight-v2` closed or shrunken with the change
      reflected in `backlog.md` + design archive location.
- [ ] Ledger reader handles v1 + v2 + v3 records on the same file.
- [ ] `history list` table renders against the fixture ledger.
- [ ] `history show` per-session detail renders the three
      sections.
- [ ] (Stretch) `history report` renders markdown.

Live / operator (not blocking v6 ship):

- [ ] Run `history list` against a real operator's
      `sessions.jsonl` (after the v1 Pass 1c smoke produces real
      archived sessions). Confirms the data v5 produces reads
      cleanly through the v6 reader against a non-fixture file.

Code-side ships once Phases 1-4 land. Phase 5 ships when it's
ready (or rolls to v7).

## Non-Goals

- Cross-session event projection (target dedup, fix-signature
  tracking, PR lifecycle): `0030-event-log-skeleton` /
  `0031-triage-merge-dedup-filter` / `0033-maintain-command`.
- Token spend / cost dashboard. No producer yet.
- TUI / web UI. Terminal stdout only.
- Modifying `sessions.jsonl` in any way. Read-only contract.

## Follow-Ups

- v7 candidate: `0033-maintain-command` — needs the v6 ledger
  reader as foundation. Reconciling PR state with GitHub gives
  the report the open/merged/closed dimension it currently
  lacks.
- `0030-event-log-skeleton` — adds the event-history projection
  that makes target dedup + fix-signature tracking trivial.
  Substantial; warrants its own iteration.
- `0038-prompt-example-concretization` — small prompt-text edit,
  pick up as a one-off PR whenever someone's in `analyzer.md`
  next.
