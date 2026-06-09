//! Phase 4: produce `summary.json` + `summary.md` for one session.
//!
//! Per-target dispatch (reads the typed `optimizer-report.json` written
//! by Phase 2 for non-consensus_issue targets):
//! - `consensus_issue` → `RoutedToIssue` if `consensus-issue.md` marker present
//!   (coordinator-written; the optimizer is skipped for this mode), else
//!   `Aborted`.
//! - `consensus_poc_pr` → `PocLanded` if `outcome=implemented`, `Aborted`
//!   otherwise (either `outcome=aborted` or no report at all).
//! - `normal_pr` → `Aborted` if `outcome=aborted` or no report or no run-ids;
//!   otherwise compare bench means. `improvement_pct > noise_floor` →
//!   `Accepted`, `< -noise_floor` → `Rejected (regression)`, otherwise
//!   `Rejected (within noise)`.
//!
//! Pass 1c invariant: every `bench_eligible` target's
//! `Experiment.improvement_pct` + `Experiment.status` are sourced
//! verbatim from the Phase 3.5 results-analyzer's verdict at
//! `analyze/<target>/results-analysis.json`. Finalize doesn't compute
//! pooled means or noise-floor thresholds — it threads the agent's
//! judgment into the summary tree. When a target's verdict file is
//! absent or invalid the experiment lands as `Aborted` with a reason
//! naming the missing file; the rest of the session ships.

use std::fs;

use anyhow::{Context as _, Result};

use crate::models::common::{DeliveryMode, SchemaVersionV4};
use crate::models::optimizer_report::OptimizerReport;
use crate::models::summary::{
    ConsensusIssueCounts, ConsensusPocPrCounts, Experiment, ExperimentStatus, NormalPrCounts,
    OutcomeCounts, Summary,
};
use crate::models::targets::{MergedTarget, OptimizationTargets};
use crate::models::{FromJsonValidated, ToJson};
use crate::session::{SessionLayout, loader, render};

/// Inputs to the finalize step.
pub struct FinalizeInputs<'a> {
    /// Resolved per-session layout.
    pub layout: &'a SessionLayout,
}

