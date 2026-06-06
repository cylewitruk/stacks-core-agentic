# v2: Cleanup And Workspace Hygiene

Successor to [v1: Live Pass 1c Smoke](v1-live-pass-1c-smoke.md). Tighten the
phase-clean surfaces so they match the Pass 1c artifact tree, add the
operator-facing workspace hygiene knobs the design docs called out, and make
disk exhaustion fail loudly and early.

> **Status:** planned.
>
> Both items are scoped to coordinator-owned scratch and operator hygiene.
> No agent-prompt contract or artifact-schema changes — the only template
> edits are non-prose `<!-- lint:example ... -->` markers added to
> existing output-example fences (Phase 2). Per-target build trees and old
> sessions are the two real disk sinks; this iteration closes both.

## Items

<a id="0020-migration-leftovers"></a>
<a id="0023-workspace-cleanup"></a>

| Item | Role | Status |
| ---- | ---- | ------ |
| `0020-migration-leftovers` | primary | planned |
| `0023-workspace-cleanup` | primary | planned |

## Why

Two leftover hygiene concerns block scheduled, autonomous operation:

- The Pass 1c artifact tree added `verify/<target>/<invocation-id>/` (Phase 1.8
  targeted baseline calibration) and `analyze/<target>/results-analysis.json`
  (Phase 3.5 verdict). The Phase 3.5 path is covered by
  `sbagent session analyze-results clean`, but the Phase 1.8 `verify/` tree is
  not removed by any phase-clean command today
  ([baseline/clean.rs:20](../../crates/stacks-bench-agent/src/cli/session/baseline/clean.rs#L20)
  only drops `baseline_dir()`;
  [bench/clean.rs:40-49](../../crates/stacks-bench-agent/src/cli/session/bench/clean.rs#L40-L49)
  only drops the Phase 3 candidate side under `optimize/<target>/`).
- Prompt lint
  ([prompts.rs:207-247](../../crates/stacks-bench-agent/src/prompts.rs#L207-L247))
  exercises MiniJinja rendering against an in-code `synthetic_for_lint()`
  context, but does not parse the templates' embedded JSON code-fence examples
  and validate them against the schema files those templates reference. A
  rewrite of `analysis.schema.json` would not fail prompt lint today.
- Per-target optimizer checkouts
  ([design/0023-workspace-cleanup.md](../design/0023-workspace-cleanup.md))
  retain full build trees for the whole session, and the operator has no
  durable command for pruning old session workspaces or for catching obvious
  disk shortfalls before Phase 0a.

## Scope

In scope:

- Phase 1.8 `verify/` cleanup coverage inside `sbagent session bench clean`.
- Embedded-example schema validation inside `sbagent prompt lint`.
- Lock-in tests + docs for the existing per-worktree `cargo clean` contract
  so the build-cache reclamation behavior cannot regress unnoticed.
- `sbagent workspace prune` (age + archive-status filters).
- Preflight disk-space check inside the existing session preflight.

Out of scope:

- Schema or agent-prompt prose changes. Phase 2 adds opt-in
  `<!-- lint:example ... -->` HTML-comment markers to existing output-example
  fences — those are lint metadata, not visible to the agent and not changes
  to the prompt contract.
- Concurrency model changes to the optimizer fan-out — the existing
  `cargo clean` contract is what bounds per-target build cache; parallelizing
  the fan-out is a separate item.
- Per-target checkout teardown before Phase 5 (the worktree is load-bearing
  for publish; see Phase 3 notes).
- `sbagent check` expansion beyond a narrative pointer to the new commands.

## Phases

### Phase 1: Phase 1.8 `verify/` Cleanup Coverage

**Goal:** No coordinator-owned Pass 1c artifact tree directory survives an
operator-issued phase clean run.

**Scope:**

- Ownership: extend `sbagent session bench clean`. Phase 3 candidate benches
  are paired with Phase 1.8 baselines on the same invocation-id set, so the
  symmetric removal matches how operators rerun — one command drops both
  sides of one invocation.
- Iterate the merged target list (same loader pattern bench clean already uses
  at [bench/clean.rs:33-39](../../crates/stacks-bench-agent/src/cli/session/bench/clean.rs#L33-L39))
  and drop `verify_dir().join(&target.id)` per target, plus the wholesale
  `verify_dir()` as a final pass when empty.
- Keep idempotence: missing files are no-ops, recorded as `skipped_missing` in
  the `CleanReport`.
- Update [docs/operations.md](../../docs/operations.md) recovery table row for
  Phase 1.8 to name the new clean command.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated (acceptance checks below run)

**Acceptance & Validation:**

- [ ] After running the chosen clean command on a session whose Phase 1.8
      completed, every `verify/<target>/<invocation-id>/bench-run.json` and
      `verify/<target>/baseline-run-ids.json` is gone.
- [ ] The command is idempotent: a second run reports `skipped_missing` and
      exits 0.
- [ ] Touching the Phase 0 archive (`baseline/bin/`) is impossible from
      this command.

**Tests:**

- [bench_clean_tests](../../crates/stacks-bench-agent/src/cli/session/bench/clean.rs)
  — new cases asserting `verify/<target>/` removal and idempotence.

### Phase 2: Schema-Example Lint For Prompts

**Goal:** Prompt lint fails when a template's output-example JSON does not
validate against the schema the same template names.

**Scope:**

- Two kinds of fenced JSON exist in the templates and only one is checkable:
  - **Input-data fences** like ```` ```json\n{{ family_json }}\n``` ```` are
    runtime-injected blobs. Treat as render-only — the existing
    `synthetic_for_lint` context already covers them.
  - **Output-example fences** with literal JSON bodies (interpolated leaves
    like `"family_id": "{{ family_id }}"` are fine) demonstrate the shape the
    agent must emit and are the only fences worth schema-checking.
- Contract: walk the **rendered** prompt produced by the existing
  `synthetic_for_lint` pass — not the raw template source — so MiniJinja
  expansion runs before parsing. For each fence flagged with an explicit
  marker, parse the rendered body and validate against the named schema.
- Marker shape (HTML comment, MiniJinja-safe and prose-invisible to the
  agent): `<!-- lint:example schema="analysis" -->` immediately above the
  fence. Unmarked fences are skipped — explicit > heuristic, as Codex flagged.
- Resolve `schema=<name>` against the bundled schemas baked into the binary
  via [schemas.rs](../../crates/stacks-bench-agent/src/schemas.rs); use the
  same JSON Schema validator the session phases use so the lint verdict
  matches runtime.
- New `LintFinding` variant:
  `ExampleSchemaMismatch { template, schema, errors }`. Existing synthetic
  render lint stays in place; this adds a parallel check.
- Mark the existing output-example fences in `analyzer.md`,
  `merge-analyses.md`, and `optimizer.md` with the new marker as part of this
  phase so lint has actual coverage from day one.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] `sbagent prompt lint` fails on a hand-injected break (e.g. delete a
      required field from analyzer.md's accepted example) and the finding
      names both the template path and the schema.
- [ ] All existing templates pass schema-example lint with no edits to their
      JSON bodies.
- [ ] Skipped blocks with the explicit marker do not run schema validation.

**Tests:**

- [prompts_lint_tests](../../crates/stacks-bench-agent/src/prompts.rs)
  — synthetic templates with valid + invalid embedded examples and a skip
  marker.

**Notes:** The Phase 5 contract that operator-side prompt edits are warned-only
([check.rs:17-19](../../crates/stacks-bench-agent/src/cli/check.rs#L17-L19))
is unchanged. `prompt lint` is the strict gate; `check` stays soft on prompt
drift.

### Phase 3: Lock In The Existing `cargo clean` Reclamation

**Goal:** Make the already-shipped per-worktree `cargo clean` contract
explicit, tested, and documented so it cannot regress unnoticed.

**Scope:**

- **Why not teardown.** The per-target checkout is load-bearing through Phase
  5: PR-writer reads it as `worktree_dir`
  ([publish.rs:264-267](../../crates/stacks-bench-agent/src/session/publish.rs#L264-L267))
  and `git push` runs from it
  ([publish.rs:983-989](../../crates/stacks-bench-agent/src/session/publish.rs#L983-L989))
  (hard-bails if missing). The existing session-end cleanup
  ([run.rs:604-610](../../crates/stacks-bench-agent/src/cli/session/run.rs#L604-L610))
  intentionally preserves implemented checkouts for this reason. Removing
  them earlier would regress publish.
- **What already ships.** `bench_experiments::build_one`
  ([bench_experiments.rs:161-167](../../crates/stacks-bench-agent/src/session/bench_experiments.rs#L161-L167))
  runs `cargo clean` inside each worktree right after the release binary is
  copied to `exp_dir/bin/stacks-bench`
  ([bench_experiments.rs:152-159](../../crates/stacks-bench-agent/src/session/bench_experiments.rs#L152-L159)),
  before the bench invocations run. Bench invocations use the copied binary,
  so the worktree's `target/` is genuinely disposable from that point. The
  opt-out is the existing `--skip-cargo-clean` flag, plumbed through
  [session run](../../crates/stacks-bench-agent/src/cli/session/run.rs#L47)
  and
  [bench run](../../crates/stacks-bench-agent/src/cli/session/bench/run.rs#L25).
- **What this phase adds.** No new flag. Three small hardening edits:
  1. An integration test that asserts the `target/` directory is empty (or
     gone) after `bench_experiments::build_one` returns, exercising both the
     default-on path and the `--skip-cargo-clean` opt-out.
  2. A negative test: `cargo-clean.log` / `cargo-clean.stderr.log` exist
     under `optimize/<target>/` after a default run and are absent under
     `--skip-cargo-clean`.
  3. Operations docs: surface the reclamation contract in
     [docs/operations.md](../../docs/operations.md) so operators know the
     worktree retains `.git/` + binary + logs through Phase 5 but not the
     build cache.

**Status:**

- [ ] Tests added
- [ ] Docs updated
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Default `session run` leaves an empty/absent `target/` under each
      per-target checkout after Phase 3.
- [ ] `--skip-cargo-clean` preserves `target/`.
- [ ] Phase 5 publish runs successfully after the default reclamation path.
- [ ] Operations docs name the contract and the `--skip-cargo-clean` escape
      hatch.

**Tests:**

- `bench_experiments_tests` — extend with the two assertions above; reuse the
  existing `CargoStub` if one already exists, otherwise add one that records
  invocations.

**Notes:** Teardown of the entire checkout still happens at session-end
([run.rs:611](../../crates/stacks-bench-agent/src/cli/session/run.rs#L611))
for aborted targets and after Phase 6 archive for the rest. This phase is a
documentation + regression-fence pass, not a feature.

### Phase 4: `sbagent workspace prune` And Disk Preflight

**Goal:** Operators have one command for pruning stale workspaces and a clear
preflight error when free space is obviously insufficient.

**Scope:**

- `sbagent workspace prune [--older-than DURATION] [--archived-only] [--dry-run]`:
  - Iterate `agent_workspace_root/sessions/`.
  - **No on-disk session-state file exists today.** Use the durable signals
    that do: (a) cross-reference `sessions.jsonl` on operator main to identify
    archived/terminal sessions, and (b) refuse to prune the session id
    matching any currently-running `sbagent` process via a best-effort PID
    file.
  - **PID file lifecycle.** `session run` writes
    `agent_workspace_root/sessions/<id>/.run.pid` at session start (after
    preflight, before Phase 0). The file is removed in a normal-exit cleanup
    after Phase 6 archive and on graceful Ctrl-C handling. A crashed or
    SIGKILL'd run leaves the file behind. `workspace prune` treats the file
    as authoritative only when the PID is actually live (POSIX `kill -0`);
    stale PIDs (process gone) fall through to the normal age + archive
    filters, so they cannot make a workspace immortal.
  - `--archived-only` is the safe default flag: requires presence in
    `sessions.jsonl`. Without it, the command refuses to remove anything
    unless `--older-than` is also explicitly provided.
  - Default to `--dry-run` if neither filter is set, to keep the destructive
    path explicit.
  - Operator-readable footer: bytes that would be freed, even in dry-run.
- Preflight: add a `check_free_disk` step inside the existing session preflight
  ([planning/archive/completed/0013-preflight-v1.md](../archive/completed/0013-preflight-v1.md)
  context). Warn-only by default (default `preflight.min_free_gib = None`);
  the operator opts into a hard-fail by setting the config. When set and
  violated, the error body includes the exact suggested
  `workspace prune --older-than 7d --archived-only` invocation.
- Document under [docs/operations.md](../../docs/operations.md) "recovery"
  section and [docs/configuration.md](../../docs/configuration.md) preflight
  stanza.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] `sbagent workspace prune --dry-run` lists candidates without removing
      anything; `--older-than 7d --archived-only` removes only sessions whose
      archive entry exists and whose age threshold is met.
- [ ] A session whose `.run.pid` matches a live process is never prunable;
      a session whose `.run.pid` is stale (PID gone) falls through to the
      normal age + archive filters.
- [ ] A session-run with `preflight.min_free_gib` set and violated fails
      preflight with the exact suggested prune invocation in the error body.
      Without `preflight.min_free_gib`, low free space emits a warning only.

**Tests:**

- `workspace_prune_tests` covering: dry-run, live-PID-file refusal,
  archived-only filter, missing `sessions.jsonl` falls back to dry-run.
- `preflight_disk_tests` mocking `fs2::available_space` (or equivalent), with
  both the `None` (warn-only) and `Some(_)` (hard-fail) shape.

**Notes:** Defer choosing a default `min_free_gib` to a follow-up — first run
a real session, watch peak usage, then pick a floor. Until then, the operator
opts in.

## Final Validation

- Every phase clean command targets the full Pass 1c artifact tree it owns;
  `find $WORKSPACE/sessions/<id>/results -type f` after running every clean
  command in order leaves only the durable archive bundle.
- `sbagent prompt lint` rejects a hand-broken schema example and accepts the
  current template set unchanged.
- A serial three-target session reclaims each per-target `target/` build cache
  by default while preserving every worktree's source + `.git/` + copied
  binary through Phase 5 publish.
- `sbagent workspace prune` and the disk preflight have at least one live
  invocation against a real workspace before this iteration ships.

## Non-Goals

- Parallel optimizer fan-out or shared checkout pools.
- Cross-session memory or PR-state reconciliation
  ([0028-optimizer-memory](../backlog.md#0028-optimizer-memory),
  [0033-maintain-command](../backlog.md#0033-maintain-command)).
- Schema or agent-prompt prose rewrites driven by the live smoke
  ([0019-prompt-hardening-live-smoke](../backlog.md#0019-prompt-hardening-live-smoke)).

## Follow-Ups

- `0021-preflight-v2` if the disk preflight reveals other drift classes worth
  the same fail-early treatment.
- `0026-phase-timing` becomes the natural next target — `workspace prune`
  surfaces session age, which makes per-phase durations the next observability
  win.
