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
//!                          place. Files include: implementation.md OR
//!                          abort.md OR consensus-issue.md, prompt.md,
//!                          events.jsonl, stderr.log, final-message.md,
//!                          conversation-id, nextest.log, run-ids,
//!                          run-N/bench-run.json, pr-title.txt,
//!                          pr-body.md, pr-writer-*, issue-*, etc.
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

    // ── Phase 0: baseline ────────────────────────────────────────────

    /// `results/baseline/`.
    pub fn baseline_dir(&self) -> PathBuf {
        self.results_dir
            .join("baseline")
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

    /// `results/optimize/<target-id>/`.
    pub fn experiment_dir(&self, target_id: &str) -> PathBuf {
        self.optimize_dir()
            .join(target_id)
    }

    /// `results/optimize/<target-id>/implementation.md`.
    pub fn experiment_implementation(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("implementation.md")
    }

    /// `results/optimize/<target-id>/abort.md`.
    pub fn experiment_abort(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("abort.md")
    }

    /// `results/optimize/<target-id>/consensus-issue.md`.
    pub fn experiment_consensus_issue(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("consensus-issue.md")
    }

    /// `results/optimize/<target-id>/run-ids` — newline-delimited list of
    /// run ids the optimizer produced (one per benchmark execution).
    pub fn experiment_run_ids_path(&self, target_id: &str) -> PathBuf {
        self.experiment_dir(target_id)
            .join("run-ids")
    }

    // ── Phase 3: bench ───────────────────────────────────────────────
    // No dedicated dir; candidate-* bench JSONs land alongside the
    // optimizer's outputs inside `optimize/<target-id>/` so each
    // target has a single audit trail. `candidate-` prefix is the
    // disambiguator (vs the optimizer's `prompt.md`, `events.jsonl`).

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
