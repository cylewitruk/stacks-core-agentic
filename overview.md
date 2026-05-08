# Stacks Bench Codex Agent Runbook

## Purpose

This runbook defines a demo-ready autonomous Codex workflow for benchmark-driven optimization of `stacks-core` using `stacks-bench`.

The first version runs benchmarks inside the agent VM. Later, the benchmark execution step can move into a resource-limited benchmark VM without changing the overall coordinator/subagent workflow.

Runtime paths are now derived from `FRAMEWORK_ROOT`, which means the
repository can be checked out anywhere. This document still uses `/work` in
many examples because it remains the recommended deployment path for the demo
VM, but the scripts no longer require it.

## Terminology

Use these terms consistently:

```text
Optimization session
  One full outer pass: baseline, target ranking, worktree experiments, comparison, summary.
  Identified by OPT_SESSION_ID.

Benchmark run
  One `cargo stacks-bench bench run` record stored in the persistent stacks-bench SQLite DB.
  Identified by a stacks-bench run id.

Optimization-session artifacts
  JSON snapshots, stderr logs, Codex JSONL event streams, notes, and summaries for one optimization session.
  Stored under /work/sessions/<OPT_SESSION_ID>/results.

Persistent benchmark data
  stacks-bench application data and SQLite database shared across optimization sessions.
  Stored under /work/data/stacks-bench.
```

## Source layout (this repo)

This repository is intended to be checked out directly at `/work` on the agent
VM. The framework files live in this repo; the target codebase lives in the
`repos/stacks-core` git submodule. See [README.md](README.md) for the clone +
submodule initialization flow.

The prompts, schemas, and scripts referenced throughout this playbook are
checked in alongside this document, so the demo is reproducible from a fresh
clone.

## Family-first agent architecture (schema v2)

