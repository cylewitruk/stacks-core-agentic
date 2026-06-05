# Workflow

The phase-by-phase contract: what each phase reads, what it writes,
and how to drive them by hand or as a single chained run.

## Tier responsibilities

The `sbagent` orchestrator does not make analytical decisions —
every analytical decision lives in an agent prompt. The
orchestrator's responsibilities are mechanical:

| Tier | Owns | Does NOT |
| --- | --- | --- |
| Triage | Pick candidate workload families (tx / block / contract) from profiler data + DB. | Read source code. Commit a target span. Run benchmarks. |
| Analyzer | Investigate ONE family deeply; commit `target_span` + `fix_signature` + `verification_replay.invocations[]` (the measurement protocol); produce an analysis the merge + optimizer phases act on. | Modify source code. Run benchmarks. |
| Merge | Dedupe analyses converging on the same structural fix; emit one canonical target per fix with `merged_from` provenance. | Re-investigate. Modify analyses' substance. |
| Optimizer | Implement the change in a per-target git clone; run `cargo nextest`; leave a release binary. | Run benchmarks. Touch other clones. |
| Results-analyzer | Per `bench_eligible` target, judge measured vs `expected_signal` (direction first, magnitude second); commit one `verdict` + `confidence`; write `pr_body_summary` for the PR. | Re-run benchmarks. Edit source. Modify the optimizer's diff. |
| PR-writer / issue-writer | Per shipping target, compose PR title + body (PR) or issue title + body (consensus_issue) from the verdict + target + experiment record. | Push to GitHub (orchestrator owns `git push` + octocrab). Re-judge the verdict. |

Benchmarking is centralized in the bench phase so all experiments use
the same parameters and the same lock. `cargo nextest` runs (in
optimizer phase) are serialized across parallel optimizers via the
test lock to avoid port/dir conflicts.

## Pipeline phases

Each phase is a typed `sbagent` subcommand that takes `--session-id`
and reads `config.toml` plus its file-based inputs from the session
dir. Each phase writes outputs back into the session dir. You can run
any of them directly for a controlled walkthrough; `sbagent session
run` just chains them in order.

Each phase writes into a dedicated subdir under `<session>/results/`
(`baseline/`, `triage/`, `analysis/<family>/`, `merge/`,
`optimize/<target>/`, `finalize/`) so a tail / audit reader can locate
"everything for phase X" or "everything for target Y" without
grepping. The `optimize/<target>/` dir is shared across Phases 2, 3,
and 5 by design — one folder per target holds the optimizer,
benchmark, and publish artifacts for that target.

