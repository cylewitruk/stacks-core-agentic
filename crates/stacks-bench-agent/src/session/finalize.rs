//! Phase 4: produce `summary.json` + `summary.md` for one session.
//!
//! Per-target dispatch:
//! - `consensus_issue` → `RoutedToIssue` if marker present, else `Aborted`.
//! - `consensus_poc_pr` → `Aborted` if `abort.md` present, `PocLanded` if
//!   `implementation.md` present, else `Aborted` (no output at all).
//! - `normal_pr` → `Aborted` if `abort.md` present or no run-ids; otherwise
//!   compare bench means. `improvement_pct > noise_floor` → `Accepted`, `<
//!   -noise_floor` → `Rejected (regression)`, otherwise `Rejected (within
//!   noise)`.

use std::fs;

use anyhow::{Context as _, Result};

use crate::models::common::{DeliveryMode, SchemaVersionV2};
use crate::models::summary::{
    ConsensusIssueCounts, ConsensusPocPrCounts, Experiment, ExperimentStatus, NormalPrCounts,
    OutcomeCounts, Summary,
};
use crate::models::targets::{MergedTarget, OptimizationTargets};
use crate::session::bench::BenchClient;
use crate::session::{SessionLayout, loader, render};

/// Inputs to the finalize step.
pub struct FinalizeInputs<'a> {
    /// Resolved per-session layout.
    pub layout: &'a SessionLayout,
    /// `BenchClient` providing `total_duration_us` for normal_pr targets.
    pub bench: &'a dyn BenchClient,
}

/// Finalize one session: produce `summary.json` + `summary.md` from the
/// merge artifact, the per-experiment markers on disk, and the bench client.
/// Returns the in-memory [`Summary`] in addition to writing it.
pub fn finalize(inputs: &FinalizeInputs<'_>) -> Result<Summary> {
    let targets = loader::read_optimization_targets(inputs.layout)
        .context("loading optimization-targets.json")?;
    let analyses =
        loader::read_all_analyses(inputs.layout).context("loading analysis/*/analysis.json")?;

    let summary = compute_summary(&targets, inputs)?;
    let notes = render::load_experiment_notes(inputs.layout, &targets);
    let summary_md = render::render_summary_md(&summary, &targets, &analyses, &notes);
    let targets_md = render::render_targets_md(&targets, &analyses);

    fs::create_dir_all(inputs.layout.finalize_dir()).with_context(|| {
        format!(
            "creating {}",
            inputs
                .layout
                .finalize_dir()
                .display()
        )
    })?;
    let json = serde_json::to_string_pretty(&summary)?;
    fs::write(inputs.layout.summary_json(), json + "\n").with_context(|| {
        format!(
            "writing {}",
            inputs
                .layout
                .summary_json()
                .display()
        )
    })?;
    fs::write(inputs.layout.summary_md(), summary_md).with_context(|| {
        format!(
            "writing {}",
            inputs
                .layout
                .summary_md()
                .display()
        )
    })?;
    fs::write(inputs.layout.targets_md(), targets_md).with_context(|| {
        format!(
            "writing {}",
            inputs
                .layout
                .targets_md()
                .display()
        )
    })?;
    Ok(summary)
}

/// Compute a [`Summary`] without writing it. Pure-ish; takes the inputs
/// already loaded so tests can drive it directly.
pub fn compute_summary(
    targets: &OptimizationTargets,
    inputs: &FinalizeInputs<'_>,
) -> Result<Summary> {
    // Compute baseline mean only when at least one normal_pr target needs
    // it (matches the bash NEEDS_BASELINE optimization).
    let needs_baseline = targets
        .targets
        .iter()
        .any(|t| t.delivery_mode == DeliveryMode::NormalPr);
    let baseline_mean = if needs_baseline {
        let b1 = inputs
            .bench
            .total_duration_us(targets.baseline_run_id)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "missing baseline summary (run_id={}); required for normal_pr targets",
                    targets.baseline_run_id
                )
            })?;
        let b2 = inputs
            .bench
            .total_duration_us(targets.baseline_rerun_id)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "missing baseline rerun summary (run_id={}); required for normal_pr targets",
                    targets.baseline_rerun_id
                )
            })?;
        Some((b1 + b2) as f64 / 2.0)
    } else {
        None
    };

    let mut experiments: Vec<Experiment> = Vec::with_capacity(targets.targets.len());
    for t in &targets.targets {
        experiments.push(evaluate_target(t, inputs, baseline_mean, targets.noise_floor_pct)?);
    }

    let outcome_counts = aggregate_counts(&experiments);
    let next_targets_hint = compute_hint(&experiments);

    Ok(Summary {
        schema_version: SchemaVersionV2,
        session_id: targets.session_id.clone(),
        baseline_run_id: targets.baseline_run_id,
        baseline_rerun_id: targets.baseline_rerun_id,
        noise_floor_pct: targets.noise_floor_pct,
        experiments,
        outcome_counts,
        lens_dispositions: targets
            .lens_dispositions
            .clone(),
        next_targets_hint: Some(next_targets_hint),
    })
}