The pipeline splits the optimization workflow across four agent tiers plus shell-owned phases. The split exists because each decision in the loop needs different context: triage needs aggregate workload signal but no code; analyzers need code + traces for a single workload; the merge phase needs to reason about cross-family equivalence; optimizers need a clean implementation environment. Concentrating these into one agent (or making any tier do another's job) costs quality.

Crucially, **triage does NOT commit a target span**. Its job is to identify WHAT to investigate (representative txs, blocks, or contract.functions). The analyzer commits the span identity using its full trace + code context, and the merge phase deduplicates analyses that converge on the same fix.

```text
                     [profiler data + workload]
                            │
                            ▼
             ┌──────────────────────────────┐
             │  Triage agent (1 instance)   │   prompts/triage.md
             │  • profiler JSON + DB        │   → candidates.json (family-shaped)
             │  • picks workload entry      │     {kind, representative_ids,
             │    points; NOT span identity │      suspected_spans?, ...}
             │  • no codebase exploration   │
             └──────────────┬───────────────┘
                            │ one analyzer per family
                ┌───────────┴────────────┐
                ▼           ▼            ▼
        ┌───────────┐ ┌───────────┐ ┌───────────┐
        │ Analyzer  │ │ Analyzer  │ │ Analyzer  │   prompts/analyzer.md
        │ (fam-A)   │ │ (fam-B)   │ │ (fam-C)   │   → analyses/<family-id>/
        │ traces +  │ │ traces +  │ │ traces +  │     analysis.json
        │ code      │ │ code      │ │ code      │     {target_span,
        │           │ │           │ │           │      fix_signature, ...}
        └─────┬─────┘ └─────┬─────┘ └─────┬─────┘
              └─────────────┼─────────────┘
                            ▼
                  ┌──────────────────────┐
                  │  Merge agent         │   prompts/merge-analyses.md
                  │  (1 instance, LLM)   │   → optimization-targets.json
                  │  • dedup convergent  │     (canonical fix per target;
                  │    fixes by         │      merged_from records the
                  │    structural       │      contributing families)
                  │    equivalence       │
                  └──────────┬───────────┘
                             │ one optimizer per merged target
                 ┌───────────┴────────────┐
                 ▼           ▼            ▼
         ┌───────────┐ ┌───────────┐ ┌───────────┐
         │ Optimizer │ │ Optimizer │ │ Optimizer │   prompts/optimizer.md
         │ (target-a)│ │ (target-b)│ │ (target-c)│   → experiments/<id>/...
         │ worktree  │ │ worktree  │ │ worktree  │
         └───────────┘ └───────────┘ └───────────┘
```

Per tier:

* **Triage** runs once. Picks 0..N candidate families (tx_family / block_family / contract_family). Reads profiler JSON + the SQLite DB but no source code. Produces `candidates.json`.
* **Analyzer** runs in parallel, one per family. Each gets its full context budget for one family; reads `${BASE}` deeply, runs trace queries on the family's representative ids, commits `target_span` + `fix_signature`. Produces `analyses/<family-id>/analysis.json` with `status: accepted | rejected`.
* **Merge** runs once over the accepted analyses (LLM consolidation pass; smaller / faster model is appropriate here). Identifies analyses that propose the same structural change and collapses them into a single optimization target with cross-family provenance. Produces `optimization-targets.json`. The coverage invariant is enforced: every accepted family appears in exactly one target's `merged_from` or in `rejected_by_merge`.
* **Optimizer** runs in parallel, one per merged target, each in its own git worktree. Implements the change, runs tests, leaves a release binary for the coordinator script to benchmark.

Shell owns: baseline + noise-check benchmarks (Phase 0), release builds + serialized benchmarks (Phase 3), and summary generation (Phase 4). Agents own: triage, per-family analysis, merge consolidation, implementation. This separation is what allows steps to be deterministic and independently resumable.

## Canonical paths

```text
<FRAMEWORK_ROOT>/repos/stacks-core
  Stable control checkout on feat/stacks-bench.

<FRAMEWORK_ROOT>/sessions/<OPT_SESSION_ID>/worktrees/<target-id>
  One experiment worktree per optimization target, scoped to this optimization session.

<FRAMEWORK_ROOT>/sessions/<OPT_SESSION_ID>/results
  Artifacts for one full optimization session.

<FRAMEWORK_ROOT>/prompts/triage.md
  Prompt for the triage agent. Reads profiler JSON, emits candidates.json.

<FRAMEWORK_ROOT>/prompts/analyzer.md
  Prompt for analyzer subagents. One instance per candidate; deep codebase reads.

<FRAMEWORK_ROOT>/prompts/optimizer.md
  Prompt for optimizer subagents. One instance per accepted target, each in its own worktree.

<FRAMEWORK_ROOT>/prompts/non-targets.md
  Read-only reference of profiler spans the agents must NOT pursue.

<FRAMEWORK_ROOT>/schemas/
  JSON schemas for each agent's output (candidates, analysis, optimization-targets, summary).

<FRAMEWORK_ROOT>/data/stacks-bench
  Persistent stacks-bench app-data directory. The SQLite DB normally lives below appdata/stacks-bench.db.

/mnt/chainstate/mainnet
  Example mounted chainstate source. Must be the Stacks node data directory containing chainstate/.
```

### Single source of truth: `FRAMEWORK_ROOT/.env`

All environment variables live in a single `.env` file at the framework root by default. The shared `_lib.sh` helper derives `FRAMEWORK_ROOT` at runtime, then sources `${AGENTIC_ENV_FILE:-$FRAMEWORK_ROOT/.env}`. This avoids drift between the playbook prose, shell scripts, prompt templates, and the resume path.

The template is checked in at [scripts/env.example](scripts/env.example) — copy it to `<FRAMEWORK_ROOT>/.env` and edit if needed. The shape (canonical Nakamoto-era ranges, lock paths, etc.) is documented in that file.

Source pattern in every script and prompt-render step:

```bash
set -a; source "$FRAMEWORK_ROOT/.env"; set +a
```

Prefer `--count` for bounded demo runs. Avoid `--with-pre-naka` unless benchmarking pre-Nakamoto data is intentional, because it can add significant chainstate copy time.

## Top-level workflow

### 0. Never run before: bootstrap the environment

Goal: prepare the agent VM so an optimization session can run without interactive setup.

One-time setup:

1. Configure Codex user settings in `~/.codex/config.toml`.
2. Create the expected layout and write `<FRAMEWORK_ROOT>/.env`.
3. Clone this repository and initialize `repos/stacks-core` as a submodule.
4. Configure the `repos/stacks-core` checkout for the benchmark branch you want to use.
5. Mount or otherwise expose the chainstate so `SOURCE_DIR` points at a directory containing `chainstate/`, for example `/mnt/chainstate/mainnet`.
6. Apply benchmark-host tuning: CPU governor, ASLR, SMT (see "Benchmark host tuning").
7. Pre-build the release `stacks-bench` binary in `BASE` so MCP and the first benchmark don't pay first-build cost.
8. Verify the framework files are present directly under `<FRAMEWORK_ROOT>/{prompts,schemas,scripts}`.

Expected state after bootstrap:

```text
<FRAMEWORK_ROOT>/repos/stacks-core
  branch: feat/stacks-bench

<FRAMEWORK_ROOT>/data/stacks-bench
  persistent stacks-bench app-data directory

<FRAMEWORK_ROOT>/sessions
  empty or containing previous optimization sessions

SOURCE_DIR
  points to a directory that contains chainstate/
```

Do not start autonomous optimization until this state is true.

### 1. First optimization session: establish the baseline path

Goal: prove that the full pipeline runs end-to-end on a fresh agent VM. The output quality is secondary on the first run; what matters is that every phase artifact appears and `validate-session.sh` reports `OK`.

The first optimization session should:

1. Create `/work/sessions/<OPT_SESSION_ID>/results`.
2. Use `/work/data/stacks-bench` as the shared stacks-bench app-data directory.
3. Set benchmark parameters explicitly via `/work/.env`; reuse them for the baseline + every experiment.
4. Run the baseline + `bench rerun` from `/work/repos/stacks-core` (Phase 0).
5. Run the triage agent → `candidates.json` (Phase 1).
6. Fan out analyzer agents → `analyses/<family-id>/analysis.json` (Phase 1.5).
7. Run `merge-analyses.sh` → `optimization-targets.json` (Phase 1.7).
8. Fan out optimizer agents → per-target `implementation.md` or `abort.md` (Phase 2).
9. Build + serially benchmark each accepted target (Phase 3).
10. Run `finalize-session.sh` → `summary.json` (Phase 4).

The first optimization session is successful if `validate-session.sh "$OPT_SESSION_DIR"` exits 0. Do not judge optimization quality from the first session — its purpose is to prove every tier runs and artifacts land in the right places.

### 2. Ongoing optimization: triage → analyzers → merge → optimizers

Goal: use one baseline + one noise floor to drive isolated optimization experiments through the family-first pipeline.

The shell coordinator script owns orchestration. It does NOT make decisions — every analytical decision lives in an agent prompt. The script's responsibilities, in order:

1. Run the baseline benchmark and a `bench rerun` of the same baseline (Phase 0). The delta is the per-host noise floor.
2. Render and launch the **triage agent** (Phase 1). Wait for `candidates.json`.
3. Fan out **analyzer agents** in parallel, one per family (Phase 1.5). Wait for every `analyses/<family-id>/analysis.json`.
4. Run **merge-analyses.sh** to consolidate accepted analyses into `optimization-targets.json` (Phase 1.7). One LLM call dedupes analyses that converge on the same fix; coverage invariant is enforced.
5. Fan out **optimizer agents** in parallel, one per merged target, each in its own git worktree (Phase 2).
6. Force-rebuild each worktree's release `stacks-bench` binary, copy it out, and `cargo clean` the worktree.
7. Run two benchmarks per experiment serialized under `BENCH_LOCK` (Phase 3).
8. Run `finalize-session.sh` to produce `summary.json` (Phase 4).

Agent responsibilities in one line each:

| Tier      | Owns                                                                                                                            | Does NOT                                                |
| --------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| Triage    | Pick candidate workload families (tx / block / contract) from profiler data + DB.                                               | Read source code. Commit a target span. Run benchmarks. |
| Analyzer  | Investigate ONE family deeply; commit `target_span` + `fix_signature`; produce an analysis the merge + optimizer phases act on. | Modify source code. Run benchmarks.                     |
| Merge     | Dedupe analyses converging on the same structural fix; emit one canonical target per fix with `merged_from` provenance.         | Re-investigate. Modify analyses' substance.             |
| Optimizer | Implement the change in a worktree; run `cargo nextest`; leave a release binary.                                                | Run benchmarks. Touch other worktrees.                  |

Benchmarking is centralized in the shell coordinator so all experiments use the same parameters and the same `BENCH_LOCK`. `cargo nextest` runs (in optimizer phase) are serialized across parallel optimizers via `TEST_LOCK` to avoid port/dir conflicts.

### 3. Results: what to inspect and how to decide

Persistent cross-session benchmark data lives in:

```text
/work/data/stacks-bench
```

Optimization-session artifacts live in:

```text
/work/sessions/<OPT_SESSION_ID>/results
```

For each experiment, inspect:

```text
experiments/<target-id>/implementation.md   # OR abort.md
experiments/<target-id>/side-observations.md  # optional, future-target evidence
experiments/<target-id>/nextest.log
experiments/<target-id>/run-1/bench-run.json
experiments/<target-id>/run-2/bench-run.json
```

Use these sources for comparison:

* `bench show --json --run-id <id>`
* `bench show --json --run-id <id> --profiler-hot 50`
* `bench list --json --all --with-args`
* direct SQL against `/work/data/stacks-bench/appdata/stacks-bench.db` if needed
* MCP/Metabase/Explorer later, once the CLI loop is stable

Accept an experiment only if:

* it builds and passes the selected checks;
* it improves the targeted hotspot or total benchmark result enough to be meaningful;
* repeated runs are not obviously noise;
* it does not introduce a clear regression elsewhere;
* it does not violate the forbidden-change rules.

Reject an experiment if:

* the result is neutral, noisy, or slower;
* the optimization only looks good theoretically but is not measured;
* tests/builds fail;
* it touches forbidden areas or changes semantics in a risky way.

The final optimization-session summary should answer:

1. What baseline was used?
2. What targets were selected and why?
3. What branches/worktrees were created?
4. What changed in each experiment?
5. What benchmark commands and parameters were used?
6. What improved, regressed, or produced noise?
7. Which experiments should be kept, discarded, or retried later?
8. What should the next optimization session target?

## Source facts incorporated

From the `stacks-bench` README and CLI help on the `cylewitruk/stacks-core` `feat/stacks-bench` branch:

* The branch includes a Cargo alias `cargo stacks-bench ...` for running `stacks-bench` with the correct release/profile parameters; prefer that alias over direct `cargo run`.
* Global options include `--db <APP_DATA_DIR>` and `--json` before the command.
* `bench run` requires `--source <SOURCE_DIR>`, where the source directory is the Stacks node data directory containing the `chainstate` folder.
* `bench run` supports `--start-at`, `--end-at`, `--tip`, `--network`, `--count`, `--txid`, `--repetitions`, `--calibration`, `--warmup`, `--filter contract-call`, `--no-profiler-kv`, `--with-pre-naka`, and `--name`.
* `bench rerun` uses `--run-id <RUN_ID>`; omit it only for interactive selection, which the headless workflow must avoid.
* `bench list` supports `--json`, `--today`, `--since`, `--incomplete`, `--all`, `--name`, `--limit`, `--sort-by`, and `--with-args`.
* `bench show` uses `--run-id <RUN_ID>` and supports `--json` and `--profiler-hot <N>`.
* `chainstate index` requires `--source <SOURCE_DIR>` and supports `--start-at`, `--end-at`, `--count`, `--tip`, and `--network`.
* `chainstate list` lists indexed chainstate data.
* `mcp` starts an MCP stdio server for agent access to benchmark data.
* `metabase` and `explorer` launch analysis UIs.
* Benchmark data is stored by default at `~/.stacks-bench/appdata/stacks-bench.db`.
* The data directory can be overridden by `--db`, then `STACKS_BENCH_DATA_DIR`, then the default `~/.stacks-bench`.

From Codex CLI docs:

* Use `codex exec` for non-interactive automation.
* `codex exec --json` emits JSONL events including command executions, file changes, MCP calls, web searches, plan updates, and errors.
* `--output-last-message` writes the final agent message to a file.
* `workspace-write` is the correct sandbox mode for autonomous file edits without disabling the sandbox.
* `sandbox_workspace_write.network_access = true` enables network for commands.
* `sandbox_workspace_write.writable_roots` allows additional writable roots.
* MCP servers can be configured under `[mcp_servers.<name>]`.

## Recommended Codex config

Create `~/.codex/config.toml` inside the agent VM:

```toml
# Codex config for isolated agent VM demo.
# Replace /absolute/path/to/stacks-core-agentic with your actual checkout root.

model = "gpt-5.5"

approval_policy = "never"
sandbox_mode = "workspace-write"
web_search = "cached"

[sandbox_workspace_write]
network_access = true
writable_roots = ["/absolute/path/to/stacks-core-agentic"]

[projects."/absolute/path/to/stacks-core-agentic/repos/stacks-core"]
trust_level = "trusted"

[projects."/absolute/path/to/stacks-core-agentic/sessions"]
trust_level = "trusted"
```

Trust the worktree root once at bootstrap so newly created session-scoped worktrees inherit trust without per-experiment config edits:

```toml
[projects."/absolute/path/to/stacks-core-agentic/sessions"]
trust_level = "trusted"
```

That entry is recursive in practice (Codex matches the longest path prefix), but if a future Codex version tightens that, render a per-session entry into `~/.codex/config.toml.d/` from the session bootstrap step. Do not leave these pinned to `/work` if the repository is checked out elsewhere.

Set permissions:

```bash
chmod 700 ~/.codex
chmod 600 ~/.codex/config.toml
```

## Workspace preparation

Create directories:

```bash
sudo mkdir -p /work/{repos,sessions,scripts,prompts,data/stacks-bench}
sudo chown -R "$USER:$USER" /work
sudo chmod 700 /work
chmod 700 /work/data/stacks-bench
```

### Build cache

Each worktree uses its own `./target/` directory (the default). This is required for parallel agents — sharing `CARGO_TARGET_DIR` across worktrees would serialize builds and break concurrent experiments. The cost is disk: ~10–20 GB per active worktree.

`sccache` is the only acceptable cross-worktree caching layer. It is best-effort: previous attempts to use it with `stacks-core` did not work cleanly, so do not block the demo on it. If you want to retry:

```bash
# Optional, may not work with stacks-core. If it fails, unset and rebuild.
export RUSTC_WRAPPER=sccache
export SCCACHE_DIR=/work/cache/sccache
export SCCACHE_CACHE_SIZE=50G
mkdir -p "$SCCACHE_DIR"
```

Do NOT set `CARGO_INCREMENTAL=1` — it interferes with full LTO release builds, which is the build profile benchmarks use.

The `RUST_BACKTRACE=1` export remains useful:

```bash
echo 'export RUST_BACKTRACE=1' >> ~/.bashrc
```

## Clone and configure repository

```bash
git clone git@github.com:cylewitruk/stacks-core-agentic.git /work
cd /work
git submodule update --init --recursive

cd /work/repos/stacks-core

git remote add upstream git@github.com:stacks-network/stacks-core.git
# Block accidental pushes to upstream while keeping the fetch URL.
git remote set-url --delete --push upstream '.*' || true

git fetch origin
git fetch upstream

git switch feat/stacks-bench
git branch --set-upstream-to=origin/feat/stacks-bench

git status -sb
git remote -v
```

Expected branch:

```text
## feat/stacks-bench...origin/feat/stacks-bench
```

## Benchmark host tuning

Run once on the agent VM, before the first benchmark. These reduce micro-architectural noise. Values reset on reboot, so re-apply if the VM is rebooted.

```bash
# Pin all cores to the performance governor.
for c in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
  echo performance | sudo tee "$c" >/dev/null
done

# Disable SMT/HT for stable single-thread timings.
# Skip if the kernel doesn't expose this knob (some VM kernels don't).
if [ -w /sys/devices/system/cpu/smt/control ]; then
  echo off | sudo tee /sys/devices/system/cpu/smt/control >/dev/null
fi

# Disable ASLR for benchmarking only. Re-enable (echo 2) before doing anything
# security-sensitive on this VM.
echo 0 | sudo tee /proc/sys/kernel/randomize_va_space >/dev/null

# Optional: reduce IRQ migration noise on the cores benchmarks run on. Skip
# if you're not pinning benchmarks to specific cores.
```

Record the applied tuning state into `/work/.env.tuning` so the coordinator can include it in `summary.json`:

```bash
{
  echo "GOVERNOR=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)"
  echo "SMT=$(cat /sys/devices/system/cpu/smt/control 2>/dev/null || echo unknown)"
  echo "ASLR=$(cat /proc/sys/kernel/randomize_va_space)"
} > /work/.env.tuning
```

## Direct stacks-bench command model

Do not use a wrapper for the demo flow. The `feat/stacks-bench` branch already provides the stable abstraction:

```bash
cargo stacks-bench ...
```

Use that directly from each checkout/worktree.

### Setup variables

Always source `/work/.env` first; do not redefine variables inline.

```bash
set -a; source /work/.env; set +a
OPT_SESSION_ID="${OPT_SESSION_ID:-$(date +%Y%m%d-%H%M%S)}"
OPT_SESSION_DIR="$OPT_SESSIONS_ROOT/$OPT_SESSION_ID/results"
WORKTREES="$OPT_SESSIONS_ROOT/$OPT_SESSION_ID/worktrees"
mkdir -p "$OPT_SESSION_DIR" "$STACKS_BENCH_DATA_DIR" "$WORKTREES"
```

### Common range args

The canonical Nakamoto-era ranges to pick from (set via `/work/.env`):

```text
5_000_000 - 5_200_000
6_500_000 - 6_800_000
7_300_000 - 7_500_000
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
{ "success": true, "duration_secs": 12.34, "data": { "run_id": 42, "blocks": ..., "summary": { ... } } }
```

Extract the run id directly from the envelope, with a SQLite fallback in case the envelope shape ever changes:

```bash
extract_run_id() {
  local json_path="$1"
  local id
  id=$(jq -r '.data.run_id // empty' "$json_path" 2>/dev/null || true)
  if [ -z "$id" ] || [ "$id" = "null" ]; then
    id=$(sqlite3 "$STACKS_BENCH_DATA_DIR/appdata/stacks-bench.db" \
           "SELECT MAX(id) FROM benchmark_run;")
  fi
  echo "$id"
}
```

### Index chainstate

```bash
cd /work/repos/stacks-core

flock "$BENCH_LOCK" \
  cargo stacks-bench \
    --db "$STACKS_BENCH_DATA_DIR" \
    --json \
    chainstate index \
    "${COMMON_RANGE_ARGS[@]}" \
    > "$OPT_SESSION_DIR/chainstate-index.json" \
    2> "$OPT_SESSION_DIR/chainstate-index.stderr.log"
```

List indexed chainstates:

```bash
cargo stacks-bench \
  --db "$STACKS_BENCH_DATA_DIR" \
  --json \
  chainstate list \
  > "$OPT_SESSION_DIR/chainstate-list.json" \
  2> "$OPT_SESSION_DIR/chainstate-list.stderr.log"
```

### Baseline benchmark

```bash
cd "$BASE"

BENCH_NAME="baseline-$OPT_SESSION_ID"

flock "$BENCH_LOCK" \
  cargo stacks-bench \
    --db "$STACKS_BENCH_DATA_DIR" \
    --json \
    bench run \
    "${COMMON_RANGE_ARGS[@]}" \
    --name "$BENCH_NAME" \
    > "$OPT_SESSION_DIR/baseline-bench-run.json" \
    2> "$OPT_SESSION_DIR/baseline-bench-run.stderr.log"

BASELINE_RUN_ID=$(extract_run_id "$OPT_SESSION_DIR/baseline-bench-run.json")
echo "$BASELINE_RUN_ID" > "$OPT_SESSION_DIR/baseline-run-id"
```

### Baseline noise-check (optional but recommended)

Run a second iteration of the baseline against the same code, via `bench rerun`, to bound the natural noise floor before comparing experiments:

```bash
flock "$BENCH_LOCK" \
  cargo stacks-bench \
    --db "$STACKS_BENCH_DATA_DIR" \
    --json \
    bench rerun \
    --run-id "$BASELINE_RUN_ID" \
    > "$OPT_SESSION_DIR/baseline-rerun.json" \
    2> "$OPT_SESSION_DIR/baseline-rerun.stderr.log"

BASELINE_RERUN_ID=$(extract_run_id "$OPT_SESSION_DIR/baseline-rerun.json")
echo "$BASELINE_RERUN_ID" > "$OPT_SESSION_DIR/baseline-rerun-id"
```

The delta between `BASELINE_RUN_ID` and `BASELINE_RERUN_ID` is the per-host noise floor; experiment results should be compared against this, not against zero. `bench rerun` re-uses the original arguments by id, so the operator does not need to track them separately.

### Profiler hotspots and listing

```bash
cargo stacks-bench --db "$STACKS_BENCH_DATA_DIR" --json \
  bench list --all --with-args --limit 100 \
  > "$OPT_SESSION_DIR/bench-list.json" \
  2> "$OPT_SESSION_DIR/bench-list.stderr.log"

cargo stacks-bench --db "$STACKS_BENCH_DATA_DIR" --json \
  bench show --run-id "$BASELINE_RUN_ID" --profiler-hot 50 \
  > "$OPT_SESSION_DIR/baseline-profiler-hotspots.json" \
  2> "$OPT_SESSION_DIR/baseline-profiler-hotspots.stderr.log"
```

### Experiment benchmark

From an experiment worktree, using the binary copied out of the worktree by the coordinator (see "Shell coordinator outline"):

```bash
OUTPUT_DIR="$OPT_SESSION_DIR/experiments/$TARGET_ID"
mkdir -p "$OUTPUT_DIR/run-1"
BENCH_NAME="$TARGET_ID-run-1"

flock "$BENCH_LOCK" \
  "$OUTPUT_DIR/bin/stacks-bench" \
    --db "$STACKS_BENCH_DATA_DIR" \
    --json \
    bench run \
    "${COMMON_RANGE_ARGS[@]}" \
    --name "$BENCH_NAME" \
    > "$OUTPUT_DIR/run-1/bench-run.json" \
    2> "$OUTPUT_DIR/run-1/bench-run.stderr.log"

EXP_RUN_ID=$(extract_run_id "$OUTPUT_DIR/run-1/bench-run.json")
echo "$EXP_RUN_ID" >> "$OUTPUT_DIR/run-ids"
```

Repeat with `run-2`. Use the same `STACKS_BENCH_DATA_DIR` and identical benchmark arguments as the baseline.

## Shell-coordinator / agent-tier model

Once the baseline flow works, the shell coordinator orchestrates four agent tiers:

```text
Shell coordinator (run-bench-agent-coordinator.sh)
  - Runs from $FRAMEWORK_ROOT or any cwd (paths derived at runtime)
  - Phase 0:   baseline + noise-check benchmark
  - Phase 1:   launches the triage agent
  - Phase 1.5: fans out analyzer agents (parallel, one per family)
  - Phase 1.7: launches the merge agent (LLM consolidation pass)
  - Phase 2:   fans out optimizer agents (parallel, one git worktree each)
  - Phase 3:   rebuilds + serially benchmarks each accepted target binary
  - Phase 4:   runs finalize-session.sh
  - Owns BENCH_LOCK and TEST_LOCK

Triage agent (one instance, prompts/triage.md)
  - Reads profiler JSON + DB + non-targets list
  - No codebase reads
  - Emits candidates.json (family-shaped: kind + representative_ids)
  - Does NOT commit target_span — that's the analyzer's job

Analyzer agent (one per family, prompts/analyzer.md)
  - Reads $BASE deeply + runs trace queries on the family's representatives
  - Does NOT modify source
  - Emits analysis.json (status: accepted | rejected)
  - On accept: commits target_span + fix_signature

Merge agent (one instance, prompts/merge-analyses.md)
  - Reads accepted analyses
  - Identifies semantic equivalence between proposed fixes
  - Emits optimization-targets.json with merged_from provenance
  - Falls back to non-zero exit on validation failure (no silent degradation)

Optimizer agent (one per accepted target, prompts/optimizer.md)
  - Runs in exactly one experiment worktree
  - Implements the target, runs cargo nextest under TEST_LOCK
  - Leaves a release stacks-bench binary
  - Does NOT run benchmarks
```

For demo reliability, prefer the shell pipeline that launches independent `codex exec` agents over a long-lived agent that recursively spawns sub-agents. The shell pipeline is deterministic, inspectable, independently resumable per tier, and avoids nested-agent runaway behavior.

### Trust model for subagent worktrees

For the demo flow, do not use a trust helper. `codex exec` is non-interactive and is launched with explicit `--cd`, `--add-dir`, sandbox, and approval flags.

Subagents are started directly inside new session-scoped worktrees, using the **rendered** prompt (with hotspot/files baked in) — never the raw template:

```bash
codex exec \
  --cd "$WORKTREE_DIR" \
  --add-dir /work \
  --sandbox workspace-write \
  --ask-for-approval never \
  --search \
  "$(cat "$OUTPUT_DIR/optimizer-prompt.md")"
```

Project trust controls whether project-local `.codex/` config, hooks, and rules are loaded. This workflow does not rely on project-local Codex config inside generated worktrees. User-level `~/.codex/config.toml` still loads. If a future workflow starts interactive `codex` from a worktree or wants project-local `.codex/` layers from that worktree, add that exact worktree path to `~/.codex/config.toml` before launch.

## How Codex receives instructions

`codex exec` receives its task from the final positional argument:

```bash
codex exec [flags] "$(cat /work/prompts/triage.md)"   # or analyzer.md, optimizer.md
```

So the prompt file is the agent's instruction payload. CLI flags such as `--cd`, `--add-dir`, `--sandbox`, `--ask-for-approval`, and `--search` define the execution environment but do not, on their own, tell the agent what benchmark parameters or output paths to use.

Use prompt templates plus `envsubst` to render concrete prompts. This avoids nested shell quoting and makes the exact prompt inspectable after the fact.

`envsubst`'s `SHELL-FORMAT` argument lists the variables to substitute as a plain space-separated list of `$VAR` tokens (no escapes needed inside single quotes). Prefer `${VAR}` in template prose because it is visually clearer.

Before invoking Codex for any agent, source `/work/.env` and render the prompt:

```bash
set -a; source /work/.env; set +a
envsubst '$VARS_TO_EXPOSE' < /work/prompts/<prompt>.md > "$RENDERED_PATH"
```

Then invoke Codex with the rendered prompt and capture the conversation id from the JSONL event stream so the session can be resumed deterministically:

```bash
codex exec \
  --cd "$CWD" --add-dir /work [--add-dir "$BASE"] \
  --sandbox workspace-write --ask-for-approval never --search --json \
  --output-last-message "$LAST_MSG" \
  "$(cat "$RENDERED_PATH")" \
  > "$EVENTS_JSONL" 2> "$STDERR_LOG"

jq -r 'select(.conversation_id // .session_id) | (.conversation_id // .session_id)' \
  "$EVENTS_JSONL" | head -1 > "$CONVERSATION_ID_FILE"
```

Per-tier exposed variables (the rest of the template prose stays literal):

**Triage** — `prompts/triage.md`, one instance per session:

```text
$OPT_SESSION_ID $OPT_SESSION_DIR $STACKS_BENCH_DATA_DIR $BASE
$BASELINE_RUN_ID $BASELINE_RERUN_ID
```

`$STACKS_BENCH_DATA_DIR` exposes the persistent SQLite DB to the triage agent for run-over-run / cross-run analysis; `$BASE` is exposed so the agent can read the schema definitions in `${BASE}/stacks-bench/migrations/` before querying.

**Analyzer** — `prompts/analyzer.md`, one instance per family:

```text
$FAMILY_ID              # stable kebab id from candidates.json (= family id)
$OUTPUT_DIR             # analyses/<family-id>/  (cwd, writable)
$BASE                   # stable read-only checkout
$STACKS_BENCH_DATA_DIR  # SQLite DB for trace queries
$QUERIES_DIR            # pre-built triage SQL queries
$BASELINE_RUN_ID        # passed as :run_id to trace queries
$FAMILY_JSON            # the family object: kind, representative_ids,
                        # suspected_spans (hint), global_materiality
```

**Merge** — `prompts/merge-analyses.md`, one instance per session:

```text
$OPT_SESSION_ID $OPT_SESSION_DIR
$BASELINE_RUN_ID $BASELINE_RERUN_ID $NOISE_FLOOR_PCT
$OPTIMIZATION_TARGETS_SCHEMA_PATH
$CODEX_MERGE_MODEL          # configurable; default gpt-5.3-codex-spark
$ACCEPTED_ANALYSES_JSON     # JSON array of accepted analysis objects
```

**Optimizer** — `prompts/optimizer.md`, one instance per merged target:

```text
$TARGET_ID          # canonical kebab id (= fix_signature from merge)
$WORKTREE_DIR       # this experiment's git worktree (cwd, writable)
$OUTPUT_DIR         # experiments/<target-id>/
$TEST_LOCK          # flock path for serialized test runs
$TARGET_JSON        # full target object from optimization-targets.json
```

`$FAMILY_JSON`, `$ACCEPTED_ANALYSES_JSON`, and `$TARGET_JSON` are sliced/aggregated by the coordinator scripts via jq before being passed inline to each agent, so no agent scans the full session-level files.

## Prompt templates

Four prompt templates, rendered into the optimization-session dir before each `codex exec` call. The rendered prompt is the contract; the template is just an editable source.

* [prompts/triage.md](prompts/triage.md) — picks candidate workload families (no span commitment).
* [prompts/analyzer.md](prompts/analyzer.md) — deep single-family analysis; commits `target_span` + `fix_signature`.
* [prompts/merge-analyses.md](prompts/merge-analyses.md) — LLM consolidation pass; dedupes convergent analyses into canonical optimization targets.
* [prompts/optimizer.md](prompts/optimizer.md) — single-target implementation in one worktree.
* [prompts/non-targets.md](prompts/non-targets.md) — read-only reference of profiler spans the agents must NOT pursue (span-level exclusion list, NOT subtree).

All three prompt templates use `${VAR}` placeholders that the shell coordinator substitutes via `envsubst` before invoking `codex exec` (see "How Codex receives instructions"). The exposed variables are listed at the top of each template.

Each agent's output schema lives in [schemas/](schemas/) and is referenced by path from the prompt that emits it. Agents are explicitly told which schema their output must conform to.

## Guardrails for optimization work

Allowed modification areas:

* `clarity/src/vm/` — Clarity VM, database layer, cost tracking
* `stackslib/src/chainstate/stacks/index/` — MARF trie implementation
* `stackslib/src/clarity_vm/` — Clarity VM integration
* `stackslib/src/chainstate/nakamoto/` — Nakamoto block processing

Forbidden changes:

* Do not modify files under `stacks-bench/`, `testnet/`, `.github/`, or `experiments/` unless the task is explicitly to fix the benchmark harness.
* Do not add `unsafe` blocks.
* Do not remove, disable, or weaken existing tests.
* Do not change consensus-critical behavior: serialization, hashing, validation, or block/transaction acceptance semantics.
* Do not change public API signatures that other crates depend on unless there is no viable alternative and all callers are updated.

Known skip spans / non-targets:

The list of profiler spans the agents must NOT pursue lives in [prompts/non-targets.md](prompts/non-targets.md) (deployed as `/work/prompts/non-targets.md`). Both prompts reference it directly so it can be updated without touching the coordinator/optimizer prompts. Append to it as additional dead-end spans are discovered; do not duplicate the list inside the prompt templates.

Experiment discipline:

* Target exactly one profiler hotspot per experiment.
* Prefer the smallest change that could plausibly move the measured hotspot.
* Good optimization categories: read-through caches for repeated lookups, avoiding redundant allocations/clones, batching I/O, reducing call counts through memoization, and fast paths that preserve identical results.
* Caching and fast paths are allowed only when they produce identical observable results.
* Rejected/aborted analyses leave `analyses/<id>/analysis.json` with `status: rejected` (and a `reason`); the next session's triage should ingest those reasons rather than re-pursuing dead ends.
* Do not retry failed approaches unless there is new evidence that invalidates the prior result.

## Future helpers

A colleague's `experiments/run.sh` has command-level ideas worth folding into `/work/scripts/` helpers later:

| Helper | What it does |
| --- | --- |
| `status.sh` | Print branch/worktree, baseline run id, latest run id, accepted/rejected counts (cumulative across `summary.json` files). |
| `compare.sh` | Compare experiment run vs baseline (and optionally vs previous best accepted run). |
| `plot.sh` | Generate a results report (HTML or markdown) from accumulated `summary.json` files for human review. |

None of these are required for the pipeline to work — `summary.json` already records accept/reject decisions, improvement percentages, and reasons. They're nice-to-haves for review ergonomics across multiple sessions.

Things deliberately NOT carried over from `experiments/run.sh`:

* **In-place branches from the repo root.** This runbook uses git worktrees instead, which is what makes parallel optimizers possible.
* **Auto-merge of accepted experiments.** For the demo, logging acceptance is safer than mutating `feat/stacks-bench`.
* **Subagent-driven benchmarking.** The shell coordinator owns benchmarks and `BENCH_LOCK`.

## Pipeline phase scripts

Each phase is a standalone script that takes `SESSION_DIR` as its only positional arg, sources shared helpers via `_lib.sh`, and then reads `${AGENTIC_ENV_FILE:-$FRAMEWORK_ROOT/.env}` plus its file-based inputs from the session dir. Each script writes outputs back into the session dir. You can run any of them directly for a controlled walkthrough; the orchestrator just chains them in order.

| Phase | Script                  | Reads                                                 | Writes (in `SESSION_DIR`)                                                                                                            |
| ----- | ----------------------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| 0a    | `run-baseline.sh`       | `FRAMEWORK_ROOT/.env` (range vars)                    | `baseline-bench-run.json`, `baseline-rerun.json`, `baseline-{run,rerun}-id`, `bench-list.json`, `baseline-profiler-hotspots.json`    |
| 0b    | `import-baseline.sh`    | existing run id(s) in stacks-bench DB                 | same as 0a (alternative; skip the live benchmark)                                                                                    |
| 1     | `run-triage.sh`         | baseline-* artifacts                                  | `candidates.json` (v2, family-shaped), `triage-*`                                                                                    |
| 1.5   | `run-analyzers.sh`      | `candidates.json`                                     | `analyses/<family-id>/analysis.json`, `analyses/<family-id>/analyzer-*`                                                              |
| 1.7   | `merge-analyses.sh`     | `candidates.json`, `analyses/*/analysis.json`         | `optimization-targets.json`, `merge-final-message.md`, `merge-events.jsonl`, `merge-conversation-id`                                 |
| 2     | `run-optimizers.sh`     | `optimization-targets.json`                           | `experiments/<target-id>/{implementation,abort}.md`, build artifacts                                                                 |
| 3     | `bench-experiments.sh`  | per-experiment release binary                         | `experiments/<target-id>/run-{1,2}/bench-run.json`, `run-ids`                                                                        |
| 4     | `finalize-session.sh`   | targets + run-ids                                     | `summary.json` (stdout), `summary.md`                                                                                                |

Walk-through example (with an existing baseline run id of 42):

```bash
FRAMEWORK_ROOT=/path/to/stacks-core-agentic
SESSION="$FRAMEWORK_ROOT/sessions/demo-001/results"
mkdir -p "$SESSION"

"$FRAMEWORK_ROOT/scripts/import-baseline.sh" "$SESSION" 42
"$FRAMEWORK_ROOT/scripts/run-triage.sh"        "$SESSION"; jq . "$SESSION/candidates.json"
"$FRAMEWORK_ROOT/scripts/run-analyzers.sh"     "$SESSION"; ls "$SESSION/analyses"
"$FRAMEWORK_ROOT/scripts/merge-analyses.sh"    "$SESSION"; jq '.targets | length' "$SESSION/optimization-targets.json"
"$FRAMEWORK_ROOT/scripts/run-optimizers.sh"    "$SESSION"
"$FRAMEWORK_ROOT/scripts/bench-experiments.sh" "$SESSION"
"$FRAMEWORK_ROOT/scripts/finalize-session.sh"  "$SESSION" > "$SESSION/summary.json"
cat "$SESSION/summary.md"
```

If a phase fails or surprises you, re-run just that phase's script — the others stay untouched.

## Orchestrator

[`scripts/run-bench-agent-coordinator.sh`](scripts/run-bench-agent-coordinator.sh) chains the phase scripts above. Two ways to invoke:

```bash
# Fresh baseline:
/work/scripts/run-bench-agent-coordinator.sh

# Reuse an existing baseline run id (skip Phase 0 baseline run):
IMPORT_BASELINE_RUN_ID=42 IMPORT_BASELINE_RERUN_ID=43 \
  /work/scripts/run-bench-agent-coordinator.sh
```

Phase 2 fan-out parallelism is capped by `STACKS_BENCH_PARALLEL_AGENTS`; analyzer fan-out by `STACKS_BENCH_PARALLEL_ANALYZERS`. Phase 3 benchmarks are always serialized under `BENCH_LOCK`.

`finalize-session.sh` produces `summary.json` (schema v2) of the form:

```json
{
  "schema_version": 2,
  "session_id": "20260507-104400",
  "baseline_run_id": 123,
  "baseline_rerun_id": 124,
  "noise_floor_pct": 0.8,
  "experiments": [
    { "target_id": "a", "delivery_mode": "normal_pr",       "status": "accepted",        "run_ids": [125, 126], "improvement_pct": 4.7 },
    { "target_id": "b", "delivery_mode": "normal_pr",       "status": "rejected",        "run_ids": [127, 128], "reason": "within noise" },
    { "target_id": "c", "delivery_mode": "normal_pr",       "status": "aborted",         "reason": "tests failed" },
    { "target_id": "d", "delivery_mode": "consensus_poc_pr","status": "poc_landed",      "breakage_class": "clarity_cost_weight" },
    { "target_id": "e", "delivery_mode": "consensus_issue", "status": "routed_to_issue", "breakage_class": "block_validation"   }
  ],
  "outcome_counts": {
    "normal_pr":        { "accepted": 1, "rejected": 1, "aborted": 1 },
    "consensus_poc_pr": { "poc_landed": 1, "aborted": 0 },
    "consensus_issue":  { "routed_to_issue": 1, "aborted": 0 }
  },
  "lens_dispositions": [
    { "family_id": "fam-a", "lens": "tx_latency",        "status": "addressed" },
    { "family_id": "fam-x", "lens": "tenure_throughput", "status": "not_actionable", "reason": "runtime is consumed by `pow` / `keccak` Clarity primitives whose cost weights are fixed by consensus; no structural change short of a HIP can move this" }
  ],
  "next_targets_hint": "1 PR + 1 PoC PR + 1 issue of 5 targets; review and re-run rejected/aborted with refined analyses"
}
```

The `delivery_mode` field on every experiment row is propagated from `optimization-targets.json` (set by the merge phase as a derived field from `consensus_breaking` + `poc_implementable`):

* **`normal_pr`** — performance fix; `status ∈ {accepted, rejected, aborted}` driven by bench measurement.
* **`consensus_poc_pr`** — deliberate consensus-breaking change shipped as a PoC; `status ∈ {poc_landed, aborted}`. `poc_landed` means scoped tests passed; no benchmark ran by design.
* **`consensus_issue`** — consensus-breaking change too large or too coverage-blocked for PoC mode; `status ∈ {routed_to_issue, aborted}`. The optimizer was skipped entirely; the analyzer's `consensus_writeup` is the shipping artifact.

`lens_dispositions[]` is propagated verbatim from `optimization-targets.json` so "real hotspot, no fix found" cases (entries with `status: not_actionable`) survive into the operator-facing summary.

## Phase 5: Autonomous publishing — PRs and issues (optional)

After `summary.json` is written, the coordinator can optionally publish autonomous-run artifacts to GitHub. The router branches per target's `delivery_mode`:

* `normal_pr` → draft PR (or non-draft per `PUBLISH_DRAFT_PRS`) with operator-configured labels.
* `consensus_poc_pr` → draft PR ALWAYS, with operator labels plus the hardcoded safety set `consensus-change,needs-HIP,do-not-merge`.
* `consensus_issue` → GitHub issue with `consensus-change,needs-HIP` labels. The optimizer never produced an implementation; the issue body comes from the analyzer's `consensus_writeup`.

The flow is split across two scripts on purpose, so the GitHub token never sits in a process the optimizer/triage agents can read:

| Script | User | What it does |
| ------ | ---- | ---- |
| `scripts/generate-pr-artifacts.sh` | agent | Iterates `optimization-targets.json` and dispatches per `delivery_mode`. For PR modes (`normal_pr`, `consensus_poc_pr`), runs `pr-writer.md` and writes `pr-title.txt` + `pr-body.md`; the prompt branches on `${DELIVERY_MODE}` so consensus PoC PRs frame benchmark-skipped/scoped-tests/HIP-coordination explicitly. For `consensus_issue`, runs `issue-writer.md` and writes `issue-title.txt` + `issue-body.md` from the analyzer's `consensus_writeup`. Section validators enforce the required body shape per mode. Stale publish artifacts are cleared on skip and on cross-mode delivery_mode changes. |
| `scripts/publish-accepted.sh` | publisher (via `sudo -H`) | Iterates `optimization-targets.json` and dispatches per `delivery_mode`. PR modes: switches the worktree to `agentic/<session>/<target>`, stages tracked-file modifications only (`git add -u`), commits, pushes, and creates a draft PR with `gh pr create` — `consensus_poc_pr` is forced draft and gets the safety label set. `consensus_issue`: no branch / no commit / no push; uses `gh issue create` with a hidden trace tag (`<!-- agentic-<session>-<target> -->`) for idempotent re-runs. Skips on existing PR/issue. |

The token never leaves the publisher's filesystem, and the agent user has no
read access to it. The agent user's only privilege over the publisher is to
invoke the single `publish-accepted.sh` script via `sudo`.

### One-time setup

```bash
sudo /work/scripts/setup-publisher.sh
```

That script:

* creates the `publisher` system user (no login shell);
* creates `/var/lib/stacks-core-agentic/` with mode `0700`, owned by `publisher`;
* installs a sudoers stanza at `/etc/sudoers.d/stacks-core-agentic-publisher` that
  allows the agent user to run `publish-accepted.sh` as `publisher` with
  NOPASSWD, and ONLY that script (validated via `visudo -cf` before install);
* prints the final manual step: drop the GitHub PAT into the token file with
  the right ownership/mode.

The agent user, `publisher` user, the framework checkout, and the token path
are all overridable via env (`AGENT_USER`, `PUBLISH_SUDO_USER`,
`FRAMEWORK_ROOT`, `PUBLISH_TOKEN_FILE`). Re-running the script is safe.

### Enabling Phase 5

In `<FRAMEWORK_ROOT>/.env`:

```bash
PUBLISH_ACCEPTED_PRS=1
PUBLISH_DRAFT_PRS=1                    # 0 to publish ready-for-review PRs
PUBLISH_BASE_REPO=cylewitruk/stacks-core   # default — your fork, low blast radius
PUBLISH_BASE_BRANCH=feat/stacks-bench
PUBLISH_REMOTE=origin                  # the remote the worktree pushes to
PUBLISH_PR_LABELS=                     # optional: comma-separated, each emitted as --label
```

The default `PUBLISH_BASE_REPO` targets your fork, not `stacks-network/stacks-core`,
so a runaway autonomous flow lands PRs in your own UI rather than upstream.
Override only when you've reviewed a session and want to escalate.

### Re-running Phase 5

`publish-accepted.sh` checks for an existing PR (head + base) before any git
mutations and skips the target entirely if one is found. Re-running the
coordinator with the same session id is therefore idempotent for any target
whose PR already exists. New accepted targets in subsequent sessions get
their own branches via the `agentic/<session>/<target>` naming.

## Benchmark serialization

No two benchmark runs should execute simultaneously. This matters because local-agent-VM benchmarking shares CPU, memory, disk, and the persistent SQLite benchmark DB.

Use this lock file for every `cargo stacks-bench bench run`, `bench rerun`, and expensive `chainstate index` command:

```bash
BENCH_LOCK=/work/data/stacks-bench/benchmark.lock
```

Subagents should not run benchmarks by default. The shell coordinator must hold this lock around each benchmark invocation.

## Optional MCP configuration for stacks-bench

This is useful after the direct CLI loop works. Point Codex at the **pre-built** binary so MCP startup doesn't pay first-build cost (`cargo run` would invoke a build check on every startup, which can blow past `startup_timeout_sec`).

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.stacks_bench]
command = "/work/repos/stacks-core/target/release/stacks-bench"
args = ["--db", "/work/data/stacks-bench", "mcp"]
startup_timeout_sec = 30
tool_timeout_sec = 600
enabled = true
```

The bootstrap step that pre-builds `stacks-bench` (see step 7 of bootstrap) is what makes this safe. If you ever wipe `target/release/`, re-run `cargo stacks-bench --help >/dev/null` from `$BASE` before launching Codex to repopulate the binary.

For the demo, keep MCP optional. The direct command-line flow is easier to debug.

## Launch a coordinator session in tmux

```bash
OPT_SESSION_ID="$(date +%Y%m%d-%H%M%S)"
OPT_SESSIONS_ROOT=/work/sessions
OPT_SESSION_DIR="$OPT_SESSIONS_ROOT/$OPT_SESSION_ID/results"
STACKS_BENCH_DATA_DIR=/work/data/stacks-bench
mkdir -p "$OPT_SESSION_DIR" "$STACKS_BENCH_DATA_DIR"

