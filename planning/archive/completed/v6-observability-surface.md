# v6: Observability Surface

Successor to [v5: Archive Metadata](v5-archive-metadata.md). v5
made `sessions.jsonl` carry the data operators actually want — per-phase
durations, PR/issue URLs, bench wall-clock totals. v6 makes that
data easy to consume: `sbagent history list` for the leaderboard,
`sbagent history show <id>` for per-session detail. Plus a small
prologue to close out the now-superseded `0021-preflight-v2`
planning debt before the next iteration uses preflight as a hook.

> **Status:** shipped (Phases 1–4). Phase 5 (markdown report)
> deferred to
> [`0043-history-report`](../../backlog.md#0043-history-report) —
> best promoted after `0033-maintain-command` lands so the report
> gains the open/merged/closed PR dimension it currently lacks.
>
> Scoped tight on purpose — v5 already shipped the data; v6 just
> reads it back. No new artifacts, no new sidecars.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0021-preflight-v2` | prologue (rescope/close) | shipped (superseded by v3) |
| `0036-observability-surface` | primary | shipped (Phases 1–4) |

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
  [archive/superseded/0021-preflight-v2.md](../superseded/0021-preflight-v2.md)).
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
- Update [planning/backlog.md](../../backlog.md) to remove `0021`'s
  entry (or shrink it). Move the design doc to
  [planning/archive/superseded/0021-preflight-v2.md](../superseded/0021-preflight-v2.md)
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
[archive/superseded/0021-preflight-v2.md](../superseded/0021-preflight-v2.md)
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
- [x] Reviewed
- [x] Validated

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

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `sbagent history list` against a 3-session fixture prints
      a 3-row table with the canonical column set; total wall-clock
      matches the sum of `phase_durations_secs` per session.
- [x] Output is pure ASCII — no glyphs, no emoji, no ANSI escape
      codes when stdout is piped (fixture tests assert byte-for-byte
      equality against a checked-in expected output).
- [x] `--limit 2` truncates to the 2 most-recent sessions.
- [x] `--since 2026-W23` filters by ISO week (Monday-of-week
      cutoff via a hand-rolled days-from-civil round-trip; no
      new deps). `--since 2026-06-01` filters by calendar date.
      Both forms covered.
- [x] Empty ledger prints "no sessions archived yet" and exits 0
      (not an error). Missing-file path also takes this branch,
      mirroring the typed reader's `Ok(empty report)` contract.

**Tests:**

- [tests/history_list.rs](../../crates/stacks-bench-agent/tests/history_list.rs)
  (new) — 6 `CARGO_BIN_EXE_sbagent` invocations against a fixture
  operator dir with a hand-rolled `sessions.jsonl`.
- 8 unit tests under
  [src/cli/history.rs](../../crates/stacks-bench-agent/src/cli/history.rs)
  covering the `mm:ss` / `hh:mm:ss` wall-clock formatter, the
  `YYYY-MM-DD` validator, the `YYYY-Www` parser, ISO week
  conversion (incl. cross-year week-1 cases), and the
  days-from-civil round-trip.

**Notes / design decisions worth review:**

- **`TargetStatus::Failed` folds into the `Ab` (aborted) bucket.**
  Phase 3's spec defines exactly three target counts (`A/R/Ab`);
  `Failed` represents the same "non-accept, non-reject" outcome
  shape, so collapsing it keeps the column honest without adding
  a fourth count the spec doesn't budget for. Documented inline.
- **Color is status-column only**, lit only when stdout is a TTY
  AND `NO_COLOR` is unset. Test forces `NO_COLOR=1` belt-and-
  braces; `Command::output()` already pipes stdout so the no-color
  path triggers regardless.
- **No new deps.** Date arithmetic uses an inline Howard Hinnant
  days-from-civil implementation (~25 lines). Avoids pulling in
  `chrono`/`jiff` for two date formats.

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

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `sbagent history show <known-id>` against the fixture
      session prints the three sections in order with correct
      values.
- [x] Output is pure ASCII — no glyphs, no Unicode box-drawing,
      no ANSI escape codes when stdout is piped. Byte-for-byte
      equality against a `#[rustfmt::skip]`'d raw-string fixture
      (rustfmt happily injects line-continuations into ordinary
      string literals — caught on first lint pass, fixed by raw
      string).
- [x] An unknown session id exits 1 with the documented error
      message (includes the offending id and points at
      `sbagent history list`).
- [x] `NO_COLOR=1 sbagent history show <id>` suppresses color
      escape codes. Caveat: `Command::output()` already pipes
      stdout, so the TTY-layer gate also engages — the env-var
      assertion is the orthogonal contract bullet.
- [x] Phase durations bar chart proportional to the values: a
      session with `optimize=1200, baseline=295` shows the
      optimize bar roughly 4x the baseline bar — asserted by
      hash-character count (60 vs 15 = exact 4x at
      `BAR_WIDTH=60`).

**Tests:**

- [tests/history_show.rs](../../crates/stacks-bench-agent/tests/history_show.rs)
  (new) — 4 integration tests: byte-equality default, unknown-id
  rejection, proportional bars, NO_COLOR no-ANSI.

**Notes / design decisions worth review:**

- **Inline rendering, no table crate.** Spec mentions
  `comfy-table::presets::ASCII_FULL` as a fallback if a table
  crate is used; the implementation uses plain padded `write!`
  calls. Adds no dep and keeps the byte-equality fixture
  trivially predictable.
- **`BAR_WIDTH = 60` unconditionally.** Spec calls for
  `min(terminal_width, 60)`; for piped output (the contract that
  matters here) 60 is the correct ceiling, and adding a
  terminal-size dep just to handle narrow-TTY-wrap cosmetics
  isn't worth it for v6.
- **Sub-second phases render as `< 1s` with no bar**, exactly per
  spec. Phases ≥1s with a tiny ratio get at least one `#` so the
  row stays visible.
- **`delivery_mode` rendered via explicit `match`**, not via
  `format!("{:?}", ...).to_lowercase()` — keeps the rendered
  values pinned to the schema's snake_case representation.

**Bug worth flagging while it's fresh:** the first rustfmt pass
mangled the byte-equality expected-output literal by inserting
`\` line-continuations + trailing spaces mid-string. Switching to
a raw string + `#[rustfmt::skip]` kept the bytes verbatim.

### Phase 5 (Stretch): `sbagent history report` — DEFERRED

Extracted to
[`0043-history-report`](../../backlog.md#0043-history-report) on
2026-06-11. Rationale: without `0033-maintain-command`'s
GitHub-side reconciliation (open / merged / closed / stale PR
state), a markdown report can only render "PR opened" — which
`history list` already covers in the terminal. Promoting the
report after `0033` lands lets it carry the merged-vs-open
dimension that makes a weekly digest meaningfully richer than
the v6 views.

Phases 1–4 are the v6 delivery.

## Final Validation

In-process / unit:

- [x] `0021-preflight-v2` closed or shrunken with the change
      reflected in `backlog.md` + design archive location.
- [x] Ledger reader handles v1 + v2 + v3 records on the same file.
- [x] `history list` table renders against the fixture ledger.
- [x] `history show` per-session detail renders the three
      sections.
- ~~(Stretch) `history report` renders markdown.~~ — deferred to
  [`0043-history-report`](../../backlog.md#0043-history-report).

Live / operator:

- [x] Run `history list` against a real operator's
      `sessions.jsonl` (after the v1 Pass 1c smoke produces real
      archived sessions). Confirms the data v5 produces reads
      cleanly through the v6 reader against a non-fixture file.
      Validated with:
      `sbagent -c ~/.config/sbagent/config.toml history list --limit 5`,
      which rendered session `20260611-172955` with `3/0/0` targets and
      3 PRs.

## Smoke-Surfaced Polish

- `history show` now renders an accepted target with
  `reason_code` starting with `mixed:` as `status = mixed` in the detail
  table. The ledger still treats mixed verdicts as accepted/shipped, and
  `history list` still counts them in the accepted bucket; the detail view
  now exposes the v7 verdict nuance operators need when inspecting a session.

Code-side shipped with Phases 1–4. Phase 5 (markdown report)
extracted to `0043-history-report`, to be promoted after
`0033-maintain-command` lands.

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
  the deferred report the open/merged/closed dimension it
  currently lacks.
- [`0043-history-report`](../../backlog.md#0043-history-report) —
  the extracted Phase 5 (markdown report). Best promoted as a
  v7 stretch (or v8 primary) immediately after `0033` lands so
  the report can carry PR state, not just "PR opened".
- `0030-event-log-skeleton` — adds the event-history projection
  that makes target dedup + fix-signature tracking trivial.
  Substantial; warrants its own iteration.
- `0038-prompt-example-concretization` — completed in v7.
