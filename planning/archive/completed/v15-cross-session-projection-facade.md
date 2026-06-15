# v15: Cross-Session Projection Facade

Successor to [v14: Local Session Systemd Schedule](v14-local-session-systemd-schedule.md).
v10-v14 made the autonomous loop useful, but they also left several read-side
features projecting the same two ledgers independently. v15 starts the 0030
cleanup as a shared projection facade rather than a new canonical event log.

> **Status:** shipped.
>
> v15 deliberately keeps `sessions.jsonl` and `maintain.jsonl` as the durable
> source ledgers. The new projection is rebuildable from those ledgers at any
> time; it is not a third ledger, SQLite cache, or migration boundary.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0030-event-log-skeleton` | primary | shipped |

## Why

Five current consumers now read and project the operator ledgers:

- `history show` reads `sessions.jsonl` and renders session / target detail.
- `maintain` projects `maintain.jsonl` to decide which GitHub artifacts need
  fresh reads.
- v11 autonomy gates read `sessions.jsonl` + `maintain.jsonl` for open PR
  count, cadence, and circuit-breaker state.
- v12 dedup reads both ledgers for per-signature blocking decisions.
- v13 optimizer memory reads both ledgers for compact family / signature
  history.

Each consumer sorts lifecycle events, joins archived targets to PR / issue
state, and derives latest state slightly differently. That is manageable today,
but every new autonomy surface would otherwise add another custom projection.

v15 creates one shared read-side projection and migrates the two consumers that
benefit most immediately: v12 dedup and v13 optimizer memory.

## Source-Of-Truth Invariants

- `sessions.jsonl` remains the durable session/archive ledger.
- `maintain.jsonl` remains the durable PR / issue lifecycle ledger.
- The projection is read-only and rebuildable from the ledgers.
- Existing raw-ledger readers continue to work during and after v15.
- v15 does not write `events/`, introduce SQLite, or change either ledger
  schema.
- The v10 maintain reconciler's write-side `ArtifactProjection` remains scoped
  to deciding which new `maintain.jsonl` events to emit. The new projection is
  read-side session-history machinery.
- `session/dedup.rs` and `session/optimizer_memory.rs` may retain
  `from_ledgers(...)` compatibility wrappers that mention `MaintEvent`; those
  wrappers must delegate to `HistoryProjectionV1` and must not own projection
  assembly logic.

## Scope

In scope:

- Add a shared read-side module, preferably
  `crates/stacks-bench-agent/src/session/history_projection.rs`.
- Version the in-memory projection shape from day one, for example
  `HistoryProjectionV1` plus `from_ledgers_v1(...)`.
- Include only the views needed by v12 + v13:
  - archived target attempts keyed by exact `fix_signature`;
  - family-grouped attempts keyed by `family_id`;
  - latest PR / issue lifecycle state per artifact URL;
  - observed-at-ordered lifecycle projection;
  - source SHA, head SHA, reason code, delivery mode, status, URLs, and
    timestamps needed by dedup + optimizer memory.
- Build golden fixtures for current behavior before migrating consumers:
  - dedup decisions for representative fixture ledgers;
  - optimizer-memory JSON output for representative fixture ledgers.
- Migrate v12 dedup to consume the shared projection.
- Migrate v13 optimizer memory to consume the shared projection.
- Keep behavior equivalent unless the migration uncovers a documented bug.

Out of scope:

- A new canonical event log under `events/`.
- Disposable SQLite projection cache.
- Migration of `history show`, `maintain`, or v11 autonomy gates. These are
  v16 candidates.
- `sbagent history report`.
- Schema changes to `sessions.jsonl`, `maintain.jsonl`, or
  `optimizer-memory.json`.
- Fuzzy similarity matching or changed dedup policy.
- Prompt changes.

## Projection Contract

The initial projection should be boring and explicit:

```rust
pub struct HistoryProjectionV1 {
    // exact fields decided during implementation, but keep this shape minimal
}

impl HistoryProjectionV1 {
    pub fn from_ledgers_v1(
        sessions: &[SessionRecord],
        maintain_events: &[MaintEvent],
    ) -> Self;

    pub fn attempts_for_signature(&self, fix_signature: &str) -> &[ProjectedAttemptV1];

    pub fn signatures_for_family(&self, family_id: &str) -> &[ProjectedSignatureV1];