| Phase | Command | Reads | Writes (in `<session>/results`) |
| --- | --- | --- | --- |
| 0a | (runs at session start; no standalone command) | `repos/stacks-core` HEAD | `baseline/bin/{stacks-bench, manifest.json}` — archived binary + `{source_sha, dirty, cargo_version, build_flags, archived_at}` manifest. Strict-binary contract: every subsequent baseline / calibration / candidate invocation uses this archived path; no silent rebuild fallback. |
| 0b | `sbagent session baseline run` OR `sbagent session baseline import` | `config.toml` (range fields), or existing run id(s) in stacks-bench DB | `baseline/{bench-run.json, rerun.json, run-id, rerun-id, bench-list.json, profiler-hotspots.json, noise-floor-pct}`. Single `bench run` invocation; rerun id aliased to run id; `noise-floor-pct` sources from `triage.single_run_noise_floor_pct` (default 1%). |
| 1 | `sbagent session triage run` | baseline artifacts | `triage/{candidates.json, candidates.md, prompt.md, events.jsonl, stderr.log, final-message.md, conversation-id, queries/, drilldowns/}` |
| 1.5 | `sbagent session analysis run` | `triage/candidates.json` | `analysis/<family-id>/{analysis.json, analysis.md, prompt.md, events.jsonl, stderr.log, final-message.md, conversation-id}` |
| 1.7 | `sbagent session analysis merge` | `triage/candidates.json`, `analysis/*/analysis.json` | `merge/{optimization-targets.json, prompt.md, events.jsonl, stderr.log, final-message.md, conversation-id}` |
| 1.8 | (runs after merge, before optimize; no standalone command) | `merge/optimization-targets.json`, `baseline/bin/stacks-bench` | `verify/<target>/<invocation-id>/bench-run.json`, `verify/<target>/baseline-run-ids.json`. Per-target calibration for every `normal_pr` target — one `stacks-bench bench run` per `verification_replay.invocations[]` entry. Pass 1c invariant: `verification_replay` is required on every `bench_eligible` target; missing → merge validation hard-fails before this phase ever runs. |
| 2 | `sbagent session optimize run` | `merge/optimization-targets.json` | `optimize/<target-id>/{prompt.md, events.jsonl, final-message.md, conversation-id, optimizer-report.json, implementation.md OR abort.md OR consensus-issue.md, nextest.log, cargo-build.log, stderr.log}` |
| 3 | `sbagent session bench run` | per-target release binary built in Phase 2 | `optimize/<target-id>/<invocation-id>/bench-run.json`, `optimize/<target-id>/candidate-run-ids.json`, `optimize/<target-id>/bin/stacks-bench`. One `stacks-bench bench run` per invocation, mirroring Phase 1.8. |
| 3.5 | `sbagent session analyze-results run` | `merge/optimization-targets.json`, `verify/<target>/...`, `optimize/<target>/...`, bench DB (read-only) | `analyze/<target-id>/{results-analysis.json, results-analysis.md, prompt.md, events.jsonl, stderr.log, final-message.md, conversation-id}`. Per-target results-analyzer agent fan-out (parallel under `analyzer.concurrency_cap`) — judges measured vs `expected_signal` and writes a typed verdict. Phase 4 sources `improvement_pct` + `status` from this file. |
| 4 | `sbagent session finalize run` | `merge/optimization-targets.json`, `analyze/<target>/results-analysis.json`, `verify/<target>/baseline-run-ids.json`, `optimize/<target>/candidate-run-ids.json` | `finalize/{summary.json, summary.md, targets.md}`. `Experiment.improvement_pct` + `Experiment.status` sourced verbatim from each target's Phase 3.5 verdict; missing verdict → Aborted. Missing baseline file → hard error. Candidate / baseline id sets MUST match the target's VR invocation set; mismatched candidate → Aborted. |
| 5 | `sbagent session publish [--dry-run]` | `merge/optimization-targets.json`, `finalize/summary.json`, `analyze/<target>/results-analysis.json` | `optimize/<target-id>/{pr,issue}-{title.txt,body.md}`; PRs/issues on GitHub. `normal_pr` targets ship only when (a) `summary.experiments[].status == Accepted`, (b) the canonical verdict is on disk + context-valid, (c) `verdict ∈ {accepted, mixed}`, and (d) `confidence >= results_analysis.confidence_floor` (default `medium`). Anything below is skipped with an explicit reason. |
| 6 | `sbagent session archive [--dry-run]` | all session bulk + operator git repo | nothing in `<session>/results` (writes happen in the operator repo — see "Filesystem & git layout" below) |

Every phase that produces artifacts has a matching `clean` subcommand
(`sbagent session <phase> clean`) that removes its outputs
idempotently. Use it when re-running a phase from scratch.

## Filesystem & git layout

Three sets of paths matter at runtime. Each lives outside the others
on purpose: the operator repo's **primary worktree** never holds
session bulk, the bulk lives in scratch space outside any tracked git
repo, and the per-target optimizer clones are stand-alone git
checkouts. Phase 6 archive creates a separate transient operator
worktree that DOES hold the bulk — but only inside that transient
checkout, never in the primary one.

### Paths derived from config

