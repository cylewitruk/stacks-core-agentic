# Operations

Day-to-day running, observability, validation, recovery, and the raw
`stacks-bench` CLI for hand-driven debugging.

## After upgrading `sbagent`

Prompts, JSON schemas, SQL queries, and reference context docs are
embedded in the binary and seeded to
`<operator>/.sbagent/{prompts,schemas,queries,context}/`. After a
`just install` (or a fresh `cargo install`) pick up the new
version's bundle:

```bash
cd <operator>
sbagent sync                          # rewrite ALL bundles on disk
                                      # (schemas, queries, prompts, context)
                                      # — the bundled versions are the
                                      # contract surface.
sbagent sync --keep-tunables          # preserve operator-edited prompts +
                                      # context docs (still refreshes
                                      # schemas + queries).
sbagent sync --commit                 # also produce one bot-authored commit
                                      # of whatever changed.
sbagent sync --push                   # implies --commit; pushes to origin
                                      # via PAT-via-env (same auth path
                                      # `sbagent init --push` uses).
sbagent check                         # confirms no remaining bundle drift.
```

The `--commit` / `--push` flags exist so a scheduled CI workflow can
run `sbagent sync --push` after each binary upgrade without spelling
out the git + auth dance in shell — the resulting commit message is
`chore: sync sbagent bundles (<sbagent-version>)` so `git log .sbagent/`
becomes an audit trail of which binary version was on disk for each
session. `--push` validates `origin` against `git.auth_url_prefix`
(default `https://github.com/`) up-front; SSH or other-prefix URLs
error rather than silently falling back to a different auth path.

`sbagent check` would have already told you something was off — it
fails on schemas / queries drift and warns on prompt drift. Running
sync without checking first is fine; it's idempotent.

## Launch a coordinator session in tmux

```bash
SBAGENT_SESSION_ID="$(date +%Y%m%d-%H%M%S)"
mkdir -p "sessions/$SBAGENT_SESSION_ID/results" data/stacks-bench
export SBAGENT_SESSION_ID

tmux new-session -d -s stacks-bench-agent "sbagent session run"

echo "session: sessions/$SBAGENT_SESSION_ID/results"
echo "attach with: tmux attach -t stacks-bench-agent"
```

Watch:

```bash
tmux attach -t stacks-bench-agent
```

Detach: <kbd>Ctrl-b</kbd> then <kbd>d</kbd>.

If you omit `--session-id` (and `SBAGENT_SESSION_ID` is unset),
`session run` mints a fresh `YYYYMMDD-HHMMSS` id automatically. Every
other subcommand requires an explicit id.

## One-screen observability: `sbagent session tail`

Every agent writes its own JSONL/stderr at unpredictable nesting
depth. Use `sbagent session tail` to multiplex them with `tail -F`
semantics — late-arriving files are picked up automatically:

```bash
sbagent session tail --session-id "$SBAGENT_SESSION_ID"
# or, if SBAGENT_SESSION_ID is exported:
sbagent session tail
```

Inspect the event-stream summary for one phase:

```bash
jq -r 'select(.type=="item.completed") | .item.type? // empty' \
  "sessions/$SBAGENT_SESSION_ID/results/triage/events.jsonl" \
  | sort | uniq -c
```

## Validate

The session artifact tree is the contract between phases. `sbagent
session validate` checks for required files and exits non-zero if
any are missing — call it after the coordinator completes, or use it
from a resumed session to figure out what's still pending.

```bash
sbagent session validate --session-id "$SBAGENT_SESSION_ID"
```

## Recovery

The pipeline doesn't have a "resume one agent in place" mode —
re-running the relevant phase is faster and more predictable. Phase
subcommands read state from the session dir, so re-running a phase
against an existing session dir picks up where the prior run left
off.

If `validate` prints `MISSING:`, the listed paths tell you which
phase to re-run:

