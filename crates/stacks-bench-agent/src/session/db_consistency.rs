//! DB ↔ artifact run-id consistency check.
//!
//! Every run-id referenced by a session artifact (baseline run/rerun
//! id, per-target verify baselines, per-target candidate run-ids) must
//! resolve to a row in `<stacks_bench_data_dir>/appdata/stacks-bench.db`.
//! When that invariant breaks — e.g. the operator wiped the DB
//! between sessions, or the data dir is misconfigured — every
//! downstream consumer (finalize's improvement_pct math, archive's
//! ledger emission, Phase 5 PR-writer's audit trail) silently produces
//! either dangling references or wrong-numerator output.
//!
//! This module surfaces such dangling references as warnings before
//! finalize / archive runs, giving the operator a chance to re-bench
//! before the integrity break gets baked into immutable artifacts
//! (the `session/<id>` write-once branch).
//!
//! Discovery is intentionally tolerant: missing files (e.g. a target
//! whose Phase 1.8 was skipped) are NOT errors — only ID references
//! that DO exist but don't resolve in the DB count as dangling. Some
//! dangling references are expected by design (session-level
//! `baseline/run-id` is audit-only when every normal_pr target has
//! per-target ids); this module reports them, callers decide how to
//! react.

use anyhow::{Context as _, Result};

use crate::session::bench::BenchClient;
use crate::session::{SessionLayout, loader};

/// One artifact ↔ DB mismatch. `source` is a human-readable path
/// (relative to the session results dir) so operators can locate
/// the artifact that referenced the dangling id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingRef {
    pub run_id: i64,
    pub source: String,
}

impl std::fmt::Display for DanglingRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "run_id={} referenced by {} is not in the bench DB", self.run_id, self.source)
    }
}

/// Gather every run-id reference under `<session>/results/` and
/// confirm each resolves via `bench.total_duration_us`. Returns the
/// list of dangling references; an empty vec means the session
/// artifacts are DB-consistent.
///
/// Coverage:
/// - `baseline/run-id`, `baseline/rerun-id`
/// - `verify/<target>/baseline-run-ids.json` (txid + block phase arrays)
/// - `optimize/<target>/run-ids` (Phase 3 candidate run-ids)
///
/// Not covered today (no per-target reference today): `summary.json`'s
/// `experiments[].run_ids` / `baseline_run_ids` — those are derived
/// from the above and re-checked by finalize itself.
pub fn collect_dangling_run_ids(
    layout: &SessionLayout,
    targets: &[String],
    bench: &dyn BenchClient,
) -> Result<Vec<DanglingRef>> {
    let mut refs: Vec<(i64, String)> = Vec::new();

    // Baseline run + rerun (Pass 1a contract: aliased, same id).
    if let Ok(id) = loader::read_run_id_file(&layout.baseline_run_id_path()) {
        refs.push((id, "baseline/run-id".into()));
    }
    if let Ok(id) = loader::read_run_id_file(&layout.baseline_rerun_id_path()) {
        refs.push((id, "baseline/rerun-id".into()));
    }

    for target_id in targets {
        // Per-target verify baselines (Pass 1a Phase 1.8 calibration).
        let verify_path = layout.verify_baseline_run_ids_json(target_id);
        if let Ok(raw) = std::fs::read_to_string(&verify_path)
            && let Ok(ids) =
                serde_json::from_str::<crate::session::calibration::BaselineRunIds>(&raw)
        {
            for id in ids.txid_run_ids {
                refs.push((id, format!("verify/{target_id}/baseline-run-ids.json (txid)")));
            }
            for id in ids.block_run_ids {
                refs.push((id, format!("verify/{target_id}/baseline-run-ids.json (block)")));
            }
        }
        // Per-target candidate bench run-ids (Phase 3).
        let cand_path = layout.experiment_run_ids_path(target_id);
        if let Ok(ids) = loader::read_experiment_run_ids(&cand_path) {
            for id in ids {
                refs.push((id, format!("optimize/{target_id}/run-ids")));
            }
        }
    }

    let mut dangling = Vec::new();
    for (run_id, source) in refs {
        let resolved = bench
            .total_duration_us(run_id)
            .with_context(|| format!("querying bench DB for run_id={run_id} ({source})"))?;
        if resolved.is_none() {
            dangling.push(DanglingRef { run_id, source });
        }
    }
    Ok(dangling)
}

/// One-shot helper: read `optimization-targets.json` if present,
/// collect dangling run-ids, emit warnings, return. Used at the
/// natural choke points (pre-finalize, pre-archive) to surface
/// dangling refs before they get baked into immutable artifacts.
///
/// Skipped silently when `optimization-targets.json` doesn't parse
/// or isn't present — finalize / archive themselves will fail with a
/// more specific error in that case. Errors during DB lookup
/// propagate (these are real misconfigurations, not advisory cases).
pub fn warn_dangling_refs(layout: &SessionLayout, bench: &dyn BenchClient) -> anyhow::Result<()> {
    let targets = match loader::read_optimization_targets(layout) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let target_ids: Vec<String> = targets
        .targets
        .iter()
        .map(|t| t.id.clone())
        .collect();
    let dangling = collect_dangling_run_ids(layout, &target_ids, bench)
        .context("DB ↔ artifact run-id consistency check")?;
    warn(&dangling);
    Ok(())
}

