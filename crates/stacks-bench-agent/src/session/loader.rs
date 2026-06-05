//! Read v2 artifacts off disk into typed models.
//!
//! Each loader function reads the file via `serde_json::from_reader` (so parse
//! errors carry line/column info) and returns a typed model. Loaders for
//! artifacts whose invariants gate downstream side effects —
//! `optimization-targets.json` (drives bench invocations) and
//! `analysis/*/analysis.json` (drives optimizer/merge) — run the model's
//! `validate_model()` inline so a hand-staged or corrupt file fails at load
//! before any bench gets invoked. Loaders for read-only/reporting artifacts
//! (`candidates.json`, `summary.json`) leave validation to the caller.

use std::collections::BTreeMap;
use std::path::Path;
use std::{fs, io};

use anyhow::{Context as _, Result};

use crate::models::ValidateModel;
use crate::models::analyze::Analysis;
use crate::models::candidates::Candidates;
use crate::models::common::DeliveryMode;
use crate::models::optimizer_report::OptimizerReport;
use crate::models::summary::Summary;
use crate::models::targets::OptimizationTargets;
use crate::session::SessionLayout;

/// Read and parse `candidates.json`.
pub fn read_candidates(layout: &SessionLayout) -> Result<Candidates> {
    parse_json(&layout.candidates_json())
}

/// Read, parse, and **validate** `optimization-targets.json`. Validation
/// covers the per-target cross-field rules in
/// [`MergedTarget::validate_model`] (consensus routing, convergence
/// count, hash forms, `verification_replay` bounds). A hand-staged
/// recipe with an out-of-range `repetitions` or `warmup` fails here,
/// before any bench is invoked.
pub fn read_optimization_targets(layout: &SessionLayout) -> Result<OptimizationTargets> {
    let doc: OptimizationTargets = parse_json(&layout.optimization_targets_json())?;
    doc.validate_model()
        .with_context(|| {
            format!(
                "validating {}",
                layout
                    .optimization_targets_json()
                    .display()
            )
        })?;
    Ok(doc)
}

/// Read and parse `summary.json` (only present after `finalize`).
pub fn read_summary(layout: &SessionLayout) -> Result<Summary> {
    parse_json(&layout.summary_json())
}

/// Read, parse, and **structurally validate** the per-target
/// `optimize/<target-id>/optimizer-report.json`. Validation covers the
/// six [`crate::models::optimizer_report::ImplementedReport::validate_model`]
/// invariants (consensus-sensitive parity proofs, unproven_risk null on
/// implemented, test failed=0, non-blank free-text, finite/non-negative
/// duration, clippy_clean by delivery_mode) and the two
/// [`crate::models::optimizer_report::AbortedReport::validate_model`]
/// invariants (non-blank reason, nextest-implies-failing_tests).
///
/// Returns `Ok(None)` when the file is missing — the agent never wrote
/// it (crashed mid-loop, sandbox killed, etc.). Callers treat that as
/// "no commit, no abort marker" the same way the prior marker contract
/// treated absent-`implementation.md`-and-absent-`abort.md`.
///
/// **Use [`read_optimizer_report_for_target`] instead** at any call site
/// that already knows the target's `delivery_mode` and the session id.
/// The bare loader here only does self-validation; it does not assert
/// that the report's `target_id`/`session_id`/`delivery_mode` match
/// the loading context, so a misbehaving agent could emit
/// `delivery_mode: consensus_poc_pr` for a `normal_pr` target and
/// bypass the `clippy_clean: true` invariant. The context-checking
/// variant closes that loophole.
pub fn read_optimizer_report(
    layout: &SessionLayout,
    target_id: &str,
) -> Result<Option<OptimizerReport>> {
    let path = layout.experiment_optimizer_report(target_id);
    match fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
        }
    }
    let report: OptimizerReport = parse_json(&path)?;
    report
        .validate_model()
        .with_context(|| format!("validating {}", path.display()))?;
    Ok(Some(report))
}

/// Read + structurally validate + **context-validate** an optimizer
/// report. In addition to the schema invariants enforced by
/// [`read_optimizer_report`], this variant verifies that the report's
/// `target_id`, `session_id`, and `delivery_mode` match the caller's
/// expected context. Without this check, an agent that emits a report
/// claiming a different `delivery_mode` than the merged target's would
/// bypass mode-specific invariants (e.g. `normal_pr` requires
/// `clippy_clean: true`; `consensus_poc_pr` doesn't).
///
/// Every load site that knows the target context (coordinator commit,
/// verify_kept_or_demote, finalize, publish) should prefer this over
/// the bare [`read_optimizer_report`].
pub fn read_optimizer_report_for_target(
    layout: &SessionLayout,
    target_id: &str,
    expected_delivery_mode: DeliveryMode,
) -> Result<Option<OptimizerReport>> {
    let report = match read_optimizer_report(layout, target_id)? {
        Some(r) => r,
        None => return Ok(None),
    };
    let expected_session_id = layout.id.as_str();
    let report_session_id = match &report {
        OptimizerReport::Implemented(r) => r.session_id.as_str(),
        OptimizerReport::Aborted(r) => r.session_id.as_str(),
    };
    let report_target_id = report.target_id();
    let report_delivery_mode = report.delivery_mode();
    if report_target_id != target_id {
        return Err(anyhow::anyhow!(
            "optimizer-report.json for {target_id}: report.target_id={report_target_id:?} does \
             not match expected target_id={target_id:?}"
        ));
    }
    if report_session_id != expected_session_id {
        return Err(anyhow::anyhow!(
            "optimizer-report.json for {target_id}: report.session_id={report_session_id:?} does \
             not match expected session_id={expected_session_id:?}"
        ));
    }
    if report_delivery_mode != expected_delivery_mode {
        return Err(anyhow::anyhow!(
            "optimizer-report.json for {target_id}: report.delivery_mode={report_delivery_mode:?} \
             does not match expected delivery_mode={expected_delivery_mode:?} (an agent that \
             claims a different mode could bypass mode-specific invariants like clippy_clean)"
        ));
    }
    Ok(Some(report))
}

