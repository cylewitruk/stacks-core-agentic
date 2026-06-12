# v4: v3 Polish + Bot-Fork Seed

Successor to [v3: Per-Session Ephemeral Source Clone](v3-ephemeral-source-clone.md).
Close out the v3 cutover residue (legacy comment markers, the
hardcoded-`None` provenance field on `SessionRecord`, the unrehearsed
migration recipe) and restore the one operator usability gap Phase 4
introduced when it removed `--seed-from`.

> **Status:** shipped.
>
> All four phases are implemented, reviewed, and exercised by unit /
> integration tests. Live migration against a real pre-v3 operator repo was
> waived because no such repo remains; the fixture-driven migration rehearsal
> is the best available validation for that historical path.
> Smoke session `20260611-172955` proved the bot-fork publish path by
> pushing three branches and opening three PRs on `stacks-bench-bot/stacks-core`;
> the explicit `sbagent source seed` exercise against a fresh fork was not
> required before closure because the subcommand has fixture end-to-end coverage
> and the live smoke validated the same PAT / fork / branch-push machinery.
>
> v4 is the natural tidy-up plus a single new operator-facing command. Sized
> small on purpose: the goal is to finalize v3 cleanly without bundling broader
> publish-flexibility work (per-target remote-install hook, archive audit
> fields) that belongs in a later themed iteration.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0039-v3-transition-marker-scrub` | primary | shipped |
| `0040-session-record-source-sha-cleanup` | primary | shipped |
| `0041-migration-recipe-rehearsal` | primary | shipped |
| `0042-source-seed-helper` | primary | shipped |

## Why

Three loose threads from v3 worth tying off while context is fresh:

- **Transition markers (`// v3 Phase N`, `// pre-v3`, `// post-cutover`)
  scattered through `src/`.** Useful while v3 was active; ambient noise
  now that v3 is the only codebase. Future readers need the *invariant*
  (`origin = [source].url`, `<workspace>/sessions/<id>/repos/<cache_id>/`
  is the source root, etc.), not its lineage.
