# Architecture

How the autonomous optimization pipeline is organized: which agent owns
which decision, what they read, and what they emit.

## Terminology

```text
Optimization session
  One full outer pass: baseline, target ranking, worktree experiments,
  comparison, summary. Identified by SBAGENT_SESSION_ID.

Benchmark run
  One `cargo stacks-bench bench run` record stored in the persistent
  stacks-bench SQLite DB. Identified by a stacks-bench run id.

Optimization-session artifacts
  JSON snapshots, stderr logs, Codex JSONL event streams, notes, and
  summaries for one optimization session.
  Stored under <sessions_root>/<session-id>/results (defaults to
  <operator>/sessions/<session-id>/results).

Persistent benchmark data
  stacks-bench application data and SQLite database shared across
  optimization sessions.
  Stored under <stacks_bench_data_dir> (defaults to
  <operator>/data/stacks-bench).
```

## Source layout

This repo is the **tool**: prompts, schemas, models, and agent code
ship together as a versioned binary release. The target codebase
(stacks-core) and all per-session state live in the operator repo
[`stacks-bench-agentic-operator`](https://github.com/cylewitruk/stacks-bench-agentic-operator).
`sbagent` reads the target path from config (`base`); no submodule is
pinned tool-side. See [setup.md](setup.md) for the
operator-aware setup flow.

## Family-first agent architecture (schema v2)

The pipeline splits the optimization workflow across four agent tiers
plus orchestrator-owned phases (run inside `sbagent`). The split
exists because each decision in the loop needs different context:
triage needs aggregate workload signal but no code; analyzers need
code + traces for a single workload; the merge phase reasons about
cross-family equivalence; optimizers need a clean implementation
environment. Concentrating these into one agent (or making any tier
do another's job) costs quality.

Crucially, **triage does NOT commit a target span**. Its job is to
identify WHAT to investigate (representative txs, blocks, or
contract.functions). The analyzer commits the span identity using its
full trace + code context, and the merge phase deduplicates analyses
that converge on the same fix.

```text
                     [profiler data + workload]
                            │
                            ▼
             ┌──────────────────────────────┐
             │  Triage agent (1 instance)   │   templates/triage.md
             │  • profiler JSON + DB        │   → triage/candidates.json
             │  • picks workload entry      │     {kind, representative_ids,
             │    points; NOT span identity │      suspected_spans?, ...}
             │  • no codebase exploration   │
             └──────────────┬───────────────┘
                            │ one analyzer per family
                ┌───────────┴────────────┐
                ▼           ▼            ▼
        ┌───────────┐ ┌───────────┐ ┌───────────┐
        │ Analyzer  │ │ Analyzer  │ │ Analyzer  │   templates/analyzer.md
        │ (fam-A)   │ │ (fam-B)   │ │ (fam-C)   │   → analysis/<family-id>/
        │ traces +  │ │ traces +  │ │ traces +  │     analysis.json
        │ code      │ │ code      │ │ code      │     {target_span,
        │           │ │           │ │           │      fix_signature, ...}
        └─────┬─────┘ └─────┬─────┘ └─────┬─────┘
              └─────────────┼─────────────┘
                            ▼
                  ┌──────────────────────┐
                  │  Merge agent         │   templates/merge-analyses.md
                  │  (1 instance, LLM)   │   → merge/optimization-targets.json
                  │  • dedup convergent  │     (canonical fix per target;
                  │    fixes by          │      merged_from records the
                  │    structural        │      contributing families)
                  │    equivalence       │
                  └──────────┬───────────┘
                             │ one optimizer per merged target
                 ┌───────────┴────────────┐
                 ▼           ▼            ▼
         ┌───────────┐ ┌───────────┐ ┌───────────┐
         │ Optimizer │ │ Optimizer │ │ Optimizer │   templates/optimizer.md
         │ (target-a)│ │ (target-b)│ │ (target-c)│   → optimize/<id>/...
         │ worktree  │ │ worktree  │ │ worktree  │
         └───────────┘ └───────────┘ └───────────┘
```

### Per tier

- **Triage** runs once. Picks 0..N candidate families
  (`tx_family` / `block_family` / `contract_family`). Reads profiler
  JSON + the SQLite DB but no source code. Produces
  `triage/candidates.json`.
- **Analyzer** runs in parallel, one per family. Each gets its full
  context budget for one family; reads `${BASE}` deeply, runs trace
  queries on the family's representative ids, commits `target_span` +
  `fix_signature`. Produces `analysis/<family-id>/analysis.json` with
  `status: accepted | rejected`.
- **Merge** runs once over the accepted analyses (LLM consolidation
  pass; smaller / faster model is appropriate here). Identifies
  analyses that propose the same structural change and collapses them
  into a single optimization target with cross-family provenance.
  Produces `merge/optimization-targets.json`. The coverage invariant
  is enforced: every accepted family appears in exactly one target's
  `merged_from` or in `rejected_by_merge`.
- **Optimizer** runs in parallel, one per merged target, each in its
  own per-target git clone. Implements the change, runs tests, leaves
  a release binary for the bench phase to measure. Writes into
  `optimize/<target-id>/` — the shared per-target audit folder also
  used by Phase 3 (bench `run-N/`) and Phase 5 (publish artifacts).

### Orchestrator vs. agent ownership

The `sbagent` orchestrator owns: baseline + noise-check benchmarks
(Phase 0), release builds + serialized benchmarks (Phase 3), and
summary generation (Phase 4). Agents own: triage, per-family
analysis, merge consolidation, implementation. This separation is
what allows steps to be deterministic and independently resumable.

For demo reliability, the orchestrator launches independent
`codex exec` agents rather than a long-lived agent that recursively
spawns sub-agents. That makes it deterministic, inspectable, and
independently resumable per tier — and avoids nested-agent runaway
behavior.

## Prompt templates

Five prompt templates, rendered into the session dir before each
`codex exec` call. The rendered prompt is the contract; the template
is just an editable source.

| Template | Role |
| -------- | ---- |
| [crates/stacks-bench-agent/templates/triage.md](../crates/stacks-bench-agent/templates/triage.md) | Picks candidate workload families (no span commitment) |
| [crates/stacks-bench-agent/templates/analyzer.md](../crates/stacks-bench-agent/templates/analyzer.md) | Deep single-family analysis; commits `target_span` + `fix_signature` |
| [crates/stacks-bench-agent/templates/merge-analyses.md](../crates/stacks-bench-agent/templates/merge-analyses.md) | LLM consolidation pass; dedupes convergent analyses |
| [crates/stacks-bench-agent/templates/optimizer.md](../crates/stacks-bench-agent/templates/optimizer.md) | Single-target implementation in one worktree |
| [prompts/non-targets.md](../prompts/non-targets.md) | Read-only reference of profiler spans agents must NOT pursue. Bundled + seeded to `<operator>/.sbagent/prompts/non-targets.md`, read from there at runtime. |

The substitution-bearing templates are bundled into the `sbagent`
binary via `include_str!` and seeded to the operator's
`.sbagent/prompts/` by `sbagent init` (and refreshed by every
`sbagent sync` — the bundled prompts are the contract surface;
operator edits survive only with `--keep-tunables`). At runtime
they render through
**[MiniJinja](https://github.com/mitsuhiko/minijinja) in strict mode**
— undefined variables are a hard error, not a silent empty string.
Each phase has a typed [`Prompt`](../crates/stacks-bench-agent/src/prompts.rs)
struct whose fields ARE the template's variables, so a typo gets
caught either at runtime (with the actual phase context) or earlier
via `sbagent prompt lint`, which dry-renders every on-disk template
against a synthetic field-complete context.

Because prompts are on disk in the operator dir, **operators retune
them without rebuilding the tool** — autoresearch's `program.md`
model. `sbagent check` warns on drift between the on-disk copy and
the binary's bundle so an operator who upgrades sbagent sees that
their tunes may now lag the new defaults.

Each agent's output schema lives in
[schemas/](../schemas/) (bundle source) and is embedded into the
binary the same way as prompts. Runtime reads are from
`<operator>/.sbagent/schemas/` — `sbagent check` **fails** on schema
drift (the operator would validate agent output against a different
contract than the binary expects). Fix with `sbagent sync`.

### Per-tier exposed variables

**Triage** — one instance per session:

```text
$OPT_SESSION_ID $OPT_SESSION_DIR $STACKS_BENCH_DATA_DIR $BASE
$BASELINE_RUN_ID $BASELINE_RERUN_ID
```

`$STACKS_BENCH_DATA_DIR` exposes the persistent SQLite DB to the
triage agent for run-over-run / cross-run analysis; `$BASE` is exposed
so the agent can read the schema definitions in
`${BASE}/stacks-bench/migrations/` before querying.

**Analyzer** — one instance per family:

```text
$FAMILY_ID              # stable kebab id from triage/candidates.json
$OUTPUT_DIR             # analysis/<family-id>/  (cwd, writable)
$BASE                   # stable read-only checkout
$STACKS_BENCH_DATA_DIR  # SQLite DB for trace queries
$QUERIES_DIR            # pre-built triage SQL queries
$BASELINE_RUN_ID        # passed as :run_id to trace queries
$FAMILY_JSON            # the family object: kind, representative_ids,
                        # suspected_spans (hint), global_materiality
```

**Merge** — one instance per session:

```text
$OPT_SESSION_ID $OPT_SESSION_DIR
$BASELINE_RUN_ID $BASELINE_RERUN_ID $NOISE_FLOOR_PCT
$OPTIMIZATION_TARGETS_SCHEMA_PATH
$CODEX_MERGE_MODEL          # configurable; default gpt-5.3-codex-spark
$ACCEPTED_ANALYSES_JSON     # JSON array of accepted analysis objects
```

**Optimizer** — one instance per merged target:

```text
$WORKTREE_DIR       # this target's per-target git clone (cwd, writable)
$OUTPUT_DIR         # optimize/<target-id>/
$TEST_LOCK          # flock path for serialized test runs
$TARGET_JSON        # full target object from merge/optimization-targets.json
```

`$FAMILY_JSON`, `$ACCEPTED_ANALYSES_JSON`, and `$TARGET_JSON` are
sliced/aggregated by `sbagent` before being passed inline to each
agent, so no agent scans the full session-level files.

Triage and merge agents are invoked with `cwd` set to their phase dir
(`triage/` and `merge/` respectively), so any relative writes the
agent makes — drilldown CSVs, ad-hoc notes — land inside the right
phase folder automatically rather than polluting the session root.

## Trust model for subagent worktrees

`codex exec` is non-interactive and is launched with explicit `--cd`,
`--add-dir`, sandbox, and approval flags. Subagents start directly
inside new session-scoped worktrees, using the **rendered** prompt
(with hotspot/files baked in) — never the raw template.

Project trust controls whether project-local `.codex/` config, hooks,
and rules are loaded. This workflow does not rely on project-local
Codex config inside generated worktrees. User-level
`~/.codex/config.toml` still loads. If a future workflow starts
interactive `codex` from a worktree or wants project-local `.codex/`
layers from that worktree, add that exact worktree path to
`~/.codex/config.toml` before launch.

## Guardrails for optimization work

Allowed modification areas:

- `clarity/src/vm/` — Clarity VM, database layer, cost tracking
- `stackslib/src/chainstate/stacks/index/` — MARF trie implementation
- `stackslib/src/clarity_vm/` — Clarity VM integration
- `stackslib/src/chainstate/nakamoto/` — Nakamoto block processing

Forbidden changes:

- Do not modify files under `stacks-bench/`, `testnet/`, or
  `.github/` unless the task is explicitly to fix the benchmark
  harness.
- Do not add `unsafe` blocks.
- Do not remove, disable, or weaken existing tests.
- Do not change consensus-critical behavior: serialization, hashing,
  validation, or block/transaction acceptance semantics — unless the
  target's `delivery_mode` is `consensus_poc_pr` or `consensus_issue`,
  in which case the change is deliberately consensus-breaking and
  shipped under the PoC-PR/issue safety rails (see
  [publishing.md](publishing.md)).
- Do not change public API signatures that other crates depend on
  unless there is no viable alternative and all callers are updated.

The list of profiler spans the agents must NOT pursue lives in
[prompts/non-targets.md](../prompts/non-targets.md). Both prompts
reference it directly so it can be updated without touching the
coordinator/optimizer prompts. Append to it as additional dead-end
spans are discovered; do not duplicate the list inside the templates.

### Experiment discipline

- Target exactly one profiler hotspot per experiment.
- Prefer the smallest change that could plausibly move the measured
  hotspot.
- Good optimization categories: read-through caches for repeated
  lookups, avoiding redundant allocations/clones, batching I/O,
  reducing call counts through memoization, fast paths that preserve
  identical results.
- Caching and fast paths are allowed only when they produce identical
  observable results.
- Rejected/aborted analyses leave `analysis/<family-id>/analysis.json`
  with `status: rejected` (and a `reason`); the next session's triage
  should ingest those reasons rather than re-pursuing dead ends.
- Do not retry failed approaches unless there is new evidence that
  invalidates the prior result.
