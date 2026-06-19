# v16: Projection Migration Completion

Successor to [v15: Cross-Session Projection Facade](v15-cross-session-projection-facade.md).
v15 created `HistoryProjectionV1` and migrated the two most urgent consumers:
v12 dedup and v13 optimizer memory. v16 finishes the migration boundary so
new read-side autonomy features can consume one shared projection instead of
adding fresh ledger passes.

> **Status:** shipped.
>
> v16 is a consolidation iteration. It should not change dedup policy,
> optimizer-memory policy, GitHub reconciliation behavior, or ledger schemas.
> The goal is to make the projection facade the default read-side API for
> autonomy and history surfaces.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0051-projection-migration-completion` | primary | shipped |

## Why

v15 deliberately left three useful migration points open:

- v11 autonomy gates still read and project the ledgers for open PR counts,
  session cadence, and circuit-breaker decisions.
- `sbagent history show` still assembles its maintenance-event view from raw
  ledger reads.
- Chained `session run` phases can now consume projection-backed APIs, but
  Phase 1.2 optimizer memory and Phase 1.7 merge/dedup still build their own
  projections independently.

Those are acceptable in v15's partial migration, but they are exactly the kind
of duplication that makes later autonomy features more expensive. v16 closes
the gap before `0043-history-report` adds another projection consumer.

## Source-Of-Truth Invariants

- `sessions.jsonl` and `maintain.jsonl` remain the durable source ledgers.
- `HistoryProjectionV1` remains read-only and rebuildable.
- No `events/` ledger, SQLite cache, or schema migration is introduced in v16.
- `sbagent maintain` remains the write-side reconciler for `maintain.jsonl`.
- Dedup and optimizer-memory behavior remain byte-equivalent to v15.
- `history list` remains the compact aggregate view unless implementation finds
  a no-risk cleanup; v16's history surface is `history show`. `history list`
  reads only `SessionRecord` fields from `sessions.jsonl` and does not join
  against `maintain.jsonl`, so it does not need the shared projection.

## Scope

In scope:

- Migrate v11 autonomy gates to consume `HistoryProjectionV1` for:
  - open agent PR counts;
  - most-recent session timing / cadence checks;
  - zero-accepted-session circuit-breaker state.
- Migrate `sbagent history show <id>` to consume `HistoryProjectionV1` for its
  maintenance-event section and lifecycle-derived target state.
- Add an orchestration-level projection cache for chained `session run` so
  Phase 1.2 optimizer memory and Phase 1.7 merge/dedup share one ledger read
  and one projection build.
- Keep standalone subcommands simple: they may still build a projection once
  inside their own process.
- Add projection methods only where a migrated consumer needs them.
  - Any new method must be consumer-neutral, return typed projection data, avoid
    consumer-side policy names, and have unit coverage in
    `history_projection.rs`.
  - New projection methods must be named in the v16 archive's shipped notes so
    API growth stays visible across iterations.
- Leave a report-ready read path for v17 without implementing
  `sbagent history report`.
- Update docs and comments that still describe raw-ledger projection as the
  intended read-side pattern.

Out of scope:

- `0043-history-report` implementation.
- A canonical event log under `events/`.
- SQLite / durable projection cache.
- Changes to `sessions.jsonl`, `maintain.jsonl`, `optimizer-memory.json`, or
  schema files.
- Prompt changes.
- New dedup policies, fuzzy matching, or optimizer-memory ranking changes.
- GitHub PR mutations or maintain reconciler behavior changes.

## Projection Sharing Contract

By the end of v16, code that needs cross-session read-side history should
prefer this shape:

```rust
let projection = read_operator_projection_v1(layout)?;
consumer_a(&projection)?;
consumer_b(&projection)?;
```

For chained `session run`, that means the orchestrator should own a cached
projection and pass references into phases that need cross-session context. The
projection is a snapshot for the current process. It is not refreshed mid-run,
because the active session has not been archived yet and maintain events are
external observations.

Implementation requirements:

- The cache must be scoped to one `session run` invocation.
- The orchestrator builds the projection once during session-start preflight,
  before Phase 1 starts. If the projection cannot be built, the run aborts with
  a specific diagnostic naming the unreadable ledger path or parse failure.
- Consumers receive `&HistoryProjectionV1` through the phase environment; the
  immutable reference is the mutation guard.
- Failed projection reads should preserve the same diagnostics the existing
  raw-ledger consumers surface.
- Standalone commands must still work without a prebuilt cache.
- Tests should prove that the chained run cache construction does not perform
  redundant ledger reads. Prefer a simple counter seam around projection
  construction and assert `build_count == 1`; Phase 1.2 and Phase 1.7 should
  consume the same immutable `PhaseEnv` projection.
- `history list` intentionally continues to read `sessions.jsonl` through the
  ledger reader and does not build a projection.
- Projection methods added for autonomy/history should stay neutral and
  consumer-facing, not encode one consumer's policy names.

## Phases

### Phase 1: Autonomy Gates Migration

**Goal:** Move v11 safety gates from custom ledger reads to the shared
projection without changing when `session run` is blocked.

**Scope:**

- Refactor `session/autonomy.rs` so its cross-session inputs come from
  `HistoryProjectionV1`.
- Add projection query helpers only if existing methods are insufficient, for
  example:
  - latest archived sessions ordered by `started_at` / `finished_at`;
  - open non-terminal PR artifacts associated with agent targets;
  - recent session outcome summaries.
- Preserve existing settings semantics:
  - `.sbagent/pause` blocks `session run`;
  - `max_open_agent_prs`;
  - `min_session_interval_hours`;
  - failure circuit breaker.
- Keep `maintain` allowed while paused.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Existing autonomy-gate tests pass with projection-backed inputs.
- [x] Any new projection method added for autonomy is consumer-neutral,
      typed-return-only, and covered by unit tests in `history_projection.rs`.
- [x] Open PR count matches the previous raw-ledger implementation against
      fixture ledgers.
- [x] Cadence and circuit-breaker checks match previous behavior against
      fixture ledgers.
- [x] `session/autonomy.rs` no longer reads `maintain.jsonl` or `sessions.jsonl`
      directly except through the shared projection helper.
- [x] Error messages remain operator-facing and specific.

**Tests:**

- Existing autonomy tests.
- Add focused fixture tests if the migration needs new projection helpers.

### Phase 2: History Show Migration

**Goal:** Move `sbagent history show <id>` to the shared projection while
preserving its existing ASCII output contract.

**Scope:**

- Refactor `cli/history.rs` show-path lifecycle/maintenance rendering to query
  `HistoryProjectionV1`.
- Do not migrate `history list` unless implementation finds a strict cleanup
  with byte-identical output; it reads only session records and has no
  maintain-side join.
- Preserve the current target table behavior, including `mixed` status
  rendering from `reason_code`.
- Preserve the `Maintenance events` section added by v10.
- Keep `history list` behavior unchanged unless implementation can reuse the
  projection without changing output or broadening scope.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Existing `history show` byte-equality fixtures remain unchanged unless
      a documented bug is corrected.
- [x] `history list` is either unchanged, or any no-risk cleanup keeps its
      byte-equality fixtures identical and still avoids `maintain.jsonl`.
- [x] Maintenance events sort and render exactly as before.
- [x] Mixed verdict display remains covered by tests.
- [x] `cli/history.rs` no longer owns lifecycle projection logic for show.
- [x] Piped output remains pure ASCII with no ANSI.

**Tests:**

- Existing `history show` integration fixtures.
- Add a projection-backed fixture if the migration exposes a missing edge case.

### Phase 3: Chained-Run Projection Cache

**Goal:** Realize v15's cost-model promise inside chained `session run`: one
ledger read, one projection build, multiple phase consumers.

**Scope:**

- Add an orchestration-scoped projection cache to the chained run environment.
- Build the cache once during session-start preflight, before Phase 1 starts.
  Failure aborts the run before agent phases begin and names the ledger path /
  parse failure.
- Phase 1.2 optimizer memory should consume the cached projection.
- Phase 1.7 merge/dedup should consume the same cached projection.
- Pass `&HistoryProjectionV1` through the phase environment; do not expose a
  mutable projection handle.
- Standalone optimizer-memory / merge subcommands may continue building their
  own projection once per invocation.
- Keep the projection snapshot stable for the whole session run.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] A chained-run test uses a counter seam around projection construction and
      asserts `build_count == 1`; Phase 1.2 optimizer memory and Phase 1.7
      merge/dedup consume the same immutable `PhaseEnv` projection.
- [x] Cache construction failure aborts before Phase 1 with a clear
      operator-facing diagnostic.
- [x] Standalone commands still work without an orchestration cache.
- [x] Projection-read failures surface before dependent phases run.
- [x] No phase mutates the cached projection.
- [x] Phase timing / archive behavior remains unchanged.

**Tests:**

- Use a lightweight fake projection reader or counter seam.
- Existing orchestrator-chain tests should continue to pass.

### Phase 4: v17 Report Readiness

**Goal:** Make the next history-report iteration consume projection data
directly, without implementing the report in v16.

**Scope:**

- Identify the minimum projection queries a markdown report will need:
  - sessions in a date range;
  - target outcome rollups;
  - PR / issue lifecycle state;
  - family / signature lineage if useful;
  - dedup skip counts if already represented in archived target rows.
- Add only methods needed by Phase 1-3 migrations now. For report-only helpers,
  document the intended API and defer implementation unless it removes
  duplication immediately.
- Do not add projection methods solely for future report needs. If a method
  added for autonomy or history-show also serves the report, document the dual
  use in the v16 archive's notes.
- Update `0043-history-report` backlog text to state that v17 should consume
  `HistoryProjectionV1`.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] No `history report` command is added in v16.
- [x] No projection method is added solely for future report needs.
- [x] Backlog `0043` points at v17 after projection migration completion.
- [x] Any projection methods added for report readiness are covered by tests.
- [x] The v17 handoff note states that reports must not read raw ledgers
      directly.

**Tests:**

- Only required for new projection methods.

### Phase 5: Cleanup + Audit

**Goal:** Leave the projection boundary obvious and prevent new raw-ledger
projection drift.

**Scope:**

- Remove helper functions made dead by the migrations.
- Update module docs in `history_projection.rs` with current consumers:
  - v11 autonomy gates (`session/autonomy.rs`);
  - v12 dedup (`session/dedup.rs`);
  - v13 optimizer memory (`session/optimizer_memory.rs`);
  - v6 history show (`cli/history.rs`);
  - chained-session orchestrator.
- Audit direct ledger-read call sites.
- Update `assets/autonomous-roadmap.md` to record that v16 completes the
  read-side migration and sets up v17.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `rg 'maintain_ledger::read_all|read_maintain|maintain_ledger::'
      crates/stacks-bench-agent/src/` shows no direct maintain-ledger reads
      in `session/autonomy.rs` or the `history show` path.
- [x] Expected remaining raw-ledger readers are documented. At minimum,
      `session/history_projection.rs`, `cli/maintain.rs`, and maintain
      reconciler internals may still read / write maintain events directly.
- [x] `history_projection.rs` module docs list all current projection consumers
      verbatim, and future-consumer updates are documented as part of the
      module contract.
- [x] `rg 'ledger_reader::read_all|session_ledger|sessions.jsonl'
      crates/stacks-bench-agent/src/` has no new raw session-ledger projection
      consumers outside the projection module and commands that intentionally
      display ledger-read diagnostics.
- [x] No generated schemas change.
- [x] Planning docs name v17 as the `0043-history-report` follow-up.

**Tests:**

- `just lint --no-sccache`
- `just test --summary --no-sccache`

## Final Validation

- [x] `just lint --no-sccache`
- [x] `just test --summary --no-sccache`
- [x] Test count is at least the v15 baseline of 582 unless redundant coverage
      is explicitly named and replaced.
- [x] Existing history output fixtures still pass.
- [x] Existing autonomy safety-gate fixtures still pass.
- [x] Existing dedup and optimizer-memory golden fixtures still pass.
- [x] No schema files changed.
- [x] `HistoryProjectionV1` is the default read-side API for autonomy,
      history-show lifecycle detail, dedup, and optimizer memory.

## Shipped Notes

- Added neutral projection views:
  - `sessions() -> &[SessionRecord]`;
  - `session(id) -> Option<&SessionRecord>`;
  - `maintenance_events_for_session(id) -> &[ProjectedMaintenanceEventV1]`.
- Migrated v11 autonomy gates and `history show` to `HistoryProjectionV1`.
- Chained `session run` now builds one projection and passes
  `&HistoryProjectionV1` into Phase 1.2 optimizer memory and Phase 1.7
  merge/dedup. The counter-seam test pins `build_count == 1`.
- `history list` intentionally remains on the lightweight session reader
  because it has no `maintain.jsonl` join.
- `--skip-preflight` does not skip the projection cache build; the cache is
  built after source materialization and before Phase 1.

## Follow-Ups

- **v17 footnote for reviewers:** v17 should implement
  [`0043-history-report`](0043-history-report.md) on top of
  `HistoryProjectionV1`. It should not add another raw ledger pass. The report
  can render session rollups, open / merged / closed PR state, issue state,
  dedup skips, and optimizer-memory lineage only where that information already
  exists in the projection.
- SQLite projection cache remains deferred until projection rebuild time is
  measured as painful.
- A canonical event log remains deferred until `sessions.jsonl` +
  `maintain.jsonl` stop being sufficient durable ledgers.