export OPT_SESSION_ID OPT_SESSION_DIR STACKS_BENCH_DATA_DIR
# export STACKS_BENCH_CHAINSTATE_DIR=/mnt/chainstate/mainnet
# export STACKS_BENCH_START_AT=...
# export STACKS_BENCH_COUNT=...

tmux new-session -d -s stacks-bench-agent "/work/scripts/run-bench-agent-coordinator.sh"

echo "$OPT_SESSION_DIR"
echo "attach with: tmux attach -t stacks-bench-agent"
```

Watch:

```bash
tmux attach -t stacks-bench-agent
```

Detach:

```text
Ctrl+b then d
```

### One-screen observability: `tail-session.sh`

Every agent writes its own JSONL/stderr at unpredictable nesting depth. Use [scripts/tail-session.sh](scripts/tail-session.sh) (deploy as `/work/scripts/tail-session.sh`) to multiplex them with `tail -F`:

```bash
/work/scripts/tail-session.sh "$OPT_SESSION_DIR"
# or, if OPT_SESSION_DIR is exported:
/work/scripts/tail-session.sh
```

Inspect event stream summary:

```bash
jq -r 'select(.type=="item.completed") | .item.type? // empty' \
  "$OPT_SESSION_DIR/triage-events.jsonl" | sort | uniq -c
