//! Per-session on-disk layout. Builds the canonical paths for every
//! artifact produced by the v2 pipeline.
//!
//! Structure under `<sessions_root>/<id>/results/`:
//!
//! ```text
//! results/
//!   baseline/   {run-id, rerun-id, bench-run.json, rerun.json, *.stderr.log,
//!                profiler-hotspots.json, bench-list.json}
//!   triage/     {candidates.{json,md}, prompt.md, events.jsonl, stderr.log,
//!                final-message.md, conversation-id,
//!                queries/<n>.csv, drilldowns/<n>.csv}
//!   analysis/<family-id>/  {analysis.json, prompt.md, events.jsonl,
//!                           stderr.log, final-message.md}
//!   merge/      {optimization-targets.json, prompt.md, events.jsonl,
//!                stderr.log, final-message.md, conversation-id}
//!   optimize/<target-id>/  one shared per-target dir; receives Phase 2
//!                          (optimizer agent), Phase 3 (candidate bench
//!                          outputs, `candidate-*` prefix), and Phase 5
//!                          (publish artifacts, `pr-writer-*` /
//!                          `issue-writer-*` prefix) outputs. Audit reads
//!                          for "what happened with target X" stay in one
//!                          place. Files include: optimizer-report.json
//!                          (agent-written, typed contract), with
//!                          coordinator-rendered companion views
//!                          implementation.md / abort.md; consensus-issue.md
//!                          (coordinator-written marker; the optimizer is
//!                          skipped for consensus_issue mode); prompt.md,
//!                          events.jsonl, stderr.log, final-message.md,
//!                          conversation-id, nextest.log,
//!                          candidate-run-ids.json,
//!                          <invocation-id>/bench-run.json, pr-title.txt,
//!                          pr-body.md, pr-writer-*, issue-*, etc.
//!   analyze/<target-id>/   {results-analysis.json, results-analysis.md,
//!                           prompt.md, events.jsonl, stderr.log,
//!                           final-message.md, conversation-id}
//!                          (Phase 3.5 results-analyzer agent output —
//!                          one verdict per `bench_eligible` target)
//!   finalize/   {summary.json, summary.md, targets.md}
//! ```
//!
//! Phase prefixes have been dropped from file names inside top-level
//! subdirs that have only one writer (e.g. `triage/prompt.md` not
//! `triage/triage-prompt.md`). Inside the shared `optimize/<target>/`
//! dir, prefixes are kept where two phases would otherwise collide
//! (pr-writer + optimizer would both want `prompt.md`).

use std::path::{Path, PathBuf};

use crate::layout::Layout;
use crate::types::SessionId;

/// Canonical session layout. Results dir is the v2 artifact-bearing root;
/// worktrees dir is the sibling where optimizer git worktrees live.
#[derive(Debug, Clone)]
pub struct SessionLayout {
    /// Session id.
    pub id: SessionId,
    /// `<sessions_root>/<id>/results`.
    pub results_dir: PathBuf,
    /// `<sessions_root>/<id>/worktrees`.
    pub worktrees_dir: PathBuf,
}

impl SessionLayout {
    /// Build a layout under the given sessions root. The sessions root is
    /// `OPT_SESSIONS_ROOT` in the bash framework — defaults to
    /// `<framework>/sessions` but is overridable.
    pub fn new(sessions_root: &Path, id: SessionId) -> Self {
        let results_dir = sessions_root
            .join(id.as_str())
            .join("results");
        let worktrees_dir = sessions_root
            .join(id.as_str())
            .join("worktrees");
        Self { id, results_dir, worktrees_dir }
    }

    /// Convenience: build from a [`Layout`] (uses `layout.sessions_root`).
    pub fn from_layout(layout: &Layout, id: SessionId) -> Self {
        Self::new(&layout.sessions_root, id)
    }

    /// `<sessions_root>/<id>/` — the session's top-level dir
    /// (parent of `results/` and `worktrees/`). Used for cross-cutting
    /// markers like `.run.pid` that don't belong inside `results/`.
    pub fn session_dir(&self) -> PathBuf {
        self.results_dir
            .parent()
            .expect("results_dir always has a parent (<sessions_root>/<id>/)")
            .to_path_buf()
    }

    // ── Phase 0: baseline ────────────────────────────────────────────

    /// `results/baseline/`.
    pub fn baseline_dir(&self) -> PathBuf {
        self.results_dir
            .join("baseline")
    }