| Missing artifact | Re-run |
| ---------------- | ------ |
| `triage/candidates.json` or `triage/final-message.md` | `sbagent session triage run` |
| `analysis/<family-id>/analysis.json` | `sbagent session analysis run` |
| `merge/optimization-targets.json` or `merge/final-message.md` | `sbagent session analysis merge` |
| `verify/<target-id>/baseline-run-ids.json` or `verify/<target-id>/<invocation-id>/bench-run.json` | Phase 1.8 has no standalone command; re-run `sbagent session run` (the target calibration baseline step is inlined). To force a fresh calibration, `sbagent session bench clean` drops both the Phase 1.8 target calibration baseline and the paired Phase 3 verification bench side. |
| `optimize/<target-id>/optimizer-report.json` (the typed authoritative report — coordinator-rendered `implementation.md` / `abort.md` companions derive from it; `consensus-issue.md` is the coordinator-written marker for `consensus_issue` targets where the optimizer is skipped) | `sbagent session optimize run` |
| `optimize/<target-id>/candidate-run-ids.json` or `optimize/<target-id>/<invocation-id>/bench-run.json` | `sbagent session bench run` |
| `analyze/<target-id>/results-analysis.json` | `sbagent session analyze-results run` |
| `finalize/summary.json` | `sbagent session finalize run` |

Each agent's `conversation-id` is captured in its phase dir for later
inspection (e.g. `triage/conversation-id`,
`analysis/<family-id>/conversation-id`, `merge/conversation-id`,
`optimize/<target-id>/conversation-id`), but resume-by-id requires CLI
flags that vary across Codex versions; the re-run-the-phase approach
above is the supported path.

To wipe a phase's artifacts before re-running, use the matching
`clean` subcommand. Every phase that writes artifacts has one
(`session baseline clean`, `session triage clean`, `session analysis
clean`, `session optimize clean`, `session bench clean`, `session
analyze-results clean`, `session finalize clean`, `publish clean`).
`session bench clean` is paired: it drops both the Phase 3 verification bench
side under `optimize/<target>/` and the Phase 1.8 target-calibration-baseline
side under `verify/<target>/`, since the two phases share one
invocation-id set per target.

### Per-worktree `cargo clean` reclamation

Each per-target optimizer worktree under
`<layout.agent_workspace_root>/sessions/<id>/optimizer-checkouts/<target>/`
runs `cargo clean` immediately after building the release binary —
between the binary copy to `optimize/<target>/bin/stacks-bench` and the
bench invocations. The bench invocations point at the copied binary,
so the worktree's `target/` directory is genuinely disposable from
that moment. Reclamation bounds peak per-target disk during a long
Phase 3 sweep without touching the worktree itself, which Phase 5
publish still needs for PR-writer context + `git push`.

Logs land at `optimize/<target>/cargo-clean.log` and
`optimize/<target>/cargo-clean.stderr.log`. Absence of those files
under `optimize/<target>/` means the operator ran with
`--skip-cargo-clean` (the explicit opt-out on `session run` and
`session bench run`, used during hand-debugging when an operator wants
to re-bench without a rebuild). Use the opt-out sparingly — it
preserves the build cache for every per-target worktree across the
entire session.

## Workspace hygiene: `sbagent workspace prune`

`sbagent workspace prune` removes stale per-session scratch under
`<layout.agent_workspace_root>/sessions/`. The command is safe by
default and refuses to remove anything without an explicit filter.

```bash
# Inspect every candidate (implicit dry-run — no filters set).
sbagent workspace prune

# Remove only sessions present in operator-main sessions.jsonl
# (archived/terminal state).
sbagent workspace prune --archived-only

# Remove archived sessions older than a week.
sbagent workspace prune --archived-only --older-than 7d

# Dry-run an explicit recipe before applying.
sbagent workspace prune --archived-only --older-than 7d --dry-run
```

`--older-than` accepts single-unit durations: `30s`, `15m`, `4h`, `7d`,
`2w`. Multi-unit strings (`1d12h`) aren't supported — pick one unit.

Two safety signals prevent accidental loss:

1. **Live-session refusal.** Each running `sbagent session run` writes
   `<session_dir>/.run.pid` after preflight passes and clears it on
   normal exit. `workspace prune` reads each candidate's `.run.pid`
   and refuses removal when the recorded PID is live (POSIX `kill -0`).
   Stale PID files (process gone, e.g. after a SIGKILL) fall through
   to the normal age + archive filters — they cannot make a workspace
   immortal.
