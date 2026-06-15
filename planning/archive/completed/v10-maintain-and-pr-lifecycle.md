# v10: Maintain Command + PR Lifecycle Reconciliation

Successor to [v9: Bench Phase Vocabulary Realignment](v9-bench-phase-vocabulary-realignment.md).
The smoke session pipeline (v1-v7) ships PRs to the bot's fork; v6 made
`sessions.jsonl` readable; v7 made verdicts evidence-backed; v8 hardened the
verdict prompts; v9 cleaned the vocabulary. The remaining gap before
autonomous loops can run is **knowing what happened to those PRs after the
bot opened them.** v10 closes that gap.

> **Status:** shipped — code-complete, reviewed, and archived. Live operator
> validation against the bot-fork PRs remains as a follow-up smoke check.
>
> v10 ships the maintenance substrate: a typed `maintain.jsonl` ledger and a
> `sbagent maintain` command that reconciles GitHub PR state against the bot's
> recorded archive. No scheduled execution (that's v11 / `0034`); no auto-merge
> or auto-close behaviors; no dedup logic in triage/merge (that's a later
> consumer of maintain.jsonl, tracked under `0031`).

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0027-maintain-ledger` | substrate | shipped |
| `0033-maintain-command` | primary | shipped; live operator validation deferred |

## Why

After the smoke session `20260611-172955` opened PRs `#1` / `#2` / `#3` on
`stacks-bench-bot/stacks-core`, sbagent has no way to ask "what state are those
PRs in now?". A future session that re-triages the same fix signatures has no
signal about whether prior attempts merged, were closed, or are still waiting
for review. That blocks three downstream items:

- **`0028-optimizer-memory`** — the optimizer needs to know which prior fixes
  shipped vs were rejected.
- **`0031-triage-merge-dedup-filter`** — triage/merge need to skip targets
  whose fix signature is already on an open or recently-closed PR.
- **`0043-history-report`** — the weekly markdown report needs the
  open/merged/closed dimension to be meaningfully richer than what
  `history list` already shows.

All three of those wait on maintain. v10 unblocks them without committing to
any particular consumer's shape.

## Scope

In scope:

- New typed event model with eight observation kinds: `PrOpen`,
  `PrMerged`, `PrClosedUnmerged`, `PrStale`, `PrForcePushed`,
  `PrBranchDeleted`, `IssueOpen`, `IssueClosed`. `PrOpen` / `IssueOpen`
  are emitted by `sbagent maintain` on first observation (publish does
  NOT emit them).
- New `maintain.jsonl` ledger as a sibling to `sessions.jsonl` on the operator
  repo's `main` branch. Append-only; one line per observed event.
- New `sbagent maintain [--since <ref>] [--dry-run] [--limit <N>]` command
  that reads `sessions.jsonl`, queries GitHub for each non-terminal artifact,
  computes the diff against `maintain.jsonl`'s projected last-known state,
  and appends events when the state changed.
- Reconciler split: `GhClient` extension exposes raw GitHub PR/issue reads
  with rate-limit metadata; `MaintainReconciler` owns the projection,
  terminal-state skipping, rate-limit budgeting, and event-kind derivation.
- **Idempotency via last-state diff against `maintain.jsonl`** — every
  invocation queries every non-terminal artifact and re-checks against the
  projection. No cross-invocation backoff cache. Cross-invocation rate
  management defers to v11's cron cadence. Within one invocation, the
  reconciler tracks rate-limit budget from each GitHub response's
  `X-RateLimit-Remaining`, deferring any further artifacts once the budget
  falls below `[maintain].secondary_rate_limit_floor_pct`.
- Extension of `sbagent history show <session-id>` to render that session's
  maintenance events as a new bottom section, ordered chronologically.
- Dedup-ready event shape: events carry `family_id` + `fix_signature` so
  `0031`'s future query "has this fix-signature shipped or recently failed?"
  is mechanical, not a schema migration.

Out of scope:

- Scheduled / cron execution. That's `0034-github-actions-wiring`.
- Auto-merge, auto-close, or any PR-mutation behavior. v10 is read-only
  against GitHub state.
- Dedup logic in triage or merge. v10 provides the substrate; the consumer
  is `0031-triage-merge-dedup-filter`.
- Mutating `session/<id>` archive branches. Maintenance state lives on
  `main` next to `sessions.jsonl`.
- Migration of `maintain.jsonl` into a unified event log. That's
  `0030-event-log-skeleton`, which becomes appropriate when a third consumer
  appears (likely `0028-optimizer-memory`).

## Phases

### Phase 1: Maintain Ledger Substrate

**Goal:** Closes `0027-maintain-ledger`. Operators have a typed, append-only
home for maintenance observations that does not touch any session's archive
branch.

**Scope:**

- New `models/maintain_event.rs` carrying:

  ```rust
  pub struct MaintEvent {
      pub schema_version: SchemaVersionV1,
      pub kind: MaintEventKind,
      pub observed_at: String,        // ISO 8601 UTC
      pub session_id: String,
      pub target_id: Option<String>,  // None for session-level events
      pub family_id: Option<String>,
      pub fix_signature: Option<String>,
      pub pr_url: Option<String>,
      pub issue_url: Option<String>,
      pub prior_state: Option<String>,  // None for the initial `PrOpen` / `IssueOpen` observation
      pub new_state: String,
      pub head_sha: Option<String>,     // for force-push detection
  }

  pub enum MaintEventKind {
      /// First observation that the PR exists and is in some open state.
      /// Emitted by `sbagent maintain` on the first reconciliation pass
      /// after publish lands the PR — publish itself does NOT emit this.
      PrOpen,
      /// PR transitioned from open to merged.
      PrMerged,
      /// PR transitioned from open to closed without merge.
      PrClosedUnmerged,
      /// PR still open but has not been touched in
      /// `[maintain].stale_after_days` (default 14).
      PrStale,
      /// PR head_sha changed while still open.
      PrForcePushed,
      /// PR head branch deleted upstream while PR still recorded as open.
      PrBranchDeleted,
      /// First observation that an issue exists. Same emission contract
      /// as `PrOpen` — emitted by maintain, not by publish.
      IssueOpen,
      /// Issue transitioned from open to closed.
      IssueClosed,
  }
  ```

- New `session/maintain_ledger.rs` mirroring the v6 ledger-reader pattern:
  `read_all(&Path) -> Result<MaintLedgerReport>` with lossy parse semantics
  (malformed lines land in `skipped`), `append_event(&Path, &MaintEvent)`
  helper.
- Schema versioning from `v1` on day one (v3 of `sessions.jsonl` proved this
  is the right discipline). `MaintEvent::from_ledger_line` reader uses the
  same shape as `SessionRecord::from_ledger_line`.
- Generate `maintain-event.schema.json` and bundle it into the operator
  schemas dir.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `MaintEvent` round-trips through JSON without losing fields.
- [x] `append_event` is idempotent against `O_CREAT|O_APPEND` semantics on
      a missing file (creates), present file (appends).
- [x] `read_all` returns all valid event lines from a fixture with
      0 skipped entries.
- [x] `read_all` against a valid + malformed fixture returns the valid
      records and reports the malformed nonblank lines with correct
      1-indexed line numbers.
- [x] Bundled schema mirror generated from the Rust model.

**Tests:**

- In-module tests in `models::maintain_event` and
  `session::maintain_ledger` cover schema validation, append/read
  round-trip, missing-file handling, and lossy reads.

### Phase 2: GitHub PR / Issue State Reader

**Goal:** A thin, testable GhClient extension that exposes raw GitHub PR
and issue reads, surfaces rate-limit metadata so callers can budget,
and contains NO sbagent-level caching, diffing, or backoff. Those
behaviors belong in Phase 3's reconciler.

**Scope:**

- Extend `GhClient` trait with:

  ```rust
  async fn query_pr_state(
      &self, owner: &str, repo: &str, number: u64,
  ) -> Result<GhStateRead<PrState>>;
  async fn query_issue_state(
      &self, owner: &str, repo: &str, number: u64,
  ) -> Result<GhStateRead<IssueState>>;
  ```

  The wrapper return shape carries both the parsed state and rate-limit
  metadata observed from the response headers:

  ```rust
  pub struct GhStateRead<T> {
      pub state: T,
      pub rate_limit: RateLimitSnapshot,
  }

  pub struct RateLimitSnapshot {
      pub remaining: u32,
      pub limit: u32,
      pub resets_at: SystemTime,
  }
  ```

  Every call always hits GitHub. No client-side cache, no skip logic.
- `PrState` carries: `is_open`, `is_merged`, `is_closed_unmerged`, `is_draft`,
  `head_sha`, `head_ref_deleted`, `base_ref`, `updated_at`. `IssueState`
  carries `is_open`, `is_closed`, `updated_at`. Both are derived purely
  from one API response — no cross-call inference.
- New `[maintain]` settings stanza for thresholds the reconciler (Phase 3)
  will read, NOT this layer:

  ```toml
  [maintain]
  stale_after_days = 14                      # default
  secondary_rate_limit_floor_pct = 10        # default
  ```

  No `min_poll_interval_sec` — see Phase 3's notes on why cross-invocation
  backoff is not needed.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `query_pr_state` against a `FakeGh` fixture returns the seeded
      `PrState` and a seeded `RateLimitSnapshot`. Every call hits the
      fake — no client-side memoization.
- [x] `head_ref_deleted = true` when the seeded response has no `head.ref`
      or an explicit deletion marker; `false` otherwise. The reconciler
      (Phase 3) derives `PrBranchDeleted` events from this flag, not this
      layer.
- [x] `updated_at` is preserved in `PrState` and `IssueState` for the
      reconciler's stale detection.
- [x] Settings parsing covers `[maintain].stale_after_days` and
      `[maintain].secondary_rate_limit_floor_pct` with the documented
      defaults when unset.

**Tests:**

- `session::maintain` in-module tests use `FakeGh` to exercise seeded
  `PrState` / `IssueLifecycleState` reads through the reconciler. This
  keeps the thin reader contract covered at the boundary where the
  returned fields matter.
- `settings::tests::maintain_settings_defaults_and_overrides_parse`
  covers default values + override parsing for the `[maintain]` stanza.

**Notes:**

The split between this layer and Phase 3's reconciler matters for
testability. `GhClient` stays a thin GitHub wrapper that's easy to mock
via the existing `FakeGh` pattern from `tests/publish_push.rs`. The
reconciler is where the interesting logic — diffing against
`maintain.jsonl`, computing event kinds, budgeting rate limits — gets
written and tested against the fake. Don't bleed reconciler concerns
into the trait.

### Phase 3: `sbagent maintain` Command + Reconciler

**Goal:** End-to-end reconciliation: read sessions.jsonl, query GitHub for
non-terminal artifacts, derive event-kind transitions against
maintain.jsonl's projected last-known state, append new events,
optionally commit + push.

**Scope:**

- New `session/maintain.rs` containing `MaintainReconciler`. This is where
  the interesting logic lives:

  ```rust
  pub struct MaintainReconciler<'a, G: GhClient> {
      pub gh: &'a G,
      pub settings: &'a MaintainSettings,
      pub now: SystemTime,    // injected for deterministic tests
  }

  impl<G: GhClient> MaintainReconciler<'_, G> {
      pub async fn reconcile(
          &self,
          sessions: &[SessionRecord],
          maintain: &MaintLedgerReport,
          limit: usize,
      ) -> Result<ReconcileOutcome>;
  }

  pub struct ReconcileOutcome {
      pub new_events: Vec<MaintEvent>,
      pub deferred: Vec<DeferredArtifact>,   // hit rate-limit floor; skipped
      pub queried: usize,
  }
  ```

  The reconciler owns: last-known-state projection from maintain.jsonl,
  terminal-state skipping, rate-limit budgeting, event-kind derivation
  (which Phase 2 explicitly does NOT do).

- New `cli/maintain.rs` (top-level `Command::Maintain` variant). Args:
  - `--since <iso-date-or-week>` — only reconcile sessions started on or
    after this date. Default: no filter (full sessions.jsonl scan).
  - `--dry-run` — run the reconciler but don't append, commit, or push.
    Prints the `ReconcileOutcome` events to stdout instead.
  - `--limit <N>` — cap the number of artifacts polled per invocation.
    Default 50.

- CLI flow:
  1. Read `sessions.jsonl` via the v6 ledger reader. Filter by `--since`.
  2. Read `maintain.jsonl` via Phase 1's reader.
  3. Construct the reconciler and call `reconcile()`. The reconciler:
     - Projects `maintain.jsonl` into a per-artifact "last-known state"
       map.
     - Walks every target with `pr_url`/`issue_url`. Skips artifacts whose
       last-known state is terminal (`PrMerged`, `PrClosedUnmerged`,
       `IssueClosed`).
     - For non-terminal artifacts, calls `gh.query_pr_state()` /
       `gh.query_issue_state()`. Tracks remaining rate-limit budget
       across calls; defers any further artifacts to the `deferred` list
       once `rate_limit.remaining` drops below
       `secondary_rate_limit_floor_pct`.
     - Derives the appropriate `MaintEventKind` from the diff (see
       transition table in Notes below). Emits new events into
       `ReconcileOutcome.new_events`.
  4. CLI writes events: `append_event(maintain_path, ...)` for each new
     event in outcome.
  5. Unless `--dry-run`: `git add maintain.jsonl && git commit -m
     "maintain: N events ..." && git push` using the v3-era
     `push_with_pat` helper.
- Output: ASCII-only terminal summary table — `kind | session | target |
  url | committed?`. Same contract as v6's `history list`. Empty case:
  "no maintenance events; lifecycle state unchanged" or equivalent.

**Idempotency model (load-bearing — read carefully):**

Cross-invocation idempotency comes from **last-state diff against
maintain.jsonl**, not from a persisted backoff cache. The reconciler
projects maintain.jsonl into a "last-known state per artifact" map and
only emits a new event when GitHub's current state differs from the
last-known state. So running `sbagent maintain` twice in a row against
the same unchanged GitHub state produces N events the first time and 0
events the second — the second invocation queries GitHub but the diff
projection sees no transition for any artifact.

There is NO cross-invocation backoff cache. Rate management at the cron
layer (v11) governs how often `sbagent maintain` invokes; within one
invocation, every non-terminal artifact gets exactly one GitHub query
(or zero, if the rate-limit floor trips and the artifact is deferred).

This is a deliberate trade-off: every invocation does the work of
querying every non-terminal artifact, paying the API cost for the
guarantee that the diff-against-projection is correct. The alternative —
a persisted backoff cache — would require an extra sidecar file that's
not part of the event log and would have to be kept in sync with
maintain.jsonl's state machine. The single-source-of-truth shape is
cleaner.

**Projection shape (load-bearing).** The reconciler folds `maintain.jsonl`
into a per-artifact projection that records every state dimension a
transition might fire against. Without this, derived events (`PrStale`,
`PrBranchDeleted`, `PrForcePushed`) re-fire every run because the
"already in derived state" signal isn't tracked.

```rust
pub struct ArtifactProjection {
    pub artifact_kind: ArtifactKind,        // Pr | Issue
    pub terminal: bool,                      // true iff last event was
                                             // PrMerged / PrClosedUnmerged /
                                             // IssueClosed
    pub head_sha: Option<String>,            // last observed head_sha (PR only)
    pub head_ref_deleted_emitted: bool,      // PrBranchDeleted already fired
                                             // for the current head_sha
    pub stale_emitted: bool,                 // PrStale already fired for the
                                             // current head_sha
    pub last_updated_at: Option<String>,     // PR/issue last_updated_at from
                                             // last successful GitHub query
}
```

`head_ref_deleted_emitted` and `stale_emitted` reset to `false` whenever
a `PrForcePushed` event lands (new head_sha → fresh derived-state slate).
Both stay `true` until that reset, which is how the duplicate-suppression
guarantee holds: an artifact that's been stale for a week emits one
`PrStale` event on the first reconciliation that crosses the threshold,
then zero on every subsequent run until something else changes.

**Event-kind transition table** (reconciler-internal):

| Last-known state | GitHub current state | Emitted event |
| ---------------- | -------------------- | ------------- |
| (none) | PR open | `PrOpen` |
| (none) | issue open | `IssueOpen` |
| open | merged | `PrMerged` |
| open | closed-unmerged | `PrClosedUnmerged` |
| open + same head_sha | open + different head_sha | `PrForcePushed` (resets `stale_emitted` + `head_ref_deleted_emitted`) |
| open + `head_ref_deleted_emitted: false` | open + head_ref_deleted | `PrBranchDeleted` |
| open + `stale_emitted: false` | open + `now - updated_at > stale_after_days` | `PrStale` |
| open + `head_ref_deleted_emitted: true` | open + head_ref_deleted | (none — already emitted) |
| open + `stale_emitted: true` | open + `now - updated_at > stale_after_days` | (none — already emitted) |
| open | open + no transition | (none) |
| terminal | (artifact skipped — no query) | (none) |

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Dry-run/reconciler path against targets with all PRs open and no prior
      `maintain.jsonl` events: reconciler emits 3 `PrOpen` events to
      the outcome; CLI write/push coverage against live GitHub is deferred.
- [ ] Real run on the same input writes 3 `PrOpen` events to
      `maintain.jsonl`, commits, and pushes via the bot PAT.
- [x] Re-running `sbagent maintain` immediately against the same
      unchanged GitHub state writes 0 new events. Idempotency comes from
      the last-state diff: every non-terminal artifact is queried again,
      but each one's current state matches the most recent
      `maintain.jsonl` entry for that artifact, so the reconciler emits
      no event.
- [x] After one PR merges externally and a subsequent `sbagent maintain`
      runs: the reconciler emits exactly one `PrMerged` event for the
      merged PR. The two remaining open PRs are queried but produce no
      events (their current state matches their last-known state).
- [x] **Duplicate suppression — stale:** an artifact already recorded
      as `PrStale` in `maintain.jsonl` (projection sees
      `stale_emitted: true`) emits NO second `PrStale` event on a
      subsequent reconciliation even when the GitHub state is still
      open + past the stale threshold. The projection's
      `stale_emitted` flag suppresses the duplicate.
- [x] **Duplicate suppression — branch deleted:** an artifact already
      recorded as `PrBranchDeleted` emits no duplicate. Same
      `head_ref_deleted_emitted` flag semantics.
- [x] **Duplicate suppression — force-pushed then stable:** an
      artifact that fired `PrForcePushed` once at a new head_sha and is
      then stable at that sha emits no duplicate `PrForcePushed`. The
      projection's recorded `head_sha` advances to the new value after
      the first emission.
- [x] **Force-push resets derived-state flags:** an artifact that was
      previously `PrStale` AND then receives a force-push emits a
      `PrForcePushed` event AND becomes eligible for a fresh `PrStale`
      after a new stale-threshold elapses. The `stale_emitted` flag
      reset is what makes this work.
- [x] `--limit 2` truncates the artifact-poll loop to the first 2
      artifacts in scan order; the third is reported in
      `ReconcileOutcome.deferred` and surfaces in stdout as "deferred to
      next invocation".
- [x] Rate-limit floor trip: when the `FakeGh` fixture seeds a
      `RateLimitSnapshot.remaining` below the configured floor, the
      reconciler stops querying mid-loop and reports the remaining
      artifacts in `deferred`. No partial-state events leak.
- [x] Output is pure ASCII for the covered CLI no-op path; no glyphs, no
      ANSI when piped.

**Tests (two-layer split — the CLI cannot inject `FakeGh` directly):**

- **Library layer — `session::maintain` in-module tests** drive
  `MaintainReconciler::reconcile` in-process against hand-rolled
  `SessionRecord`s, hand-rolled `MaintLedgerReport`s, and an in-process
  `FakeGh`. Covers:
  - every row of the transition table (positive emit cases);
  - every duplicate-suppression case named above;
  - `--limit` truncation + rate-limit deferral via `ReconcileOutcome`;
  - rate-limit floor trip with no partial-state event leak.

  This is where the v10 logic gets its real exercise.

- **CLI layer — `tests/maintain_command.rs`** drives
  `CARGO_BIN_EXE_sbagent` against a fixture operator dir. Because the
  binary spawns a fresh process, it cannot use the in-process `FakeGh`.
  Coverage here is scoped to surfaces that don't need live GitHub
  queries:
  - **all-terminal short-circuit:** when the fixture
    `maintain.jsonl` already records every artifact in terminal state
    (`PrMerged`/`PrClosedUnmerged`/`IssueClosed`), the CLI emits 0
    events without querying GitHub. This exercises the CLI flow
    end-to-end (ledger read → projection → terminal-skip → no-op output
    path with no append, no commit, no push) on a `FakeGh`-free fast
    path;
  - `--dry-run` output formatting for the no-op path.

  Argument parsing, non-empty dry-run output, and git commit/push path
  coverage are deferred to the live/operator validation pass.

  CLI-level coverage of the live-GitHub path is deferred to operator
  validation (Final Validation's live bullet against the smoke session's
  bot-fork PRs).

### Phase 4: History Surface for Maintenance Events

**Goal:** `sbagent history show <session-id>` renders the session's
maintenance events in a new bottom section, so operators see the full
arc — archive + PR open + maintenance follow-ups — in one view.

**Scope:**

- Extend the `history show` renderer to read `maintain.jsonl` after rendering
  the Targets section. Filter to events whose `session_id` matches.
- New "Maintenance events" section. Empty case: omit the section entirely
  (don't render an empty header).
- ASCII contract preserved (matches v6 Phase 4 byte-equality discipline).
- One row per event, chronological order:

  ```text
  Maintenance events
    2026-06-12T08:00:00Z  pr_merged             https://github.com/.../pull/1
    2026-06-12T14:15:00Z  pr_stale              https://github.com/.../pull/2
    2026-06-15T09:30:00Z  pr_closed_unmerged    https://github.com/.../pull/3
  ```

- `history list` is NOT extended (the aggregate counters there don't need
  per-event detail).

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Session with maintenance events renders the new section with rows
      in chronological order.
- [x] Session with 0 maintenance events renders no section.
- [x] Byte-equality fixture test pinning the section layout (mirrors v6
      Phase 4's `tests/history_show.rs` discipline).
- [x] ASCII-only output when stdout is piped.

**Tests:**

- Extend `tests/history_show.rs` with a maintenance-events fixture case.

### Phase 5 (stretch): Reconciliation Hardening

**Goal:** Tighter PR state machine for edge cases that didn't show up in the
smoke but will eventually.

**Scope:** Skip if budget runs short. Candidates:

- PR convert draft ↔ ready transitions (emit `PrReadyForReview` /
  `PrConvertedToDraft` events).
- Reviewer interactions: detect `request-changes` reviews and emit a
  `PrChangesRequested` event.
- Bot self-protection: refuse to re-open a closed PR even if a stale
  artifact's head_sha drifts (defensive — shouldn't happen, but
  cheap to enforce).
- `maintain.jsonl` schema bump (v1 → v2) if hardening adds required fields.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Phase 4's history-show rendering handles the new event kinds without
      layout regression.

**Tests:**

- Extend `tests/maintain_command.rs` with the new event kinds.

**Notes:** Phase 5 is the explicitly-stretchable phase. Skip it if v10's
implementation budget gets tight — the additional event kinds can land
piecemeal in later iterations or whenever the next live smoke surfaces a
real need.

## Final Validation

In-process / unit:

- [x] `maintain.jsonl` round-trips through the typed reader.
- [x] GitHub PR-state reader handles all four edge cases (force-push,
      branch-deleted, stale, rate-limit floor) against `FakeGh`.
- [x] `sbagent maintain --dry-run` against an all-terminal fixture session
      exits 0 without reading a publish token, querying GitHub, appending,
      committing, or pushing.
- [ ] `sbagent maintain` against a fixture session writes events, commits,
      and (in `--dry-run`) shows the would-be diff.
- [x] `sbagent history show` renders maintenance events for sessions that
      have any.
- [x] `just lint --no-sccache` clean.
- [x] `just test --summary --no-sccache` clean (`530/530`).

Live / operator (not blocking v10 ship):

- [ ] Run `sbagent maintain --dry-run` against the smoke session's 3 bot-fork
      PRs. Verify the output reflects whatever state PRs `#1`/`#2`/`#3`
      actually carry on `stacks-bench-bot/stacks-core`.
- [ ] If any PR has merged or been closed since the smoke, run
      `sbagent maintain` (without `--dry-run`) and confirm a non-empty
      `maintain.jsonl` lands on the operator repo's `main` with a clean
      bot-PAT push.

## Non-Goals

- **Scheduled / cron execution.** `0034-github-actions-wiring` is the next
  iteration; v10 ships the command, v11 wires the cron.
- **PR mutations.** v10 is read-only against GitHub. No auto-merge,
  auto-close, auto-comment, or auto-label.
- **Dedup behavior.** v10 provides the substrate; `0031` is the consumer.
- **Unified event log.** Maintain.jsonl is a sibling to sessions.jsonl, not
  a stepping stone toward `0030`. The migration to a unified log becomes
  worth doing when a third consumer (likely `0028`) appears — flagged as
  follow-up, not committed to in v10.
- **Schema-level changes to `sessions.jsonl`.** All v10 work lives in the
  new ledger; `sessions.jsonl` stays at v3.

## Follow-Ups

- v11 candidate: `0034-github-actions-wiring` + `0035-autonomy-hygiene`.
  Schedules `sbagent maintain` (and eventually `sbagent session run`) under
  cron with concurrency guards, rate-limit budgeting, and circuit breakers.
- `0028-optimizer-memory` — shipped in v13, reading maintain.jsonl to surface
  "this fix signature shipped" / "this fix signature was rejected" to the
  optimizer.
- `0031-triage-merge-dedup-filter` — now reads maintain.jsonl to skip
  targets whose fix_signature is already on an open or recently-terminal
  artifact.
- `0043-history-report` — unblocked; the markdown report can now carry
  merged-vs-open dimensions, making it meaningfully richer than
  `history list`.
- **`0030-event-log-skeleton` rollup question** — once `0028` + `0031` both
  consume maintain.jsonl, evaluate whether unifying sessions.jsonl +
  maintain.jsonl into a single event log is worth a migration. Until a
  third consumer appears, the sibling-ledger shape is the right tactical
  call.
