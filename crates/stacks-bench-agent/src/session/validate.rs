//! `sbagent session validate`: confirm every required artifact is on disk.
//!
//! Walks the session results dir checking for every required artifact and
//! every cross-file invariant the v2 pipeline depends on. Returns a
//! [`ValidationReport`]; the CLI command formats and prints it. Returning
//! a structured report (rather than just a bool) keeps the logic testable.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::models::ValidateModel;
use crate::models::analyze::Analysis;
use crate::models::common::DeliveryMode;
use crate::session::{SessionLayout, loader};

/// Result of validating one session.
#[derive(Debug, Default)]
pub struct ValidationReport {
    /// Files the validator expected to find but didn't (or that exist but
    /// are empty).
    pub missing: Vec<String>,
    /// Soft warnings (e.g. `schema_version` mismatch on an otherwise valid
    /// file).
    pub schema_warnings: Vec<String>,
}

impl ValidationReport {
    /// True iff every check passed (no missing entries; warnings still OK).
    pub fn ok(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Run every check from `validate-session.sh` against `layout`.
pub fn validate(layout: &SessionLayout) -> Result<ValidationReport> {
    let mut report = ValidationReport::default();

    // Phase 0: shell-owned baseline artifacts.
    require_non_empty(&layout.baseline_bench_run_json(), "baseline/bench-run.json", &mut report);
    require_non_empty(&layout.baseline_rerun_json(), "baseline/rerun.json", &mut report);
    require_non_empty(&layout.bench_list_json(), "baseline/bench-list.json", &mut report);
    require_non_empty(
        &layout.baseline_profiler_hotspots_json(),
        "baseline/profiler-hotspots.json",
        &mut report,
    );
    require_non_empty(&layout.baseline_run_id_path(), "baseline/run-id", &mut report);
    require_non_empty(&layout.baseline_rerun_id_path(), "baseline/rerun-id", &mut report);

    // Phase 1: triage. The audit content (rejection narrative + lens
    // coverage) lives in typed fields on `candidates.json` now —
    // `final-message.md` is just the captured codex assistant turn,
    // useful for debugging but not a semantic artifact, so it's not
    // in the required-file check.
    require_non_empty(&layout.candidates_json(), "triage/candidates.json", &mut report);
    require_non_empty(&layout.triage_conversation_id(), "triage/conversation-id", &mut report);

    // Phase 1.7: merge.
    require_non_empty(
        &layout.optimization_targets_json(),
        "merge/optimization-targets.json",
        &mut report,
    );
    require_non_empty(&layout.merge_final_message(), "merge/final-message.md", &mut report);

    // Phase 4: summary.
    require_non_empty(&layout.summary_json(), "finalize/summary.json", &mut report);

    // Schema-version sanity for v2 artifacts. Any parse error here surfaces
    // as a missing entry; mismatched schema_version is a soft warning to
    // mirror the bash behavior.
    check_schema_version(&layout.candidates_json(), "triage/candidates.json", &mut report);
    check_schema_version(
        &layout.optimization_targets_json(),
        "merge/optimization-targets.json",
        &mut report,
    );
    check_schema_version(&layout.summary_json(), "finalize/summary.json", &mut report);

    // Phase 1.5: every candidate family must have an analysis.json.
    // Surface load + cross-field-validate failures as hard errors.
    if is_non_empty_file(&layout.candidates_json()) {
        match loader::read_candidates(layout) {
            Ok(candidates) => {
                if let Err(e) = candidates.validate_model() {
                    report
                        .missing
                        .push(format!("candidates.json failed validation: {e:#}"));
                }
                for c in &candidates.candidates {
                    let path = layout.analysis_json(&c.id);
                    if !is_non_empty_file(&path) {
                        report
                            .missing
                            .push(format!("analysis/{}/analysis.json", c.id));
                    }
                }
            }
            Err(e) => report
                .missing
                .push(format!("candidates.json failed to parse: {e:#}")),
        }
    }

    // Phase 2/3: per-target terminal markers.
    if is_non_empty_file(&layout.optimization_targets_json()) {
        match loader::read_optimization_targets(layout) {
            Ok(targets) => {
                if let Err(e) = targets.validate_model() {
                    report
                        .missing
                        .push(format!("optimization-targets.json failed validation: {e:#}"));
                }
                for t in &targets.targets {
                    match t.delivery_mode {
                        DeliveryMode::ConsensusIssue => {
                            let path = layout.experiment_consensus_issue(&t.id);
                            if !is_non_empty_file(&path) {
                                report.missing.push(format!(
                                    "optimize/{}/consensus-issue.md (consensus_issue routing \
                                     marker)",
                                    t.id
                                ));
                            }
                        }
                        DeliveryMode::NormalPr | DeliveryMode::ConsensusPocPr => {
                            // Validate against the typed optimizer-report.json
                            // (the authoritative contract). The companion
                            // implementation.md / abort.md are derived from
                            // it post-hoc and can drift, so we don't check
                            // for them — a stale companion alongside a
                            // missing/malformed report would otherwise mask
                            // the real problem.
                            match loader::read_optimizer_report_for_target(
                                layout,
                                &t.id,
                                t.delivery_mode,
                            ) {
                                Ok(Some(_)) => {}
                                Ok(None) => {
                                    report.missing.push(format!(
                                        "optimize/{}/optimizer-report.json (agent never wrote it; \
                                         Phase 2 crashed or didn't run)",
                                        t.id
                                    ));
                                }
                                Err(e) => {
                                    report.missing.push(format!(
                                        "optimize/{}/optimizer-report.json failed validation: \
                                         {e:#}",
                                        t.id
                                    ));
                                }
                            }
                        }
                    }
                }

                // Coverage invariants: every accepted analyzer target
                // accounted for; every accepted family appears in lens_dispositions.
                match loader::read_all_analyses(layout) {
                    Ok(analyses) => {
                        // Per-analysis cross-field validation. Bad analyses are
                        // reported as `<path> failed validation: <reason>`.
                        for (fid, a) in &analyses {
                            if let Err(e) = a.validate_model() {
                                report.missing.push(format!(
                                    "analysis/{fid}/analysis.json failed validation: {e:#}"
                                ));
                            }
                        }
                        check_target_coverage(&analyses, &targets, &mut report);
                        check_lens_disposition_coverage(&analyses, &targets, &mut report);
                    }
                    Err(e) => {
                        report
                            .missing
                            .push(format!("analysis/*/analysis.json failed to parse: {e:#}"));
                    }
                }
            }
            Err(e) => {
                report
                    .missing
                    .push(format!("optimization-targets.json failed to parse: {e:#}"));
            }
        }
    }

    Ok(report)
}

/// Append `label` to `report.missing` when `path` doesn't exist or is empty.
fn require_non_empty(path: &Path, label: &str, report: &mut ValidationReport) {
    if !is_non_empty_file(path) {
        report
            .missing
            .push(label.to_owned());
    }
}

/// File exists, is a regular file, and has nonzero length.
fn is_non_empty_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// Soft-check `schema_version` field on a v2 artifact. Mirrors the bash
/// behavior: emits a warning, not a missing entry.
fn check_schema_version(path: &PathBuf, label: &str, report: &mut ValidationReport) {
    if !is_non_empty_file(path) {
        return;
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return,
    };
    let n = v
        .get("schema_version")
        .and_then(|x| x.as_u64());
    if n != Some(2) {
        report
            .schema_warnings
            .push(format!(
                "{label} schema_version={} (expected 2)",
                n.map(|x| x.to_string())
                    .unwrap_or_else(|| "<unset>".to_owned())
            ));
    }
}

/// Coverage invariant: every accepted analyzer-emitted target appears in
/// exactly one of the merged target's `merged_from` arrays or in
/// `rejected_by_merge`. Mirrors the jq invariant in `validate-session.sh`.
fn check_target_coverage(
    analyses: &std::collections::BTreeMap<String, Analysis>,
    targets: &crate::models::targets::OptimizationTargets,
    report: &mut ValidationReport,
) {
    // Expected = (family_id, target_index) for every accepted analyzer's
    // every emitted target.
    let mut expected: BTreeSet<(String, usize)> = BTreeSet::new();
    for (fid, analysis) in analyses {
        if let Analysis::Accepted(a) = analysis {
            for (i, _) in a.targets.iter().enumerate() {
                expected.insert((fid.clone(), i));
            }
        }
    }

    // Accounted = entries from merged_from + rejected_by_merge.
    let mut accounted: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut duplicate = false;
    for t in &targets.targets {
        for mf in &t.merged_from {
            if !accounted.insert((mf.family_id.clone(), mf.target_index)) {
                duplicate = true;
            }
        }
    }
    for r in &targets.rejected_by_merge {
        if !accounted.insert((r.family_id.clone(), r.target_index)) {
            duplicate = true;
        }
    }

    if accounted != expected || duplicate {
        report.missing.push(
            "optimization-targets.json target coverage invariant (every (family_id, target_index) \
             from accepted analyses must appear once across merged_from / rejected_by_merge)"
                .to_owned(),
        );
    }
}

/// Coverage invariant: every accepted analysis's family_id appears in
/// `lens_dispositions[]` exactly once.
fn check_lens_disposition_coverage(
    analyses: &std::collections::BTreeMap<String, Analysis>,
    targets: &crate::models::targets::OptimizationTargets,
    report: &mut ValidationReport,
) {
    let expected: BTreeSet<String> = analyses
        .iter()
        .filter(|(_, a)| matches!(a, Analysis::Accepted(_)))
        .map(|(fid, _)| fid.clone())
        .collect();
    let mut present: BTreeSet<String> = BTreeSet::new();
    let mut duplicate = false;
    for d in &targets.lens_dispositions {
        if !present.insert(d.family_id.clone()) {
            duplicate = true;
        }
    }
    if present != expected || duplicate {
        report.missing.push(
            "optimization-targets.json lens_dispositions coverage (every accepted family_id must \
             appear once in lens_dispositions[])"
                .to_owned(),
        );
    }
}