```

## Expected artifacts

The session artifact tree is the contract between phases. [scripts/validate-session.sh](scripts/validate-session.sh) (deployed at `/work/scripts/validate-session.sh`) checks for required files and exits non-zero if any are missing — call it after the coordinator script completes, or use it from a resumed session to figure out what's still pending.

```text
/work/sessions/<OPT_SESSION_ID>/results/
  # Phase 0: baseline (shell-owned)
  baseline-bench-run.json
  baseline-bench-run.stderr.log
  baseline-rerun.json
  baseline-rerun.stderr.log
  baseline-run-id            # plain text: numeric id
  baseline-rerun-id
  bench-list.json
  baseline-profiler-hotspots.json

  # Phase 1: triage agent
  triage-prompt.md           # rendered prompt (envsubst output)
  triage-events.jsonl
  triage-stderr.log
  triage-final-message.md
  triage-conversation-id
  candidates.json            # schema: schemas/candidates.schema.json
  candidates.md              # human view, derived from JSON

  # Phase 1.5: analyzer agents (one per family)
  analyses/
    <family-id>/
      analyzer-prompt.md
      analyzer-events.jsonl
      analyzer-stderr.log
      analyzer-final-message.md
      analyzer-conversation-id
      analysis.json          # schema: schemas/analysis.schema.json (status: accepted | rejected)
      analysis.md            # human-readable analysis writeup

  # Phase 1.7: merge phase (LLM consolidation)
  merge-prompt.md            # rendered prompt
  merge-events.jsonl
  merge-stderr.log
  merge-final-message.md     # audit summary including coverage check
  merge-conversation-id
  optimization-targets.json  # schema: schemas/optimization-targets.schema.json
                             # (built by scripts/merge-analyses.sh from accepted analyses;
                             # carries merged_from / convergence_count provenance)

  # Phase 2/3: optimizer subagents + serialized benchmarks (one per accepted target)
  experiments/
    <target-id>/
      optimizer-prompt.md
      subagent-events.jsonl
      subagent-stderr.log
      subagent-final-message.md
      subagent-conversation-id
      implementation.md         # OR abort.md (mutually exclusive)
      side-observations.md      # optional, future-target evidence
      nextest.log
      nextest.stderr.log
      cargo-build.log
      cargo-build.stderr.log
      cargo-clean.log
      cargo-clean.stderr.log
      bin/stacks-bench          # release binary copied from worktree
      run-ids                   # one numeric id per line
      run-1/
        bench-run.json
        bench-run.stderr.log
      run-2/
        bench-run.json
        bench-run.stderr.log

  # Phase 4: session summary
  summary.json                  # schema: schemas/summary.schema.json
  summary.md                    # human-readable, derived