/// Finalize one session: produce `summary.json` + `summary.md` from the
/// merge artifact and the per-target results-analysis verdicts.
/// Returns the in-memory [`Summary`] in addition to writing it.
pub fn finalize(inputs: &FinalizeInputs<'_>) -> Result<Summary> {
    let targets = loader::read_optimization_targets(inputs.layout)?;
    let analyses = loader::read_all_analyses(inputs.layout)?;
    // Load every target's verdict ONCE with full context-checking
    // (session_id + target_id + schema). compute_summary and render
    // both consume from this map so summary.json and summary.md
    // cannot disagree about which verdicts are valid for this session.
    let verdicts = load_verdicts(inputs.layout, &targets)?;

    let summary = compute_summary(&targets, inputs, &verdicts)?;
    let notes = render::load_experiment_notes(inputs.layout, &targets);
    let summary_md = render::render_summary_md(&summary, &targets, &analyses, &notes, &verdicts);
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
    let json = summary.to_json_pretty()?;
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

/// Map of per-target verdicts indexed by target id. Built once by
/// [`load_verdicts`] and threaded through `compute_summary` + render so
/// the summary table and the per-target verdict blocks cannot disagree
/// about which verdicts are valid for this session.
pub type VerdictMap =
    std::collections::BTreeMap<String, crate::models::results_analysis::ResultsAnalysis>;

/// Load + context-check every target's
/// `analyze/<target>/results-analysis.json`. Missing / invalid /
/// wrong-context files are silently elided (warnings flow via
/// [`loader::read_results_analysis_for_target`]) so a single bad file
/// can't take out the whole session.
fn load_verdicts(layout: &SessionLayout, targets: &OptimizationTargets) -> Result<VerdictMap> {
    let mut out = VerdictMap::new();
    for t in &targets.targets {
        if let Some(ra) = loader::read_results_analysis_for_target(layout, &t.id)? {
            out.insert(t.id.clone(), ra);
        }
    }
    Ok(out)
}

/// Compute a [`Summary`] without writing it. Pure-ish; takes the
/// already-loaded inputs so tests can drive it directly.
pub fn compute_summary(
    targets: &OptimizationTargets,
    inputs: &FinalizeInputs<'_>,
    verdicts: &VerdictMap,
) -> Result<Summary> {
    let mut experiments: Vec<Experiment> = Vec::with_capacity(targets.targets.len());
    for t in &targets.targets {
        let mut exp = evaluate_target(t, inputs, verdicts)?;
        // Thread coordinator-provenance (base_sha + head_sha) into the
        // experiment record when the sidecar exists. The sidecar is
        // only written for targets whose coordinator commit landed
        // (i.e. shipped optimizer-report.json with outcome=implemented);
        // aborted experiments + consensus_issue rows leave both fields
        // None.
        if let Some(p) = load_coordinator_provenance(inputs, t)
            .with_context(|| format!("loading coordinator provenance for {}", t.id))?
        {
            exp.base_sha = Some(p.base_sha);
            exp.head_sha = Some(p.head_sha);
        }
        experiments.push(exp);
    }

    let outcome_counts = aggregate_counts(&experiments);
    let next_targets_hint = compute_hint(&experiments);

    // v3 Phase 3 cutover: populate source-provenance fields from
    // `<session>/results/source.json` when it exists (every
    // post-cutover session writes it at session start). Sessions that
    // ran pre-cutover may have no source.json — leave fields `None`
    // in that case so legacy artifacts continue to finalize cleanly.
    let source_path = inputs.layout.source_json();
    let (source_url, source_branch, source_sha, source_fetched_at) = if source_path.exists() {
        let s = crate::models::source::SourceJson::read(&source_path)
            .with_context(|| format!("loading source.json at {}", source_path.display()))?;
        (Some(s.url), Some(s.branch), Some(s.sha), Some(s.fetched_at))
    } else {
        (None, None, None, None)
    };

    Ok(Summary {
        schema_version: SchemaVersionV4,
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
        source_url,
        source_branch,
        source_sha,
        source_fetched_at,
    })
}

/// Evaluate one merged target into an [`Experiment`] row.
fn evaluate_target(
    t: &MergedTarget,
    inputs: &FinalizeInputs<'_>,
    verdicts: &VerdictMap,
) -> Result<Experiment> {
    // consensus_issue: marker present → RoutedToIssue, else Aborted.
    // This is the only branch that still reads a marker file directly,
    // because the coordinator (not the agent) writes consensus-issue.md.
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

    // Non-issue modes read the typed optimizer report. The report's
    // `outcome` is the agent's authoritative claim about what happened;
    // for `normal_pr` the bench-eval flow layers on top. The
    // context-checking variant rejects reports whose target_id /
    // session_id / delivery_mode don't match the merged target's, so a
    // misbehaving agent can't claim a different mode to bypass
    // mode-specific invariants.
    let report = loader::read_optimizer_report_for_target(inputs.layout, &t.id, t.delivery_mode)
        .with_context(|| format!("reading optimizer-report.json for {}", t.id))?;

    let implemented = match report {
        None => {
            return Ok(experiment(
                t,
                ExperimentStatus::Aborted,
                None,
                None,
                Some(
                    "no optimizer-report.json emitted — Phase 2 agent crashed or never ran"
                        .to_owned(),
                ),
            ));
        }
        Some(OptimizerReport::Aborted(r)) => {
            return Ok(experiment(t, ExperimentStatus::Aborted, None, None, Some(r.reason)));
        }
        Some(OptimizerReport::Implemented(r)) => r,
    };

    // consensus_poc_pr: implemented → PocLanded (no bench).
    if t.delivery_mode == DeliveryMode::ConsensusPocPr {
        let _ = implemented; // landed; agent's pr_title etc. stay in the typed report
        return Ok(experiment(t, ExperimentStatus::PocLanded, None, None, None));
    }

    // normal_pr: source verdict from the Phase 3.5 results-analyzer
    // agent. Pass 1c invariant — every bench_eligible target carries a
    // `verification_replay` with N invocations; Phase 1.8 wrote
    // baseline run-ids, Phase 3 wrote candidate run-ids, Phase 3.5
    // wrote the typed verdict. Canonicalize each side to the target's
    // VR invocation order so the run-id sequence on the Experiment row
    // matches the rendered invocation order (`render_run_ids` links by
    // index against the same order).
    let expected_order = expected_invocation_order(t)?;

    let raw_candidate_ids = match load_invocation_run_ids(
        &inputs
            .layout
            .experiment_candidate_run_ids_json(&t.id),
    )? {
        Some(ids) => ids,
        None => {
            return Ok(experiment(
                t,
                ExperimentStatus::Aborted,
                None,
                None,
                Some(
                    "no candidate-run-ids.json — Phase 3 (`session bench run`) did not complete \
                     for this target"
                        .to_owned(),
                ),
            ));
        }
    };
    let candidate_ids = match reorder_to_match(&raw_candidate_ids, &expected_order, "candidate") {
        Ok(ids) => ids,
        Err(e) => {
            return Ok(experiment(
                t,
                ExperimentStatus::Aborted,
                Some(raw_candidate_ids.run_ids()),
                None,
                Some(format!("for target `{}`: {e:#}", t.id)),
            ));
        }
    };

    // Baseline (Phase 1.8 output). Missing file is a hard error: Pass
    // 1c invariant says every bench_eligible target ran Phase 1.8.
    let baseline_path = inputs
        .layout
        .verify_baseline_run_ids_json(&t.id);
    let raw_baseline_ids = load_invocation_run_ids(&baseline_path)?.ok_or_else(|| {
        anyhow::anyhow!(
            "verify/{}/baseline-run-ids.json missing for bench_eligible target `{}`; Phase 1.8 \
             calibration MUST run before finalize (Pass 1c invariant)",
            t.id,
            t.id,
        )
    })?;
    let baseline_ids = reorder_to_match(&raw_baseline_ids, &expected_order, "baseline")
        .with_context(|| format!("for target `{}`", t.id))?;

    // Pull Phase 3.5 verdict from the pre-loaded map. Missing →
    // Aborted (the results-analyzer either didn't run or its file was
    // dropped by the load step because it failed context-checking).
    // The orchestrator's Phase 3.5 fan-out records the failure reason
    // in its console output; finalize surfaces the file-level absence
    // to the experiment row so the next session-archive + PR-writer
    // paths see a clean "did not ship" signal.
    let ra = match verdicts.get(t.id.as_str()) {
        Some(ra) => ra,
        None => {
            return Ok(experiment_with_baseline(
                t,
                ExperimentStatus::Aborted,
                Some(candidate_ids.run_ids()),
                Some(baseline_ids.run_ids()),
                None,
                Some(
                    "results-analyzer did not produce a verdict — \
                     analyze/<target>/results-analysis.json absent or invalid"
                        .to_owned(),
                ),
            ));
        }
    };

    // Sanity: cross-check the per-invocation run-ids the agent
    // recorded against the run-id files. The validator already
    // confirmed `per_invocation.invocation_id` matches the schema's
    // INVOCATION_ID_PATTERN; here we additionally verify each
    // invocation id is one we expect, in the right order, with
    // matching run-ids, AND the agent's chosen `axis` matches the
    // target's invocations (the schema-level axis-parity check on
    // `verification_replay` guarantees the invocations agree; here
    // we ensure the agent committed to the same one).
    if let Err(e) = cross_check_axis(ra, t) {
        return Ok(experiment_with_baseline(
            t,
            ExperimentStatus::Aborted,
            Some(candidate_ids.run_ids()),
            Some(baseline_ids.run_ids()),
            None,
            Some(format!("results-analysis axis-check failed: {e:#}")),
        ));
    }
    if let Err(e) = cross_check_per_invocation(ra, &baseline_ids, &candidate_ids) {
        return Ok(experiment_with_baseline(
            t,
            ExperimentStatus::Aborted,
            Some(candidate_ids.run_ids()),
            Some(baseline_ids.run_ids()),
            None,
            Some(format!("results-analysis cross-check failed: {e:#}")),
        ));
    }

    let (status, reason) = map_verdict_to_status(ra);
    Ok(experiment_with_baseline(
        t,
        status,
        Some(candidate_ids.run_ids()),
        Some(baseline_ids.run_ids()),
        ra.headline_improvement_pct,
        reason,
    ))
}

/// Map the typed verdict to an [`ExperimentStatus`] + optional reason
/// for the summary table. Pass 1c flow:
///
/// - `Verdict::Accepted` → `Accepted`. No reason (the headline number carries
///   the signal).
/// - `Verdict::Mixed` → `Accepted` (publish, but surface caveats). The Phase 5
///   publisher will eventually demote to draft based on the deferred
///   `results_analysis.confidence_floor`; until then the caveats ride in
///   `reason` so summary.md surfaces them.
/// - `Verdict::Rejected` → `Rejected`. Reason = `headline_rationale`.
fn map_verdict_to_status(
    ra: &crate::models::results_analysis::ResultsAnalysis,
) -> (ExperimentStatus, Option<String>) {
    use crate::models::results_analysis::Verdict;
    match ra.verdict {
        Verdict::Accepted => (ExperimentStatus::Accepted, None),
        Verdict::Mixed => {
            // Pull caveats into the reason cell so reviewers see them
            // even before the publisher's confidence gate lands.
            let mut caveats = ra.caveats.clone();
            if caveats.is_empty() {
                caveats.push(ra.headline_rationale.clone());
            }
            (ExperimentStatus::Accepted, Some(format!("mixed: {}", caveats.join("; "))))
        }
        Verdict::Rejected => (ExperimentStatus::Rejected, Some(ra.headline_rationale.clone())),
    }
}

/// Verify the results-analyzer's chosen `axis` matches the target's
/// `verification_replay.invocations[].expected_signal.axis` set. The
/// schema-level [`VerificationReplay::validate_model`] guarantees
/// every invocation on a target shares one axis; this just confirms
/// the agent committed to that same one. Without the cross-check, an
/// agent could ship a verdict denominated against the wrong lens and
/// the headline would be meaningless.
fn cross_check_axis(
    ra: &crate::models::results_analysis::ResultsAnalysis,
    t: &MergedTarget,
) -> Result<()> {
    let vr = t
        .verification_replay
        .as_ref()
        .with_context(|| {
            format!("bench_eligible target `{}` has no verification_replay (caller bug)", t.id)
        })?;
    let expected = vr.invocations[0]
        .expected_signal
        .axis;
    if ra.axis != expected {
        anyhow::bail!(
            "results-analysis.axis = {:?} but the target's invocations expect axis = {:?}",
            ra.axis,
            expected,
        );
    }
    Ok(())
}

/// Cross-check that the agent's `per_invocation[]` lines up with the
/// run-id files. The schema already ensures `invocation_id` is a
/// well-formed key; here we enforce: same length, same ordering, and
/// matching run-ids for both sides.
fn cross_check_per_invocation(
    ra: &crate::models::results_analysis::ResultsAnalysis,
    baseline: &crate::models::common::InvocationRunIds,
    candidate: &crate::models::common::InvocationRunIds,
) -> Result<()> {
    if ra.per_invocation.len() != baseline.entries.len() {
        anyhow::bail!(
            "per_invocation length {} != baseline run-ids length {}",
            ra.per_invocation.len(),
            baseline.entries.len(),
        );
    }
    if ra.per_invocation.len() != candidate.entries.len() {
        anyhow::bail!(
            "per_invocation length {} != candidate run-ids length {}",
            ra.per_invocation.len(),
            candidate.entries.len(),
        );
    }
    for (i, row) in ra
        .per_invocation
        .iter()
        .enumerate()
    {
        let b = &baseline.entries[i];
        let c = &candidate.entries[i];
        if row.invocation_id != b.invocation_id {
            anyhow::bail!(
                "per_invocation[{i}].invocation_id = {:?} but baseline-run-ids[{i}] = {:?}",
                row.invocation_id,
                b.invocation_id,
            );
        }
        if row.invocation_id != c.invocation_id {
            anyhow::bail!(
                "per_invocation[{i}].invocation_id = {:?} but candidate-run-ids[{i}] = {:?}",
                row.invocation_id,
                c.invocation_id,
            );
        }
        if row.baseline_run_id != b.run_id {
            anyhow::bail!(
                "per_invocation[{i}].baseline_run_id = {} but baseline-run-ids[{i}].run_id = {}",
                row.baseline_run_id,
                b.run_id,
            );
        }
        if row.candidate_run_id != c.run_id {
            anyhow::bail!(
                "per_invocation[{i}].candidate_run_id = {} but candidate-run-ids[{i}].run_id = {}",
                row.candidate_run_id,
                c.run_id,
            );
        }
    }
    Ok(())
}

/// Collect the expected invocation-id order from the merged target's
/// `verification_replay.invocations[]`. The order is canonical: both
/// baseline and candidate run-ids get reordered to match it before
/// downstream math + rendering. Hard error if VR is absent — Pass 1c
/// invariant says every `bench_eligible` target carries one (enforced
/// by [`MergedTarget::validate_model`]), so reaching this without one
/// is a caller bug.
fn expected_invocation_order(t: &MergedTarget) -> Result<Vec<String>> {
    let vr = t
        .verification_replay
        .as_ref()
        .with_context(|| {
            format!(
                "bench_eligible target `{}` has no verification_replay; merge validator should \
                 have rejected this (Pass 1c invariant)",
                t.id
            )
        })?;
    Ok(vr
        .invocations
        .iter()
        .map(|i| i.id.clone())
        .collect())
}

/// Canonicalize an [`InvocationRunIds`] to match `expected` order
/// exactly. Returns a new value whose `entries[i].invocation_id ==
/// expected[i]`. Errors when the actual id set doesn't equal
/// `expected`'s set (missing or extra invocations), which would mean
/// the file was produced for a different VR than the merged target
/// carries. `side` is `"baseline"` or `"candidate"`; embedded in the
/// error so the operator can locate the offending file.
fn reorder_to_match(
    actual: &crate::models::common::InvocationRunIds,
    expected: &[String],
    side: &str,
) -> Result<crate::models::common::InvocationRunIds> {
    let expected_set: std::collections::BTreeSet<&str> = expected
        .iter()
        .map(String::as_str)
        .collect();
    let actual_set: std::collections::BTreeSet<&str> = actual
        .entries
        .iter()
        .map(|e| e.invocation_id.as_str())
        .collect();
    if expected_set != actual_set {
        let missing: Vec<&&str> = expected_set
            .difference(&actual_set)
            .collect();
        let extra: Vec<&&str> = actual_set
            .difference(&expected_set)
            .collect();
        anyhow::bail!(
            "{side}-run-ids invocation set mismatch: missing {missing:?}, extra {extra:?}; \
             baseline + candidate must mirror verification_replay.invocations[].id exactly (Pass \
             1c invariant)"
        );
    }
    // Reorder: walk `expected`, locate the matching entry in `actual`.
    // Set equality above guarantees every expected id has exactly one
    // match, so the find is total.
    let mut entries = Vec::with_capacity(expected.len());
    for inv_id in expected {
        let entry = actual
            .entries
            .iter()
            .find(|e| &e.invocation_id == inv_id)
            .expect("set equality just verified this id exists");
        entries.push(entry.clone());
    }
    Ok(crate::models::common::InvocationRunIds { entries })
}

/// Construct an [`Experiment`] row carrying the target's `breakage_class`
/// when relevant. `base_sha` / `head_sha` start as `None`; the
/// outer `finalize` loop fills them from `coordinator-provenance.json`
/// after construction for experiments whose commit landed.
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
        baseline_run_ids: None,
        improvement_pct,
        breakage_class: t.breakage_class,
        base_sha: None,
        head_sha: None,
        reason,
    }
}

