# v2: Cleanup And Workspace Hygiene

Successor to [v1: Live Pass 1c Smoke](v1-live-pass-1c-smoke.md). Tighten the
phase-clean surfaces so they match the Pass 1c artifact tree, add the
operator-facing workspace hygiene knobs the design docs called out, and make
disk exhaustion fail loudly and early.

> **Status:** shipped.
>
> All four phases are implemented, reviewed, exercised by unit /
> integration tests, and validated against smoke session `20260611-172955`.
> The session published successfully after default cargo-clean reclamation,
> and `sbagent workspace prune --dry-run --archived-only` found the archived
> session through the real operator ledger without removing it.
>
> Both items are scoped to coordinator-owned scratch and operator
> hygiene. No agent-prompt contract or artifact-schema changes — the
> only template edits are non-prose `<!-- lint:example ... -->`
> markers added to existing output-example fences (Phase 2).
> Per-target build trees and old sessions are the two real disk
> sinks; this iteration closes both.

## Items

<a id="0020-migration-leftovers"></a>
<a id="0023-workspace-cleanup"></a>

| Item | Role | Status |
| ---- | ---- | ------ |
| `0020-migration-leftovers` | primary | shipped |
| `0023-workspace-cleanup` | primary | shipped |

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
  ([0023-workspace-cleanup.md](0023-workspace-cleanup.md))
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

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated (acceptance checks below run)

**Acceptance & Validation:**

- [x] After running the chosen clean command on a session whose Phase 1.8
      completed, every `verify/<target>/<invocation-id>/bench-run.json` and
      `verify/<target>/baseline-run-ids.json` is gone.
- [x] The command is idempotent: a second run reports `skipped_missing` and
      exits 0.
- [x] Touching the Phase 0 archive (`baseline/bin/`) is impossible from
      this command.

**Tests:**

- [tests/bench_clean.rs](../../crates/stacks-bench-agent/tests/bench_clean.rs)
  — five cases: per-target Phase 1.8 + Phase 3 removal across two
  targets, idempotence, wholesale `verify/` sweep without targets
  loaded, optimizer-owned artifacts left alone, and corrupt-targets
  error propagation (no half-clean).

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
- Reuse the existing `LintFinding { template, message }` shape; schema-
  example failures surface as findings with descriptive messages naming
  the template, schema, and reported error paths. Existing synthetic
  render lint stays in place; this adds a parallel check on rendered
  output. (Earlier drafts proposed a new enum variant —
  `ExampleSchemaMismatch { template, schema, errors }` — but the flat
  struct already carries everything the CLI consumer needs, and the
  extra refactor would have churned multiple call sites for no operator-
  visible benefit.)