    pub fn latest_artifact_state(&self, url: &str) -> Option<&ProjectedArtifactStateV1>;
}
```

Implementation requirements:

- `from_ledgers_v1` takes parsed slices. It does not read files, emit stderr,
  or report skipped lines. Skipped-line warnings stay with the CLI /
  orchestration layer, matching the v6 ledger reader contract.
- Sort maintain events by `observed_at` before deriving latest lifecycle state.
- Treat file order as non-semantic.
- Preserve legacy session rows that lack newer fields, including missing
  `source_sha`.
- Keep exact-signature matching exact. No family-level or fuzzy dedup gates.
- Make projection fields read-only. Consumers should not mutate projection
  state to record derived policy decisions.
- Document the distinction from `session::maintain::ArtifactProjection`:
  maintain's projection is write-side and invocation-scoped; this one is
  read-side and shared by cross-session consumers.

## Phases

### Phase 1: Design Rescope + Projection Skeleton

**Goal:** Replace the old 0030 event-log sketch with a shared read-side
projection contract and land the module skeleton.

**Scope:**

- Update [planning/design/0030-event-log-skeleton.md](../design/0030-event-log-skeleton.md)
  to name the v15 rescope:
  ledgers remain source of truth; projection facade first; event log / SQLite
  deferred.
- Add `session/history_projection.rs` and wire it through `session/mod.rs`.
- Define `HistoryProjectionV1` and `from_ledgers_v1`.
- Add small unit tests for:
  - missing ledgers / empty inputs;
  - `observed_at` ordering independent of file order;
  - multiple lifecycle events for one artifact resolving to the latest state;
  - missing `source_sha` preserved as drift unknown.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] 0030 design doc no longer claims `events/<session-id>.jsonl` is the next
      implementation step.
- [x] Projection module is read-only and has no writer API.
- [x] Projection type and constructor are versioned with `V1` naming.
- [x] Projection exposes neutral consumer query methods, not dedup or memory
      policy decisions.
- [x] Projection constructor accepts parsed records/events and does not emit
      skipped-line warnings.
- [x] Unit tests prove lifecycle events are sorted by `observed_at`.
- [x] Code comments distinguish read-side history projection from maintain's
      write-side `ArtifactProjection`.

**Tests:**

- Focused Rust unit tests in `session/history_projection.rs`.

### Phase 2: Golden Fixtures For Current Consumers

**Goal:** Capture current v12 + v13 behavior before migration so the refactor
has a byte-level safety net.

**Scope:**

- Add or reuse fixture ledgers that cover:
  - open PR blocks;
  - stale PR does not block;
  - force-push after stale re-blocks;
  - merged PR blocks;
  - repeated failures block at threshold;
  - open issue blocks;
  - optimizer memory with same-family siblings, exact-signature attempts,
    lifecycle state, and missing `source_sha`.
- Prefer fixture directories under
  `crates/stacks-bench-agent/tests/fixtures/projection/`, for example:
  - `open-pr-blocks/{sessions.jsonl,maintain.jsonl}`;
  - `stale-no-block/{sessions.jsonl,maintain.jsonl}`;
  - `force-push-reblocks/{sessions.jsonl,maintain.jsonl}`;
  - `merged-blocks/{sessions.jsonl,maintain.jsonl}`;
  - `repeated-failures/{sessions.jsonl,maintain.jsonl}`;
  - `open-issue-blocks/{sessions.jsonl,maintain.jsonl}`;
  - `memory-family-context/{sessions.jsonl,maintain.jsonl}`.
- Snapshot or assert the current dedup decisions before changing their data
  source.
- Snapshot or assert the current optimizer-memory JSON output before changing
  its data source.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Dedup fixture decisions are captured before the migration.
- [x] Optimizer-memory fixture output is captured before the migration.
- [x] Dedup golden outputs are committed under
      `crates/stacks-bench-agent/tests/fixtures/projection/golden/dedup/`.
- [x] Optimizer-memory golden outputs are committed under
      `crates/stacks-bench-agent/tests/fixtures/projection/golden/memory/`.
- [x] Fixtures cover both `sessions.jsonl`-only and `sessions.jsonl` +
      `maintain.jsonl` cases.
- [x] Fixture names describe the policy edge being protected.

**Tests:**

- In-module tests or integration fixtures, whichever best matches existing
  dedup / optimizer-memory test style.

### Phase 3: Dedup Projection Migration

**Goal:** Move v12 dedup from custom ledger projection to
`HistoryProjectionV1` without changing dedup policy.

**Scope:**

- Refactor `session/dedup.rs` so `DedupProjection` is built from the shared
  history projection or becomes a thin policy view over it.
- Preserve closed reason categories:
  - `dedup:open-pr`;
  - `dedup:open-issue`;
  - `dedup:merged`;
  - `dedup:repeated-failure`.
- Preserve lifetime repeated-failure counts.
- Preserve exact-signature-only matching.
- Preserve stale / force-push semantics from v12.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Existing dedup tests pass unchanged or with only fixture plumbing edits.
- [x] Golden dedup decisions from Phase 2 are unchanged.
- [x] `DedupProjection::from_ledgers` either remains as a compatibility wrapper
      or all call sites move to a clearer projection-backed API.
- [x] No optimizer fan-out behavior changes.
- [x] No new dedup reason strings are introduced.

**Tests:**

- Existing `session/dedup.rs` policy tests.
- Golden decision fixture from Phase 2.

### Phase 4: Optimizer Memory Projection Migration

**Goal:** Move v13 optimizer memory from custom ledger projection to
`HistoryProjectionV1` without changing the generated artifact.

**Scope:**

- Refactor `session/optimizer_memory.rs` to consume the shared projection for:
  - family rows;
  - exact-signature attempts;
  - latest lifecycle state;
  - source/head SHA drift context.
- Preserve compactness defaults:
  - last 5 attempts per signature;
  - last 3 sibling signatures per family;
  - existing render budget and omitted-row marker behavior.
- Preserve prompt-rendered memory text.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Golden optimizer-memory JSON from Phase 2 is unchanged.
- [x] Existing optimizer-memory unit tests pass.
- [x] Prompt contract tests still pass.
- [x] Missing `source_sha` remains represented as drift unknown.
- [x] No `optimizer-memory.json` schema change is required.

**Tests:**

- Existing optimizer-memory tests.
- Golden JSON fixture from Phase 2.

### Phase 5: Cleanup + v16 Handoff

**Goal:** Leave the partial migration legible and prepare the next consumer
migration without hiding remaining duplication.

**Scope:**

- Remove now-dead helper code from dedup / optimizer memory.
- After Phase 3, audit `session/dedup.rs` for functions whose only remaining
  reason to exist is projection assembly. Move that logic into
  `history_projection.rs`, or document why it remains policy-side.
- After Phase 4, do the same audit for `session/optimizer_memory.rs`.
- Add a short module doc or architecture note describing current projection
  consumers and remaining raw-ledger consumers.
  - Note the cost-model shift: v15 enables one ledger read and one projection
    build for migrated consumers, then querying the cached
    `HistoryProjectionV1`. The orchestrator does not yet share one projection
    across Phase 1.2 and Phase 1.7; that wiring is a v16 follow-up.
- Update comments in v11 gates / `history show` if they still intentionally
  project ledgers directly.
- Add v16 follow-up notes for migrating:
  - v11 autonomy gates;
  - `history show`;
  - future `history report`;
  - possibly extracting shared lifecycle state from maintain's write-side
    projection only if it reduces real duplication.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] No dead duplicate projection helpers remain in migrated consumers.
- [x] `session/dedup.rs` and `session/optimizer_memory.rs` retain only
      backward-compatible ledger-wrapper signatures that delegate to the
      projection-backed APIs.
- [x] Remaining duplicate projection call sites are named as v16 candidates.
- [x] `rg 'maintain_ledger::read_all|read_maintain|maintain_ledger::'
      crates/stacks-bench-agent/src/` returns only the intended
      partial-migration boundary: `cli/history.rs`, `cli/maintain.rs`,
      `session/autonomy.rs`, `session/maintain.rs`, and
      `session/history_projection.rs`.
- [x] Planning notes explicitly state v15 is a partial migration.

**Tests:**

- `just lint --no-sccache`
- `just test --summary --no-sccache`

## Final Validation

- [x] `just lint --no-sccache`
- [x] `just test --summary --no-sccache`
- [x] Golden dedup decisions unchanged.
- [x] Golden optimizer-memory JSON unchanged.
- [x] No schema files changed unless implementation proves a generated-schema
      description update is unavoidable.
- [x] Design doc and backlog agree that 0030 is currently projection-first,
      not event-log-first.
- [x] Test count is at least the v14 baseline of 576. Any reduction is
      documented as redundant coverage removed by projection consolidation,
      with the deleted test names and their new coverage path.

## Follow-Ups

- v16: migrate v11 autonomy gates and `history show` to
  `HistoryProjectionV1`.
- v16: optionally add an orchestration-level projection cache so Phase 1.2
  optimizer memory and Phase 1.7 merge/dedup share one ledger read and one
  projection build during chained `session run`.
- `0043-history-report` — consume the shared projection rather than adding a
  fresh ledger pass.
- SQLite cache — reconsider only if projection rebuild time becomes measurable
  pain.
- Canonical event log — reconsider only if `sessions.jsonl` + `maintain.jsonl`
  stop being sufficient as durable source ledgers.