/// Evaluate one merged target into an [`Experiment`] row.
fn evaluate_target(
    t: &MergedTarget,
    inputs: &FinalizeInputs<'_>,
    baseline_mean: Option<f64>,
    noise_floor_pct: f64,
) -> Result<Experiment> {
    // consensus_issue: marker present → RoutedToIssue, else Aborted.
    if t.delivery_mode == DeliveryMode::ConsensusIssue {
        let marker = inputs
            .layout
            .experiment_consensus_issue(&t.id);
        let status = if is_non_empty_file(&marker) {
            ExperimentStatus::RoutedToIssue
        } else {
            ExperimentStatus::Aborted
        };
        let reason = (status == ExperimentStatus::Aborted).then(|| {
            "consensus-issue.md marker missing — Phase 2 (`session optimize run`) did not complete"
                .to_owned()
        });
        return Ok(experiment(t, status, None, None, reason));
    }

    // Subagent abort marker (any non-issue mode).
    let abort_path = inputs
        .layout
        .experiment_abort(&t.id);
    if is_non_empty_file(&abort_path) {
        let reason = read_abort_reason(&abort_path)?;
        return Ok(experiment(t, ExperimentStatus::Aborted, None, None, Some(reason)));
    }

    // consensus_poc_pr: implementation.md presence is the success gate.
    if t.delivery_mode == DeliveryMode::ConsensusPocPr {
        let impl_path = inputs
            .layout
            .experiment_implementation(&t.id);
        let status = if is_non_empty_file(&impl_path) {
            ExperimentStatus::PocLanded
        } else {
            ExperimentStatus::Aborted
        };
        let reason = (status == ExperimentStatus::Aborted).then(|| {
            "no implementation.md emitted (PoC-mode optimizer produced no output)".to_owned()
        });
        return Ok(experiment(t, status, None, None, reason));
    }

    // normal_pr: bench-eval flow.
    let run_ids_path = inputs
        .layout
        .experiment_run_ids_path(&t.id);
    let run_ids = loader::read_experiment_run_ids(&run_ids_path)?;
    if run_ids.is_empty() {
        return Ok(experiment(
            t,
            ExperimentStatus::Aborted,
            None,
            None,
            Some("no benchmark runs recorded".to_owned()),
        ));
    }

    let mut durations: Vec<i64> = Vec::with_capacity(run_ids.len());
    for &id in &run_ids {
        if let Some(d) = inputs
            .bench
            .total_duration_us(id)?
        {
            durations.push(d);
        }
    }
    if durations.is_empty() {
        return Ok(experiment(
            t,
            ExperimentStatus::Aborted,
            Some(run_ids),
            None,
            Some("all benchmark runs missing summaries".to_owned()),
        ));
    }

    let baseline_mean = baseline_mean.expect("baseline_mean is computed when needed");
    let exp_mean = durations
        .iter()
        .copied()
        .sum::<i64>() as f64
        / durations.len() as f64;
    let improvement_pct =
        if baseline_mean == 0.0 { 0.0 } else { (baseline_mean - exp_mean) / baseline_mean * 100.0 };

    let (status, reason) = if improvement_pct > noise_floor_pct {
        (ExperimentStatus::Accepted, None)
    } else if improvement_pct < -noise_floor_pct {
        (
            ExperimentStatus::Rejected,
            Some(format!("regression: {:.2}% slower than baseline", -improvement_pct)),
        )
    } else {
        (
            ExperimentStatus::Rejected,
            Some(format!(
                "within noise floor ({improvement_pct:.2}% improvement vs {noise_floor_pct:.2}% \
                 noise)"
            )),
        )
    };
    Ok(experiment(t, status, Some(run_ids), Some(improvement_pct), reason))
}