| Where | Default | What it holds |
| --- | --- | --- |
| **Session bulk** | `<layout.sessions_root>/<id>/results/`, where `layout.sessions_root` defaults to `<layout.agent_workspace_root>/sessions/` (so the bulk dir is `<layout.agent_workspace_root>/sessions/<id>/results/`) | All phase outputs (baseline, triage, analyses, merge, optimize, finalize). The audit trail. |
| **Per-target optimizer clones** | `<layout.agent_workspace_root>/optimizers/<id>/<target-id>/` | Stand-alone git clones of the operator's `repos/stacks-core` submodule (`git clone --reference --branch --local`, sharing the base's object store), one per merged target. Each holds an `agentic/<id>/<target-id>` branch. |
| **Archive worktree (transient)** | `<layout.agent_workspace_root>/archive-worktrees/<id>/` | Created during Phase 6 as a `git worktree add` of the operator repo, torn down at the end. The only place `sessions/<id>/` ever appears tracked in an operator-repo working tree. |
| **Persistent stacks-bench app data** | `<stacks_bench.data_dir>/appdata/stacks-bench.db` | Indexed chainstate + cross-session `benchmark_run` rows. Never per-session. |
| **Coordination lockfiles** | `<lock_dir>/{benchmark.lock,test.lock}` | `fd-lock`-held during bench runs (cross-process) and optimizer `cargo nextest` (cross-clone). |
| **Operator git repo** | `<layout.operator_repo_root>/` (no default; explicit setting required for archive) | Primary worktree holds `sessions.jsonl` ledger on `main` + serves as the source for `session/<id>` archive branches. Never holds session bulk in its **primary** working tree (the transient archive worktree above is the exception). |

`<layout.agent_workspace_root>` should be a path OUTSIDE the operator repo
(e.g. `/private/tmp/sbagent-workspaces/`). Anchoring session bulk
outside the operator repo avoids the "branch switch wipes
tracked-on-source-not-on-dest-but-ignored files" hazard the archive
flow's worktree-based design exists to prevent.

### Per-phase git activity

Most phases are pure file I/O against the session-bulk dir. Three
phases produce git effects:

