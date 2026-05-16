//! Read v2 artifacts off disk into typed models.
//!
//! Each loader function reads the file via `serde_json::from_reader` (so parse
//! errors carry line/column info) and returns a typed model. Loaders for
//! artifacts whose invariants gate downstream side effects —
//! `optimization-targets.json` (drives bench invocations) and
//! `analysis/*/analysis.json` (drives optimizer/merge) — run the model's
//! `validate()` inline so a hand-staged or corrupt file fails at load before
//! any bench gets invoked. Loaders for read-only/reporting artifacts
//! (`candidates.json`, `summary.json`) leave validation to the caller.

use std::collections::BTreeMap;
use std::path::Path;
use std::{fs, io};

use anyhow::{Context as _, Result};

use crate::models::analyze::Analysis;
use crate::models::candidates::Candidates;
use crate::models::summary::Summary;
use crate::models::targets::OptimizationTargets;
use crate::session::SessionLayout;

/// Read and parse `candidates.json`.
pub fn read_candidates(layout: &SessionLayout) -> Result<Candidates> {
    parse_json(&layout.candidates_json())
}

/// Read, parse, and **validate** `optimization-targets.json`. Validation
/// covers the per-target cross-field rules in [`MergedTarget::validate`]
/// (consensus routing, convergence count, hash forms, `verification_replay`
/// bounds). A hand-staged recipe with an out-of-range `repetitions` or
/// `warmup` fails here, before any bench is invoked.
pub fn read_optimization_targets(layout: &SessionLayout) -> Result<OptimizationTargets> {
    let doc: OptimizationTargets = parse_json(&layout.optimization_targets_json())?;
    doc.validate().map_err(|e| {
        anyhow::anyhow!(
            "validating {}: {e}",
            layout
                .optimization_targets_json()
                .display(),
        )
    })?;
    Ok(doc)
}

/// Read and parse `summary.json` (only present after `finalize`).
pub fn read_summary(layout: &SessionLayout) -> Result<Summary> {
    parse_json(&layout.summary_json())
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
            .validate()
            .map_err(|e| anyhow::anyhow!("validating {}: {e}", path.display()))?;
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