/// Variant of [`experiment`] that records the per-target baseline
/// run ids used as the improvement_pct denominator. Used by
/// `evaluate_normal_pr` when Phase 1.8 calibration produced a
/// per-target baseline. Same `base_sha` / `head_sha` post-construction
/// fill semantics as [`experiment`].
fn experiment_with_baseline(
    t: &MergedTarget,
    status: ExperimentStatus,
    run_ids: Option<Vec<i64>>,
    baseline_run_ids: Option<Vec<i64>>,
    improvement_pct: Option<f64>,
    reason: Option<String>,
) -> Experiment {
    Experiment {
        target_id: t.id.clone(),
        delivery_mode: t.delivery_mode,
        status,
        run_ids,
        baseline_run_ids,
        improvement_pct,
        breakage_class: t.breakage_class,
        base_sha: None,
        head_sha: None,
        reason,
    }
}

/// Read + validate `optimize/<target>/coordinator-provenance.json` for
/// the audit-trail surface on [`Experiment`]. Returns `Ok(None)` when
/// the sidecar is absent (expected for aborted experiments + sessions
/// that predate Pass 1c's provenance contract), `Ok(Some(_))` on a
/// clean sidecar, and an error on parse / validation failure (same
/// strict-fallback contract as [`load_per_target_baseline_ids`]).
fn load_coordinator_provenance(
    inputs: &FinalizeInputs<'_>,
    target: &MergedTarget,
) -> Result<Option<crate::models::coordinator_provenance::CoordinatorProvenance>> {
    let path = inputs
        .layout
        .experiment_dir(&target.id)
        .join("coordinator-provenance.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
        }
    };
    let p = crate::models::coordinator_provenance::CoordinatorProvenance::from_json_validated(&raw)
        .with_context(|| format!("parsing/validating {}", path.display()))?;
    // Context cross-check: catches a sidecar that parses + validates
    // but belongs to a different target / session / delivery mode.
    // Without this a stale sidecar (e.g. copied from another session's
    // exp_dir during operator recovery) could leak the wrong head_sha
    // into summary.json.
    p.validate_context(inputs.layout.id.as_str(), &target.id, target.delivery_mode)
        .with_context(|| path.display().to_string())?;
    Ok(Some(p))
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
            // Other combinations are invalid per Experiment::validate_model().
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

/// Load + validate an `InvocationRunIds` JSON file. Returns `Ok(None)`
/// when the file is absent, `Ok(Some(_))` on a valid file, or `Err` on
/// IO / parse / cross-field-validation failure. Callers gate on
/// presence (baseline missing for normal_pr = hard error; candidate
/// missing = Aborted experiment).
fn load_invocation_run_ids(
    path: &std::path::Path,
) -> Result<Option<crate::models::common::InvocationRunIds>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
        }
    };
    let ids: crate::models::common::InvocationRunIds =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    use crate::models::ValidateModel as _;
    ids.validate_model()
        .with_context(|| format!("invalid InvocationRunIds in {}", path.display()))?;
    Ok(Some(ids))
}
