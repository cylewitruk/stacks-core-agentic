//! End-to-end test for `sbagent session validate`: runs the same logic the CLI
//! invokes, against the fixture session.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use stacks_bench_agent::session::bench::BenchClient;
use stacks_bench_agent::session::finalize::{FinalizeInputs, finalize};
use stacks_bench_agent::session::{SessionLayout, validate};
use stacks_bench_agent::types::SessionId;

/// Test fake mirroring `tests/finalize.rs`. Only
/// `validate_fixture_session_passes` uses it (to populate `summary.json` before
/// validating).
struct FakeBench(HashMap<i64, i64>);

impl BenchClient for FakeBench {
    fn total_duration_us(&self, run_id: i64) -> anyhow::Result<Option<i64>> {
        Ok(self.0.get(&run_id).copied())
    }

    fn invoke(
        &self,
        _opts: stacks_bench_agent::session::bench::InvokeOptions<'_>,
    ) -> anyhow::Result<()> {
        unimplemented!("FakeBench in validate tests doesn't drive invoke")
    }
}

fn baseline_canned() -> FakeBench {
    let mut m = HashMap::new();
    m.insert(100i64, 1_010_000i64);
    m.insert(101, 990_000);
    m.insert(500, 940_000);
    m.insert(501, 950_000);
    FakeBench(m)
}

/// Path to the fixture session results dir.
fn fixture_session_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session"))
}

/// Copy `tests/fixtures/session/*` into `<tempdir>/<id>/results/` so the
/// validator finds the canonical layout. The tempdir IS the sessions root.
fn stage_fixture(tmp: &tempfile::TempDir, id: &SessionId) -> SessionLayout {
    let layout = SessionLayout::new(tmp.path(), id.clone());
    layout
        .create_all_phase_dirs()
        .unwrap();
    // Recursive copy via `cp -R` keeps the test concise; the fixture is
    // small + synthesized.
    let status = Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", fixture_session_dir().display()))
        .arg(&layout.results_dir)
        .status()
        .expect("cp -R");
    assert!(status.success(), "cp -R failed");
    layout
}

#[test]
fn validate_fixture_session_passes() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Materialize summary.json by running finalize first; the fixture
    // doesn't ship summary.json (it's a Phase 4 output). validate then
    // runs the full Phase 0–4 artifact check.
    let bench = baseline_canned();
    finalize(&FinalizeInputs { layout: &layout, bench: &bench }).expect("finalize");

    let report = validate::validate(&layout).expect("validate runs");
    assert!(
        report.ok(),
        "validate should pass on finalized fixture; missing: {:#?}",
        report.missing
    );
    assert!(
        report
            .schema_warnings
            .is_empty(),
        "no schema warnings expected; got: {:#?}",
        report.schema_warnings
    );
}

#[test]
fn validate_detects_missing_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Don't add summary.json to the fixture (we never copied it).
    let report = validate::validate(&layout).unwrap();
    assert!(!report.ok());
    assert!(
        report
            .missing
            .iter()
            .any(|m| m == "finalize/summary.json"),
        "expected summary.json missing; got: {:#?}",
        report.missing
    );
}

/// `read_optimization_targets` runs `OptimizationTargets::validate`
/// inline (which fans out to `MergedTarget::validate` and through to
/// `VerificationReplay::validate`). A hand-staged recipe with an
/// out-of-range `repetitions` value must fail at load — before any
/// bench gets invoked downstream.
#[test]
fn loader_rejects_out_of_range_verification_replay_repetitions() {
    use stacks_bench_agent::session::loader;
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Inject a bogus verification_replay onto the first target.
    let path = layout.optimization_targets_json();
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    doc["targets"][0]["verification_replay"] = serde_json::json!({
        "txids": ["0xaa00000000000000000000000000000000000000000000000000000000000000"],
        "repetitions": 99999,
        "warmup": 10,
        "rationale": "bogus repetitions for the loader-rejection test"
    });
    std::fs::write(&path, doc.to_string()).unwrap();

    let err = loader::read_optimization_targets(&layout)
        .expect_err("loader must reject out-of-range repetitions");
    let msg = format!("{err:#}");
    assert!(msg.contains("repetitions"), "{msg}");
    assert!(msg.contains("99999") || msg.contains("[1, 200]"), "{msg}");
}

#[test]
fn validate_detects_missing_analysis() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Remove one analysis to trigger the per-candidate-family check.
    std::fs::remove_file(layout.analysis_json("dlmm-router-contract-family")).unwrap();
    let report = validate::validate(&layout).unwrap();
    assert!(
        report
            .missing
            .iter()
            .any(|m| m == "analysis/dlmm-router-contract-family/analysis.json"),
        "expected per-family analysis missing; got: {:#?}",
        report.missing
    );
}