    /// `results/source.json` — per-session source-provenance record
    /// (v3 iteration). Sits at the results-tree root because every
    /// phase reads it; written once at session start, never mutated.
    /// See [`crate::models::source::SourceJson`].
    pub fn source_json(&self) -> PathBuf {
        self.results_dir
            .join("source.json")
    }

    /// `results/baseline/bench-run.json`.
    pub fn baseline_bench_run_json(&self) -> PathBuf {
        self.baseline_dir()
            .join("bench-run.json")
    }

    /// `results/baseline/rerun.json`.
    pub fn baseline_rerun_json(&self) -> PathBuf {
        self.baseline_dir()
            .join("rerun.json")
    }

    /// `results/baseline/bench-list.json`.
    pub fn bench_list_json(&self) -> PathBuf {
        self.baseline_dir()
            .join("bench-list.json")
    }

    /// `results/baseline/profiler-hotspots.json`.
    pub fn baseline_profiler_hotspots_json(&self) -> PathBuf {
        self.baseline_dir()
            .join("profiler-hotspots.json")
    }

    /// `results/baseline/run-id`.
    pub fn baseline_run_id_path(&self) -> PathBuf {
        self.baseline_dir()
            .join("run-id")
    }

    /// `results/baseline/rerun-id`.
    pub fn baseline_rerun_id_path(&self) -> PathBuf {
        self.baseline_dir()
            .join("rerun-id")
    }

    /// `results/baseline/noise-floor-pct` — single line with the
    /// computed noise floor percentage, written when baseline produces
    /// a (run, rerun) pair OR when import resolves a single-run
    /// fallback. Triage reads this to render the prompt.
    pub fn baseline_noise_floor_path(&self) -> PathBuf {
        self.baseline_dir()
            .join("noise-floor-pct")
    }

    /// `results/baseline/bin/` — directory holding the archived
    /// `stacks-bench` binary used by Phase 0b baseline, Phase 1.8
    /// calibration. See Phase 0a in
    /// `planning/archive/superseded/0017-pass-1c-historical-plan.md`, Sub-step
    /// A.
    pub fn baseline_bin_dir(&self) -> PathBuf {
        self.baseline_dir()
            .join("bin")
    }

    /// `results/baseline/bin/stacks-bench` — the archived binary
    /// itself. Strict-binary code paths read this path directly;
    /// missing file → hard error, no `cargo stacks-bench` fallback.
    pub fn baseline_bin_path(&self) -> PathBuf {
        self.baseline_bin_dir()
            .join("stacks-bench")
    }

    /// `results/baseline/bin/manifest.json` — provenance for the
    /// archived binary: source sha, cargo version, build flags,
    /// archived_at timestamp.
    pub fn baseline_bin_manifest_path(&self) -> PathBuf {
        self.baseline_bin_dir()
            .join("manifest.json")
    }

    /// `results/baseline/bench-run.stderr.log`.
    pub fn baseline_bench_run_stderr(&self) -> PathBuf {
        self.baseline_dir()
            .join("bench-run.stderr.log")
    }

    /// `results/baseline/rerun.stderr.log`.
    pub fn baseline_rerun_stderr(&self) -> PathBuf {
        self.baseline_dir()
            .join("rerun.stderr.log")
    }

    // ── Phase 1: triage ──────────────────────────────────────────────

    /// `results/triage/`.
    pub fn triage_dir(&self) -> PathBuf {
        self.results_dir
            .join("triage")
    }

    /// `results/triage/candidates.json`.
    pub fn candidates_json(&self) -> PathBuf {
        self.triage_dir()
            .join("candidates.json")
    }

    /// `results/triage/candidates.md`.
    pub fn candidates_md(&self) -> PathBuf {
        self.triage_dir()
            .join("candidates.md")
    }

    /// `results/triage/prompt.md`.
    pub fn triage_prompt(&self) -> PathBuf {
        self.triage_dir()
            .join("prompt.md")
    }

    /// `results/triage/events.jsonl`.
    pub fn triage_events(&self) -> PathBuf {
        self.triage_dir()
            .join("events.jsonl")
    }

    /// `results/triage/stderr.log`.
    pub fn triage_stderr(&self) -> PathBuf {
        self.triage_dir()
            .join("stderr.log")
    }

    /// `results/triage/conversation-id`.
    pub fn triage_conversation_id(&self) -> PathBuf {
        self.triage_dir()
            .join("conversation-id")
    }