2. **`sessions.jsonl` cross-reference.** With `--archived-only`, prune
   only considers sessions whose id is present in the operator-main
   `sessions.jsonl` ledger. A session that hasn't reached Phase 6
   archive is never prunable under that flag.

The command prints a per-candidate decision (PRUNED / KEPT with a
reason) and a totals footer listing freed bytes. In dry-run mode the
footer reports the would-be-freed figure so an operator can size up
recovery before committing.

## Session-start disk preflight

The session-start preflight includes a `free-disk` check probing
the filesystem holding `layout.agent_workspace_root`:

- `preflight.min_free_gib` **unset**: low free space emits a `Warn`
  only (below 10 GiB; the conservative warn floor).
- `preflight.min_free_gib = n`: free space below `n` GiB emits a
  hard `Fail` with the exact `sbagent workspace prune --older-than 7d
  --archived-only` invocation in the remediation field.

The check is skipped when `layout.agent_workspace_root` isn't
configured (operator deployments that pin everything inline). See
[configuration.md](configuration.md) for the config knob.

## No targets remaining: recovery flow

If `merge/optimization-targets.json` ends up with an empty `targets[]`,
the cause is one of:

1. **Triage emitted zero candidates.** Every workload-entry pattern
   fell below the noise floor, was outlier-driven, or every
   alternative family was investigated and rejected via the
   counter-search step. See `triage/final-message.md` (especially the
   "Rejected alternative families" section) for the agent's
   reasoning.
2. **Every analyzer rejected its family.** See each
   `analysis/<family-id>/analysis.json` for the `reason`. Common:
   hotspot is real but inherent / already cached / `target_span`
   overlaps with a non-target.
3. **Merge phase rejected all surviving analyses.** See
   `merge/final-message.md` and the `rejected_by_merge` array in
   `merge/optimization-targets.json`.

Recover by one of:

1. **Wider profiler view** — re-run `bench show --profiler-hot 200`
   (or higher) on the same discovery-pass run, then re-run triage with the
   wider hotspot file. Spans below the top-50 cutoff sometimes
   contain real opportunities once the obvious ones are exhausted.
2. **Different block range** — pick the next canonical range and
   update `stacks_bench.start_at` / `stacks_bench.count` in
   `config.toml` (`5_000_000–5_025_000` → `6_500_000–6_525_000` →
   `7_300_000–7_325_000`). Different transaction mixes light up
   different hotspots. Run a fresh discovery pass against the new range
   and start a new session.
3. **Update non-targets.md** — if analyzers keep rejecting candidates
   for the same novel reason, append it to
   `<operator>/.sbagent/context/non-targets.md` so the next triage
   pass excludes it earlier. No rebuild is needed; the agents read
   it at its absolute path each invocation.

`finalize/summary.json`'s `next_targets_hint` exists specifically for
`sbagent session finalize run` to leave a recommendation for which of
these the operator should try next.

## Expected artifact tree

See [git-topology.md](git-topology.md) for the full lifecycle
walkthrough — what each dir is, when it's written, and what git
operations touch it. This section is the per-file enumeration of the
results tree as a quick lookup.

Each phase writes into its own subdir under `results/`. The
`optimize/<target-id>/` dir is shared across Phase 2 (optimizer agent),
Phase 3 (per-invocation `<invocation-id>/bench-run.json` benchmark
outputs), and Phase 5 (`pr-writer-*` / `issue-writer-*` publish
artifacts) — one audit folder per target with everything that
happened to it. Phase 1.8 calibration outputs live under
`verify/<target-id>/`, mirroring Phase 3's per-invocation shape;
Phase 3.5 results-analyzer verdicts under `analyze/<target-id>/`.