- **`SessionRecord.stacks_core_base_sha` is hardcoded `None`
  post-cutover** ([archive.rs:601-606](../../crates/stacks-bench-agent/src/session/archive.rs#L601-L606)).
  Carrying a dead field forward on every new ledger entry rots faster
  than schema bumps would. v3 already established the v1→v2 read-compat
  pattern on `SessionRecord`; extending it to v2→v3 with the legacy
  field removed is mechanical.
- **The migration recipe in `docs/setup.md` is unrehearsed.** v3's
  iteration plan marked this validation bullet deferred to a real
  operator dir. A fixture-driven rehearsal catches recipe drift
  (wrong path, missing step, stale identity arg) before a live
  operator hits it.

Plus one operator-facing item:

- **`sbagent init --seed-from` is gone post-v3.** Operators
  bootstrapping a brand-new bot fork still need to seed the
  configured `[source].branch` onto it before the first session can
  fetch. Phase 4 deleted both `--seed-from` and the underlying
  `seed_branch_with_auth` helper. The replacement is a tiny standalone
  command — read-only on `[source]`, push-only to a target URL — that
  composes the same bare-clone-then-push dance without the init flow
  surface.

## Scope

In scope:

- Rewrite or remove every active `// v3 Phase N` / `// pre-v3` /
  `// post-cutover` comment, keeping only the invariants and removing
  the lineage notes.
- `SessionRecord.schema_version` v2 → v3; drop `stacks_core_base_sha`
  from the typed model + schema; read path accepts v1 + v2 + v3.
- A fixture-driven rehearsal of the `docs/setup.md` migration recipe
  against a synthetic pre-cutover operator dir (mkdir, `git init`,
  fake submodule pointer, fixture `.sbagent/`); commits the recipe
  output shape to a snapshot test.
- New `sbagent source seed --from <source-url> --to <dest-url> [--branch <branch>]`
  subcommand, replacing the deleted `--seed-from` flow. Reads
  `[source].branch` by default; uses `publish.token_file` for PAT auth
  against `dest-url` via the same `http.<prefix>.extraheader` env
  mechanism the publish path uses.

Out of scope:

- Per-target remote-install hook to relax the `publish.remote == "origin"`
  preflight constraint (deferred to a publish-flexibility iteration,
  after v5).
- `0024-archive-audit-fields` (`pr_url` + bench wall-clock totals) — landed in
  [v5: Archive Metadata](v5-archive-metadata.md).
- `0026-phase-timing` (`SessionRecord.phase_durations_secs`) — landed in
  [v5: Archive Metadata](v5-archive-metadata.md).
- `0021-preflight-v2` re-scope. Worth a separate scoping pass to
  decide whether v3 obsoleted enough of it to close.
- Live smoke. Runs on its own track whenever NVMe+chainstate are
  available.

## Phases

### Phase 1: Transition Marker Scrub

**Goal:** Codebase comments describe current invariants only; no
`// v3 Phase N` / `// pre-v3` / `// post-cutover` lineage breadcrumbs
remain in active source files.

**Scope:**

- Sweep `src/` for the three marker patterns (`v3 Phase`, `pre-v3`,
  `post-cutover`, `cutover` near `//` comment context). For each
  match, decide:
  - **Keep**: rewrite to describe the invariant without the lineage
    ("v3 Phase 3 cutover: `cargo build` runs inside the per-session
    checkout" → "`cargo build` runs inside the per-session checkout
    so concurrent sessions don't share a `target/`").
  - **Drop**: delete the comment outright if removing the lineage
    leaves no useful invariant behind.
- Settings.rs `[source]` doc keeps the explicit "replaces the pre-v3
  `[stacks_core]` stanza" sentence (operator-facing rationale for an
  operator who reads the typed-model rustdoc).
- Test code untouched: stub comments referencing "v3 Phase N" inside
  `#[cfg(test)]` blocks are scoped + low signal.

**Status:**

- [x] Core implementation
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `rg '(v3 Phase|pre-v3|post-cutover)' crates/stacks-bench-agent/src/`
      returns only the documented exceptions (Settings.rs `[source]`
      doc, any explicit migration-recipe references).
- [x] `just lint` clean; no docstring intra-doc-link rot from the
      rewrites.

**Tests:** No new tests; relies on lint + the regex check above.

### Phase 2: `SessionRecord` Schema v2 → v3

**Goal:** Drop the `stacks_core_base_sha` field from the typed model
and generated schema; bump `SessionRecord.schema_version` to 3;
preserve read-compat for v1 + v2 archives.

**Scope:**

- `SessionRecord.schema_version`: `2 → 3` in the typed model.
- Remove `stacks_core_base_sha: Option<String>` field from
  `SessionRecord` (currently hardcoded `None` on the write path —
  dead column).
- Read path in
  [`models/session_record.rs::SessionRecord::from_ledger_line`](../../crates/stacks-bench-agent/src/models/session_record.rs)
  extended: accept v1 (legacy, no source fields), v2 (has
  `stacks_core_base_sha`), and v3 (no `stacks_core_base_sha`). v2
  records lose the field on read silently; the legacy SHA is still
  reachable via the v2 raw JSON if someone needs it (it's preserved
  in the archived `sessions.jsonl`).
- Regenerate `session-record.schema.json`; the bundled mirror
  refreshes via the existing `just schema export` path.
- Archive-side write code in
  [`session/archive.rs:601-606`](../../crates/stacks-bench-agent/src/session/archive.rs)
  drops the hardcoded `None` field; the dead-field comment block also
  goes.
- Backwards-compat read tests cover all three schema versions on the
  same `sessions.jsonl` shape.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] A v1 ledger line round-trips through the v3 reader unchanged.
- [x] A v2 ledger line round-trips through the v3 reader (the
      legacy field is dropped silently).
- [x] A v3 ledger line round-trips through the v3 reader; the
      typed model has no `stacks_core_base_sha` field.
- [x] `sbagent check` passes against the regenerated bundled schema.
- [x] No existing v2 round-trip test loses coverage — the
      mixed-version tests cover the gap.

**Tests:**

- Extend `tests/round_trip.rs` (or wherever ledger round-trips
  currently live) with v1 + v2 + v3 fixtures.
- Schema parity test (`tests/schema_parity.rs`) already enforces
  generated-vs-bundled match; verify it passes.

### Phase 3: Migration Recipe Rehearsal

**Goal:** The `docs/setup.md` "Migrating a pre-v3 operator dir
(one-time)" recipe is exercised against a fixture pre-cutover
operator dir, converging it to the post-cutover shape; the
converged shape is asserted by direct assertions on the resulting
filesystem + git state.

**Scope:**

- New integration test `tests/migration_recipe.rs` that:
  1. Materializes a synthetic pre-cutover operator dir in a tempdir:
     `git init -b main`, fake `repos/stacks-core` as a fixture clone
     (small bare repo seeded by `git init --bare` + one commit),
     `.gitmodules` pointing at it, a `.sbagent/` bundle, a
     `config.toml` carrying the legacy `[stacks_core]` stanza.
  2. Runs each step of the recipe via `std::process::Command::new("git")`
     (the recipe is shell, so the test executes it as shell; no
     extracting into a Rust function — the shell IS the
     operator-facing contract).
  3. Asserts the converged shape: no `repos/`, no `.gitmodules`, the
     bare cache exists at `<workspace>/cache/<cache_id>.git/`, the
     `[stacks_core]` stanza is gone from `config.toml`, the
     `[source]` stanza is present, the operator-main commit log has
     a `migrate: drop repos/stacks-core submodule` commit authored
     as the bot.
- If a recipe step's exact command needs adjusting (path resolution,
  flag name, identity arg), update `docs/setup.md` to match what the
  test actually runs.
