# Workflow

The phase-by-phase contract: what each phase reads, what it writes,
and how to drive them by hand or as a single chained run.

## Tier responsibilities

The `sbagent` orchestrator does not make analytical decisions —
every analytical decision lives in an agent prompt. The
orchestrator's responsibilities are mechanical:

| Tier      | Owns                                                                                                                            | Does NOT                                                |
| --------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| Triage    | Pick candidate workload families (tx / block / contract) from profiler data + DB.                                               | Read source code. Commit a target span. Run benchmarks. |
| Analyzer  | Investigate ONE family deeply; commit `target_span` + `fix_signature`; produce an analysis the merge + optimizer phases act on. | Modify source code. Run benchmarks.                     |
| Merge     | Dedupe analyses converging on the same structural fix; emit one canonical target per fix with `merged_from` provenance.         | Re-investigate. Modify analyses' substance.             |
| Optimizer | Implement the change in a worktree; run `cargo nextest`; leave a release binary.                                                | Run benchmarks. Touch other worktrees.                  |

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
(`baseline/`, `triage/`, `analysis/<family>/`, `merge/`, `optimize/<target>/`,
`finalize/`) so a tail / audit reader can locate "everything for phase X"
or "everything for target Y" without grepping. The `optimize/<target>/`
dir is shared across Phases 2, 3, and 5 by design — one folder per target
holds the optimizer, benchmark, and publish artifacts for that target.

| Phase | Command | Reads | Writes (in `<session>/results`) |
| ----- | ------- | ----- | ------------------------------- |
| 0a | `sbagent session baseline run` | `config.toml` (range fields) | `baseline/{bench-run.json, rerun.json, run-id, rerun-id, bench-list.json, profiler-hotspots.json}` |
| 0b | `sbagent session baseline import` | existing run id(s) in stacks-bench DB | same as 0a; additionally writes `baseline/noise-floor-pct` only when `--run-id` and `--rerun-id` collide (single-run-import fallback) |
| 1 | `sbagent session triage run` | baseline artifacts | `triage/{candidates.json (v2, family-shaped), candidates.md, prompt.md, events.jsonl, stderr.log, final-message.md, conversation-id, queries/, drilldowns/}` |
| 1.5 | `sbagent session analysis run` | `triage/candidates.json` | `analysis/<family-id>/{analysis.json, analysis.md, prompt.md, events.jsonl, stderr.log, final-message.md, conversation-id}` |
| 1.7 | `sbagent session analysis merge` | `triage/candidates.json`, `analysis/*/analysis.json` | `merge/{optimization-targets.json, prompt.md, events.jsonl, stderr.log, final-message.md, conversation-id}` |
| 2 | `sbagent session optimize run` | `merge/optimization-targets.json` | `optimize/<target-id>/{implementation,abort,consensus-issue}.md`, build artifacts |
| 3 | `sbagent session bench run` | per-target release binary | `optimize/<target-id>/run-{1,2}/bench-run.json`, `optimize/<target-id>/run-ids` |
| 4 | `sbagent session finalize run` | targets + run-ids | `finalize/{summary.json, summary.md, targets.md}` |
| 5 | `sbagent publish generate` + `sbagent publish push` | `merge/optimization-targets.json`, `finalize/summary.json` | `optimize/<target-id>/{pr,issue}-{title.txt,body.md}`; PRs/issues on GitHub |

Every phase that produces artifacts has a matching `clean` subcommand
(`sbagent session <phase> clean`) that removes its outputs
idempotently. Use it when re-running a phase from scratch.

## Walkthrough (with an existing baseline run id of 42)

```bash
SESSION_ID=demo-001

sbagent session baseline import --session-id "$SESSION_ID" --run-id 42
sbagent session triage run --session-id "$SESSION_ID"
jq . "sessions/$SESSION_ID/results/triage/candidates.json"

sbagent session analysis run --session-id "$SESSION_ID"
ls "sessions/$SESSION_ID/results/analysis"

sbagent session analysis merge --session-id "$SESSION_ID"
jq '.targets | length' "sessions/$SESSION_ID/results/merge/optimization-targets.json"

sbagent session optimize run --session-id "$SESSION_ID"
sbagent session bench run --session-id "$SESSION_ID"
sbagent session finalize run --session-id "$SESSION_ID"
cat "sessions/$SESSION_ID/results/finalize/summary.md"
```