    /// `results/triage/final-message.md`.
    pub fn triage_final_message(&self) -> PathBuf {
        self.triage_dir()
            .join("final-message.md")
    }

    /// `results/triage/queries` — pre-rendered run-id-scoped SQL outputs
    /// produced before the triage agent runs, so the agent doesn't burn
    /// `command_execution` events re-running the same orientation /
    /// candidate-ranking queries itself. See
    /// [`crate::session::triage_queries`].
    pub fn triage_queries_dir(&self) -> PathBuf {
        self.triage_dir()
            .join("queries")
    }

    /// `results/triage/drilldowns` — per-span / per-tx CSV slices the
    /// triage agent generates while investigating candidates. Kept
    /// distinct from the prerendered orientation set under `queries/`.
    pub fn triage_drilldowns_dir(&self) -> PathBuf {
        self.triage_dir()
            .join("drilldowns")
    }

    // ── Phase 1.5: analysis (analyzer fan-out) ───────────────────────

    /// `results/analysis` — root for `analysis/<family-id>/...`.
    pub fn analysis_dir(&self) -> PathBuf {
        self.results_dir
            .join("analysis")
    }

    /// `results/analysis/<family-id>/`.
    pub fn analysis_family_dir(&self, family_id: &str) -> PathBuf {
        self.analysis_dir()
            .join(family_id)
    }

    /// `results/analysis/<family-id>/analysis.json`.
    pub fn analysis_json(&self, family_id: &str) -> PathBuf {
        self.analysis_family_dir(family_id)
            .join("analysis.json")
    }

    // ── Phase 1.7: merge ─────────────────────────────────────────────

    /// `results/merge/`.
    pub fn merge_dir(&self) -> PathBuf {
        self.results_dir.join("merge")
    }

    /// `results/merge/optimization-targets.json`.
    pub fn optimization_targets_json(&self) -> PathBuf {
        self.merge_dir()
            .join("optimization-targets.json")
    }

    /// `results/merge/prompt.md`.
    pub fn merge_prompt(&self) -> PathBuf {
        self.merge_dir()
            .join("prompt.md")
    }

    /// `results/merge/events.jsonl`.
    pub fn merge_events(&self) -> PathBuf {
        self.merge_dir()
            .join("events.jsonl")
    }

    /// `results/merge/stderr.log`.
    pub fn merge_stderr(&self) -> PathBuf {
        self.merge_dir()
            .join("stderr.log")
    }

    /// `results/merge/final-message.md`.
    pub fn merge_final_message(&self) -> PathBuf {
        self.merge_dir()
            .join("final-message.md")
    }

    /// `results/merge/conversation-id`.
    pub fn merge_conversation_id(&self) -> PathBuf {
        self.merge_dir()
            .join("conversation-id")
    }

    // ── Phase 2: optimize ────────────────────────────────────────────

    /// `results/optimize` — root for `optimize/<target-id>/...` (Phase 2
    /// optimizer agent artifacts).
    pub fn optimize_dir(&self) -> PathBuf {
        self.results_dir
            .join("optimize")
    }

    // ── Phase 1.8: targeted baseline calibration (Pass 1a) ───────────

    /// `results/verify/` — root for per-target Phase 1.8 (and future
    /// Phase 1.9 verifier) artifacts. Separate from `optimize/`
    /// because verification owns its own audit trail; conflating
    /// the two confuses re-runs and clean-step semantics.
    pub fn verify_dir(&self) -> PathBuf {
        self.results_dir
            .join("verify")
    }

    /// `results/verify/<target-id>/`.
    pub fn verify_target_dir(&self, target_id: &str) -> PathBuf {
        self.verify_dir()
            .join(target_id)
    }

    /// Per-invocation baseline calibration run dir. One subdir per
    /// `BenchInvocation.id` on the target's `verification_replay`.
    pub fn verify_baseline_invocation_dir(&self, target_id: &str, invocation_id: &str) -> PathBuf {
        self.verify_target_dir(target_id)
            .join(invocation_id)
    }

    /// `results/verify/<target-id>/<invocation-id>/bench-run.json` — the
    /// stacks-bench output for one Phase 1.8 invocation.
    pub fn verify_baseline_bench_run_json(&self, target_id: &str, invocation_id: &str) -> PathBuf {
        self.verify_baseline_invocation_dir(target_id, invocation_id)
            .join("bench-run.json")
    }