| Phase | Git ops |
| --- | --- |
| **2 (optimize)** | For each merged target, `git clone --reference <base> --branch <base_branch> --local <base> <checkout>` into `<workspace>/optimizers/<id>/<target>/` (shares the base's object store, fresh refs), then `git switch -c agentic/<id>/<target>`. The optimizer agent edits files inside the clone but does NOT commit — the coordinator runs `coordinator_commit_if_kept` AFTER validating `optimizer-report.json`, and only then stages + commits the change authored as `Stacks BenchBot`. This sandbox/coordinator boundary keeps a misbehaving agent from producing arbitrary commits. Aborted targets' clones are pruned at session end; accepted ones stay until manually cleaned. |
| **5 (publish)**, when run with PAT | Per accepted normal-PR / consensus-PoC-PR target, `git push -u <publish.remote> agentic/<id>/<target>` from inside the optimizer clone. PAT travels via `http.<prefix>.extraheader` env override — never argv, never `.git/config`. Then octocrab opens the PR / issue against `publish.base_repo` (defaults: `cylewitruk/stacks-core`). |
| **6 (archive)** | `git worktree add <workspace>/archive-worktrees/<id>` against the operator repo, branched as `session/<id>`. Bulk is COPIED (not moved) into the worktree's `sessions/<id>/`, force-added, committed as `Stacks BenchBot` (no GPG signing). Worktree pushed to operator's `origin`. Worktree torn down. Then on the operator's main worktree: pull-rebase, append one line to `sessions.jsonl`, commit, push with retry on race. |

`agentic/<id>/<target-id>` branches stay in their per-target clones —
they are never pushed to the operator repo. They reach GitHub only
via Phase 5 publish, which pushes them directly to the
upstream `stacks-core` fork (e.g. `cylewitruk/stacks-core`).

`session/<id>` branches DO get pushed to the operator's own fork
(e.g. `stacks-bench-bot/stacks-core-autopilot`). They are
**write-once after archive** — never re-pushed, never edited. The
`SessionRecord`'s `artifact_sha` field is the permanent anchor; if
anyone moves the branch after archive, that field becomes a lie.
See [session-archive.md](session-archive.md) for the full contract.

### Side effects, by destination

| Destination | What gets written | When |
| --- | --- | --- |
| Local disk (session bulk) | All phase artifacts (events, prompts, JSON outputs, CSVs, logs) | Phases 0–4 |
| Local disk (per-target clones) | Per-target git clone + optimizer commits + release binary | Phase 2 |
| Local disk (archive worktree) | Transient worktree, removed at phase end | Phase 6 |
| stacks-bench DB | New `benchmark_run` rows: one for the Phase 0b baseline, one per Phase 1.8 calibration invocation per `normal_pr` target with `verification_replay`, one per Phase 3 candidate invocation. No separate rerun row — Phase 0b aliases `rerun-id` to the single baseline run id. | Phases 0b, 1.8, 3 |
| Operator repo, main branch | One new commit per archive run: `archive: ledger <id>` (appends one JSONL line to `sessions.jsonl`) | Phase 6, when not dry-run |
| Operator repo, write-once branches | New `session/<id>` branch with the full session bulk committed under `sessions/<id>/` | Phase 6, when not dry-run |
| Operator fork on GitHub | Push of operator-main commit + push of `session/<id>` branch | Phase 6, when not dry-run + remote configured |
| stacks-core fork on GitHub (`cylewitruk/stacks-core` by default) | Push of `agentic/<id>/<target>` branches + opened PRs / issues | Phase 5, when `--publish-accepted-prs` |

The two-fork split is deliberate: PRs land where the upstream
maintainers can review them (`stacks-core` repo); the bot's own
operational ledger and archive branches land where they don't pollute
the target repo's branch listing (`stacks-core-autopilot` repo).

## Walkthrough (with an existing baseline run id of 42)

```bash
SESSION_ID=demo-001

sbagent session baseline import --session-id "$SESSION_ID" --run-id 42
sbagent session triage run --session-id "$SESSION_ID"
jq . "/private/tmp/sbagent-workspaces/sessions/$SESSION_ID/results/triage/candidates.json"

sbagent session analysis run --session-id "$SESSION_ID"
sbagent session analysis merge --session-id "$SESSION_ID"
jq '.targets | length' "/private/tmp/sbagent-workspaces/sessions/$SESSION_ID/results/merge/optimization-targets.json"

sbagent session optimize run --session-id "$SESSION_ID"
sbagent session bench run --session-id "$SESSION_ID"
sbagent session analyze-results run --session-id "$SESSION_ID"
sbagent session finalize run --session-id "$SESSION_ID"
sbagent session archive --session-id "$SESSION_ID"  # optional, commits to operator repo
cat "/private/tmp/sbagent-workspaces/sessions/$SESSION_ID/results/finalize/summary.md"
```

If a phase fails or surprises you, re-run just that phase — the
others stay untouched. See [operations.md](operations.md#recovery)
for the resume-from-validate flow.

## Orchestrator: `sbagent session run`

`sbagent session run` chains phases 0–4 (including Phase 1.8
calibration and Phase 3.5 results-analyzer fan-out). Phase 5
(publish) and Phase 6 (archive) are opt-in via flags. Common
invocations:

```bash
# Fresh baseline (mints a YYYYMMDD-HHMMSS session id), no publish or archive:
sbagent session run --start-at 5000000 --count 200 --warmup 50

# Reuse an existing baseline run id (skips Phase 0 baseline run):
sbagent session run \
  --import-baseline-run-id 42 \
  --import-baseline-rerun-id 43

# Full pipeline including archive (commits + pushes to operator repo at end):
sbagent session run --start-at 5000000 --count 200 --warmup 50 --archive

# Full pipeline including publish (PRs to upstream stacks-core) AND archive:
sbagent session run \
  --start-at 5000000 --count 200 --warmup 50 \
  --publish-accepted-prs \
  --archive
```

Phase 2 (optimizer) fan-out parallelism is set by `--parallel-agents`
(with an internal clamp on top of whatever the operator requested).
Analyzer fan-out is set by `--parallel-analyzers`, capped by the
optional `analyzer.concurrency_cap` setting. The Phase 3.5
results-analyzer fan-out reuses the same `analyzer.concurrency_cap`
(its per-target workload is shaped the same way). Phase 3 benchmarks
are always serialized under the bench lock. Phase 5 push and Phase 6
push both use the same PAT-via-env mechanism — the token never
enters argv, `.git/config`, or shell history.

## Top-level workflow

### First optimization session: establish the baseline path

Goal: prove that the full pipeline runs end-to-end on a fresh agent
VM. Output quality is secondary on the first run; what matters is
that every phase artifact appears and `sbagent session validate`
reports `OK`.

The first session should:

1. Materialize the workspace at `<layout.agent_workspace_root>/sessions/<id>/results/`.
2. Use `<stacks_bench.data_dir>` as the shared stacks-bench app-data
   dir (indexed chainstate persists across sessions).
3. Set benchmark parameters explicitly on the CLI (or in
   `config.toml`); reuse them for the baseline + every experiment.
4. Archive the baseline `stacks-bench` binary built from
   `repos/stacks-core` HEAD → `baseline/bin/{stacks-bench, manifest.json}`
   (Phase 0a).
5. Run the baseline benchmark (single invocation; rerun id aliased)
   (Phase 0b).
6. Run the triage agent → `triage/candidates.json` (Phase 1).
7. Fan out analyzer agents → `analysis/<family-id>/analysis.json`
   (Phase 1.5).
8. Run `sbagent session analysis merge` →
   `merge/optimization-targets.json` (Phase 1.7).
9. Per-target targeted baseline calibration — one stacks-bench run
   per `verification_replay.invocations[]` entry on every `normal_pr`
   target → `verify/<target>/baseline-run-ids.json` +
   `verify/<target>/<invocation-id>/bench-run.json` (Phase 1.8).
10. Fan out optimizer agents → per-target
    `optimize/<target>/implementation.md` (or `abort.md`) (Phase 2).
    Each target gets its own git clone under
    `<workspace>/optimizers/<id>/<target>/`.
11. Build + serially benchmark each accepted target — one
    `stacks-bench bench run` per invocation →
    `optimize/<target>/<invocation-id>/bench-run.json` and
    `optimize/<target>/candidate-run-ids.json` (Phase 3).
12. Fan out results-analyzer agents → `analyze/<target>/results-
    analysis.json` (Phase 3.5). Each agent judges measured vs
    `expected_signal` per invocation and commits a verdict +
    confidence.
13. Run `sbagent session finalize run` → `finalize/summary.json`,
    sourcing each `Experiment.improvement_pct` + `Experiment.status`
    verbatim from the Phase 3.5 verdict (Phase 4).
14. (Optional) `sbagent session archive --dry-run` to rehearse the
    archive flow locally before shipping anything to the operator's
    remote.

The first session is successful if `sbagent session validate
--session-id "$SESSION_ID"` exits 0. Do not judge optimization
quality from the first session — its purpose is to prove every tier
runs and artifacts land in the right places.

### Ongoing optimization

Re-run `sbagent session run` for each subsequent session. The
persistent stacks-bench DB at `<stacks_bench.data_dir>/appdata/stacks-bench.db`
accumulates baseline + experiment runs across sessions, so `bench
list` / `bench show` work cross-session. The operator's
`sessions.jsonl` ledger accumulates one line per archived session,
indexing the full audit trail back to the matching `session/<id>`
branch.

## Results: what to inspect and how to decide

Cross-session bench data: `<stacks_bench.data_dir>/appdata/stacks-bench.db`.

Per-session artifacts: `<layout.sessions_root>/<id>/results/` (defaults to
`<layout.agent_workspace_root>/sessions/<id>/results/`).

Per-target inspection:

```text
optimize/<target-id>/implementation.md    # OR abort.md / consensus-issue.md
optimize/<target-id>/optimizer-report.json
optimize/<target-id>/side-observations.md  # optional, future-target evidence
optimize/<target-id>/nextest.log
optimize/<target-id>/<invocation-id>/bench-run.json
optimize/<target-id>/candidate-run-ids.json
```

Use these sources for comparison:

- `bench show --json --run-id <id>`
- `bench show --json --run-id <id> --profiler-hot 50`
- `bench list --json --all --with-args`
- direct SQL against `<stacks_bench.data_dir>/appdata/stacks-bench.db`

Accept an experiment only if:

- it builds and passes the selected checks;
- it improves the targeted hotspot or total benchmark result enough
  to be meaningful;
- repeated runs are not obviously noise;
- it does not introduce a clear regression elsewhere;
- it does not violate the forbidden-change rules
  ([architecture.md](architecture.md#guardrails-for-optimization-work)).

Reject an experiment if:

- the result is neutral, noisy, or slower;
- the optimization only looks good theoretically but is not measured;
- tests/builds fail;
- it touches forbidden areas or changes semantics in a risky way.

The final session summary should answer:

1. What baseline was used?
2. What targets were selected and why?
3. What clones / branches were created? (`agentic/<id>/<target>` in
   each per-target clone; `session/<id>` on the operator repo after
   archive.)
4. What changed in each experiment?
5. What benchmark commands and parameters were used?
6. What improved, regressed, or produced noise?
7. Which experiments should be kept, discarded, or retried later?
8. What should the next session target?

## `summary.json` shape (schema v3)

```json
{
  "schema_version": 3,
  "session_id": "20260507-104400",
  "baseline_run_id": 123,
  "baseline_rerun_id": 124,
  "noise_floor_pct": 0.8,
  "experiments": [
    { "target_id": "a", "delivery_mode": "normal_pr",        "status": "accepted",        "run_ids": [500, 501], "baseline_run_ids": [200, 201], "improvement_pct": 4.7 },
    { "target_id": "b", "delivery_mode": "normal_pr",        "status": "rejected",        "run_ids": [502, 503], "baseline_run_ids": [202, 203], "reason": "warm-steady regressed 3% — cache eviction the analyzer missed" },
    { "target_id": "c", "delivery_mode": "normal_pr",        "status": "aborted",         "reason": "results-analyzer did not produce a verdict — analyze/<target>/results-analysis.json absent or invalid" },
    { "target_id": "d", "delivery_mode": "consensus_poc_pr", "status": "poc_landed",      "breakage_class": "clarity_cost_weight" },
    { "target_id": "e", "delivery_mode": "consensus_issue",  "status": "routed_to_issue", "breakage_class": "block_validation" }
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

`delivery_mode` on every experiment row is propagated from
`merge/optimization-targets.json` (set by the merge phase as a derived
field from `consensus_breaking` + `poc_implementable`):

- **`normal_pr`** — performance fix; `status ∈ {accepted, rejected,
  aborted}` sourced verbatim from the Phase 3.5 results-analyzer
  verdict (`Verdict::Accepted | Verdict::Mixed → Accepted`;
  `Verdict::Rejected → Rejected`; missing/invalid verdict →
  `Aborted`). `improvement_pct` and `reason` come from the verdict's
  `headline_improvement_pct` and `headline_rationale` /
  `caveats[]` — finalize does not compute pooled means or threshold
  against the noise floor.
- **`consensus_poc_pr`** — deliberate consensus-breaking change
  shipped as a PoC; `status ∈ {poc_landed, aborted}`. `poc_landed`
  means scoped tests passed; no benchmark ran by design.
- **`consensus_issue`** — consensus-breaking change too large or too
  coverage-blocked for PoC mode; `status ∈ {routed_to_issue,
  aborted}`. The optimizer was skipped entirely; the analyzer's
  `consensus_writeup` is the shipping artifact.

The per-target results-analyzer verdict (the source of truth for
`normal_pr` numbers) lives at
`analyze/<target-id>/results-analysis.json` — see
[`crate::models::results_analysis::ResultsAnalysis`](../crates/stacks-bench-agent/src/models/results_analysis.rs)
for the typed shape and
[`docs/architecture.md`](architecture.md#per-tier-exposed-variables)
for the agent contract.

`lens_dispositions[]` is propagated verbatim from
`merge/optimization-targets.json` so "real hotspot, no fix found" cases
(entries with `status: not_actionable`) survive into the
operator-facing summary.

The ledger entry that Phase 6 archive appends to `sessions.jsonl`
mirrors the relevant subset of this — see
[session-archive.md](session-archive.md) for the `SessionRecord`
schema.
