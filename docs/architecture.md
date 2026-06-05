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
  Stored under <layout.sessions_root>/<session-id>/results, which
  defaults to <layout.agent_workspace_root>/sessions/<session-id>/
  results — OUTSIDE the operator repo. (The recommended workspace
  layout deliberately lives outside the operator so branch switches
  can't wipe session bulk; see docs/session-archive.md.)

Persistent benchmark data
  stacks-bench application data and SQLite database shared across
  optimization sessions.
  Stored under <stacks_bench.data_dir>; operators typically set this
  to a workspace path (e.g. /var/lib/stacks-bench/) outside the
  operator repo for the same isolation reason.
```

## Source layout

This repo is the **tool**: prompts, schemas, models, and agent code
ship together as a versioned binary release. The target codebase
(stacks-core) and all per-session state live in the operator repo
[`stacks-bench-agentic-operator`](https://github.com/cylewitruk/stacks-bench-agentic-operator).
`sbagent` reads the target path from config (`base`); no submodule is
pinned tool-side. See [setup.md](setup.md) for the
operator-aware setup flow.

## Family-first agent architecture (schema v3)

The pipeline splits the optimization workflow across **seven agent
tiers** plus orchestrator-owned phases (run inside `sbagent`). The split
exists because each decision in the loop needs different context:
triage needs aggregate workload signal but no code; analyzers need
code + traces for a single workload; the merge phase reasons about
cross-family equivalence; optimizers need a clean implementation
environment; the results-analyzer judges measured signal against the
analyzer's hypothesis; pr-writer / issue-writer compose
operator-facing prose. Concentrating these into one agent (or making
any tier do another's job) costs quality.

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
         └─────┬─────┘ └─────┬─────┘ └─────┬─────┘
               │             │             │
               ▼             ▼             ▼          (Phase 3: orchestrator
        per-invocation candidate bench runs            runs stacks-bench per
        under bench lock; ids → candidate-run-          target invocation)
        ids.json keyed by VR.invocations[].id
               │             │             │
               └──────┬──────┴──────┬──────┘
                      ▼             ▼          one results-analyzer per
              ┌───────────┐ ┌───────────┐      bench_eligible target
              │ Results-  │ │ Results-  │      templates/results-analyzer.md
              │ analyzer  │ │ analyzer  │      → analyze/<target-id>/
              │ (target-a)│ │ (target-b)│        results-analysis.json
              │ judges    │ │ judges    │        (verdict + confidence +
              │ measured  │ │ measured  │         per-invocation breakdown +
              │ vs        │ │ vs        │         pr_body_summary)
              │ expected  │ │ expected  │
              │ _signal   │ │ _signal   │
              └─────┬─────┘ └─────┬─────┘
                    └──────┬──────┘
                           ▼          (Phase 4: finalize sources
                  ┌──────────────────┐  Experiment.improvement_pct +
                  │  Phase 4         │  status verbatim from each
                  │  finalize        │  verdict; absent → Aborted)
                  │  (orchestrator)  │  → finalize/{summary.json,
                  └────────┬─────────┘    summary.md, targets.md}
                           │
                ┌──────────┴──────────┐  Phase 5 publish: per-target
                ▼                     ▼  gate on summary status +
        ┌───────────────┐    ┌──────────────┐   verdict-present +
        │ PR-writer     │    │ Issue-writer │   verdict-content +
        │ (per shipping │    │ (per         │   confidence-floor.
        │  PR target)   │    │ consensus    │   templates/pr-writer.md /
        └───────────────┘    │ _issue       │   templates/issue-writer.md
                             │  target)     │   → optimize/<id>/
                             └──────────────┘     {pr,issue}-{title.txt,
                                                   body.md}
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
  used by Phase 3 (per-invocation candidate bench outputs under
  `<invocation-id>/bench-run.json`) and Phase 5 (publish artifacts).
- **Results-analyzer** runs in parallel after Phase 3, one per
  `bench_eligible` target. Reads the target's `verification_replay`
  (analyzer hypothesis), the `optimizer-report.json` (claim + diff),
  and the per-invocation baseline + candidate `bench-run.json` files;
  judges measured vs `expected_signal` (direction first, magnitude
  second). Commits one `verdict` (accepted | mixed | rejected) and
  one `confidence` (high | medium | low) per target. Writes
  `analyze/<target-id>/results-analysis.json` — finalize sources
  `Experiment.improvement_pct` + `Experiment.status` from this verdict.
- **PR-writer / issue-writer** run once per shipping target during
  Phase 5. PR-writer reads the verdict's `pr_body_summary` verbatim
  into the PR body's Result section; issue-writer composes the
  consensus-issue prose from the merged target. Output:
  `optimize/<target-id>/{pr,issue}-{title.txt,body.md}`.

### Orchestrator vs. agent ownership

The `sbagent` orchestrator owns: archiving the strict baseline
`stacks-bench` binary (Phase 0a) + a single baseline benchmark run
with `rerun-id` aliased to the run id (Phase 0b — no second `bench
rerun` is taken; the noise floor sources from
`triage.single_run_noise_floor_pct`), Phase 1.8 per-target baseline
calibration (one stacks-bench run per VR invocation against the
archived baseline binary), release builds + per-invocation candidate
benchmarks (Phase 3), and finalize (Phase 4) — which now **sources**
`Experiment.improvement_pct` and `Experiment.status` verbatim from
each target's verdict rather than computing them from pooled bench
means. Agents own: triage, per-family analysis, merge consolidation,
implementation, post-bench results analysis, and PR/issue
composition. This separation is what allows steps to be
deterministic and independently resumable.

For demo reliability, the orchestrator launches independent
`codex exec` agents rather than a long-lived agent that recursively
spawns sub-agents. That makes it deterministic, inspectable, and
independently resumable per tier — and avoids nested-agent runaway
behavior.

## Prompt templates

Seven prompt templates, rendered into the session dir before each
`codex exec` call. The rendered prompt is the contract; the template
is just an editable source.

| Template | Role |
| -------- | ---- |
| [crates/stacks-bench-agent/templates/triage.md](../crates/stacks-bench-agent/templates/triage.md) | Picks candidate workload families (no span commitment) |
| [crates/stacks-bench-agent/templates/analyzer.md](../crates/stacks-bench-agent/templates/analyzer.md) | Deep single-family analysis; commits `target_span` + `fix_signature` + `verification_replay.invocations[]` (one self-contained `stacks-bench bench run` per measurement intent) |
| [crates/stacks-bench-agent/templates/merge-analyses.md](../crates/stacks-bench-agent/templates/merge-analyses.md) | LLM consolidation pass; dedupes convergent analyses |
| [crates/stacks-bench-agent/templates/optimizer.md](../crates/stacks-bench-agent/templates/optimizer.md) | Single-target implementation in one worktree |
| [crates/stacks-bench-agent/templates/results-analyzer.md](../crates/stacks-bench-agent/templates/results-analyzer.md) | Per-target post-bench verdict (direction first, magnitude second); writes `pr_body_summary` |
| [crates/stacks-bench-agent/templates/pr-writer.md](../crates/stacks-bench-agent/templates/pr-writer.md) | Per-PR-target PR title + body composition; reads `pr_body_summary` verbatim |
| [crates/stacks-bench-agent/templates/issue-writer.md](../crates/stacks-bench-agent/templates/issue-writer.md) | Per-consensus_issue-target issue title + body composition |

Plus three **read-only context docs** seeded to
`<operator>/.sbagent/context/` (different bundle from prompts; tunable
but not substitution-bearing). `sbagent check` validates each
phase's required context docs are present:

- [`non-targets.md`](../context/non-targets.md) — profiler spans
  agents must NOT pursue. Referenced by triage + analyzer.
- [`bucket-anchors.md`](../context/bucket-anchors.md) — trace-segment
  taxonomy mapping span names to value lenses.
- [`stacks-domain-context.md`](../context/stacks-domain-context.md) —
  Clarity cost axes, validation-path coverage gaps, scale anchors.
  Referenced by analyzer + results-analyzer.

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

Templates render through MiniJinja in strict mode, so each variable
appears in the template as `{{ snake_case_field }}`. The canonical
type for each phase's variable set is the matching
[`*Prompt`](../crates/stacks-bench-agent/src/prompts.rs) struct;
substitution drift is caught by `sbagent prompt lint`. The lists
below are the high-signal fields per phase — see the structs for the
full set.

**Triage** — one instance per session:

```text
opt_session_id, opt_session_dir, stacks_bench_data_dir, base,
baseline_run_id, baseline_rerun_id
```

`stacks_bench_data_dir` exposes the persistent SQLite DB for
run-over-run / cross-run analysis; `base` is exposed so the agent can
read the schema definitions in `<base>/stacks-bench/migrations/`
before querying.

**Analyzer** — one instance per family:

```text
family_id              # stable kebab id from triage/candidates.json
output_dir             # analysis/<family-id>/  (cwd, writable)
base                   # stable read-only checkout
stacks_bench_data_dir  # SQLite DB for trace queries
queries_dir            # pre-built triage SQL queries
baseline_run_id        # passed as :run_id to trace queries
family_json            # the family object: kind, representative_ids,
                       # suspected_spans (hint), global_materiality
max_invocations_per_target  # operator cap on emitted invocations[];
                            # defaults to 8 (schema hard max 16)
```

**Merge** — one instance per session:

```text
opt_session_id, opt_session_dir,
baseline_run_id, baseline_rerun_id, noise_floor_pct,
optimization_targets_schema_path,
codex_merge_model,         # configurable; default gpt-5.5
accepted_analyses_json     # JSON array of accepted analysis objects
```

**Optimizer** — one instance per merged target:

```text
worktree_dir       # this target's per-target git clone (cwd, writable)
output_dir         # optimize/<target-id>/
test_lock          # flock path for serialized test runs
target_json        # full target object from merge/optimization-targets.json
```

**Results-analyzer** — one instance per `bench_eligible` target:

```text
session_id, target_id,
output_dir,                       # analyze/<target-id>/  (writable)
base, stacks_bench_data_dir, queries_dir,
target_json, optimizer_report_json,
candidate_invocations_dir,        # optimize/<target>/  (per-inv bench-run.json)
baseline_invocations_dir,         # verify/<target>/  (per-inv bench-run.json)
candidate_run_ids_path,           # optimize/<target>/candidate-run-ids.json
baseline_run_ids_path,            # verify/<target>/baseline-run-ids.json
results_analysis_schema_path
```

**PR-writer** — one instance per shipping PR target:

```text
opt_session_id, target_id,
output_dir,                     # optimize/<target-id>/  (writable)
worktree_dir,
target_json, experiment_json,
results_analysis_json,          # verdict carrying pr_body_summary +
                                # per-invocation + caveats; "{}" when
                                # absent (consensus_poc_pr / aborted)
delivery_mode                   # "normal_pr" | "consensus_poc_pr"
```

**Issue-writer** — one instance per `consensus_issue` target:

```text
opt_session_id, target_id,
output_dir,                     # optimize/<target-id>/  (writable)
target_json
```

`family_json`, `accepted_analyses_json`, `target_json`,
`optimizer_report_json`, and `results_analysis_json` are
sliced/aggregated by `sbagent` before being passed inline to each
agent, so no agent scans the full session-level files.

Triage and merge agents are invoked with `cwd` set to their phase dir
(`triage/` and `merge/` respectively); analyzer / results-analyzer /
PR-writer / issue-writer are invoked with `cwd` set to their per-id
output dir. Relative writes (drilldown CSVs, ad-hoc notes) land
inside the right phase folder automatically rather than polluting the
session root.

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
[context/non-targets.md](../context/non-targets.md). The triage and
analyzer prompts reference it by absolute path (rendered into the
prompt at invoke time) so it can be updated without touching the
templates themselves. Append to it as additional dead-end spans are
discovered; do not duplicate the list inside the templates.

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