    /// `results/verify/<target-id>/<invocation-id>/bench-run.stderr.log`.
    pub fn verify_baseline_bench_run_stderr(
        &self,
        target_id: &str,
        invocation_id: &str,
    ) -> PathBuf {
        self.verify_baseline_invocation_dir(target_id, invocation_id)
            .join("bench-run.stderr.log")
    }

    /// `results/verify/<target-id>/baseline-run-ids.json` —
    /// [`InvocationRunIds`](crate::models::common::InvocationRunIds) JSON
    /// pairing each invocation `id` with the stacks-bench `benchmark_run`
    /// row Phase 1.8 produced for it.
    pub fn verify_baseline_run_ids_json(&self, target_id: &str) -> PathBuf {
        self.verify_target_dir(target_id)
            .join("baseline-run-ids.json")
    }

    /// `results/optimize/<target-id>/`.
    pub fn experiment_dir(&self, target_id: &str) -> PathBuf {
        self.optimize_dir()
            .join(target_id)
    }

    /// `results/optimize/<target-id>/optimizer-report.json` — typed
    /// authoritative output of the optimizer agent (Phase 2/3 contract).
    /// The coordinator dispatches commit/abort decisions on this file's
    /// parsed contents; `implementation.md` / `abort.md` are
    /// coordinator-rendered companions derived from it.
    pub fn experiment_optimizer_report(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("optimizer-report.json")
    }

    /// `results/optimize/<target-id>/implementation.md` — coordinator-
    /// rendered companion view of an `outcome=implemented` optimizer
    /// report. The agent does NOT write this; it's regenerated from
    /// [`Self::experiment_optimizer_report`] post-validation.
    pub fn experiment_implementation(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("implementation.md")
    }

    /// `results/optimize/<target-id>/abort.md` — coordinator-rendered
    /// companion view of an `outcome=aborted` optimizer report (or a
    /// demoted implementation that didn't actually commit). The agent
    /// does NOT write this directly; it's regenerated from
    /// [`Self::experiment_optimizer_report`] post-validation.
    pub fn experiment_abort(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("abort.md")
    }

    /// `results/optimize/<target-id>/consensus-issue.md` — coordinator-
    /// written marker for `delivery_mode=consensus_issue` targets.
    /// Unlike the other markers, this one IS authoritative (the
    /// coordinator skips the optimizer entirely for issue-only routing,
    /// so there's no agent-written report to derive from).
    pub fn experiment_consensus_issue(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("consensus-issue.md")
    }

    /// `results/optimize/<target-id>/candidate-run-ids.json` —
    /// [`InvocationRunIds`](crate::models::common::InvocationRunIds) JSON
    /// pairing each invocation `id` with the stacks-bench `benchmark_run`
    /// row Phase 3 produced for it. Symmetric with
    /// [`Self::verify_baseline_run_ids_json`].
    pub fn experiment_candidate_run_ids_json(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("candidate-run-ids.json")
    }

    /// Per-invocation candidate bench run dir. One subdir per
    /// `BenchInvocation.id`.
    pub fn experiment_candidate_invocation_dir(
        &self,
        target_id: &str,
        invocation_id: &str,
    ) -> PathBuf {
        self.experiment_dir(target_id)
            .join(invocation_id)
    }

    /// `results/optimize/<target-id>/<invocation-id>/bench-run.json`.
    pub fn experiment_candidate_bench_run_json(
        &self,
        target_id: &str,
        invocation_id: &str,
    ) -> PathBuf {
        self.experiment_candidate_invocation_dir(target_id, invocation_id)
            .join("bench-run.json")
    }

    /// `results/optimize/<target-id>/<invocation-id>/bench-run.stderr.log`.
    pub fn experiment_candidate_bench_run_stderr(
        &self,
        target_id: &str,
        invocation_id: &str,
    ) -> PathBuf {
        self.experiment_candidate_invocation_dir(target_id, invocation_id)
            .join("bench-run.stderr.log")
    }

    // ── Phase 3: bench ───────────────────────────────────────────────
    // No dedicated dir; candidate-* bench JSONs land alongside the
    // optimizer's outputs inside `optimize/<target-id>/` so each
    // target has a single audit trail. `candidate-` prefix is the
    // disambiguator (vs the optimizer's `prompt.md`, `events.jsonl`).

    // ── Phase 3.5: results-analyzer ──────────────────────────────────

