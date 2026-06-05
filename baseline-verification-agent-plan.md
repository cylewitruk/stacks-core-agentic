# Baseline calibration + verification agent — execution spec

**Status:** Pass 1a SHIPPED at the wiring level — code complete, structurally
validated end-to-end against session `20260521-051649` (Phase 0a → 0b → 1.8
→ 2 → 3 → 4). Quantitative validation deferred to Pass 1c: independent
review of `20260521-051649` flagged measurement-methodology gaps (cache-
priming amplification on the cache target's 73% number; per-invocation
hypothesis vs measurement requires the analyzer-emitted invocations + the
post-bench results-analyzer agent that Pass 1c lands). Execution notes +
deviations captured in [Pass 1a execution notes](#pass-1a-execution-notes)
below.

**Implementation gate (resolved):** the targeted-replay `bench-run.json` schema
exposes only aggregate per-target summaries, no per-rep samples — Pass 1a
proceeds WITHOUT a `targeted_variance` field. Details in [Gate resolution
(implementation finding)](#gate-resolution-implementation-finding) below.

## Problem

Today's `improvement_pct` (`summary.json`, `sessions.jsonl`) compares a Phase 3
candidate run in **targeted-replay mode** against the session-level Phase 0
baseline in **full-range mode**. The two regimes are not directly comparable —
CPU cache, OS page cache, MARF working set, disk locality, and allocator state
all differ materially between a sequential 25k-block sweep and a forked-snapshot
replay of a handful of blocks.

Two systematic failure modes:

- **Inflated improvement** — candidate looks better in P3 than it is in
  production because the tight warmup loop favors hot-path fixes.
- **Deflated improvement** — sustained-load pathologies don't surface in P3's
  small working set and get rejected when they'd help.

**Goal:** compare each target's candidate bench against a matching targeted
baseline bench, measured under the same cache regime, instead of against the
full-range P0 baseline.

## Target architecture

```text
0a  build / pin baseline binary, archive to baseline/bin/stacks-bench
    ↓
0b  baseline full-range bench (single run, rerun id aliased to run id)
    ↓
1.7 merge → optimization-targets.json
    ↓
1.8 targeted baseline calibration (per-target, under bench lock)
    ↓
1.9 verification agent fanout (advisory)
    ↓
[coordinator: apply verification_floor + budget caps → decision.json]
    │  below-floor → demote effective_mode to full_range
    │  drop target only if no acceptable mode remains (e.g.
    │    signal_quality=incompatible AND full_range budget exhausted)
    ↓
2   optimize fanout (only kept targets; reads decision + verification)
    ↓
3   bench per target using coordinator-decided mode
    │  targeted_replay: candidate bench mirrors 1.8's phase structure
    │  full_range: archived baseline + candidate, both at full-range
    ↓
4   finalize
```

Pass 1a lands 0a, 0b-aliased, 1.8, and the finalize comparison change. Pass 2
lands 1.9 + coordinator + the full-range fallback machinery. Pass 1b lands the
lazy empirical noise floor for full-range fallback (deferred). Pass 3 polishes
(deferred).

## Implementation passes

**Pass 1a** — *SHIPPED (wiring level; quantitative validation pending Pass 1c).*

- Phase 0a binary archival.
- Phase 0b single-run + rerun alias.
- Phase 1.8 per-target calibration.
- Finalize denominator switch.

**Pass 1c** — *Now.* ~25-30 h + prompt-iteration calendar. (Full design:
[roadmap.md §"Pass 1c — analyzer-defined invocations + post-bench
results-analyzer"](roadmap.md).)

- Schema rewrite: `verification_replay` → analyzer-emitted `invocations[]`,
  each with a per-invocation `expected_signal`.
- Phase 1.8 + Phase 3 retooling to iterate the analyzer's invocations
  symmetrically (one stacks-bench call per entry per side).
- **Phase 3.5 results-analyzer agent** — per-target fanout, synthesizes
  per-invocation measurements into a structured verdict
  (`results-analysis.json`).
- Phase 4 finalize / Phase 5 PR-writer / SessionRecord source headline +
  caveats + `pr_body_summary` from the analyzer verdict.
- Clean-break of pre-Pass-1c data / fixtures (no migration shim).

Pass 1c is the trigger for promoting Pass 1a from "shipped at wiring level"
to "shipped without caveats." Splittable as 1c-α (schema + plumbing) and
1c-β (results-analyzer + downstream sourcing) if the bundle is too large
to land in one go.

**Pass 1b** — *After Pass 2.* ~4-6 h.

- `baseline_rerun_id` → `Option<i64>` across consumers.
- Lazy empirical noise calibration inside Phase 3 full-range fallback.

**Pass 2** — *After Pass 1c.* ~18-24 h.

- Phase 1.9 pre-bench verifier agent (advisory).
- Coordinator decision logic.
- Full-range fallback path.
- Budget gate.

Becomes a compute-saving optimization once Pass 1c's post-bench analyzer is
the load-bearing quality gate — Pass 2 catches bad-fit targets BEFORE
burning bench time; Pass 1c catches them honestly AFTER measurement.

**Pass 3** — *After Pass 2 runs real sessions.* ~5-8 h.

- sqlite MCP wrapper.
- PR-body templating polish (the results-analyzer's `pr_body_summary` is
  already authoritative; Pass 3 is structural / template-set polish).
- Operator tuning docs.

Pass 1b's lazy calibration co-locates with Pass 2's full-range fallback
path, so landing it after Pass 2 keeps the touch points contiguous.

## Implementation gate: variance source

Resolve this **before** any `targeted_variance` field ships to schemas,
finalize, `SessionRecord`, or operator-facing artifacts.

1. Run a real **targeted-replay** invocation matching the shape Phase 1.8 and
   Phase 3 will use — e.g. `stacks-bench bench run --block <hash> --repetitions
   10 --warmup 5` (or `--txid <hash>` for the txid phase). Full-range
   `--repetitions > 1` is NOT the right shape to inspect; its `bench-run.json`
   may carry per-rep fields that targeted mode doesn't, or vice versa.
2. Inspect the resulting `bench-run.json` for per-repetition samples in the
   targeted-replay output (look for `targets[].per_rep_total_us` or equivalent
   fields beneath the per-target summary).
3. **If per-rep samples exist:** Path A. Compute mean/stddev/CI from a single
   calibration invocation per phase. Cheap.
4. **If they don't:** either add per-rep emission to `stacks-bench` upstream
   (preferred), or implement Path B (3-5 calibration invocations per phase per
   target, variance computed across run ids). Path B is methodologically honest
   but costs 3-5× per target.

Do not ship a "variance" field computed from a single sample.

Once decided, record the choice + sample-source enum (`per_repetition` |
`per_invocation`) in the variance artifact so downstream consumers know which
precision regime applies.

### Gate resolution (implementation finding)

Inspected the smoke session's targeted-replay `bench-run.json` at
`optimize/marf-deferred-node-hash-direct-digest/run-1/bench-run.json`
(produced by `stacks-bench` at `cylewitruk/feat/stacks-bench@f4cab0a01`,
`--block --repetitions 10 --warmup 10`).

The schema exposes only aggregate per-target summaries
(`data.targets[].summary.{total_duration_us, ...}` summed across
`measured_count` reps). No `per_rep_total_us` or equivalent
per-repetition arrays exist. **Path A is unavailable** without an
upstream `stacks-bench` change.

**Pass 1a decision: defer `targeted_variance` entirely.**

- Pass 1a runs ONE calibration invocation per phase per target.
- `baseline-run-ids.json` is written; per-phase `*-noise.json` files
  are NOT (no variance data to put in them).
- `Experiment.targeted_variance` and `TargetBench.targeted_variance`
  schema fields are NOT added in Pass 1a.
- The apples-to-apples comparison (numerator + denominator under
  matched cache regimes) still ships — variance is for noise-floor
  judgment, which `single_run_noise_floor_pct` (constant) already
  covers.
- Variance lands in a separate follow-up once either upstream
  `stacks-bench` emits per-rep samples (preferred — opens Path A
  cheaply) OR we commit to Path B's 3-5× per-target cost.

This deviation is recorded here rather than amending Sub-step C's
text, so the original "if variance-source decision allows"
conditional remains the contract.

## Pass 1a — execution plan

### Sub-step A: archive the baseline binary (Phase 0a)

`StacksBenchCli` (see
[bench.rs:145](crates/stacks-bench-agent/src/session/bench.rs)) currently falls
back to `cargo stacks-bench` if `target/release/stacks-bench` is absent. That
fallback is forbidden in the new baseline / calibration / full-range code paths
— a missing archived binary is a hard error.

Phase 0a, before any bench runs:

1. Read operator's `repos/stacks-core` submodule HEAD sha; record it for
   `SessionRecord.stacks_core_base_sha`.
2. `cargo build --release -p stacks-bench` in the operator's stacks-core
   checkout. Same invocation as
   [`session/cargo.rs`](crates/stacks-bench-agent/src/session/cargo.rs).
3. Copy `target/release/stacks-bench` →
   `<session>/results/baseline/bin/stacks-bench`.
4. Write `<session>/results/baseline/bin/manifest.json` with
   `{source_sha, cargo_version, build_flags, archived_at}`.

**Strict-binary contract:** introduce `StacksBenchCli::strict_archived()` (or a
sibling `StacksBenchArchivedCli`). All Phase 0b, Phase 1.8, and Phase 3
full-range invocations use the strict variant. Missing archived binary → error,
never silent rebuild.

**Cost:** one `cargo build --release` per session (no-op when HEAD is already
built; ~5-30 min cold on a fresh VM).

**Tests:**

- Unit: strict CLI fails on missing path; permissive CLI behaves as before.
- Integration: Phase 0a produces both files; manifest is parseable.

### Sub-step B: Phase 0b — single run, rerun aliased

Phase 0 today invokes `bench run` then `bench rerun`. The rerun exists to
characterize session-level noise floor. Per-target calibration (Sub-step C)
supersedes it for the dominant comparison path; the rerun's ~30 min of work is
now wasted in the common case.

Phase 0b under Pass 1a:

1. Run `stacks-bench bench run` once via the strict archived binary. Write
   `baseline/bench-run.json` and `baseline/run-id`.
2. **Skip the `bench rerun` invocation.**
3. Write `baseline/rerun-id` with the same value as `baseline/run-id`.
4. Compute `baseline/noise-floor-pct` from `settings.single_run_noise_floor_pct`
   (default 1%). This is the existing single-run-fallback path the framework
   already exercises.
5. Log: `baseline rerun aliased to run id <N>; using configured single-run noise
   floor <P>%`.

Schema contracts stay intact — both ids populated, downstream consumers (triage,
summary, archive ledger, baseline import) see no breakage.

**Acceptance:** session completes Phase 0 with both `baseline/run-id` and
`baseline/rerun-id` populated, equal values, and triage/summary/archive consume
them without error.

### Sub-step C: per-target targeted baseline calibration (Phase 1.8)

For each `normal_pr` target with a non-empty `verification_replay`:

1. Acquire the bench lock.
2. **Mirror replay phase structure:** `verification_replay` carries `txids`
   and/or `blocks`. Run one calibration per non-empty phase:
   - `verify/<target>/baseline-txid-run-K/bench-run.json` for txid replay.
   - `verify/<target>/baseline-block-run-K/bench-run.json` for block replay.
3. **Use rich profile flags** — NOT `--bench-spans-only`, NOT
   `--no-profiler-kv`. The Pass 2 pre-bench verifier and the Pass 1c
   post-bench results-analyzer both need span + profiler-kv data, and Phase
   3 candidate benches now use the same rich profile shape (flag symmetry
   shipped 2026-05-21).
4. Invoke the strict archived binary with `--repetitions N --warmup M` matching
   the candidate bench.
5. Persist:
   - `verify/<target>/baseline-run-ids.json` — structured `{txid_run_ids: [...],
     block_run_ids: [...]}`.
   - `verify/<target>/baseline-txid-noise.json` and/or
     `verify/<target>/baseline-block-noise.json` — per-phase variance, format
     dictated by the variance-source decision (see [Implementation
     gate](#implementation-gate-variance-source)).
6. Release the bench lock.

Targets without `verification_replay` skip Phase 1.8 and stay on today's P0 ↔
candidate-full-range path until Pass 2 replaces it.

**Cost:**

| Variance source | Per-target | 5 targets, 1-2 phases each |
| --- | --- | --- |
| Path A (per-rep) | 1 calibration / phase | 25-50 min |
| Path B (multi-invocation) | 3-5 calibrations / phase | 1.25-4.2 h |

### Sub-step D: finalize denominator switch

`session/finalize.rs` currently computes `improvement_pct` from
`baseline_run_id` (session-level) and the target's candidate `run_ids`. Pass 1a
changes this to a per-target, phase-aware comparison.

For each target's `Experiment` row in summary.json:

1. If the target has phase-aware baseline run ids (`baseline_run_ids =
   {txid_run_ids, block_run_ids}`), compare phase-by-phase (txid candidate ↔
   txid baseline; block candidate ↔ block baseline) and aggregate to a single
   per-target `improvement_pct`.
2. Otherwise (no `verification_replay`), fall back to the legacy P0 ↔
   candidate-full-range comparison using the session-level `baseline_run_id`.

**Acceptance:** finalize output for a `verification_replay`-bearing target
sources its denominator from `baseline_run_ids` (not session-level
`baseline_run_id`).

## Pass 1a — files & expected outputs

### `src/session/baseline.rs`

- Phase 0a build + archive; manifest write.
- Phase 0b single-run + rerun alias + log line.

Expected outputs:

- `baseline/bin/stacks-bench`
- `baseline/bin/manifest.json`
- `baseline/run-id`
- `baseline/rerun-id` (equal to `run-id`)
- `baseline/noise-floor-pct`

### `src/session/bench.rs`

- Add `StacksBenchCli::strict_archived` constructor; missing archived binary is
  a hard error.

Expected: strict + permissive variants coexist; existing callers of the
permissive variant unchanged.

### `src/session/calibration.rs` (new module)

- Phase 1.8 implementation: per-target, phase-aware, rich profile flags.

Expected outputs (per target):

- `verify/<target>/baseline-{txid,block}-run-K/bench-run.json`
- `verify/<target>/baseline-run-ids.json`
- `verify/<target>/baseline-{txid,block}-noise.json` (per phase)

### `src/cli/session/run.rs`

- Wire pipeline: Phase 0a → 0b → 1.7 merge → 1.8 calibration → 2 optimize → 3
  bench → 4 finalize.

Expected: orchestrator runs Phase 1.8 between merge and optimize.

### `src/session/finalize.rs`

- Per-target phase-aware comparison.
- Legacy fallback for targets without `verification_replay`.

Expected: `summary.json` `Experiment` row sources `improvement_pct` denominator
from per-target baseline ids.

### `src/models/summary.rs`

- Add `Experiment.baseline_run_ids: Option<{txid_run_ids: Vec<i64>,
  block_run_ids: Vec<i64>}>`.
- Add optional per-phase `targeted_variance` (only if variance-source decision
  allows — see Implementation gate).

Expected: schema regenerates; bundled mirror updates; drift gate passes.

### `src/models/session_record.rs`

- `TargetBench.baseline_run_ids: {txid_run_ids: Vec<i64>, block_run_ids:
  Vec<i64>}` (phase-aware).
- Optional `targeted_variance` per phase.

Expected: schema regenerates; ledger backward-compatible (old `None` defaults
still parse).

### `docs/workflow.md`

- Phase 1.8 row in phase table.
- Methodology subsection.

Expected: doc reflects new pipeline shape.

## Pass 1a — acceptance criteria

1. Phase 0a archives a binary at `baseline/bin/stacks-bench` matching the
   recorded `stacks_core_base_sha`.
2. Phase 0b invokes `stacks-bench bench run` exactly once and does NOT invoke
   `bench rerun` (verifiable by tracing the bench-client call log or by counting
   the lines in `baseline/bench-run.stderr.log`). `baseline/run-id` and
   `baseline/rerun-id` are equal. The alias log line (`baseline rerun aliased to
   run id <N>; ...`) appears in session output.
3. Strict-binary contract: hand-deleting `baseline/bin/stacks-bench` between
   Phase 0b and Phase 1.8 causes Phase 1.8 to fail with a clear error (no silent
   `cargo stacks-bench` rebuild).
4. Phase 1.8 produces per-target baseline runs whose `bench-run.json` includes
   span and profiler-kv data (verify by inspecting one).
5. The variance-source decision is recorded in `baseline-*-noise.json` and
   matches the result of running the Implementation gate inspection.
6. For at least one `verification_replay`-bearing target, inspect (or log) the
   denominator run ids `finalize` used when computing that target's
   `improvement_pct` and assert they come from
   `verify/<target>/baseline-run-ids.json`, NOT from `baseline/run-id`. Targets
   without `verification_replay` continue to source the denominator from
   `baseline/run-id` (legacy fallback).
7. `just test` + `just lint` clean.
8. `docs/workflow.md` updated.

## Pass 1a execution notes

What landed against the execution plan, what deviated, and what
still needs live validation.

### Shipped

- **Strict-binary contract.** `StacksBenchCli` gained a `strict:
  bool` field plus a `StacksBenchCli::strict_archived(...)`
  constructor. `build_cmd()` now returns `Result<Command>`;
  missing-binary-with-strict surfaces a hard error mentioning
  Phase 0a. All 6 existing call sites updated with `strict:
  false` to preserve permissive defaults; baseline / calibration
  paths use the strict variant.
  ([`session/bench.rs`](crates/stacks-bench-agent/src/session/bench.rs))
- **Phase 0a archival.** New `archive_baseline_binary` function
  resolves `repos/stacks-core` HEAD sha, invokes `cargo build
  --release -p stacks-bench`, copies the binary to
  `baseline/bin/stacks-bench`, writes `baseline/bin/manifest.json`
  with `{source_sha, cargo_version, build_flags, archived_at,
  archived_path}`. Wired into both the orchestrator (`session
  run`) and the standalone `session baseline run` command.
  ([`session/baseline.rs`](crates/stacks-bench-agent/src/session/baseline.rs))
- **Phase 0b single-run + alias.** `baseline::run` no longer
  invokes the second `bench rerun`. Same run id is written to
  both `baseline/run-id` and `baseline/rerun-id`;
  `baseline/rerun.json` is a file-copy of `baseline/bench-run.json`
  to preserve validate.rs's require-non-empty check;
  `baseline/noise-floor-pct` comes from
  `settings.single_run_noise_floor_pct`. The aliasing log line
  appears on session start.
- **Phase 1.8 calibration.** New `session/calibration.rs` module
  iterates over `normal_pr` targets with non-empty
  `verification_replay`. Per replay phase (txid + block), invokes
  the strict archived binary without `--bench-spans-only` /
  `--no-profiler-kv` (rich profile data preserved for the future
  verifier). Writes
  `verify/<target>/baseline-{txid,block}-run-K/bench-run.json`,
  `verify/<target>/baseline-run-ids.json` (structured
  `{txid_run_ids, block_run_ids}`), and corresponding stderr
  logs. Wired into the orchestrator between Phase 1.7 merge and
  Phase 2 optimize.
  ([`session/calibration.rs`](crates/stacks-bench-agent/src/session/calibration.rs))
- **Finalize denominator switch.** `finalize::evaluate_normal_pr`
  now calls `load_per_target_baseline_ids` first. When
  `verify/<target>/baseline-run-ids.json` exists, the per-target
  pooled run-ids feed the `improvement_pct` denominator; otherwise
  the legacy session-level baseline mean still applies. The
  `Experiment` row gains `baseline_run_ids: Option<Vec<i64>>` so
  the audit trail records which denominator finalize used.
  ([`session/finalize.rs`](crates/stacks-bench-agent/src/session/finalize.rs),
  [`models/summary.rs`](crates/stacks-bench-agent/src/models/summary.rs))
- **Schemas regenerated.** `summary.schema.json` updated for the
  new optional field; both `<repo>/schemas/` and
  `<repo>/.sbagent/schemas/` mirrors written. Schema drift gate
  passes.
- **Tests.** 17 new unit + integration tests cover the strict-CLI
  contract, Phase 0a archival, Phase 1.8 phase-building, and
  updated baseline.rs integration test expectations (3 bench
  invocations under aliasing, not 4). 253/253 tests pass.

### Deviations from the plan

1. **Variance source — no `targeted_variance` field shipped.**
   The implementation gate inspection (smoke session's
   targeted-replay `bench-run.json` from `stacks-bench` at
   `cylewitruk/feat/stacks-bench@f4cab0a01`) found only aggregate
   per-target `summary.total_duration_us` — no `per_rep_total_us`
   or equivalent. Path A is unavailable without an upstream
   `stacks-bench` change. Pass 1a runs one calibration per
   phase per target (k=1) and SKIPS the variance field entirely.
   Variance lands in a follow-up that either upstreams per-rep
   sample emission to `stacks-bench` (preferred) or commits to
   Path B's 3-5× per-target multi-invocation cost.
2. **Pooled-mean comparison, not strict phase-by-phase.** The
   plan called for "phase-by-phase comparison (txid candidate ↔
   txid baseline; block candidate ↔ block baseline) aggregated
   to a single per-target `improvement_pct`." Pass 1a's
   implementation pools all phase samples on each side (mean of
   txid + block calibration run-ids as the denominator, mean of
   the candidate's flat `run-ids` file as the numerator). This
   is operationally equivalent for a single-invocation-per-phase
   regime — both sides pool the same number of phase-aligned
   samples — but a strict phase-by-phase comparison would matter
   if Phase 1.8 ever runs different invocation counts per phase
   (Path B multi-invocation territory). When that lands, the
   pooled comparison becomes a bias source and the phase-aligned
   version should ship.
3. **`SessionRecord` schema unchanged.**
   `TargetBench.baseline_run_ids` stays a flat `Vec<i64>` in v1
   of the session-record schema. The plan called for phase-aware
   restructuring; deferred to Pass 1b along with the
   `baseline_rerun_id → Option<i64>` migration so all the
   schema_version=1 → 2 churn happens together. The phase-aware
   breakdown is already preserved on disk at
   `verify/<target>/baseline-run-ids.json`.
4. **Profile-flag asymmetry between calibration and candidate
   — historical, SHIPPED 2026-05-21.** Pass 1a originally shipped
   with Phase 1.8 calibration using rich profile flags (no
   `--bench-spans-only`, no `--no-profiler-kv`) while Phase 3
   candidate bench kept lean flags. Independent review of
   `20260521-051649` showed the "constant overhead cancels in
   `improvement_pct`" framing was wrong — profiler overhead varies
   with span density, which varies across targets. Mitigation
   landed 2026-05-21: lean flags dropped on the candidate side so
   both sides run rich. See roadmap §"Flag symmetry between
   baseline and candidate benches — shipped 2026-05-21" for the
   shipped change. Pass 1c's per-invocation `profiler` field
   carries the same-flags-within-comparison invariant forward into
   the analyzer-emitted schema.

### Live end-to-end validation — structural ✓, quantitative deferred

Session `20260521-051649` exercised Phase 0a → 0b → 1.8 → 2 → 3 → 4
end-to-end against a real chainstate and a real stacks-bench DB.
Five separable outcomes:

**Structurally validated (Pass 1a is shipped at the wiring level):**

- Phase 0a archives the binary with `source_sha`, `dirty`, `cargo_version`,
  and `build_flags` recorded in `baseline/bin/manifest.json`.
- Phase 0b runs once with `baseline_run_id == baseline_rerun_id`
  ("aliased to run id 40" log line confirmed); `noise_floor_pct`
  defaults to `single_run_noise_floor_pct`.
- Phase 1.8 produced `verify/<target>/baseline-{txid,block}-run-1/`
  for each of 4 normal_pr targets, phase-aware (2 txid + 2 block),
  with rich profile data in each `bench-run.json`.
- Phase 4 finalize uses per-target `baseline_run_ids` from
  `verify/<target>/baseline-run-ids.json` for each normal_pr
  experiment; the session-level `baseline_run_id` remained as an
  audit field only (and is dangling in the DB without finalize
  caring, because the patch deferring the session-level lookup
  landed mid-session — see test
  `finalize_skips_session_baseline_when_all_targets_have_per_target_ids`).
- Typed `optimizer-report.json` contract emitted for all 5 targets;
  the `--resume` flag added this session correctly skipped 2
  already-completed targets on re-invocation.
- State-root mismatch on targeted-replay repetitions confirmed as
  an upstream `stacks-bench` bug; fix landed at commit
  `90b509612e` on `cylewitruk/feat/stacks-bench` and is verified
  here against `--repetitions 20`.

**Quantitatively NOT PR-grade.** The specific `improvement_pct`
numbers from session 20260521-051649 should not be quoted
externally. Three reasons surfaced by independent review:

1. **Flag asymmetry between baseline and candidate.** Phase 1.8
   baseline runs use rich profiler flags
   ([calibration.rs](crates/stacks-bench-agent/src/session/calibration.rs)
   ~L187: no `--bench-spans-only`, no `--no-profiler-kv`); Phase 3
   candidate runs use lean flags
   ([bench_experiments.rs](crates/stacks-bench-agent/src/session/bench_experiments.rs)
   ~L238: both flags set). The plan's "Deviations" section
   originally framed this as a constant-percentage overhead bias
   that cancels in `improvement_pct`; that's wrong. Profiler
   overhead varies with span density, which varies across targets
   and across the workloads they replay. The asymmetry materially
   inflates wins on profile-heavy workloads. Same flags must run
   on both sides to keep the comparison honest.
2. **Cache-priming amplification.** Phase 1.8 baseline and Phase 3
   candidate run the same `(txids, repetitions, warmup)` shape.
   For cache-introducing optimizations, the candidate's warmup
   phase populates the cache that baseline lacks entirely;
   measurement-phase repetitions then read warm vs cold. The
   target `marf-historical-read-node-cache` reported 73% on this
   shape — internally consistent but unseparable into
   "real-cache-hit gains" vs "warmup-smeared-into-measurement
   gains" without a cold-vs-warm split. Generalizes beyond caches:
   any optimization that benefits from state accumulating across
   repetitions will be over-credited under the current protocol.
3. **PoC target shipped against the pre-bump base SHA.** Independent
   review caught that `recalibrate-large-local-variable-lookup-cost`
   was rebased mid-session onto `0ad33704c2`, but only after the
   session results were written. The other 4 targets rebased
   correctly. The optimizer report schema doesn't record
   `base_sha` / `head_sha`, so this passed silently.

The structural Pass 1a invariants hold (denominator switch fires
on the right artifact, schema contracts intact, --resume + typed
reports stable). The methodology underneath needs work before the
numbers are quotable. See [roadmap.md](roadmap.md) entries:

- "Pass 1c — analyzer-defined invocations + post-bench
  results-analyzer" — replaces the single one-shape-fits-all
  `verification_replay` with analyzer-emitted self-contained
  bench invocations, and folds in the post-bench results-analyzer
  agent (was deferred as Pass 4) that synthesizes per-invocation
  results into a structured verdict feeding summary/PR-body/ledger.
- "Flag symmetry between Phase 1.8 baseline and Phase 3 candidate"
  — SHIPPED 2026-05-21 (lean flags dropped on both sides; Pass 1c
  carries the contract forward per-invocation).
- "Coordinator-provenance sidecar (base + head SHA)" — SHIPPED
  2026-05-21; closes the silent-rebase-mismatch hole.

**Open follow-up before Pass 1a is fully done:**

- Re-run Pass 1a quantitative validation after Pass 1c lands (the
  analyzer-emitted invocations + post-bench results-analyzer). That
  is the trigger for promoting Pass 1a to "shipped without caveats";
  until then, treat the live data as a wiring smoke test, not a
  numeric baseline.

## Deferred: Pass 1b

Make `baseline_rerun_id` an `Option<i64>` across consumers and run full-range
noise calibration lazily inside Phase 3's full-range fallback path.

- `baseline_rerun_id` → `Option<i64>` in `summary.json`,
  `optimization-targets.json`, `SessionRecord`, baseline import. None becomes
  the default once "no rerun happened" is honest (Pass 1a writes equal ids; Pass
  1b makes that explicit as None).
- New on-demand full-range noise calibration step: when Phase 3's first
  full-range fallback target needs an empirical noise floor, run one
  duplicate-baseline-binary pass under the bench lock; subsequent fallback
  targets in the session reuse it.
- Uses the strict archived binary from Pass 1a's Sub-step A.

**Sequence:** after Pass 2 lands, so the lazy calibration
co-locates with the full-range fallback machinery Pass 2
introduces.

**Consumers to migrate:**
[`session/triage.rs`](crates/stacks-bench-agent/src/session/triage.rs)
(stops reading `baseline/rerun-id` when None);
`optimization-targets.json` schema (drift gate refresh);
`summary.json` schema (drift gate refresh); `SessionRecord` schema
(drift gate refresh); `sbagent session baseline import` (accept
None rerun id); triage noise-floor logic (already tolerates the
single-run fallback constant from Pass 1a).

## Deferred: Pass 2

Phase 1.9 verification agent (advisory, per-target fanout), coordinator-owned
decision logic, full-range fallback machinery, and budget gate.

**Rationale (compact):** static thresholds for comparing P0 sequential profile
data against targeted-replay profile data are brittle because the workloads
sample legitimately different contracts and tx types. Agent judgment fits the
fuzzy case; coordinator-owned effective decisions keep the system testable and
auditable.

**Phase position:** Phase 1.9, between Phase 1.8 calibration and Phase 2
optimize. Verify-fanout runs after calibration but before optimize, so targets
the verifier rejects don't burn Codex tokens or build/test compute.

**Advisory contract:**

- `verify/<target>/verification.json` — agent recommendation + evidence (written
  by agent, read-only after first write unless `--force`).
- `verify/<target>/decision.json` — coordinator effective decision (mode used,
  rationale chain combining agent rec + operator settings + budget state).
  Written by coordinator.

**Operator settings:**

- `verification_floor: high | medium | low` (default `medium`) —
  minimum agent-reported `signal_quality` to honor `targeted_replay`.
- `max_full_range_fallbacks: usize` (default 3) — budget cap on full-range
  fallback runs per session.
- `full_range_budget_hard: bool` (default false) — when true, exceeding the
  budget aborts the session; when false, surplus fallback targets get dropped
  with a diagnostic.
- `verifier_concurrency_cap: usize` (default 4).
- `--parallel-verifiers` CLI flag.
- `--fresh-targeted-baseline` CLI flag (forces a re-run of Phase 1.8 immediately
  before candidate bench; used when the verifier marks a target's signal_quality
  as `medium` and the operator wants a tighter back-to-back comparison).

**DB access:** `sqlite3 -readonly <db>` inside the codex sandbox. Add the bench
DB path to read-grants. Pass 3 may replace with an MCP wrapper.

**Schemas (sketched in Appendix B):** `verification.json` carries
`input_fingerprint` (for idempotency), `recommended_mode`, `signal_quality`,
`baseline_calibration_run_ids` (phase-aware), `rationale`, `observations[]`,
`caveats[]`, `db_queries[]` (stored as `{purpose, query_digest, output_path}`;
raw SQL lives beside its CSV output, not in the JSON).

**Audit:** agent's rationale + caveats flow into `summary.md`, the PR body (when
publish runs), and `SessionRecord.targets[].verification`.

## Deferred: Pass 3

- Replace `sqlite3 -readonly` with an MCP server providing typed query tools
  over the bench DB.
- PR body templating: structured "Verification methodology" section sourced from
  the verifier's rationale/observations/caveats.
- `docs/verification-methodology.md` written from real-session data:
  signal_quality distributions, recommended floor settings, common
  signal-mismatch patterns.

**Trigger:** Pass 2 has produced enough real sessions for the SQL patterns,
PR-body needs, and signal_quality distributions to be empirical rather than
speculative.

## Appendix A — sequencing nuance for Pass 1b

Two valid orderings:

- **Pass 1b before Pass 2:** the lazy calibration sits in Phase 3 but is rarely
  triggered (no verifier to demote targets to full_range). Adds optionality
  early.
- **Pass 1b after Pass 2:** the lazy calibration co-locates with the full-range
  fallback machinery Pass 2 introduces. Cleaner diff; preferred.

## Appendix B — Pass 2 schema sketches

For reference when Pass 2 starts. Full design lives in this doc's Pass 2
section; these are the wire-level shapes implementers will hit first.

These sketches assume Pass 1c has landed: analyzer-emitted `invocations[]`
and label-indexed `baseline_run_ids.json`. The pre-1c phase-shaped variant
(`{txid_run_ids, block_run_ids}`) is obsolete and not preserved here.

### `verify/<target-id>/verification.json` (agent-written)

```json
{
  "schema_version": 2,
  "target_id": "marf-deferred-node-hash-direct-digest",
  "input_fingerprint": "sha256:...",
  "recommended_mode": "targeted_replay",
  "signal_quality": "high",
  "baseline_calibration_run_ids": [
    { "label": "cold first-touch", "run_id": 123 },
    { "label": "warmed steady-state", "run_id": 124 },
    { "label": "block-context cross-check", "run_id": 125 }
  ],
  "rationale": "...",
  "observations": ["...", "..."],
  "caveats": ["..."],
  "db_queries": [
    {
      "purpose": "verify-replay-block-contracts",
      "query_digest": "sha256:...",
      "rows_returned": 12,
      "output_path": "verify/<target>/queries/replay-block-contracts.csv"
    }
  ]
}
```

`input_fingerprint` covers: target id + sha256(target JSON) +
label-indexed run-id set (a deterministic encoding of
`[{label, run_id}]` — analyzer-chosen order is preserved) +
schema_version + verifier prompt version.

### `verify/<target-id>/decision.json` (coordinator-written)

```json
{
  "schema_version": 1,
  "target_id": "marf-deferred-node-hash-direct-digest",
  "agent_recommended_mode": "targeted_replay",
  "agent_signal_quality": "high",
  "effective_mode": "targeted_replay",
  "kept_for_optimize": true,
  "rationale_chain": [
    "agent recommended targeted_replay with signal_quality=high",
    "verification_floor=medium → high passes floor → honor agent",
    "max_full_range_fallbacks budget: not consumed"
  ],
  "fresh_baseline_required": false,
  "budget_state": {
    "full_range_fallbacks_used_this_session": 0,
    "full_range_fallbacks_budget": 3
  }
}
```

### Signal quality → action map

- **`high`** — Honor agent's `recommended_mode`.
- **`medium`** — Honor agent's `recommended_mode`; surface caveats in PR body.
- **`low`** — Demote to `full_range` unless
  `verification_floor=low`.
- **`incompatible`** — Demote to `full_range` always; rationale becomes a forced
  PR-body disclaimer.
