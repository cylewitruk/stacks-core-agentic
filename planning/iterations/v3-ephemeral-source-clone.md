# v3: Per-Session Ephemeral Source Clone

Successor to [v2: Cleanup And Workspace Hygiene](v2-cleanup-and-workspace-hygiene.md).
Replace the shared `<operator>/repos/stacks-core` submodule with per-session
source checkouts under `<workspace>`, backed by a shared bare object cache,
and record explicit source provenance in every session bulk.

> **Status:** planned.
>
> Decision is committed
> ([Decision 0003](../decisions/0003-ephemeral-source-clone.md)). The four
> drift modes that motivate the change are documented there, three of which
> are empirically confirmed (the cargo-build cross-session interference Codex
> caught during the v2 doc pass is one of them). This iteration is the
> implementation.

## Items

<a id="0022-ephemeral-source-clone"></a>

| Item | Role | Status |
| ---- | ---- | ------ |
| `0022-ephemeral-source-clone` | primary | planned |

## Why

The shared submodule at `<operator>/repos/stacks-core/` is the source of four
drift modes
([Decision 0003 §Rationale](../decisions/0003-ephemeral-source-clone.md#rationale)):

- **SHA staleness** — operators who `git submodule update --remote` without
  committing see different state across sessions in the same operator-main
  shape.
- **Detached-HEAD vs branch-ref divergence** — per-target optimizer clones
  fork via `--branch <base_branch>` (a ref); Phase 0a samples
  `git rev-parse HEAD` (a SHA). Manual `git checkout` inside `<base>` between
  sessions desyncs ref and HEAD invisibly.
- **Cross-session interference** — Phase 0a's
  `cargo build --release -p stacks-bench` writes into `<base>/target/`
  ([baseline.rs:65](../../crates/stacks-bench-agent/src/session/baseline.rs#L65)),
  the shared submodule filesystem. Concurrent sessions cannot share `<base>`
  safely.
- **Implicit provenance** — `session/<id>` archive branches identify the
  source SHA only via `baseline/manifest.json`. Confirming the operator-main
  submodule pointer at session-time requires consulting operator git history,
  which is fragile after subsequent commits.

A per-session ephemeral clone backed by a shared bare cache eliminates all
four without losing the disk-economy benefit of object-store sharing.

## Config contract (pinned before Phase 1)

Introduce a new `[source]` stanza separating "the upstream repo we're
optimizing" from `[stacks_core]` (which today carries both the submodule
path and the URL) and from `[publish]` (which carries the *target* base
branch PRs file against, conceptually distinct from the source branch
sessions clone from):

```toml
[source]
# Clone URL. Required.
url = "https://github.com/stacks-network/stacks-core.git"
# Branch sessions fetch + clone. Required.
branch = "feat/stacks-bench"
# Optional stable id for the bare cache dir naming. When unset, derived
# from a sanitized + hashed canonical URL (see Phase 1). Setting this
# explicitly is useful when you want a human-readable cache path.
id = "stacks-core-feat-stacks-bench"
```

`source.id` becomes a path segment in two places —
`<workspace>/cache/<source.id>.git/` and
`<workspace>/sessions/<id>/repos/<source.id>/` — so it must validate
against `^[a-z](?:[a-z0-9-]{0,62}[a-z0-9])?$` (lowercase ASCII slug,
leading letter, optional middle of `[a-z0-9-]`, mandatory trailing
`[a-z0-9]`, ≤64 chars total). The stricter shape forbids trailing
hyphens — path slugs stay clean. Settings parsing rejects any other
value at config-load time; preflight re-validates as defense-in-depth.
Rejects: `../escape`, `foo/bar`, `Foo`, `1leading`, `trailing-`,
empty string, anything over 64 chars. Accepts:
`stacks-core-feat-stacks-bench`, `my-fork`, `s` (degenerate but valid).

Migration of the existing fields:

- `stacks_core.base` (submodule path) — **removed**. Sessions resolve
  the source checkout from `[source]` at start.
- `stacks_core.base_repo_url` — **removed**, superseded by `source.url`.
- `[stacks_core]` stanza itself — **removed** in Phase 4. The new
  `[source]` stanza is the single source of truth for the upstream repo.
- `publish.base_branch` — **kept**. PRs target this branch (today
  `feat/stacks-bench`, same as source.branch by convention; semantically
  distinct). Keeping it separate means a future operator could optimize
  one branch and publish PRs against a different review branch.

Post-cutover, `source.url` and `source.branch` are required for any phase
that needs source. A session run with `[source]` unset fails fast at
preflight with a remediation pointer to the migration recipe (Phase 4) —
there is no transitional fallback to the submodule (see Phase 3 Notes).

## Scope

In scope:

- New `<workspace>/cache/<base>.git/` shared bare object cache.
- New `<workspace>/sessions/<id>/repos/<base>/` per-session source checkout.
- `source.json` v1 provenance contract written at session start; fields flow
  into `summary.json` and `SessionRecord`.
- Cutover for every consumer of `<base>` today (Phase 0a baseline binary
  build, Phase 2 per-target optimizer clones forking via `--reference`,
  Phase 1.8 calibration, finalize/archive provenance fields).
- Migration recipe + one-shot operator-side script for dropping the
  submodule from existing operator repos.
- Preflight check for source reachability + cache state.
- `sbagent init` simplification (no `git submodule add`, no `.gitmodules`).

Out of scope:

- Multi-operator shared cache (Decision 0003 Open Question — deferred).
- Schema or agent-prompt prose changes beyond the new
  `source.json` field set propagating into existing schema definitions for
  `summary.json` + `SessionRecord` (mechanical additions only).
- Replacing `git clone --reference --local` with a different sharing
  primitive — the bare cache + `--reference` pattern is what makes the disk
  economy work.
- Changing the `agentic/<id>/<target>` per-target branch flow (Phase 2 still
  creates per-target clones; the only change is what they fork *from*).
- Removing the existing submodule from already-archived `session/<id>`
  branches — those are write-once.

## Phases

### Phase 1: Bare Cache + Per-Session Source Clone Primitives

**Goal:** Land the new filesystem layer + clone primitives behind a
trait-injected seam, with no callsite changes yet. Tests exercise both the
fresh-cache path and the warm-cache path against synthetic remotes.

**Scope:**

- Define `SourceRepo` trait carrying the operations needed for source
  materialization: `ensure_cache`, `clone_session_checkout`, `record_sha`,
  `prune_session_checkout`. Real impl shells out to `git`; stub for tests
  records calls + simulates `--reference --local` behavior.
- Bare cache lives at
  `<layout.agent_workspace_root>/cache/<cache_id>.git/`. `<cache_id>`
  resolution:
  - **`source.id` set** — use it verbatim (operator picks a
    human-readable name).
  - **`source.id` unset** — derive from a canonicalized URL: lowercase
    host, owner, repo with `.git` suffix stripped; sanitize non-`[a-z0-9-]`
    chars to `-`; append `-<first-8-hex-of-sha256(canonical_url)>` to
    disambiguate against same-name forks. Example:
    `https://github.com/stacks-network/stacks-core.git` →
    `github-com-stacks-network-stacks-core-3a7f2b91`.
  - **Two different remotes named `stacks-core.git` cannot collide** —
    the suffix hash differentiates `stacks-network/stacks-core` from any
    `cylewitruk/stacks-core` fork even when an operator forgets to set
    `source.id`.
- First-touch initializes via `git clone --bare <source.url> <cache>` +
  fetches `source.branch`.
- Subsequent-touch refreshes via `git fetch <source.url> <source.branch>`
  against the cache. Cache is single-operator; v1 doesn't coordinate
  multi-operator access (Decision 0003 Open Question — Non-Goals).
- Per-session checkout at
  `<layout.agent_workspace_root>/sessions/<id>/repos/<cache_id>/`.
  Materialized via
  `git clone --reference <cache> --branch <source.branch> --local <cache> <session_checkout>`.
  Records the resolved HEAD SHA back to caller for the `source.json` write.
- **Lock scope** covers the **entire materialization window**, not just
  the fetch step. File lock on `<cache>/.materialize.lock`
  (`fd-lock`-held) is acquired before `fetch`, released after the
  caller's `source.json` write completes. Concurrent sessions starting
  against the same cache then serialize end-to-end: fetch → resolve
  branch tip SHA → clone session checkout → write source.json. Without
  this scope, two sessions could fetch concurrently, resolve different
  SHAs against a moving ref between fetch and clone, and record
  inconsistent provenance.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [ ] Validated (live smoke deferred to v1 Pass 1c re-run)

**Acceptance & Validation:**

- [ ] Fresh-cache path: `<cache>` materializes on first call;
      `<session_checkout>` resolves to the configured branch's tip SHA.
- [ ] Warm-cache path: second call against the same cache fetches the
      configured branch (no re-clone) and produces a fresh
      `<session_checkout>` for a different session id.
- [ ] Two concurrent session starts serialize on the cache lock — no race
      corruption of the fetch.
- [ ] Per-session checkout is independent: removing it does not touch the
      cache; removing the cache does not touch any session checkout (only
      future ones).
- [ ] `source.id` validation: settings parsing rejects every entry in the
      rejection list above with a clear error citing the regex; preflight
      re-validates and produces the same rejection for the same inputs.

**Tests:**

- `source_clone_tests` — bare cache bootstrap + fetch idempotence + concurrent
  lock contention against a stub `SourceRepo`.
- `source_id_validation_tests` — table-driven against the accept/reject
  list named above (`../escape`, `foo/bar`, `Foo`, `1leading`, `trailing-`,
  empty, 65-char string, valid slugs). Settings-side rejection (parses to
  `Err`) and preflight-side rejection (produces a `Severity::Fail`
  finding) both covered.
- Integration test against a local bare repo seeded by `git init --bare` +
  one commit, to confirm the production `--reference --local` path works
  end-to-end without an external network call.

### Phase 2: `source.json` Provenance Contract

**Goal:** Schema-validated source provenance committed to the session
bulk, propagated into the archive ledger and summary, written exactly
once per session at start, never mutated.

**Scope:**

- New `source.json` v1 schema:

  ```json
  {
    "schema_version": 1,
    "url": "https://...",
    "branch": "feat/stacks-bench",
    "sha": "abc...",
    "fetched_at": "2026-06-07T12:00:00Z"
  }
  ```

  Typed model + generated JSON Schema, bundled into the operator's
  `.sbagent/schemas/` on next sync.
- Path:
  `<workspace>/sessions/<id>/results/source.json`. Sits at the results-tree
  root, not inside any per-phase dir, because every phase reads from the
  same source state.
- Phase 0 (session start) is the sole writer. Read at every subsequent
  phase — fail loudly on missing-or-mutated.
- New fields on `Summary` + `SessionRecord`: `source_url`, `source_branch`,
  `source_sha`, `source_fetched_at`. Both schemas bump:
  - **`summary.schema.json` v3 → v4** — `Summary.schema_version: 3 → 4`
    in the typed model; schemars export refreshes the bundled schema.
  - **`session-record.schema.json` v1 → v2** —
    `SessionRecord.schema_version: 1 → 2`. The v2 fields are part of the
    durable provenance contract; new writes emit v2.
  - **Backwards-compat read path:** `sessions.jsonl` may still carry
    pre-cutover v1 records. The reader accepts both v1 (legacy: source
    fields absent) and v2 (new: source fields present); the v2 writer is
    the only emitter post-cutover.
- Archive (Phase 6) copies `results/source.json` into the
  `session/<id>` branch as part of the standard bulk copy — no special
  handling.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [ ] Validated (live smoke deferred to v1 Pass 1c re-run)

**Acceptance & Validation:**

- [ ] `source.json` is written once at session start; subsequent phases
      read it and fail-loud on missing.
- [ ] Schema validation passes for every emitted `source.json`.
- [ ] `summary.json` carries the four source fields; matching round-trip
      tests.
- [ ] `SessionRecord` (the ledger line) carries the same four fields;
      `sessions.jsonl` round-trip tests.

**Tests:**

- `source_provenance_tests` — write/read round-trip, missing-file
  failure, mutated-file failure, schema mismatch failure.
- Extend existing `archive_tests` to confirm `source.json` lands in the
  `session/<id>` tree and the ledger `SessionRecord` carries the source
  fields.

### Phase 3: Consumer Cutover

**Goal:** Every existing caller of `<operator>/repos/stacks-core/` is
re-pointed at the per-session checkout. The operator submodule is still
present on disk (Phase 4 removes it) but is no longer read by any phase.

**Scope:**

- **Phase 0a baseline binary build** — switch the path passed to
  [`baseline::archive_baseline_binary`](../../crates/stacks-bench-agent/src/session/baseline.rs#L70)
  from `<operator>/repos/stacks-core/` to
  `<workspace>/sessions/<id>/repos/<base>/`. Cargo build now writes into
  the per-session checkout's `target/`, eliminating the cross-session
  `<base>/target/` pollution drift mode named in Decision 0003.
- **Phase 2 optimizer fan-out** — repoint
  [`StdGitCheckoutManager::recreate_checkout`'s
  `base` argument](../../crates/stacks-bench-agent/src/session/optimizers.rs#L110)
  from `<operator>/repos/stacks-core/` to
  `<workspace>/sessions/<id>/repos/<base>/`. Per-target clones now
  `--reference --local` against the session-pinned checkout — they
  start from exactly the SHA recorded in `source.json`, not whatever the
  submodule pointer happens to be.
- **Phase 1.8 calibration + Phase 3 candidate bench** — neither reads
  `<base>` directly today (both run against the bench binary copied to
  `exp_dir/bin/stacks-bench`). Mechanical confirm-no-regression.
- **Phase 4 finalize + Phase 6 archive** — provenance fields now sourced
  from `source.json` instead of sampling `git -C <base> rev-parse HEAD`
  at archive time.
- **No transitional fallback.** A pre-migration operator (no `[source]`
  stanza in config) fails fast at preflight with a remediation pointer
  to the migration recipe (Phase 4). Codex's review flagged that a
  fallback writing a legacy-derived `source.json` would muddy the
  fail-loud contract Phase 2 establishes; a clean break is simpler and
  matches the "one-time operator-side migration" intent of Decision 0003.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [ ] Validated (live smoke deferred to v1 Pass 1c re-run)

**Acceptance & Validation:**

- [ ] Phase 0a's `cargo build` writes into the per-session `target/`, NOT
      into `<operator>/repos/stacks-core/target/`. Confirmable with a
      fixture session + `find <base>/target -type f` (empty) +
      `find <session_checkout>/target -type f` (populated).
- [ ] Per-target optimizer clones forked via `--reference` against the
      session checkout still build cleanly and share object storage
      (verifiable via `git -C <target_clone> count-objects -v`).
- [ ] Finalize + archive read provenance from `source.json` and produce
      the same `summary.json` / `SessionRecord` SHAs as a control run
      against the legacy submodule path.
- [ ] Missing `[source]` config fails fast at preflight with the
      remediation pointer (no silent fallback, no legacy code path
      kept alive).

**Tests:**

- New `ephemeral_source_phase_0a_tests` confirming Phase 0a writes
  exclusively to the per-session checkout.
- Extend `optimizers` tests to confirm `--reference` correctly points at
  the per-session checkout.
- `source_config_required_tests` — preflight rejects missing `[source]`
  with the expected error body.

**Notes:** Phase 0a's `cargo build` warm-cache benefit moves from
"operator-shared across sessions" to "per-session only". This is an
intentional trade — disk economy via the bare cache for *git objects*
stays, but Cargo build artifacts no longer share across sessions. The
v2 `cargo clean` reclamation step (between binary copy and bench
invocations) already discards these mid-session, so the practical impact
is bounded to Phase 0a's initial build wall-clock.

### Phase 4: `sbagent init` Cutover + Operator Migration

**Goal:** Fresh operator repos no longer carry a submodule; existing
operators have a one-shot recipe to drop theirs. The `[stacks_core]`
stanza is removed; `[source]` is the single source of truth.

**Scope:**

- `sbagent init` no longer runs `git submodule add` or writes
  `.gitmodules`. Initial commit no longer carries a submodule pointer.
  The `[stacks_core]` stanza is removed from the bundled
  `assets/example.config.toml`; `[source]` is added.
- **Migration via documented shell recipe** in
  [docs/setup.md](../../docs/setup.md), not a CLI subcommand. Per Codex's
  judgment call: this is a one-time operator-only destructive change;
  keeping it explicit and inspectable (each step a visible `git`
  invocation the operator can stop at) is healthier than adding a CLI
  surface that may only ever run once. Recipe shape (verbatim in
  setup.md):

  ```bash
  # 1. Confirm clean state on operator main and inside the submodule.
  git -C <operator> status
  git -C <operator>/repos/stacks-core status

  # 2. Seed the bare cache from the existing submodule (fast — local).
  mkdir -p <workspace>/cache
  CACHE_ID="$(sbagent source cache-id)"   # prints the derived id (see Phase 1)
  git clone --bare --local <operator>/repos/stacks-core "<workspace>/cache/${CACHE_ID}.git"

  # 3. Remove the submodule from the operator.
  git -C <operator> submodule deinit -f repos/stacks-core
  git -C <operator> rm -rf repos/stacks-core
  rm -rf <operator>/.git/modules/repos/stacks-core
  rm -f <operator>/.gitmodules    # if no other submodules remain

  # 4. Commit the removal on operator main as the bot identity.
  git -C <operator> -c user.name="Stacks BenchBot" \
      -c user.email="<bot-email>" \
      commit -m "migrate: drop repos/stacks-core submodule (Decision 0003)"
  ```

  A small read-only helper `sbagent source cache-id` (prints the
  derived cache id given the configured `[source]`) is added to support
  step 2 — the deterministic naming scheme from Phase 1 isn't trivially
  reproducible by hand, but a CLI sub-subcommand that *prints* one
  string is cheap and operator-friendly.
- `docs/git-topology.md`, `docs/configuration.md`, `docs/setup.md`,
  `docs/operations.md` updated to reflect the post-cutover state. The
  §1 and §2 sections of `git-topology.md` (the submodule references)
  get a full rewrite.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [ ] Validated (live smoke deferred to v1 Pass 1c re-run)

**Acceptance & Validation:**

- [ ] `sbagent init` on a fresh dir produces no `.gitmodules`, no
      `repos/` subdir, and no submodule pointer in the initial commit.
- [ ] The migration recipe applied to a pre-cutover operator dir
      converges to the post-cutover shape without losing the operator's
      configured `source.url` / `source.branch` / `.sbagent/` bundle.
- [ ] A session run on a fresh post-cutover operator dir produces a
      `source.json` and completes Phases 0–4 without consulting any
      `<operator>/repos/` path.
- [ ] A session run with no `[source]` stanza fails fast at preflight
      with a remediation pointer to the migration recipe (no fallback
      to a submodule was ever provided — Phase 3 already pinned this).

**Tests:**

- `init_no_submodule_tests` confirming the post-cutover init shape.
- `source_cache_id_print_tests` exercising the `sbagent source cache-id`
  helper (deterministic, matches Phase 1's naming scheme).
- `source_config_required_preflight_tests` confirming missing `[source]`
  hard-fails preflight after the cutover.

The migration recipe itself is **manually validated** (it's a one-time
operator step). The recipe lives in `docs/setup.md` with a "ran this
against a synthetic operator dir, here's the resulting state" reference
appendix.

**Notes:** The migration recipe is operator-side; it doesn't touch any
archived `session/<id>` branches (those are write-once and continue to
carry their pre-cutover layout). A reader of an archived branch sees
`baseline/manifest.json` with the submodule-pointer SHA exactly as it
was at session-time; the new `source.json` only appears in
post-cutover archives.

## Final Validation

In-process / unit (each phase tests its own slice; rolled-up here):

- [x] Bare cache + per-session checkout: fresh + warm + concurrent paths
      (Phase 1 — `tests/source_clone.rs`).
- [x] `source.json` provenance: written once, validated, propagated to
      `summary.json` + `SessionRecord` (Phase 2).
- [x] Phase 0a + Phase 2 cutover: no writes to `<operator>/repos/<base>/`
      during a session (Phase 3 — the entire `<base>` field is gone).
- [x] `sbagent init` produces no submodule on fresh dirs (Phase 4 —
      `tests/init.rs::init_writes_no_gitmodules_and_no_submodule_pointer`).
- [ ] Migration recipe converges a pre-cutover operator dir to the
      post-cutover shape (recipe shipped in `docs/setup.md`; manual
      validation pending on a real operator repo).

Live / operator (deferred to a v3 smoke; folds into the rerun of the
[v1 live Pass 1c smoke](v1-live-pass-1c-smoke.md) post-cutover):

- [ ] One end-to-end session on a post-cutover operator dir, including
      Phase 5 publish. Confirms `source.json` flows all the way through to
      the operator's published PR body / archive branch / ledger entry.
- [ ] Operator-side migration recipe applied to a real existing operator
      repo without losing any configuration.

Code-side ships as of the Phase 4 commit. Move v3 to `shipped` once both
live bullets above are checked.

## Non-Goals

- Multi-operator shared bare cache (Decision 0003 Open Question — deferred
  until a real multi-operator deployment exists).
- Replacing the v2 `agentic/<id>/<target>` per-target branch shape
  ([0025-named-phases](../backlog.md#0025-named-phases) tracks broader
  naming work).
- Rewriting any already-archived `session/<id>` branches. Those carry
  their original layout; a reader needs to consult operator git history
  for sessions archived before the cutover.
- Source provenance for stacks-bench itself (the `sbagent_git_sha` field
  on `SessionRecord` already covers the binary's own provenance).
- Cross-session source-state sharing beyond the bare cache (per-session
  isolation is the entire point of this iteration).

## Follow-Ups

- `0026-phase-timing` — once `source.json` is in place, session timing
  instrumentation can surface fetch-vs-clone-vs-checkout times as
  separate Phase 0 sub-steps for the operator dashboard.
- `0033-maintain-command` — the post-cutover `source.json` makes
  maintenance's PR-state reconciliation against a specific source SHA
  trivial (each PR points at the exact upstream commit it was authored
  against).
- Once v3 ships, `docs/git-topology.md` §1 (Initial install) and §2
  (Session created) need a pass to remove submodule references — that
  doc was authored mid-iteration assuming the submodule was permanent.