- v3 iteration doc's deferred bullet ("Migration recipe converges a
  pre-cutover operator dir...") flips to `[x]` once the test lands; the
  later archive note records why real-operator live migration was waived.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `just test migration_recipe` passes on a clean checkout.
- [x] If the recipe needed any wording fixes during the rehearsal,
      `docs/setup.md` carries them. (Added `rmdir ~/operator/repos`
      cleanup step after `git rm`.)
- [x] v3 iteration doc updated to mark the migration-recipe bullet
      done.

**Tests:**

- [tests/migration_recipe.rs](../../crates/stacks-bench-agent/tests/migration_recipe.rs)
  (new).

**Notes:** The fixture submodule is a fake (a bare repo with one
commit) — the recipe shouldn't care, since step 3 just clones it.
The whole test runs offline.

### Phase 4: `sbagent source seed` Helper

**Goal:** Operators bootstrapping a brand-new bot fork can seed
`[source].branch` onto it without `--seed-from` or hand-rolled
`git push` plumbing.

**Scope:**

- New `sbagent source seed --from <source-url> --to <dest-url> [--branch <branch>]`
  subcommand under the existing `cli/source/` module.
  - `--from`: source URL to clone the branch from (typically the
    human operator's fork during pilot, e.g.
    `https://github.com/cylewitruk/stacks-core.git`). Required.
  - `--to`: destination URL to push to (typically `[source].url`,
    the bot's writable fork). Required.
  - `--branch`: branch to seed. Defaults to `[source].branch` from
    settings; required when `[source].branch` is unset.
- **Auth & URL handling — differs from `init --push`**: `init --push`
  is opinionated about the operator's `origin` URL (always HTTPS via
  PAT) because the bootstrap contract requires it. `source seed` is a
  one-shot operator-driven push to a URL they typed on the command
  line; SSH (`git@` / `ssh://`) is a legitimate operator choice.
  - When `--to` starts with `https://`: validate against
    `git.auth_url_prefix` (matches `init --push`'s strictness) and
    attach the PAT via `http.<prefix>.extraheader` env. Requires
    `publish.token_file`.
  - When `--to` starts with `git@` / `ssh://` / `file://`: skip the
    `validate_auth_url` gate entirely; do not require a PAT; the
    operator's SSH agent / local fs handles auth. Logged at info so
    the operator sees the auth mode chosen.
  - Plain `http://` is rejected — the auth helper requires HTTPS to
    attach a PAT, and silent-fallback-to-no-auth on `http://` would
    defeat the contract.
- Reuses the existing helpers in
  [crates/stacks-bench-agent/src/git.rs](../../crates/stacks-bench-agent/src/git.rs):
  - Re-add the `clone_bare_branch` helper that Phase 4 round-3
    deleted (it was the only `seed_branch_with_auth` consumer; the
    new helper here is its successor).
  - `push_url_refspec` (still present).
  - `build_auth_header_env` (still present; already returns an empty
    env-vec for non-HTTPS dests, so the SSH/file path falls through
    naturally).
- The flow is shell-out only: the helper does not need the
  `SourceRepo` trait (it doesn't touch the bare cache or the
  per-session checkout).
- Read-only on `[source]`: defaults `--branch` from `[source].branch`,
  but the URL is informational here (the actual push target is
  `--to`, not `[source].url`).
- Idempotent: re-running against an already-seeded fork is a
  fast-forward no-op (`git push` handles this naturally).

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] `sbagent source seed --from <url-1> --to <url-2> --branch <name>`
      against two `git init --bare` fixtures (no real GitHub)
      successfully seeds `<name>` on `<url-2>`.