/// Emit dangling references to stderr as warnings. Always returns
/// `Ok(())` — this is an advisory surface, not a gate. Callers that
/// want a hard-fail behaviour (e.g. archive in strict mode) inspect
/// the `Vec` directly.
pub fn warn(dangling: &[DanglingRef]) {
    if dangling.is_empty() {
        return;
    }
    eprintln!(
        "db-consistency: {} run-id reference(s) in session artifacts don't resolve in the bench \
         DB. This typically means the DB was wiped between sessions, the data_dir path changed, \
         or a run-id was hand-edited. Downstream finalize / archive / PR-writer paths may emit \
         dangling references; re-bench the affected ids before publishing.",
        dangling.len()
    );
    for d in dangling {
        eprintln!("  - {d}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::bench::InvokeOptions;

    /// Test fake: lookup by run_id with canned answers.
    struct FakeBench(std::collections::HashMap<i64, i64>);

    impl BenchClient for FakeBench {
        fn total_duration_us(&self, run_id: i64) -> Result<Option<i64>> {
            Ok(self.0.get(&run_id).copied())
        }

        fn invoke(&self, _opts: InvokeOptions<'_>) -> Result<()> {
            unimplemented!("FakeBench in db_consistency tests doesn't drive invoke")
        }
    }

    fn stage_session(tmp: &tempfile::TempDir) -> SessionLayout {
        let layout = SessionLayout::new(
            tmp.path(),
            "20260521-180000-test"
                .to_owned()
                .try_into()
                .unwrap(),
        );
        layout
            .create_all_phase_dirs()
            .unwrap();
        layout
    }

    #[test]
    fn empty_session_returns_no_dangling() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = stage_session(&tmp);
        let bench = FakeBench(std::collections::HashMap::new());

        let dangling = collect_dangling_run_ids(&layout, &[], &bench).unwrap();
        assert!(dangling.is_empty(), "no artifacts → no references → no dangling");
    }

    #[test]
    fn baseline_run_id_in_db_passes_check() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = stage_session(&tmp);
        std::fs::write(layout.baseline_run_id_path(), "40\n").unwrap();
        std::fs::write(layout.baseline_rerun_id_path(), "40\n").unwrap();
        let mut canned = std::collections::HashMap::new();
        canned.insert(40i64, 1_000_000i64);
        let bench = FakeBench(canned);

        let dangling = collect_dangling_run_ids(&layout, &[], &bench).unwrap();
        assert!(dangling.is_empty(), "id 40 resolves → no dangling");
    }

    #[test]
    fn baseline_run_id_missing_from_db_is_dangling() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = stage_session(&tmp);
        std::fs::write(layout.baseline_run_id_path(), "40\n").unwrap();
        std::fs::write(layout.baseline_rerun_id_path(), "40\n").unwrap();
        // Empty DB — neither id resolves.
        let bench = FakeBench(std::collections::HashMap::new());

        let dangling = collect_dangling_run_ids(&layout, &[], &bench).unwrap();
        assert_eq!(dangling.len(), 2, "both baseline + rerun dangle");
        let sources: Vec<&str> = dangling
            .iter()
            .map(|d| d.source.as_str())
            .collect();
        assert!(sources.contains(&"baseline/run-id"));
        assert!(sources.contains(&"baseline/rerun-id"));
        for d in &dangling {
            assert_eq!(d.run_id, 40);
        }
    }

    #[test]
    fn per_target_ids_resolve_or_dangle_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = stage_session(&tmp);
        let target = "marf-read-cache-rollback-wrapper";

        // Stage verify + candidate ids.
        let verify_dir = layout
            .results_dir
            .join("verify")
            .join(target);
        std::fs::create_dir_all(&verify_dir).unwrap();
        std::fs::write(
            verify_dir.join("baseline-run-ids.json"),
            r#"{"txid_run_ids":[10,11],"block_run_ids":[]}"#,
        )
        .unwrap();
        let opt_dir = layout
            .results_dir
            .join("optimize")
            .join(target);
        std::fs::create_dir_all(&opt_dir).unwrap();
        std::fs::write(opt_dir.join("run-ids"), "20\n21\n").unwrap();

        // DB has 10, 20 but NOT 11 or 21.
        let mut canned = std::collections::HashMap::new();
        canned.insert(10i64, 1i64);
        canned.insert(20i64, 1);
        let bench = FakeBench(canned);

        let dangling = collect_dangling_run_ids(&layout, &[target.to_owned()], &bench).unwrap();
        let dangling_ids: Vec<i64> = dangling
            .iter()
            .map(|d| d.run_id)
            .collect();
        assert!(dangling_ids.contains(&11), "verify id 11 must be flagged: {dangling:?}");
        assert!(dangling_ids.contains(&21), "candidate id 21 must be flagged: {dangling:?}");
        assert_eq!(dangling.len(), 2, "only 11 + 21 should dangle: {dangling:?}");
    }
}