```

## Continue a previous optimization session

The pipeline doesn't have a "resume one agent in place" mode — re-running the relevant phase script is faster and more predictable. Phase scripts read state from `SESSION_DIR` files, so re-running a phase against an existing session dir picks up where the prior run left off.

```bash
OPT_SESSION_DIR="/work/sessions/<OPT_SESSION_ID>/results"
/work/scripts/validate-session.sh "$OPT_SESSION_DIR"
```

If validate prints `MISSING:`, the listed paths tell you which phase to re-run. Examples:

* Missing `candidates.json` or `triage-final-message.md` → re-run `run-triage.sh`.
* Missing `analyses/<family-id>/analysis.json` for one or more families → re-run `run-analyzers.sh` (it will re-render prompts and re-invoke Codex for every family; if you want to skip already-completed analyses, hand-delete the missing dirs and re-run, or invoke Codex by hand for just the missing one).
* Missing `optimization-targets.json` or `merge-final-message.md` → re-run `merge-analyses.sh`.
* Missing `experiments/<target-id>/{implementation,abort}.md` → re-run `run-optimizers.sh`.
* Missing `experiments/<target-id>/run-N/bench-run.json` → re-run `bench-experiments.sh`.
* Missing `summary.json` → re-run `finalize-session.sh`.

Each agent's `*-conversation-id` is captured in the session dir for later inspection (e.g. `triage-conversation-id`, `analyses/<family-id>/analyzer-conversation-id`, `merge-conversation-id`, `experiments/<target-id>/subagent-conversation-id`), but resume-by-id requires CLI flags that vary across Codex versions; the re-run-the-phase approach above is the supported path.

## No targets remaining: recovery flow

If `optimization-targets.json` ends up with an empty `targets[]` array, the cause is one of:

1. **Triage emitted zero candidates.** Every workload-entry pattern fell below the noise floor, was outlier-driven, or every alternative family was investigated and rejected via the counter-search step. See `triage-final-message.md` (especially the "Rejected alternative families" section) for the agent's reasoning.
2. **Every analyzer rejected its family.** See each `analyses/<family-id>/analysis.json` for the `reason`. Common: hotspot is real but inherent / already cached / target_span overlaps with a non-target.
3. **Merge phase rejected all surviving analyses.** See `merge-final-message.md` and the `rejected_by_merge` array in `optimization-targets.json`.

Recover by one of:

1. **Wider profiler view** — re-run `bench show --profiler-hot 200` (or higher) on the same baseline, then re-run triage with the wider hotspot file. Spans that were below the top-50 cutoff sometimes contain real opportunities once the obvious ones are exhausted.
2. **Different block range** — pick the next canonical range from `/work/.env` (`5_000_000–5_200_000` → `6_500_000–6_800_000` → `7_300_000–7_500_000`). Different transaction mixes light up different hotspots. Run a fresh baseline against the new range and start a new session.
3. **Update non-targets.md** — if analyzers keep rejecting candidates for the same novel reason, append it to `prompts/non-targets.md` so the next triage pass excludes it earlier.

`summary.json`'s `next_targets_hint` field exists specifically for `finalize-session.sh` to leave a recommendation for which of these the operator should try next.

## Later benchmark-VM replacement

After the demo, introduce a small host-side benchmark orchestrator only for VM lifecycle. Keep the same logical command model:

```text
1. Build stacks-bench in the agent VM or inside the benchmark VM.
2. Create per-benchmark-run VM OS overlay.
3. Reflink clone chainstate.raw.
4. Boot benchmark VM.
5. Run the same cargo stacks-bench commands with the same --db, --json, --source, and block-range arguments.
6. Copy JSON/stderr/optimization-session artifacts back to /work/sessions/<OPT_SESSION_ID>/results.
7. Preserve /work/data/stacks-bench as the shared benchmark app-data directory.
8. Destroy VM and delete disposable run dir.
```

For the demo, skip this and run directly inside the agent VM.