/// Construct an [`Experiment`] row carrying the target's `breakage_class`
/// when relevant.
fn experiment(
    t: &MergedTarget,
    status: ExperimentStatus,
    run_ids: Option<Vec<i64>>,
    improvement_pct: Option<f64>,
    reason: Option<String>,
) -> Experiment {
    Experiment {
        target_id: t.id.clone(),
        delivery_mode: t.delivery_mode,
        status,
        run_ids,
        improvement_pct,
        breakage_class: t.breakage_class,
        reason,
    }
}

/// First 300 chars of the abort.md, with newlines collapsed and edges
/// trimmed. Mirrors the bash `REASON=$(head -c 4096 ... | tr ... | awk ... |
/// cut -c1-300)`.
fn read_abort_reason(path: &std::path::Path) -> Result<String> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let head: String = raw
        .chars()
        .take(4096)
        .collect();
    let collapsed: String = head
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let trimmed = collapsed
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Ok(trimmed
        .chars()
        .take(300)
        .collect())
}

/// Aggregate experiment outcomes into the schema-shaped `outcome_counts`.
fn aggregate_counts(experiments: &[Experiment]) -> OutcomeCounts {
    let mut np = NormalPrCounts::default();
    let mut cp = ConsensusPocPrCounts::default();
    let mut ci = ConsensusIssueCounts::default();
    for e in experiments {
        match (e.delivery_mode, e.status) {
            (DeliveryMode::NormalPr, ExperimentStatus::Accepted) => np.accepted += 1,
            (DeliveryMode::NormalPr, ExperimentStatus::Rejected) => np.rejected += 1,
            (DeliveryMode::NormalPr, ExperimentStatus::Aborted) => np.aborted += 1,
            (DeliveryMode::ConsensusPocPr, ExperimentStatus::PocLanded) => cp.poc_landed += 1,
            (DeliveryMode::ConsensusPocPr, ExperimentStatus::Aborted) => cp.aborted += 1,
            (DeliveryMode::ConsensusIssue, ExperimentStatus::RoutedToIssue) => {
                ci.routed_to_issue += 1
            }
            (DeliveryMode::ConsensusIssue, ExperimentStatus::Aborted) => ci.aborted += 1,
            // Other combinations are invalid per Experiment::validate().
            _ => {}
        }
    }
    OutcomeCounts {
        normal_pr: np,
        consensus_poc_pr: cp,
        consensus_issue: ci,
    }
}

/// Bash-equivalent next-targets hint logic.
fn compute_hint(experiments: &[Experiment]) -> String {
    let n_total = experiments.len();
    let n_accepted = count(experiments, ExperimentStatus::Accepted);
    let n_poc = count(experiments, ExperimentStatus::PocLanded);
    let n_issue = count(experiments, ExperimentStatus::RoutedToIssue);
    let n_aborted = count(experiments, ExperimentStatus::Aborted);
    let n_regressions = experiments
        .iter()
        .filter(|e| {
            e.status == ExperimentStatus::Rejected
                && e.reason
                    .as_deref()
                    .is_some_and(|r| r.starts_with("regression"))
        })
        .count();

    if n_total == 0 {
        return "zero targets reached benchmarking; check analysis/*/analysis.json".to_owned();
    }

    if n_accepted == 0 && n_poc == 0 && n_issue == 0 {
        if n_aborted == n_total {
            return "all experiments aborted before benchmarking; review optimize/*/abort.md and \
                    optimize/*/stderr.log"
                .to_owned();
        }
        if n_regressions > 0 {
            return format!(
                "rejected: {n_regressions} regression(s); rest within noise. Try smaller-scope \
                 changes or tighter targets."
            );
        }
        return "all rejected within noise floor; try wider profiler view (--profiler-hot 100+) \
                or different block range"
            .to_owned();
    }

    format!(
        "{n_accepted} PR(s) + {n_poc} PoC PR(s) + {n_issue} issue(s) of {n_total} target(s); \
         review and re-run rejected/aborted with refined analyses"
    )
}

fn count(experiments: &[Experiment], status: ExperimentStatus) -> usize {
    experiments
        .iter()
        .filter(|e| e.status == status)
        .count()
}

/// File exists and has nonzero length.
fn is_non_empty_file(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}
