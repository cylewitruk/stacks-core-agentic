# v5: Archive Metadata

Successor to [v4: v3 Polish + Bot-Fork Seed](v4-v3-polish-and-bot-fork-seed.md).
Populate the fields on `SessionRecord` that already exist but currently
write as `None` / empty: PR URLs from successful publish, per-target
bench wall-clock totals, and per-phase wall-clock durations. Make the
schema honest so operators can answer "where did this session spend
its time" and "what PR did this target land in" without rehydrating
the archive branch.

> **Status:** planned.
>
> All three target fields are already on `SessionRecord` (`pr_url`,
> `issue_url`, `phase_durations_secs`, `TargetBench.baseline_total_us`
> / `candidate_total_us`) — they just write empty / `None` because
> the producers were never wired up. v5 wires the producers.
> Reuses the schema-version + sidecar-file patterns from v3 and v4,
> so each phase is small and reviewable.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0043-example-config-load-test` | prologue | planned |
| `0026-phase-timing` | primary | planned |
| `0024-archive-audit-fields` | primary | planned |

## Why

Three loose threads worth tying off:

- **`pr_url` is hardcoded `None`** at
  [archive.rs:813](../../crates/stacks-bench-agent/src/session/archive.rs#L813),
  even on targets where Phase 5 successfully opened a PR. Today the
  URL flows to stdout via `println!` and is lost the moment Phase 5
  returns. Operators reviewing `sessions.jsonl` can't link a target
  row to the PR it produced — they have to grep stderr logs or click
  through GitHub by hand.
- **`phase_durations_secs` is an empty `BTreeMap`** at
  [archive.rs:631](../../crates/stacks-bench-agent/src/session/archive.rs#L631),
  documented as "empty until phase-timing instrumentation lands"
  ([archive.rs:25](../../crates/stacks-bench-agent/src/session/archive.rs#L25)).
  Without it, session-timing analysis is manual: scrape stderr,
  squint at timestamps. The field is right there in the schema —
  the producer just doesn't exist.
- **Bench wall-clock totals** are already-typed
  (`TargetBench.baseline_total_us` / `candidate_total_us`) but the
  archive writer always emits `0` because the values aren't lifted
  into summary rows on the way through. Aborted targets correctly
  carry `bench: None`; the broken case is targets that DID reach
  bench, where the field exists but is meaningless. v5 audits the
  artifact set the bench phase produces and either aggregates real
  values from `bench-run.json` files or documents the gap as a
  follow-up (see Phase 3 Notes).

Plus a small prologue:

- **`assets/example.config.toml` has drifted from `Settings` shape
  more than once** during v3/v4 (the user's local config still
  carries the pre-v3 `[stacks_core]` stanza after the v3 cutover
  removed it — surfaced when we tried `cargo run -- schema export`
  against the real config). A test that deserializes the bundled
  template into the `Settings` struct would have caught this class
  of regression cheaply.

## Scope

In scope:

- Test: deserialize `assets/example.config.toml` into the
  `Settings` struct via `toml::from_str::<Settings>(&body)`, assert
  no error. Skips the runtime helpers `Settings::load` layers on
  top of deserialization (path canonicalization, file-existence
  probes). Run on every PR via `just test`.
- Producer: a phase-timing recorder that wraps each `phase_X`
  function in `cli/session/run.rs` and accumulates wall-clock
  elapsed into a session-scoped `BTreeMap<String, f64>`. Written
  incrementally to `<session>/results/timings.json` after each
  phase completes (atomic rename via `tempfile::persist`); archive
  reads it during `SessionRecord` construction.
- Producer: a publish-feedback sidecar
  (`<session>/results/optimize/<target>/publish-feedback.json`)
  written by Phase 5 immediately after `octocrab.create_pr` /
  `create_issue` returns. Carries `{pr_url, issue_url, opened_at}`.
  Archive reads it during `build_target_records` and populates
  `TargetRecord.pr_url` / `issue_url`.
- Audit + (if needed) fix the bench wall-clock totals path so
  targets that reached bench have populated `TargetBench` fields.

Out of scope:

- Mutating already-archived `session/<id>` branches. The sidecar
  files land in `results/` BEFORE Phase 6 archive runs; archive
  reads them; the write-once contract on archive branches stays
  intact.
- Wall-clock timing for the bench *subprocess* (different from the
  bench *phase*). The persistent stacks-bench DB already records
  per-invocation timing; we link to those run-ids rather than
  duplicating.
- Per-target remote-install hook (the `publish.remote != "origin"`
  preflight relaxation). Sized as its own publish-flexibility
  iteration.
- `sbagent maintain` (cross-session PR lifecycle reconciliation).
  Folds into a future closed-loop autonomy iteration.

## Phases

### Phase 1: `assets/example.config.toml` Load Test (prologue)

**Goal:** A failing build catches any drift between the bundled
example config and the live `Settings` shape, instead of an operator
discovering the drift months later when their `sbagent` command
errors out.

**Scope:**

- New test
  `tests/example_config.rs::example_config_template_parses_into_settings`.
  Reads `assets/example.config.toml` from the workspace root (via
  `env!("CARGO_MANIFEST_DIR")`-relative path) AS-IS and deserializes
  it directly into `Settings` via `toml::from_str::<Settings>(&body)`,
  asserting it returns `Ok`. **Intentionally bypasses
  [`Settings::load`](../../crates/stacks-bench-agent/src/settings.rs#L996)**,
  which layers a `config` crate builder + `try_deserialize` +
  validation pass on top — those are operator-environment concerns
  (path canonicalization, file-existence probes for
  `publish.token_file`, etc.), not template-shape concerns. If a
  placeholder in the template breaks deserialization (e.g. an
  uncommented `<absolute path>` that fails a `deny_unknown_fields`
  check), that's exactly what this test should catch.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `just test example_config` passes against the current
      `assets/example.config.toml`.
- [x] A deliberate `[bogus_section]` injection into the template
      makes the test fail with a clear error (manual verification:
      injected `[bogus_section]`, ran the test, observed the
      `TOML parse error at line 335` with the valid-stanzas
      enumeration; restored cleanly).

**Tests:**

- [tests/example_config.rs](../../crates/stacks-bench-agent/tests/example_config.rs)
  (new).

### Phase 2: Phase Timing Instrumentation

**Goal:** `SessionRecord.phase_durations_secs` is populated with
wall-clock elapsed seconds per phase. Operators can answer "where
did this session spend its time" from a single `sessions.jsonl`
line.

**Scope:**

- New module `session/phase_timing.rs` exposing a small
  `PhaseTimings` struct backed by a `BTreeMap<String, f64>` with two
  ops: `record(phase, duration)` and `into_map()`.
- Wrap each `phase_X` function in
  [`cli/session/run.rs`](../../crates/stacks-bench-agent/src/cli/session/run.rs)
  with `Instant::now()` timing. Keys match the documented set:
  `baseline`, `triage`, `analysis`, `merge`, `optimize`, `bench`,
  `finalize`, `publish`. (Phase 0a + 0b stay under `baseline`;
  Phase 1.5 + 1.7 stay under `analysis` + `merge` respectively.)
- New typed model `Timings` →
  `<session>/results/timings.json` (`schema_version: 1`).
  **Written incrementally after each phase completes**, not in a
  single end-of-session write: each phase's wrapper records its
  duration and re-emits the full file via the
  `tempfile::persist` atomic-rename pattern. This way a session
  that crashes mid-pipeline still leaves `timings.json` with
  whichever phases finished — useful for triaging hung or aborted
  sessions. (The per-phase write cost is negligible: ~8 phases,
  each write is a single small JSON dump under 1 KB.)
- `archive.rs` reads `timings.json` during `SessionRecord`
  construction; absent file → empty map (legacy sessions). The
  current `phase_durations_secs: BTreeMap::new()` at
  [archive.rs:631](../../crates/stacks-bench-agent/src/session/archive.rs#L631)
  becomes the read result.
- **Standalone phase commands stay out of scope.** `sbagent session
  triage run` and friends don't archive a session and have no
  natural consumer for a one-phase `timings.json`. The
  full-pipeline `session run` is the only path that writes
  `timings.json`; standalone commands neither read it nor write
  to it.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [ ] Validated (live smoke deferred to v1 Pass 1c re-run; in-process
      mock-pipeline + fixture tests cover the recorder-to-archive
      contract)

**Acceptance & Validation:**

- [x] A mock pipeline (canonical-key recorder calls in
      `tests/phase_timing.rs::mock_pipeline_round_trips_through_timings_json`)
      produces a `timings.json` with one entry per phase that ran;
      live-pipeline verification rolls into the v1 Pass 1c smoke.
- [x] The same `timings.json` flows through archive into
      `SessionRecord.phase_durations_secs` exactly
      (`tests/archive.rs::archive_populates_phase_durations_secs_from_timings_json`).
- [x] A "crashed mid-pipeline" session leaves a partial
      `timings.json` carrying the completed phases and no entry for
      the phase that was running when the crash happened
      (`tests/phase_timing.rs::crashed_pipeline_leaves_partial_timings_json_with_completed_phases`
      + the recorder-level
      `crashed_session_leaves_partial_file_with_completed_phases`).

**Tests:**

- [tests/phase_timing.rs](../../crates/stacks-bench-agent/tests/phase_timing.rs)
  (new) — drives a small mock pipeline through the recorder and
  asserts the JSON round-trip + archive integration.
- Extend
  [tests/archive.rs](../../crates/stacks-bench-agent/tests/archive.rs)
  with a `phase_durations_secs` population assertion (mirroring the
  `source_*` field assertion).

**Notes:** Granularity is wall-clock-seconds-per-named-phase, not
sub-phase or per-target. Per-target timing is a Phase 3 concern
below (or a future iteration); this phase keeps the surface small.

### Phase 3: Publish Feedback Capture

**Goal:** `TargetRecord.pr_url` and `issue_url` carry the GitHub URL
when Phase 5 successfully opened a PR / issue; operators reviewing
`sessions.jsonl` see the link directly instead of grepping logs.

**Scope:**

- `GhClient::create_pr` (publish.rs:785) returns the PR struct (or
  at least `pr.html_url + pr.number`) instead of returning `()`
  after `println!`-ing the URL. Same for `create_issue` (815).
  The `println!` stays for operator-visible feedback during the
  command's run.
- New typed model
  `PublishFeedback { pr_url, issue_url, opened_at }` →
  `<session>/results/optimize/<target>/publish-feedback.json`
  (`schema_version: 1`). Written immediately after the GitHub API
  call returns successfully — before any subsequent target's
  publish runs, so a Phase 5 crash mid-fanout doesn't lose
  already-opened PR URLs.
- `archive.rs::build_target_records` reads
  `publish-feedback.json` per target during row construction;
  populates `TargetRecord.pr_url` / `issue_url`. Absent file leaves
  the fields `None` (consistent with legacy sessions).
- Per-target bench wall-clock audit landed as **archive-side
  aggregation** in
  [`session/archive.rs`](../../crates/stacks-bench-agent/src/session/archive.rs):
  for targets with `verification_replay`, archive sums
  `.data.summary.total_duration_us` across per-invocation
  `verify/<target>/<inv>/bench-run.json` files (baseline) and
  `optimize/<target>/<inv>/bench-run.json` files (candidate) at
  record-build time. Hand-side aggregation in `finalize.rs` was
  considered and rejected: the data the archiver needs already lives
  in per-invocation files; lifting it into `summary.json` first
  would just add a duplication hop. Targets without
  `verification_replay` (full-range fallback) keep totals at 0 —
  documented limitation per "document, don't scope-creep."

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [ ] Validated (live smoke deferred to v1 Pass 1c re-run; in-process
      fixture tests cover the publish→sidecar→archive flow and the
      per-invocation bench-run aggregation)

**Acceptance & Validation:**

- [x] A session with one `delivery_mode: normal_pr` target that
      successfully publishes has `TargetRecord.pr_url ==
      Some(<github-url>)` in `sessions.jsonl`
      (`tests/publish_push.rs::push_writes_publish_feedback_sidecar_with_returned_pr_url`
      → `tests/archive.rs::archive_populates_target_pr_url_from_publish_feedback_sidecar`).
- [x] A session with one `delivery_mode: consensus_issue` target
      has `TargetRecord.issue_url == Some(<github-url>)`
      (`push_writes_publish_feedback_sidecar_with_returned_issue_url`
      + the same archive test covers the issue path).
- [x] A session where Phase 5 was skipped writes no
      `publish-feedback.json` and `pr_url == None`
      (`archive_leaves_target_urls_absent_when_publish_feedback_sidecar_missing`).
- [x] Bench wall-clock audit: targets with `verification_replay`
      that reached bench have BOTH `bench.baseline_total_us > 0` AND
      `bench.candidate_total_us > 0` (aggregated from per-invocation
      `verify/<target>/<inv>/bench-run.json` + `optimize/<target>/<inv>/bench-run.json`
      files at archive time —
      `archive_aggregates_target_bench_wall_clock_totals_from_per_invocation_bench_run_json`).
      Targets WITHOUT `verification_replay` (full-range fallback
      path) keep totals at 0 — documented limitation: the canonical
      post-Pass-1c flow always emits `verification_replay`, so the
      gap only manifests on legacy/unstructured sessions and is
      acceptable per the iteration's "document, don't scope-creep"
      clause.

**Tests:**

- Extend
  [tests/publish_push.rs](../../crates/stacks-bench-agent/tests/publish_push.rs)
  with a fake `GhClient` that returns canned URLs; assert
  `publish-feedback.json` lands.
- Extend
  [tests/archive.rs](../../crates/stacks-bench-agent/tests/archive.rs)
  with a `publish-feedback.json` fixture and assert the URL flows
  to `TargetRecord.pr_url`.

**Notes:** No schema bump on `SessionRecord` — the v4 Phase 2 v3
bump already lives with the four `source_*` fields all having
`#[serde(default)]`. `pr_url` and `issue_url` were already
`Option<String>` with `skip_serializing_if = "Option::is_none"`,
so populating them is purely a write-side change. New typed models
(`Timings`, `PublishFeedback`) are their own `schema_version: 1`
artifacts — independent of `SessionRecord`'s versioning.

