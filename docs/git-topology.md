# Git topology

Where every file and every git branch lives across the sbagent
pipeline, from initial install to maintenance. This is the canonical
walkthrough — other docs ([workflow.md](workflow.md),
[session-archive.md](session-archive.md), [publishing.md](publishing.md),
[operations.md](operations.md)) cover their respective phases in
depth and link back here for "where does X live?" questions.

## Cast of characters

Three on-disk repos and one workspace are involved:

| Path variable | What it is | Why it exists where it is |
| ------------- | ---------- | ------------------------- |
| `<operator>` | Operator git repo (the dir `sbagent init` creates). Holds the `repos/stacks-core` submodule, the `.sbagent/` bundle, and the `sessions.jsonl` ledger on `main`. | Operator-owned source of truth for the *configuration* of an autonomous fleet. Lives where the operator chooses (e.g. a fork of [`stacks-bench-agentic-operator`](https://github.com/cylewitruk/stacks-bench-agentic-operator)). |
| `<operator>/repos/stacks-core/` | Submodule of the **target** repo to optimize (default: a fork of `stacks/stacks-core` checked out at `feat/stacks-bench`). | The *base* every per-target optimizer clone forks from. Pinned at a specific sha so every session in a given operator-repo state starts from the same baseline. |
| `<workspace>` | Mutable scratch root. `<layout.agent_workspace_root>` in config — **no Rust default**; set explicitly. Recommended value: `/private/tmp/sbagent-workspaces` (macOS) or `/var/tmp/sbagent-workspaces` (Linux). When unset, sbagent falls back to a legacy layout that anchors session bulk inside `<operator>` or the framework dir — discouraged because branch switches in `<operator>` then risk wiping tracked-but-ignored session files. | Heavy/mutable state (session bulk, per-target clones, archive worktrees) lives **outside** `<operator>` when this is set, so branch switches and `git status` in `<operator>` stay fast and uncluttered. |
| `<stacks-core fork>` | Remote GitHub repo where Phase 5 pushes the `agentic/<id>/<target>` branches and opens PRs. Default: `cylewitruk/stacks-core`. | Decouples the bot's per-target work from the upstream `stacks/stacks-core` repo so PRs go through a fork-then-PR flow that's standard for outside-contributor work. |

Path conventions used throughout this doc:

- `<id>` is a session id (e.g. `20260607-104400`).
- `<target>` is a merged-target id (e.g. `marf-trie-cache`).
- `<inv>` is a `verification_replay.invocations[].id`.
- `<base>` is the `<operator>/repos/stacks-core/` submodule path.

## 1. Initial install

When you run `sbagent init`.

### Local files / dirs created

```text
<operator>/
  .gitignore                            # ignores /sessions/, target/, etc.
  .gitmodules                           # records repos/stacks-core submodule
  .sbagent/
    prompts/                            # bundled MiniJinja templates seeded
    schemas/                            # bundled JSON Schemas seeded
    queries/                            # bundled SQL queries seeded
    context/                            # bundled reference docs seeded
  repos/
    stacks-core/                        # submodule checkout at the configured
                                        # base_branch tip; provides the base
                                        # every per-target optimizer clone
                                        # forks from
  memory/                               # lifted OUT of .sbagent/; holds
                                        # cross-session bot memory (today:
                                        # analyzed-rejections ledger)
```

Nothing is created under `<workspace>` yet — that materializes on the
first session.

### Git operations

In `<operator>`:

1. `git init -b <push_branch>` (if no `.git/` present; `push_branch`
   defaults to `main`).
2. `git submodule add <base_repo_url> repos/stacks-core -b <base_branch>`.
3. `git add` the seeded bundle + `.gitignore` + `.gitmodules` +
   submodule pointer.
4. One initial commit authored as `Stacks BenchBot`, no GPG signing.
5. `git push -u origin <push_branch>` only when `--push` is passed
   (PAT-via-env auth, no `.git/config` mutation).

After this, `<operator>` is on `main` (or the configured push branch),
clean, with the seeded bundle committed.

No branches exist anywhere except `main` in `<operator>` and the
`base_branch` ref inside the `repos/stacks-core/` submodule.

## 2. When a session is created

When `sbagent session run` starts a fresh `<id>`.

### Local files / dirs created

```text
<workspace>/sessions/<id>/              # session root — siblings:
  results/                              # phase outputs (canonical artifact tree)
    baseline/                           # Phase 0: run-id, rerun-id,
                                        #   bench-run.json, *.stderr.log,
                                        #   profiler-hotspots.json, bin/
    triage/                             # Phase 1: candidates.json, prompt,
                                        #   final-message, queries/, drilldowns/
    analysis/<family>/                  # Phase 1.5: per-family analyzer output
    merge/                              # Phase 1.7: optimization-targets.json
    verify/<target>/<inv>/              # Phase 1.8: per-invocation baseline
                                        #   calibration outputs
    optimize/<target>/                  # Phase 2 + 3 + 5 per-target outputs
                                        #   (added per target as they're processed)
    analyze/<target>/                   # Phase 3.5: results-analysis.json
    finalize/                           # Phase 4: summary.json, summary.md
  worktrees/                            # (legacy fallback if agent_workspace_root
                                        #  is unset — see §3)
  .run.pid                              # written after preflight passes; cleared
                                        # by RAII drop on normal exit / ? bail /
                                        # unwinding panics. SIGINT/SIGKILL leave
                                        # it behind (no signal handler).
```

The session dir is **workspace-rooted** when `agent_workspace_root` is
configured (the recommended layout). This is the key invariant:
heavy and mutable bulk stays out of `<operator>` so branch switches
there don't churn it. When `agent_workspace_root` is unset (legacy),
`sessions_root` falls back to a framework/cwd-relative
`sessions/` path — non-archive operations still work, but archive
itself refuses to run because the transient archive worktree must
live outside `<operator>`. See
[session-archive.md](session-archive.md) for the archive-side
requirement.

Phase 0a also writes `<workspace>/sessions/<id>/results/baseline/bin/stacks-bench`
— the *archived* baseline binary used by Phase 1.8 calibration and any
imported-baseline sessions. The same binary is *copied* (not moved) into
the per-target optimizer dirs in §3.

### Git operations

**None in `<operator>`.** A session start does not touch the operator
repo. The bot remembers a baseline sha by sampling
`git -C <base> rev-parse HEAD` for the [`SessionRecord`](../crates/stacks-bench-agent/src/models/session_record.rs)
that gets written at archive time (§5b), but no commit / no branch /
no push happens at session start.

**Git/source state in `<base>` (`<operator>/repos/stacks-core/`) is
read-only.** The submodule's working tree (source files), index, and
refs are never mutated by a running session. The base sha is recorded
in `baseline/manifest.json` so a later reader can trace which exact
upstream commit the session ran against.

**But the filesystem under `<base>` is NOT read-only.** Phase 0a
runs `cargo build --release -p stacks-bench` in `<base>` to produce
the baseline binary that gets archived under
`<workspace>/sessions/<id>/results/baseline/bin/stacks-bench`. That
build populates `<base>/target/release/` with normal Cargo build
artifacts. The Cargo state is operator-shared and persists across
sessions (warm-cache benefit on the next run); only the source state
is invariant. Per-target optimizer clones in §3 get their own
`target/` directories and run a separate build; those build caches
are reclaimed by the `cargo clean` step (§4).

## 3. When an optimization experiment begins

Phase 2: the optimizer agent gets dispatched for one merged target.

### Local files / dirs created

```text
<workspace>/optimizers/<id>/<target>/   # per-target git clone (NEW)
  .git/                                 # standalone clone — own git state,
                                        # not a worktree of <base>
  <all source files from base @ base_branch>
                                        # working tree at base_branch tip
  target/                               # cargo build output (created during
                                        # build step; reclaimed after Phase 3
                                        # bench by `cargo clean` — see Phase 3
                                        # of v2 iteration / operations.md)
```

When `layout.agent_workspace_root` is unset (legacy), this lands at
`<workspace>/sessions/<id>/worktrees/<target>/` instead — same
contents, different parent. The
[`session_optimizer_checkouts_dir`](../crates/stacks-bench-agent/src/layout.rs#L499)
helper picks the right root.

Inside `<operator>` and `<base>`: **nothing**. The clone is
self-contained.

The optimizer agent's prompt, events, and outputs land under
`<workspace>/sessions/<id>/results/optimize/<target>/`:

```text
<workspace>/sessions/<id>/results/optimize/<target>/
  prompt.md, events.jsonl, stderr.log,
  final-message.md, conversation-id, nextest.log
  (optimizer-report.json written by the agent on exit)
```

### Git operations

In a new `<workspace>/optimizers/<id>/<target>/`:

1. `git clone --reference <base> --branch <base_branch> --local <base> <checkout>`
   — shares `<base>`'s object store via `--reference --local`, so the
   new clone is tiny on disk. Per-target clones are independent git
   repos with their own refs; they are NOT linked worktrees of `<base>`.
2. `git switch -c agentic/<id>/<target>` — fresh branch, pointed at
   `base_branch`'s tip in the new clone. The branch lives ONLY inside
   this per-target clone until Phase 5 pushes it.
3. `git remote set-url origin <base>'s origin url>` — `--local` defaults
   `origin` to the local `<base>` path; we rewrite it to the operator's
   configured GitHub URL so Phase 5's `git push` targets GitHub.
4. Every remote that `<base>` had gets replicated. The
   `publish.remote` config picks which one Phase 5 pushes to (e.g.
   `origin` for the bot's own fork, or a separate `fork` remote).

**Crucial sandbox boundary**: the optimizer agent runs inside the
clone and *edits source files*, but it CANNOT commit. The Codex
`workspace-write` sandbox on macOS denies writes to `.git/` even when
`.git/` is inside the agent's cwd (Seatbelt + the
`com.apple.provenance` xattr enforce this). The agent leaves
modifications in the working tree at exit; the coordinator commits
them (or doesn't) after validating the agent's typed
`optimizer-report.json`. See
[optimizers.rs](../crates/stacks-bench-agent/src/session/optimizers.rs)
for the full agent/coordinator split.

In `<operator>`, `<base>`, and the GitHub fork: **nothing** at this
point. `agentic/<id>/<target>` exists only inside the per-target
clone's git database.

## 4. When an optimization experiment concludes

Two distinct outcomes.

### 4a. Implemented (agent emits `outcome: "implemented"`)

Local files / dirs:

```text
<workspace>/sessions/<id>/results/optimize/<target>/
  optimizer-report.json                 # agent-written, typed contract
  implementation.md                     # coordinator-rendered companion
  nextest.log, cargo-build.log,
  cargo-clean.log (default path)
```

`<workspace>/optimizers/<id>/<target>/` is unchanged in shape, but:

- `target/` is wiped by the per-worktree `cargo clean` step (between
  the binary copy and Phase 3 bench invocations). The build cache is
  disposable from this point on; the worktree still has `.git/` +
  source.
- `<workspace>/sessions/<id>/results/optimize/<target>/bin/stacks-bench`
  is the binary Phase 3 bench will run against.

Git operations:

1. **Coordinator** stages the agent's source modifications inside the
   per-target clone (`git add -u`).
2. **Coordinator** commits, authored as `Stacks BenchBot` (no GPG
   signing). This is the ONLY commit on `agentic/<id>/<target>`.
3. The commit lives only in the per-target clone's git database. No
   push, no operator-repo write yet.

Phase 3 then runs the bench against the copied binary (no git ops);
Phase 3.5 emits the results-analyzer verdict (no git ops); Phase 4
finalizes (no git ops).

### 4b. Aborted (agent emits `outcome: "aborted"` or never writes a report)

Local files / dirs:

```text
<workspace>/sessions/<id>/results/optimize/<target>/
  optimizer-report.json                 # OR absent (sandbox-killed)
  abort.md                              # coordinator-rendered if report
                                        #   exists with outcome=aborted
```

Git operations: **none**. The per-target clone stays at `base_branch`
tip (no coordinator commit), and at session end (§5) the aborted
clone is torn down by [`session_end_cleanup`](../crates/stacks-bench-agent/src/cli/session/run.rs#L611).

### 4c. Consensus issue (`delivery_mode: consensus_issue`)

Optimizer is skipped entirely; the coordinator writes
`<workspace>/sessions/<id>/results/optimize/<target>/consensus-issue.md`
from the analyzer's `consensus_writeup`. No clone, no branch, no
commit; Phase 5 opens an issue (no PR).

## 5. When a session concludes

Two distinct things can happen, in either order, depending on
operator flags: **publish** (`--publish-accepted-prs`) and
**archive** (`--archive`). Both, neither, or one of them is
legitimate.

### 5a. Publish (Phase 5) — only for `bench_eligible` accepted/mixed targets

For each accepted target whose Phase 3.5 verdict passes the publish
gates (status, verdict present, `confidence >= results_analysis.confidence_floor`):

Local files / dirs:

```text
<workspace>/sessions/<id>/results/optimize/<target>/
  pr-title.txt, pr-body.md              # pr-writer agent output
  pr-writer-prompt.md, pr-writer-events.jsonl, ...
                                        # OR issue-title.txt / issue-body.md
                                        # for consensus_issue mode
```

Git operations: inside the per-target clone at
`<workspace>/optimizers/<id>/<target>/`:

1. `git switch agentic/<id>/<target>` — the branch already exists
   from §3; the switch is a no-op when already on it.
2. `git add -u` — stage any tracked-file modifications that are still
   in the working tree. Normally there are none: the Phase 2
   coordinator commit (§4a) already captured the agent's
   implementation when it was kept. This `git add -u` exists to
   catch any post-Phase-2 touches the operator might have made
   between phases.
3. `commit_if_staged` — call into `git commit` only when step 2
   actually staged something. Authored as `Stacks BenchBot`, no GPG.
   When there's nothing staged (the typical case), this is a no-op
   and Phase 5 pushes the Phase 2 coordinator commit unchanged.
4. `git push -u <publish.remote> agentic/<id>/<target>` — pushes to
   the configured remote (default `cylewitruk/stacks-core`). PAT
   travels via `http.<prefix>.extraheader` env override; the token
   is never in argv and never in `.git/config`.
5. `octocrab` opens a draft PR against `publish.base_repo` with head
   `<head_owner>:agentic/<id>/<target>`. For consensus_poc_pr,
   forced draft + safety labels (`consensus-change`, `needs-HIP`,
   `do-not-merge`).

In practice the `agentic/<id>/<target>` branch typically carries
exactly one commit at push time: the **Phase 2 coordinator commit**
([optimizers.rs `coordinator_commit_if_kept`](../crates/stacks-bench-agent/src/session/optimizers.rs#L967))
that captured the optimizer agent's source modifications after the
typed report passed validation. For `consensus_poc_pr`, the first
commit may instead come from a Phase 5 staged change if the agent
edits were applied between Phase 2 and Phase 5 — but the routine
shape is "one Phase 2 commit, push it as-is in Phase 5".

For consensus_issue (no benchmark, no implementation): no branch / no
commit / no push. The publisher opens an issue with a hidden trace
tag (`<!-- agentic-<id>-<target> -->`) in the body for idempotent
re-runs.

Nothing in `<operator>` or `<base>` is mutated by publish.

### 5b. Archive (Phase 6)

Files / dirs:

```text
<workspace>/archive-worktrees/<id>/     # transient git worktree (NEW)
  sessions/<id>/results/                # bulk COPIED here from
                                        # <workspace>/sessions/<id>/results/
                                        # (the original stays in place)
                                        # — torn down at end of phase
```

Git operations: in `<operator>`:

1. **Idempotency probe**: read `sessions.jsonl` on `main`. If `<id>`
   is already there, the archive is a no-op; return the existing
   `session/<id>` branch metadata.
2. **Sanity check**: operator-repo `main` must be clean and on a
   non-`session/` branch.
3. `git worktree add -b session/<id> <workspace>/archive-worktrees/<id> <starting_branch>`
   — creates a new write-once branch off `main`'s tip and a
   transient worktree under `<workspace>`. The operator's primary
   worktree is **untouched** — it stays on `main`.
4. **Copy** (not move) the bulk results dir into
   `<archive worktree>/sessions/<id>/results/`. Bulk on disk stays
   at `<workspace>/sessions/<id>/results/` so a re-archive would
   work.
5. `git add -f` (force, because `<operator>/sessions/` is
   `.gitignore`d) + `git commit` authored as `Stacks BenchBot`,
   message `archive: <id>`.
6. `git push <operator-origin> session/<id>` (when a remote exists
   and `--dry-run` not set).
7. `git worktree remove --force <workspace>/archive-worktrees/<id>`
   — tears down the transient worktree. The branch object stays in
   `<operator>`'s git database (and on the remote, if pushed).

Then, back on the operator's primary worktree (still on `main`):

1. `git pull --rebase` (when a remote exists) — absorb peer pushes
   to `main` before we add our own.
2. Append one JSON line to `sessions.jsonl`.
3. `git add sessions.jsonl && git commit` message
    `archive: ledger <id>`.
4. `git push origin main`. Retry-on-race when another archive
    pushed concurrently.

End state in `<operator>`:

- `main` has one new commit appending one line to `sessions.jsonl`.
- A new write-once branch `session/<id>` exists carrying the full
  evidence bundle at `sessions/<id>/results/`. **Never moved
  again** — the `artifact_sha` in `sessions.jsonl` would become a
  lie.

End state in the per-target optimizer clones:

- Aborted clones: torn down by
  [`session_end_cleanup`](../crates/stacks-bench-agent/src/cli/session/run.rs#L611).
- Implemented + accepted clones (whether publish happened or not):
  preserved until manually pruned by `optimize clean` or the
  operator. Phase 5 publish needs them; an aborted publish leaves
  them around so the operator can retry.

End state in `<workspace>/sessions/<id>/`:

- All bulk stays in place (the archive *copied* it, didn't move).
- `.run.pid` is cleared on normal return.
- `sbagent workspace prune --archived-only --older-than <duration>`
  is the operator-side recipe for reclaiming this once the session
  is sufficiently old.

## 6. When maintenance is run

**Planned, not yet implemented** — see
[design/0033-maintain-command.md](../planning/design/0033-maintain-command.md)
for the spec. Describing the intended shape so this doc covers the
full lifecycle:

`sbagent maintain` is intended to observe **post-publish state** —
PRs and issues opened in §5a — and reconcile their lifecycle into a
durable event log. It is read-only on the GitHub side (no PR
modifications) and append-only on the operator side.

### Local files / dirs (planned)

```text
<operator>/events/maintenance/<utc-ts>.jsonl
                                        # one append-only file per maintain run
                                        # carrying pr_merged / pr_closed_unmerged
                                        # / pr_stale events
```

### Git operations (planned)

In `<operator>`:

1. Query GitHub via `octocrab` for the PRs/issues opened in earlier
   sessions (cross-reference via `sessions.jsonl` archived state).
2. Diff observed lifecycle vs the last-known event log.
3. Append maintenance events to a new
   `events/maintenance/<utc-ts>.jsonl`.
4. `git add events/maintenance/<utc-ts>.jsonl && git commit`
   authored as `Stacks BenchBot`.
5. `git push origin main`.

Nothing touches `session/<id>` branches (they're write-once). Nothing
touches the `agentic/<id>/<target>` branches in the
`<stacks-core fork>` (those are owned by upstream review now —
reviewers may push to them, the bot doesn't).

Until `0033-maintain-command` ships, post-publish state lives only on
GitHub; the operator inspects it manually via `gh pr list` /
`gh pr view`. There is no event log on the operator side today.

## Where does X live? — quick lookup

| Question | Answer |
| -------- | ------ |
| Where do session phase outputs land? | `<workspace>/sessions/<id>/results/<phase>/` — see §2 for the per-phase breakdown. |
| Where does the baseline binary archive live? | `<workspace>/sessions/<id>/results/baseline/bin/stacks-bench`. |
| Where does the per-target optimizer's working tree live? | `<workspace>/optimizers/<id>/<target>/` (when `agent_workspace_root` is set); legacy fallback `<workspace>/sessions/<id>/worktrees/<target>/`. |
| Where does the per-target `agentic/<id>/<target>` branch live? | Inside the per-target clone above. It also gets pushed to `<stacks-core fork>` in Phase 5. |
| Where does the Phase 3.5 verdict for a target live? | `<workspace>/sessions/<id>/results/analyze/<target>/results-analysis.json`. |
| Where is the live-session PID marker? | `<workspace>/sessions/<id>/.run.pid`. |
| Where does `session/<id>` archive live? | A write-once branch in `<operator>`'s git database, pushed to `<operator>`'s remote. The on-disk bulk under `<workspace>` does NOT move into `<operator>` — only a copy gets committed onto `session/<id>` via a transient worktree at `<workspace>/archive-worktrees/<id>/`. |
| Where does the `sessions.jsonl` ledger live? | `<operator>/sessions.jsonl` on `main`, append-only. |
| Where does cross-session bot memory live? | `<operator>/memory/` (default, lifted out of `.sbagent/`). |
| Where do bundled prompts / schemas / queries / context docs live? | `<operator>/.sbagent/{prompts,schemas,queries,context}/`. |
| Where do per-target Cargo build artifacts live before reclamation? | `<workspace>/optimizers/<id>/<target>/target/`. Reclaimed by `cargo clean` between binary copy and bench invocations — see [operations.md](operations.md#per-worktree-cargo-clean-reclamation). |

## Glossary

- **Session bulk** — everything under `<workspace>/sessions/<id>/results/`. The evidence trail for one autonomous run.
- **Per-target clone** — a standalone git clone (NOT a worktree) at `<workspace>/optimizers/<id>/<target>/`, holding the `agentic/<id>/<target>` branch. One per merged target.
- **Archive worktree** — a transient `git worktree add` at `<workspace>/archive-worktrees/<id>/`, used by Phase 6 to author the `session/<id>` write-once branch without touching the operator's primary worktree. Removed at end of phase.
- **Archive branch** — `session/<id>` in `<operator>`. Write-once; carries `sessions/<id>/results/` (the full bulk) at archive time. Never moved after the `sessions.jsonl` record's `artifact_sha` is committed.
- **Publish branch** — `agentic/<id>/<target>` in `<stacks-core fork>`. Written by Phase 5's `git push`. Upstream reviewers / mergers may move it; the bot does not touch it post-push.
- **Coordinator commit** — the commit Phase 2's coordinator authors after validating the optimizer agent's typed report. Lives only in the per-target clone until Phase 5 pushes it.