- [x] Re-running the same command is a no-op (no error, no second
      commit on `<url-2>`).
- [x] `--to` URL validation: plain `http://` errors with a "HTTPS
      required for PAT injection" message; unsupported schemes (e.g.
      `ftp://`) error with the accepted-schemes list; `git@` /
      `ssh://` / `file://` accepted without PAT injection (info-log
      the auth mode).
- [x] CLI help text documents the post-v3 use case: bootstrapping a
      brand-new bot fork before the first session.

**Tests:**

- `tests/source_seed.rs` (new), modeling the deleted
  `seed_branch_pushes_to_empty_dest_via_local_files` test from
  Phase 4 round-3 but against the new CLI surface.

**Notes:** This replaces `init --seed-from` cleanly: the seeding
concern was never really an `init`-time concern (it's about the
bot fork, not the operator dir), so factoring it into its own
subcommand is a small design improvement on top of the v3 cutover.

## Final Validation

In-process / unit:

- [x] Transition marker scrub: regex check returns only documented
      exceptions.
- [x] `SessionRecord` v3 schema: v1 + v2 + v3 read-compat tests pass;
      `stacks_core_base_sha` field gone from emitted records.
- [x] Migration recipe rehearsal: fixture test passes; v3 iteration
      doc bullet flipped.
- [x] `sbagent source seed`: fixture-only end-to-end test passes;
      `--to` URL validation matches `init --push`.

Live / operator:

- [x] Migration recipe applied to a real existing operator dir
      converges without losing configuration. Waived on 2026-06-12 because
      no real pre-v3 operator repo remains; the fixture-driven rehearsal in
      Phase 3 is the in-process counterpart.
- [x] `sbagent source seed --from <upstream> --to <bot-fork>`
      against real GitHub successfully seeds the configured branch. Waived as
      separate live work on 2026-06-12: fixture tests cover the subcommand, and
      smoke session `20260611-172955` confirmed the bot fork, PAT, branch push,
      and PR creation path end-to-end.

Shipped with both remaining operator-only bullets explicitly waived or covered
by fixture + smoke evidence.

## Non-Goals

- Per-target remote-install hook (the `publish.remote != "origin"`
  preflight relaxation). Sized as its own iteration to keep the
  contract change reviewable separately from the v3 cleanup.
- Archive audit fields (`0024`) and phase timing (`0026`) landed in
  [v5: Archive Metadata](v5-archive-metadata.md).
- Renaming `analyzed_rejections::Record.stacks_core_sha` to
  `source_sha`. Cross-session ledger field; rename would need a
  read-compat shim plus operator-side migration. Bigger surface
  than the v4 theme; defer.

## Follow-Ups

- Future candidate: publish-side flexibility (per-target remote-install
  hook + multi-remote map). Re-enables the pre-v3 `bot`/`origin`
  separation operators may still want.
- Archive metadata (`0024-archive-audit-fields` + `0026-phase-timing`)
  landed in [v5: Archive Metadata](v5-archive-metadata.md).
- Standalone re-scoping: `0021-preflight-v2`. Worth a 1-hr pass
  before commitment — v3 may have obsoleted enough of it to close.