```text
<layout.agent_workspace_root>/sessions/<session-id>/results/
  # (defaults to /private/tmp/sbagent-workspaces/sessions/<id>/ on macOS;
  #  /var/tmp/sbagent-workspaces/sessions/<id>/ on Linux. Sits outside
  #  the operator repo by design so branch switches can't wipe it —
  #  see docs/session-archive.md.)
  #
  # Phase 0: discovery pass (orchestrator-owned; legacy path name)
  baseline/
    bench-run.json
    bench-run.stderr.log
    rerun.json                  # copy of bench-run.json — Phase 0b runs
    rerun.stderr.log            # ONE benchmark; rerun-id is aliased
    run-id                      # plain text: numeric stacks-bench run id
    rerun-id                    # plain text: same id as run-id (no second
                                # bench run is taken — the single-run
                                # noise floor is sourced from settings)
    bench-list.json
    profiler-hotspots.json
    noise-floor-pct             # plain text: percent. Sourced from
                                # `triage.single_run_noise_floor_pct`
                                # (default 1.0). Both `baseline run` and
                                # `baseline import` write it.

  # Phase 1: triage agent (cwd here, so drilldown CSVs land below)
  triage/
    prompt.md                   # rendered prompt
    events.jsonl
    stderr.log
    final-message.md
    conversation-id
    candidates.json             # schema: schemas/candidates.schema.json
    candidates.md               # human view, derived from JSON
    queries/<n>.csv             # prerendered orientation queries
    drilldowns/<n>.csv          # agent-issued drilldowns

  # Phase 1.5: analyzer agents (one per family)
  analysis/
    <family-id>/
      prompt.md
      events.jsonl
      stderr.log
      final-message.md
      conversation-id
      analysis.json             # schema: schemas/analysis.schema.json
      analysis.md               # human-readable analysis writeup

  # Phase 1.7: merge phase (LLM consolidation)
  merge/
    prompt.md
    events.jsonl
    stderr.log
    final-message.md            # audit summary including coverage check
    conversation-id
    optimization-targets.json   # schema: schemas/optimization-targets.schema.json
                                # carries merged_from / convergence_count provenance

  # Phase 1.8: target calibration baseline (orchestrator-owned)
  verify/
    <target-id>/
      baseline-run-ids.json     # InvocationRunIds: {"entries":[
                                # {"invocation_id","run_id"}, ...]}
      <invocation-id>/          # one per verification_replay.invocations[]
        bench-run.json          # Phase 1.8 target calibration output
        bench-run.stderr.log

  # Phase 2/3/3.5/5: per-target shared dir
  optimize/
    <target-id>/
      prompt.md                 # Phase 2 optimizer's rendered prompt
      events.jsonl              # Phase 2 optimizer's event stream
      stderr.log
      final-message.md
      conversation-id
      nextest.log               # written by the optimizer agent
      nextest.stderr.log
      implementation.md         # OR abort.md OR consensus-issue.md
      side-observations.md      # optional, future-target evidence
      candidate-run-ids.json    # InvocationRunIds, same shape as
                                # verify/ above; written by Phase 3
      <invocation-id>/          # one per VR.invocations[]
        bench-run.json          # Phase 3 verification bench output
        bench-run.stderr.log
      pr-writer-prompt.md       # Phase 5 publish artifacts (if shipped)
      pr-writer-events.jsonl
      pr-writer-stderr.log
      pr-writer-final-message.md
      pr-title.txt
      pr-body.md
      issue-writer-prompt.md    # for consensus_issue targets
      issue-writer-events.jsonl
      issue-writer-stderr.log
      issue-writer-final-message.md
      issue-title.txt
      issue-body.md

  # Phase 3.5: per-target results-analyzer verdict
  analyze/
    <target-id>/
      prompt.md
      events.jsonl
      stderr.log
      final-message.md
      conversation-id
      results-analysis.json     # schema: schemas/results-analysis.schema.json
                                # carries verdict + confidence +
                                # per-invocation breakdown +
                                # pr_body_summary (Phase 5 reads
                                # verbatim into the PR body)
      results-analysis.md       # operator-facing companion (short prose)

  # Phase 4: session summary
  finalize/
    summary.json                # schema: schemas/summary.schema.json
    summary.md
    targets.md                  # human catalog rendered from optimization-targets.json
```