    /// `results/analyze/` — root for per-target results-analyzer agent
    /// artifacts. Holds `<target>/results-analysis.{json,md}` plus the
    /// per-target subagent log (prompt, events, etc.). Distinct from
    /// `analysis/` (Phase 1.5 analyzer-agent output, family-keyed) and
    /// from `verify/` + `optimize/` (which hold raw bench outputs).
    pub fn analyze_dir(&self) -> PathBuf {
        self.results_dir
            .join("analyze")
    }

    /// `results/analyze/<target-id>/`.
    pub fn analyze_target_dir(&self, target_id: &str) -> PathBuf {
        self.analyze_dir()
            .join(target_id)
    }

    /// `results/analyze/<target-id>/results-analysis.json` — typed
    /// verdict written by the Phase 3.5 results-analyzer agent.
    pub fn analyze_results_analysis_json(&self, target_id: &str) -> PathBuf {
        self.analyze_target_dir(target_id)
            .join("results-analysis.json")
    }

    /// `results/analyze/<target-id>/results-analysis.md` — operator-
    /// facing companion (short prose).
    pub fn analyze_results_analysis_md(&self, target_id: &str) -> PathBuf {
        self.analyze_target_dir(target_id)
            .join("results-analysis.md")
    }

    /// `results/analyze/<target-id>/prompt.md`.
    pub fn analyze_prompt(&self, target_id: &str) -> PathBuf {
        self.analyze_target_dir(target_id)
            .join("prompt.md")
    }

    /// `results/analyze/<target-id>/events.jsonl`.
    pub fn analyze_events_jsonl(&self, target_id: &str) -> PathBuf {
        self.analyze_target_dir(target_id)
            .join("events.jsonl")
    }

    /// `results/analyze/<target-id>/stderr.log`.
    pub fn analyze_stderr(&self, target_id: &str) -> PathBuf {
        self.analyze_target_dir(target_id)
            .join("stderr.log")
    }

    /// `results/analyze/<target-id>/final-message.md`.
    pub fn analyze_final_message(&self, target_id: &str) -> PathBuf {
        self.analyze_target_dir(target_id)
            .join("final-message.md")
    }

    /// `results/analyze/<target-id>/conversation-id`.
    pub fn analyze_conversation_id(&self, target_id: &str) -> PathBuf {
        self.analyze_target_dir(target_id)
            .join("conversation-id")
    }

    // ── Phase 4: finalize ────────────────────────────────────────────

    /// `results/finalize/`.
    pub fn finalize_dir(&self) -> PathBuf {
        self.results_dir
            .join("finalize")
    }

    /// `results/finalize/summary.json`.
    pub fn summary_json(&self) -> PathBuf {
        self.finalize_dir()
            .join("summary.json")
    }

    /// `results/finalize/summary.md`.
    pub fn summary_md(&self) -> PathBuf {
        self.finalize_dir()
            .join("summary.md")
    }

    /// `results/finalize/targets.md` — human-readable catalog rendered from
    /// `optimization-targets.json` alongside the summary.
    pub fn targets_md(&self) -> PathBuf {
        self.finalize_dir()
            .join("targets.md")
    }

    // ── Phase 5: publish ─────────────────────────────────────────────
    // No dedicated dir; pr-writer / issue-writer artifacts land
    // alongside the optimizer's outputs inside `optimize/<target-id>/`
    // so each target has a single audit trail. `pr-writer-` /
    // `issue-writer-` prefix is the disambiguator (vs the optimizer's
    // `prompt.md`, `events.jsonl`).

    // ── Test / bootstrap helper ──────────────────────────────────────

    /// Create every phase-level subdir under `results/`. Production
    /// phase writers create their own dir as they go; this exists for
    /// test fixtures (and the occasional one-shot migration script)
    /// that want to materialize a fully-shaped session tree up-front
    /// so subsequent helper-driven writes don't need to think about
    /// parent-dir creation.
    pub fn create_all_phase_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.results_dir.clone(),
            self.baseline_dir(),
            self.triage_dir(),
            self.triage_queries_dir(),
            self.triage_drilldowns_dir(),
            self.analysis_dir(),
            self.merge_dir(),
            self.optimize_dir(),
            self.verify_dir(),
            self.analyze_dir(),
            self.finalize_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

impl AsRef<Path> for SessionLayout {
    fn as_ref(&self) -> &Path {
        &self.results_dir
    }
}