/// Read + validate `analyze/<target-id>/results-analysis.json` for one
/// target. Returns `Ok(None)` when the file is absent OR when its
/// `session_id` / `target_id` don't match the caller's context (treated
/// as "no verdict for this target" rather than a hard error — a stale
/// verdict from a different session should not feed into publish copy
/// or render verdict blocks). Any other failure (IO, parse, schema,
/// cross-field validation) is logged at WARN and returns `Ok(None)`
/// too, so a single bad file can't take out the whole session.
///
/// This is the canonical loader: finalize, render, and publish all
/// go through it so they agree on which verdicts are present.
pub fn read_results_analysis_for_target(
    layout: &SessionLayout,
    target_id: &str,
) -> Result<Option<crate::models::results_analysis::ResultsAnalysis>> {
    use crate::models::FromJsonValidated;
    use crate::models::results_analysis::ResultsAnalysis;

    let path = layout.analyze_results_analysis_json(target_id);
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
        }
    };
    let ra = match ResultsAnalysis::from_json_validated(&raw) {
        Ok(ra) => ra,
        Err(e) => {
            tracing::warn!(
                target_id,
                error = %e,
                "results-analysis.json failed parse/validate; treating as absent",
            );
            return Ok(None);
        }
    };
    if let Err(e) = ra.validate_model() {
        tracing::warn!(
            target_id,
            error = %e,
            "results-analysis.json failed cross-field validation; treating as absent",
        );
        return Ok(None);
    }
    let expected_session = layout.id.as_str();
    if ra.session_id != expected_session || ra.target_id != target_id {
        tracing::warn!(
            target_id,
            actual_session = %ra.session_id,
            actual_target = %ra.target_id,
            expected_session,
            "results-analysis.json carries the wrong session/target context; treating as \
             missing-verdict",
        );
        return Ok(None);
    }
    Ok(Some(ra))
}

/// Read and parse all `analysis/<family-id>/analysis.json` files. Keyed by
/// family_id; sort order is BTreeMap-deterministic for snapshot stability.
pub fn read_all_analyses(layout: &SessionLayout) -> Result<BTreeMap<String, Analysis>> {
    let dir = layout.analysis_dir();
    let mut out = BTreeMap::new();
    let entries = match fs::read_dir(&dir) {
        Ok(it) => it,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", dir.display())));
        }
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let family_id = entry
            .file_name()
            .to_string_lossy()
            .into_owned();
        let path = entry
            .path()
            .join("analysis.json");
        if !path.is_file() {
            continue;
        }
        let analysis: Analysis = parse_json(&path)?;
        if analysis.family_id() != family_id {
            return Err(anyhow::anyhow!(
                "{}: analysis.family_id={:?} but parent dir is {:?}",
                path.display(),
                analysis.family_id(),
                family_id
            ));
        }
        analysis
            .validate_model()
            .with_context(|| format!("validating {}", path.display()))?;
        out.insert(family_id, analysis);
    }
    Ok(out)
}

/// Read a `baseline-*-id` plain-text file (numeric run id, one line).
pub fn read_run_id_file(path: &Path) -> Result<i64> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let trimmed = raw.trim();
    trimmed
        .parse::<i64>()
        .with_context(|| format!("parsing run id from {}: {:?}", path.display(), trimmed))
}

/// Read an experiment's `run-ids` file: one i64 per line. Empty lines and
/// trailing whitespace are tolerated.
pub fn read_experiment_run_ids(path: &Path) -> Result<Vec<i64>> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
        }
    };
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let id = trimmed
            .parse::<i64>()
            .with_context(|| {
                format!("parsing run id at {}:{}: {:?}", path.display(), i + 1, trimmed)
            })?;
        out.push(id);
    }
    Ok(out)
}

/// Parse a JSON file into the requested type. Wraps the
/// `serde_json::from_reader`
/// + `BufReader` boilerplate plus an `anyhow` context with the path.
fn parse_json<T: for<'de> serde::Deserialize<'de>>(path: &Path) -> Result<T> {
    let f = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let r = io::BufReader::new(f);
    let v: T = serde_json::from_reader(r).with_context(|| format!("parsing {}", path.display()))?;
    Ok(v)
}