## Benchmark serialization

No two benchmark runs should execute simultaneously. This matters
because local-agent-VM benchmarking shares CPU, memory, disk, and the
persistent SQLite benchmark DB.

`sbagent` holds a cross-process exclusive lock around every
`cargo stacks-bench bench run`, `bench rerun`, and expensive
`chainstate index` command. The lockfile lives under sbagent's
`layout.lock_dir` (default `<operator>/data/run/`):

```text
<layout.lock_dir>/benchmark.lock     # default sbagent bench lock
<layout.lock_dir>/test.lock          # default sbagent test (cargo nextest) lock
```

Override the directory via `layout.lock_dir` in `config.toml`. Hand-running
a benchmark outside `sbagent`? Use `flock` against the same path so
you serialize against any concurrent `sbagent session run`.

## Hand-running stacks-bench

`sbagent` calls `cargo stacks-bench` internally; you don't normally
need to touch it. The snippets below are for hand-debugging a phase
in isolation.

### Setup variables

```bash
OPERATOR_DIR="$(pwd)"      # the dir `sbagent init` created
SBAGENT_SESSION_ID="${SBAGENT_SESSION_ID:-$(date +%Y%m%d-%H%M%S)}"

# Per-session source checkout (post-v3 — the operator submodule is gone).
# sbagent materializes this at session start; for hand-debugging you can
# point at an existing checkout, or `git clone` a fresh one yourself.
AGENT_WORKSPACE_ROOT="${AGENT_WORKSPACE_ROOT:-/private/tmp/sbagent-workspaces}"
BASE="$AGENT_WORKSPACE_ROOT/sessions/$SBAGENT_SESSION_ID/repos/$(sbagent source cache-id)"
SESSION_DIR="$AGENT_WORKSPACE_ROOT/sessions/$SBAGENT_SESSION_ID/results"
WORKTREES="$AGENT_WORKSPACE_ROOT/optimizers/$SBAGENT_SESSION_ID"

STACKS_BENCH_DATA_DIR="$OPERATOR_DIR/data/stacks-bench"
BENCH_LOCK="$OPERATOR_DIR/data/run/benchmark.lock"
mkdir -p "$SESSION_DIR/baseline" "$STACKS_BENCH_DATA_DIR" "$WORKTREES" \
  "$(dirname "$BENCH_LOCK")"
```

### Common range args

The canonical Nakamoto-era ranges to pick from:

```text
5_000_000 – 5_025_000
6_500_000 – 6_525_000
7_300_000 – 7_325_000
```

```bash
COMMON_RANGE_ARGS=(
  --source "$SOURCE_DIR"
  --network "$STACKS_BENCH_NETWORK"
  --start-at "$STACKS_BENCH_START_AT"
  --count   "$STACKS_BENCH_COUNT"
)
```

### Run-id extraction helper

`bench run --json` writes a `CommandResult` envelope to stdout:

```json
{ "success": true, "duration_secs": 12.34, "data": { "run_id": 42 } }
```

Extract the run id directly from the envelope:

```bash
extract_run_id() {
  jq -er '.data.run_id' "$1"
}
```

`sbagent` does the same thing internally and treats a missing
`.data.run_id` as a hard error rather than guessing. Don't fall back
to `SELECT MAX(id) FROM benchmark_run` — that's racy with any
concurrent `bench run` writer holding the bench lock.

### Index chainstate

```bash
cd "$BASE"  # per-session source checkout (see "Setup variables" above)

flock "$BENCH_LOCK" \
  cargo stacks-bench \
    --db "$STACKS_BENCH_DATA_DIR" \
    --json \
    chainstate index \
    "${COMMON_RANGE_ARGS[@]}" \
    > "$SESSION_DIR/chainstate-index.json" \
    2> "$SESSION_DIR/chainstate-index.stderr.log"
```

### Discovery-pass benchmark