If a phase fails or surprises you, re-run just that phase — the
others stay untouched. See [operations.md](operations.md#recovery)
for the resume-from-validate flow.

## Orchestrator: `sbagent session run`

`sbagent session run` chains phases 0–4 (and 5 if enabled). Two ways
to invoke:

```bash
# Fresh baseline (mints a YYYYMMDD-HHMMSS session id):
sbagent session run

# Reuse an existing baseline run id (skip Phase 0 baseline run):
sbagent session run \
  --import-baseline-run-id 42 \
  --import-baseline-rerun-id 43
```

Phase 2 fan-out parallelism is capped by configured agent
concurrency; analyzer fan-out by configured analyzer concurrency.
Phase 3 benchmarks are always serialized under the bench lock.

## Top-level workflow

### First optimization session: establish the baseline path

Goal: prove that the full pipeline runs end-to-end on a fresh agent
VM. Output quality is secondary on the first run; what matters is
that every phase artifact appears and `sbagent session validate`
reports `OK`.

The first session should:

1. Create `sessions/<session-id>/results`.
2. Use `data/stacks-bench` as the shared stacks-bench app-data dir.
3. Set benchmark parameters explicitly in `config.toml`; reuse them
   for the baseline + every experiment.
4. Run the baseline + `bench rerun` (Phase 0).
5. Run the triage agent → `triage/candidates.json` (Phase 1).
6. Fan out analyzer agents → `analysis/<family-id>/analysis.json`
   (Phase 1.5).
7. Run `sbagent session analysis merge` →
   `merge/optimization-targets.json` (Phase 1.7).
8. Fan out optimizer agents → per-target `optimize/<target>/implementation.md`
   or `abort.md` (Phase 2).
9. Build + serially benchmark each accepted target (Phase 3).
10. Run `sbagent session finalize run` → `finalize/summary.json` (Phase 4).

The first session is successful if `sbagent session validate
--session-id "$SESSION_ID"` exits 0. Do not judge optimization
quality from the first session — its purpose is to prove every tier
runs and artifacts land in the right places.

### Ongoing optimization

Re-run `sbagent session run` for each subsequent session. The
persistent stacks-bench DB at `<stacks_bench_data_dir>/appdata/stacks-bench.db`
accumulates baseline + experiment runs across sessions, so `bench
list` / `bench show` work cross-session.

## Results: what to inspect and how to decide

Persistent cross-session benchmark data lives in
`<stacks_bench_data_dir>` (defaults to `<operator>/data/stacks-bench`).
Per-session artifacts live in `<sessions_root>/<session-id>/results`
(defaults to `<operator>/sessions/<session-id>/results`).

For each experiment, inspect:

```text
optimize/<target-id>/implementation.md   # OR abort.md / consensus-issue.md
optimize/<target-id>/side-observations.md  # optional, future-target evidence
optimize/<target-id>/nextest.log
optimize/<target-id>/run-1/bench-run.json
optimize/<target-id>/run-2/bench-run.json
```

Use these sources for comparison:

- `bench show --json --run-id <id>`
- `bench show --json --run-id <id> --profiler-hot 50`
- `bench list --json --all --with-args`
- direct SQL against `data/stacks-bench/appdata/stacks-bench.db`

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
3. What branches/worktrees were created?
4. What changed in each experiment?
5. What benchmark commands and parameters were used?
6. What improved, regressed, or produced noise?
7. Which experiments should be kept, discarded, or retried later?
8. What should the next session target?

## `summary.json` shape (schema v2)

```json
{
  "schema_version": 2,
  "session_id": "20260507-104400",
  "baseline_run_id": 123,
  "baseline_rerun_id": 124,
  "noise_floor_pct": 0.8,
  "experiments": [
    { "target_id": "a", "delivery_mode": "normal_pr",        "status": "accepted",        "run_ids": [125, 126], "improvement_pct": 4.7 },
    { "target_id": "b", "delivery_mode": "normal_pr",        "status": "rejected",        "run_ids": [127, 128], "reason": "within noise" },
    { "target_id": "c", "delivery_mode": "normal_pr",        "status": "aborted",         "reason": "tests failed" },
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
  aborted}` driven by bench measurement.
- **`consensus_poc_pr`** — deliberate consensus-breaking change
  shipped as a PoC; `status ∈ {poc_landed, aborted}`. `poc_landed`
  means scoped tests passed; no benchmark ran by design.
- **`consensus_issue`** — consensus-breaking change too large or too
  coverage-blocked for PoC mode; `status ∈ {routed_to_issue,
  aborted}`. The optimizer was skipped entirely; the analyzer's
  `consensus_writeup` is the shipping artifact.

`lens_dispositions[]` is propagated verbatim from
`merge/optimization-targets.json` so "real hotspot, no fix found" cases
(entries with `status: not_actionable`) survive into the
operator-facing summary.