## Final Validation

In-process / unit:

- [x] `assets/example.config.toml` deserializes into `Settings`
      without error (Phase 1 —
      `tests/example_config.rs::example_config_template_parses_into_settings`).
- [x] Phase timing: `timings.json` produced by a full-pipeline run;
      `SessionRecord.phase_durations_secs` mirrors it.
- [x] Publish feedback: `pr_url` / `issue_url` flow from GitHub API
      → sidecar → archive → ledger.

Live / operator (deferred to the same v1 Pass 1c smoke as v3+v4):

- [ ] Inspecting a live session's archived `SessionRecord` shows
      meaningful phase durations + linked PR URLs without operator
      log-grepping.

Code-side ships once the three phases land. The live bullet folds
into the v1 smoke track.

## Non-Goals

- New schema version on `SessionRecord`. All target fields already
  exist; v5 is purely about populating them.
- Cross-session aggregation / dashboards. Operators consuming
  `sessions.jsonl` can roll up themselves; building a dashboard is
  `0036-observability-surface` (separate iteration).
- Per-phase sub-step timing (e.g. Phase 2 breakdown by target
  optimizer wall-clock). Single-level `BTreeMap<String, f64>` is
  the v5 surface; deeper structure can come with phase-level
  events in a follow-up.

## Follow-Ups

- v6 candidate: publish-side flexibility (per-target remote-install
  hook + multi-remote map). Re-enables the pre-v3 `bot`/`origin`
  separation operators may still want.
- `0036-observability-surface` — once `phase_durations_secs` is
  populated, a markdown report of `sessions.jsonl` becomes
  trivially renderable.
- `0033-maintain-command` — `publish-feedback.json`'s `opened_at`
  field becomes the natural anchor for "how long has this PR been
  open" reconciliation.