```bash
cd "$BASE"  # per-session source checkout (see "Setup variables" above)

BENCH_NAME="baseline-$SBAGENT_SESSION_ID"

flock "$BENCH_LOCK" \
  cargo stacks-bench \
    --db "$STACKS_BENCH_DATA_DIR" \
    --json \
    bench run \
    "${COMMON_RANGE_ARGS[@]}" \
    --name "$BENCH_NAME" \
    > "$SESSION_DIR/baseline/bench-run.json" \
    2> "$SESSION_DIR/baseline/bench-run.stderr.log"

BASELINE_RUN_ID=$(extract_run_id "$SESSION_DIR/baseline/bench-run.json")
echo "$BASELINE_RUN_ID" > "$SESSION_DIR/baseline/run-id"

# Phase 0b alias: rerun-id matches run-id, and rerun.json is a copy of
# bench-run.json. No second benchmark is taken — the operator-side
# `triage.single_run_noise_floor_pct` (default 1.0) supplies the
# discovery-pass noise floor instead.
echo "$BASELINE_RUN_ID" > "$SESSION_DIR/baseline/rerun-id"
cp "$SESSION_DIR/baseline/bench-run.json"        "$SESSION_DIR/baseline/rerun.json"
cp "$SESSION_DIR/baseline/bench-run.stderr.log"  "$SESSION_DIR/baseline/rerun.stderr.log"
printf '%s\n' "1.0" > "$SESSION_DIR/baseline/noise-floor-pct"
```

The single-run noise floor is a deliberate simplification: per-host
noise-floor calibration via `bench rerun` would cost a second
multi-minute run on every session, and Pass 1c's per-target
results-analyzer judges direction first / magnitude second against
each invocation's `expected_signal.tolerance_pct` — the
session-level noise floor is not load-bearing for the verdict math.
A future revision may reintroduce optional rerun if the
results-analyzer's tolerance bands prove insufficient for some
workloads; today, prefer tuning each invocation's tolerance over
running a rerun.

### Profiler hotspots and listing

```bash
cargo stacks-bench --db "$STACKS_BENCH_DATA_DIR" --json \
  bench list --all --with-args --limit 100 \
  > "$SESSION_DIR/baseline/bench-list.json"

cargo stacks-bench --db "$STACKS_BENCH_DATA_DIR" --json \
  bench show --run-id "$BASELINE_RUN_ID" --profiler-hot 50 \
  > "$SESSION_DIR/baseline/profiler-hotspots.json"
```

## Source facts incorporated

From the `stacks-bench` README and CLI help on the
`cylewitruk/stacks-core` `feat/stacks-bench` branch:

- The branch includes a Cargo alias `cargo stacks-bench ...` for
  running `stacks-bench` with the correct release/profile parameters;
  prefer that alias over direct `cargo run`.
- Global options include `--db <APP_DATA_DIR>` and `--json` before
  the command.
- `bench run` requires `--source <SOURCE_DIR>`, where the source
  directory is the Stacks node data directory containing the
  `chainstate` folder.
- `bench run` supports `--start-at`, `--end-at`, `--tip`, `--network`,
  `--count`, `--txid`, `--repetitions`, `--calibration`, `--warmup`,
  `--filter contract-call`, `--no-profiler-kv`, `--with-pre-naka`,
  and `--name`.
- `bench rerun` uses `--run-id <RUN_ID>`; omit it only for
  interactive selection, which the headless workflow must avoid.
- `bench list` supports `--json`, `--today`, `--since`,
  `--incomplete`, `--all`, `--name`, `--limit`, `--sort-by`, and
  `--with-args`.
- `bench show` uses `--run-id <RUN_ID>` and supports `--json` and
  `--profiler-hot <N>`.
- `chainstate index` requires `--source <SOURCE_DIR>` and supports
  `--start-at`, `--end-at`, `--count`, `--tip`, and `--network`.
- `chainstate list` lists indexed chainstate data.
- `mcp` starts an MCP stdio server for agent access to benchmark
  data.
- Benchmark data is stored by default at
  `~/.stacks-bench/appdata/stacks-bench.db`. The data directory can
  be overridden by `--db`, then `STACKS_BENCH_DATA_DIR`, then the
  default `~/.stacks-bench`.
