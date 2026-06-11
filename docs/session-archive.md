# Session archive

`sbagent session archive` takes a completed session's local-only
working-tree state and commits it to the operator git repo's permanent
history. This is the boundary between **what's on this operator's
disk** and **what every future reader of the bot's work can find**.

## What gets written

Each archive run produces two git objects in the operator repo:

| Where | What it holds | Lifecycle |
| --- | --- | --- |
| `session/<id>` branch | The full `sessions/<id>/` evidence bundle (`git add -f` bypasses main's `/sessions/` ignore). | **Write-once**. Never re-pushed. |
| `sessions.jsonl` on `main` | One append-only line per session — the terminal index record. | Append-only. |

The ledger line is a [`SessionRecord`](../crates/stacks-bench-agent/src/models/session_record.rs).
Its JSON Schema is committed at
[`schemas/session-record.schema.json`](../schemas/session-record.schema.json).

## Why the split

A session can produce gigabytes of evidence: profiler hotspots, agent
event streams, drilldown CSVs, codex prompts, optimizer reports,
per-invocation Phase 1.8 calibration outputs and Phase 3 candidate
bench-runs (one `bench-run.json` per invocation under each target's
`verify/<id>/` and `optimize/<id>/` trees), Phase 3.5 results-analyzer
verdicts under `analyze/<id>/`. Committing all of that on `main` would
make `main` unusable for daily browsing within months — at 365
sessions/year, a five-year horizon gives ~1825 top-level entries
under `sessions/`.

The split solves both problems:

- **Aggregate / leaderboard / timeline views** read `sessions.jsonl`
  on `main`. One file, one line per session, fast to grep, cheap to
  clone.
- **Detail views** follow `artifact_branch` to the write-once branch.
  Each archive branch is its own git object tree; readers fetch only
  the ones they care about.

## The write-once contract

Once a session has been archived, **its archive branch never moves
again**. Even a future `sbagent maintain` (Layer 3B, not yet built)
will write a separate `maintain.jsonl` on `main` rather than mutating
the session branch.

This is the contract the `artifact_sha` field in the ledger relies on
to be a permanent audit anchor. If anyone moves the branch after
archive, the ledger's claim about what was archived becomes a lie.

If you need to correct a published archive, append a corrective record
to `sessions.jsonl` (or its sibling) rather than rewriting history.

## Idempotency

Re-running `sbagent session archive` against an already-archived
session is a no-op:

```text
$ sbagent session archive --session-id 20260518-190321-nextest-flags-smoke
archive: session already in ledger (branch=session/20260518-190321-nextest-flags-smoke); no-op
```

The idempotency check happens by id-match against `sessions.jsonl`.
Scheduled CI can safely call archive after every session without
coordination.

## Layout: workspace-based session bulk

Session evidence bundles live **outside** the operator git repo, in a
workspace dir under `<layout.agent_workspace_root>/sessions/<id>/`. The
archive flow copies (not moves) the bulk into a transient worktree
when authoring the `session/<id>` branch — the operator's primary
checkout never sees `sessions/<id>/` on disk. See
[git-topology.md](git-topology.md) §5b for the full set of paths and
pushes; the legacy `<operator>/sessions/<id>/` layout is still
supported for non-archive operations but archive itself requires
`layout.agent_workspace_root` to be set outside the operator repo
(the worktree must live there).

## Configuration

Operator must set:

| Setting | Purpose | Default when unset |
| --- | --- | --- |
| `layout.operator_repo_root` | Absolute path to the operator git repo (holds `sessions.jsonl` + archive branches). | `sessions_root.parent()` ONLY when `layout.sessions_root` was set explicitly (legacy layout). Otherwise `None` — `sbagent session archive` requires the setting to be explicit and bails with a clear error if missing. |
| `layout.agent_workspace_root` | Workspace path that holds session bulk + the transient archive worktree. Required by archive (the archive worktree must live outside the operator repo). | None — when also unset, `layout.sessions_root` falls back to `<cwd>/sessions/` (legacy layout) and archive refuses to run. |

The push step uses the same `publish.token_file` + git auth-header
plumbing as Phase 5 publish. With `--dry-run`, the push (and the
publish-style auth preflight) is skipped — local commits still
produced.

### Migrating from the legacy layout

If your operator currently has bulk at `<operator>/sessions/<id>/`:

```bash
# 1. Pick a workspace path (or accept the layout.agent_workspace_root default).
mkdir -p /var/tmp/sbagent-workspaces

# 2. Move existing session dirs out of the operator.
mv /path/to/operator/sessions/* /var/tmp/sbagent-workspaces/sessions/

# 3. Set layout.agent_workspace_root in ~/.config/sbagent/config.toml.
# 4. Set layout.operator_repo_root explicitly (the new default derivation
#    points at the workspace dir, not the operator).
```

## v1 simplifications

These are documented limitations of the v1 record, slated for
follow-up:

| Field | v1 value | v2 plan |
| --- | --- | --- |
| `started_at` | Derived from session id's `YYYYMMDD-HHMMSS` prefix. | Pulled from a per-session manifest written by orchestrator. |
| `finished_at` | Latest mtime under `sessions/<id>/`. | Same manifest source. |
| `phase_durations_secs` | Populated from `<session>/results/timings.json`, written incrementally by `cli/session/run.rs` after each phase completes. Legacy sessions (no `timings.json`) archive with an empty `{}` — landed v5 Phase 2. | — |
| `targets[].head_sha` | Populated from `summary.json.experiments[].head_sha` (which finalize reads from the coordinator-provenance sidecar). `None` for targets whose optimizer never committed. | — landed 2026-05-21. |
| `targets[].pr_url` / `issue_url` | Populated from `<session>/results/optimize/<target>/publish-feedback.json`, written by Phase 5 publish after each successful `octocrab.create_pr` / `create_issue` call. `None` when Phase 5 was skipped, when the target didn't reach publish, or for legacy sessions that predate the sidecar contract — landed v5 Phase 3. | — |
| `bench.baseline_total_us` / `candidate_total_us` | Aggregated at archive time from per-invocation `verify/<target>/<inv>/bench-run.json` (baseline calibration) + `optimize/<target>/<inv>/bench-run.json` (candidate bench) via `.data.summary.total_duration_us`. Targets with a `verification_replay` get real totals; targets without it (full-range fallback path) keep totals at 0 — landed v5 Phase 3. | — |

None of these are load-bearing for the ledger's primary use case
(leaderboard / timeline). They're audit nice-to-haves.

## Common pitfalls

### `git branch -D session/<id>` is now safe

Under the legacy layout (bulk under operator/sessions/), deleting the
archive branch could wipe the bulk from the operator's working tree
on the next checkout. The workspace layout fixes that — the bulk
lives outside the operator entirely, so deleting an archive branch
only loses the *git-tracked* copy. The remote retains it; local
recovery (if needed) is via `git fetch origin session/<id>:session/<id>`.

The write-once contract still holds for the published audit trail:
once a `session/<id>` branch has been pushed, don't mutate it.

### Dirty worktree fails fast

Archive bails if the operator repo has uncommitted changes. Stash or
commit them first. This is deliberate: an archive that swept up
unrelated working-tree edits would silently corrupt the bot's audit
trail.

### Detached HEAD or already-on-archive-branch is rejected

Archive expects to be invoked from the operator's tracking branch
(typically `main`). Running it from inside a `session/<id>` checkout
or with HEAD detached returns an error.

## Push race resolution

`sessions.jsonl` lives on the shared tracking branch, so peer
archives can race. The push path uses `git push -u <remote>
<branch>`; on `non-fast-forward` rejection, archive does
`git fetch && git rebase <remote>/<branch>` and retries up to three
times before bailing.

The archive branch itself is never raced — write-once means a
collision implies someone else archived the same session id, which
should never happen with the YYYYMMDD-HHMMSS-suffix scheme. If it
does, the archive surfaces the push error so the operator can
investigate.

## Useful commands

```bash
# Archive a finished session, push to remote.
sbagent session archive --session-id 20260518-190321-nextest-flags-smoke

# Local rehearsal — no PAT needed.
sbagent session archive --session-id 20260518-190321-nextest-flags-smoke --dry-run

# Auto-archive at the end of a fresh pipeline run.
sbagent session run --archive

# Read the ledger.
jq '.id' sessions.jsonl       # all archived session ids
jq 'select(.status == "succeeded") | .targets[] | select(.status == "accepted") | .id' sessions.jsonl

# Browse archive history.
git log --oneline sessions.jsonl
```