- Mark every existing output-example fence whose body validates against
  its named schema with no prose changes. Confirmed candidates on
  inspection: `merge-analyses.md` (output is `optimization-targets`) and
  `triage.md` (output is `candidates`). See **Notes** below for the
  templates that need follow-up work before they can be marked.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `sbagent prompt lint` fails on a hand-injected break (e.g. delete a
      required field from `merge-analyses.md`'s output example) and the
      finding names both the template path and the schema.
- [x] All marked output examples in shipped templates pass schema-example
      lint with no edits to their JSON bodies.
- [x] Unmarked fences are skipped — the contract that keeps input-data
      fences (`{{ var_json }}`) and inline shape illustrations from
      showering false positives.

**Tests:**

- [prompts.rs::tests](../../crates/stacks-bench-agent/src/prompts.rs)
  — six new cases under the existing test module: marker parser shape
  coverage, valid body acceptance, schema mismatch surfacing, unknown
  schema name, dangling marker, unparseable JSON, and explicit skip-when-
  unmarked.

**Notes:**

- The Phase 5 contract that operator-side prompt edits are warned-only
  ([check.rs:17-19](../../crates/stacks-bench-agent/src/cli/check.rs#L17-L19))
  is unchanged. `prompt lint` is the strict gate; `check` stays soft on
  prompt drift.
- **Deferred markers — `analyzer.md`.** Its two output examples (accepted
  and rejected) use literal `"selection_lens": "..."` and `"lens": "..."`
  placeholders that don't satisfy the schema's enum constraint. Adding
  the marker requires replacing those with concrete enum values like
  `"tx_latency"`, which is a prompt-prose change v2 ruled out of scope.
  Later closed by
  [`0038-prompt-example-concretization`](0038-prompt-example-concretization.md)
  in v7.
- **Deferred markers — `optimizer.md`.** It has no top-level output
  example fence; its only ```json fence is the input-data
  `{{ target_json }}` placeholder. No marker work to do here.
- **Deferred markers — `results-analyzer.md`, `pr-writer.md`,
  `issue-writer.md`.** Same reason as `optimizer.md` — JSON fences in
  these are input-data placeholders, and their non-JSON outputs
  (markdown PR/issue bodies) aren't schema-validatable in this scheme.

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

- [x] Tests added
- [x] Docs updated
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Default `session run` leaves an empty/absent `target/` under each
      per-target checkout after Phase 3.
- [x] `--skip-cargo-clean` preserves `target/`.
- [x] Phase 5 publish runs successfully after the default reclamation path.
      Validated by smoke session `20260611-172955`, which pushed three PRs
      from per-target worktrees after the default Phase 3 cargo-clean path.
- [x] Operations docs name the contract and the `--skip-cargo-clean` escape
      hatch.

**Tests:**

- [tests/bench_experiments.rs](../../crates/stacks-bench-agent/tests/bench_experiments.rs)
  — two new cases: `bench_experiments_reclaims_target_dir_by_default`
  (asserts `target/` is wiped, `cargo-clean.log` lands, copied binary
  survives) and
  `bench_experiments_skip_cargo_clean_preserves_target_dir` (asserts
  `target/release/stacks-bench` survives, no `cargo-clean.log`
  fingerprint — the gate suppresses the call, not just the disk wipe).
  The shared `StubCargo` was tightened to actually wipe `target/` on
  `clean()` so the positive assertion is meaningful; existing tests
  unaffected because they all pass `skip_cargo_clean: true`.

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
    preflight, before Phase 0). The file is cleared by a `Drop`-guard on
    every exit path the runtime unwinds through — normal return after Phase
    6, `?` bail, and unwinding panics. SIGINT (Ctrl-C) and SIGKILL terminate
    without unwinding (sbagent installs no signal handler today) and leave
    the file behind. `workspace prune` treats the file as authoritative only
    when the PID is actually live (POSIX `kill -0`); stale PIDs (process
    gone) fall through to the normal age + archive filters, so they cannot
    make a workspace immortal.
  - `--archived-only` is the safe default flag: requires presence in
    `sessions.jsonl`. Without it, the command refuses to remove anything
    unless `--older-than` is also explicitly provided.
  - Default to `--dry-run` if neither filter is set, to keep the destructive
    path explicit.
  - Operator-readable footer: bytes that would be freed, even in dry-run.
- Preflight: add a `check_free_disk` step inside the existing session preflight
  ([planning/archive/completed/0013-preflight-v1.md](0013-preflight-v1.md)
  context). Warn-only by default (default `preflight.min_free_gib = None`);
  the operator opts into a hard-fail by setting the config. When set and
  violated, the error body includes the exact suggested
  `workspace prune --older-than 7d --archived-only` invocation.
- Document under [docs/operations.md](../../docs/operations.md) "recovery"
  section and [docs/configuration.md](../../docs/configuration.md) preflight
  stanza.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `sbagent workspace prune --dry-run` lists candidates without removing
      anything; `--older-than 7d --archived-only` removes only sessions whose
      archive entry exists and whose age threshold is met.
- [x] A session whose `.run.pid` matches a live process is never prunable;
      a session whose `.run.pid` is stale (PID gone) falls through to the
      normal age + archive filters.
- [x] A session-run with `preflight.min_free_gib` set and violated fails
      preflight with the exact suggested prune invocation in the error body.
      Without `preflight.min_free_gib`, low free space emits a warning only.

**Tests:**

- [session::workspace::tests](../../crates/stacks-bench-agent/src/session/workspace.rs)
  — 7 cases: parse_duration suffix coverage, no-filter protects every
  session, live PID blocks even with filters set, stale PID falls
  through to age + archive filters, archived-only requires ledger
  match, missing sessions_root is a no-op, dry-run never removes.
- [session::run_pid::tests](../../crates/stacks-bench-agent/src/session/run_pid.rs)
  — 6 cases: write/read round-trip, idempotent clear, missing-file
  read, garbled content, `is_live` for current process, `RunPidGuard`
  RAII drop semantics.
- [session::preflight::tests](../../crates/stacks-bench-agent/src/session/preflight.rs)
  — 6 new cases on top of the existing 7: above warn floor (no
  finding), below default floor unconfigured (Warn), below configured
  floor (Fail), above configured floor (no finding), probe failure
  (Warn), missing workspace root (skipped).

**Notes:**

- Defer choosing a default `min_free_gib` to a follow-up — first run a
  real session, watch peak usage, then pick a floor. Until then, the
  operator opts in.
- Phase 5 publish is unaffected: `.run.pid` lives at
  `<sessions_root>/<id>/.run.pid` (next to `results/` + `worktrees/`),
  not inside the worktree path Phase 5's PR-writer + `git push` use.
- `workspace prune` resolves the operator ledger at
  `<layout.operator_repo_root>/sessions.jsonl`. When
  `operator_repo_root` isn't configured, `--archived-only` treats the
  archived set as empty — every candidate is kept under that flag.
  This matches the existing layout-derivation contract (no implicit
  ledger discovery).

## Final Validation

In-process / unit (complete, code-side):

- [x] Every phase clean command targets the full Pass 1c artifact tree it
      owns; `find $WORKSPACE/sessions/<id>/results -type f` after running
      every clean command in order leaves only the durable archive bundle.
      Covered by `bench_clean` / `optimize_clean` / `triage_clean` /
      `analyze_results_clean` test files plus the existing per-phase
      clean modules.
- [x] `sbagent prompt lint` rejects a hand-broken schema example and
      accepts the current template set unchanged. Pinned by
      `bundled_templates_lint_clean` plus the schema-mismatch /
      unknown-schema / dangling-marker / unparseable-JSON cases under
      `prompts::tests`.

Live / operator:

- [x] A serial three-target session reclaims each per-target `target/`
      build cache by default while preserving every worktree's source +
      `.git/` + copied binary through Phase 5 publish.
- [x] `sbagent workspace prune --dry-run` and the disk preflight fire
      against the workspace left behind by that smoke session. Confirms
      the prune candidate enumeration, `--archived-only` ledger lookup,
      `.run.pid` liveness gating, and `preflight.min_free_gib` warn-only
      default behave on real operator paths and not just on `tempdir()`
      seams.
      Validated with
      `sbagent -c ~/.config/sbagent/config.toml workspace prune --dry-run --archived-only`,
      which reported session `20260611-172955` as one dry-run prunable
      archived session.

v2 is shipped.

## Non-Goals

- Parallel optimizer fan-out or shared checkout pools.
- Cross-session memory or PR-state reconciliation
  ([0028-optimizer-memory](../../backlog.md#0028-optimizer-memory),
  [0033-maintain-command](0033-maintain-command.md)).
- Schema or agent-prompt prose rewrites driven by the live smoke
  ([0019-prompt-hardening-live-smoke](v8-smoke-informed-prompt-hardening.md)).

## Follow-Ups

- `0021-preflight-v2` was later closed as superseded by v3's per-session source
  clone.
- `0026-phase-timing` landed in
  [v5: Archive Metadata](v5-archive-metadata.md).
- `0038-prompt-example-concretization` later concretized the `"..."` placeholder
  values in `analyzer.md`'s two output examples and shipped in v7.
