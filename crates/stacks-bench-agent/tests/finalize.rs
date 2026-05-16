//! End-to-end test for `sbagent session finalize`: runs the finalize logic
//! against the fixture session with a canned `BenchClient`, then snapshots
//! `summary.json` and `summary.md` via insta.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::bench::BenchClient;
use stacks_bench_agent::session::finalize::{FinalizeInputs, finalize};
use stacks_bench_agent::types::SessionId;

/// Path to the fixture session results dir.
fn fixture_session_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session"))
}

/// Stage a copy of the fixture session under
/// `<tempdir>/sessions/<id>/results/`.
fn stage_fixture(tmp: &tempfile::TempDir, id: &SessionId) -> SessionLayout {
    let layout = SessionLayout::new(tmp.path(), id.clone());
    layout
        .create_all_phase_dirs()
        .unwrap();
    let status = Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", fixture_session_dir().display()))
        .arg(&layout.results_dir)
        .status()
        .unwrap();
    assert!(status.success());
    layout
}

/// Test fake: lookup table from run id to canned `total_duration_us`.
struct FakeBench(HashMap<i64, i64>);

impl BenchClient for FakeBench {
    fn total_duration_us(&self, run_id: i64) -> anyhow::Result<Option<i64>> {
        Ok(self.0.get(&run_id).copied())
    }

    fn invoke(
        &self,
        _opts: stacks_bench_agent::session::bench::InvokeOptions<'_>,
    ) -> anyhow::Result<()> {
        unimplemented!("FakeBench in finalize tests doesn't drive invoke")
    }
}

#[test]
fn finalize_snapshot_against_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Canned durations:
    // - baseline: 1_000_000us mean (run 100=1_010_000, run 101=990_000)
    // - normal_pr accepted experiment (target marf-read-cache-rollback-wrapper):
    //   run 500=940_000, run 501=950_000 → mean 945_000 → ~5.5% improvement, well
    //   above the 0.8% noise floor → Accepted.
    let mut canned = HashMap::new();
    canned.insert(100i64, 1_010_000i64);
    canned.insert(101, 990_000);
    canned.insert(500, 940_000);
    canned.insert(501, 950_000);
    let bench = FakeBench(canned);

    let summary = finalize(&FinalizeInputs { layout: &layout, bench: &bench }).expect("finalize");

    // Sanity: outcome counts match the fixture's three targets.
    assert_eq!(
        summary
            .outcome_counts
            .normal_pr
            .accepted,
        1
    );
    assert_eq!(
        summary
            .outcome_counts
            .consensus_poc_pr
            .poc_landed,
        1
    );
    assert_eq!(
        summary
            .outcome_counts
            .consensus_issue
            .routed_to_issue,
        1
    );
    assert_eq!(summary.experiments.len(), 3);

    // Snapshot the on-disk artifacts so reviewers can see the full output.
    let summary_json =
        std::fs::read_to_string(layout.summary_json()).expect("summary.json written");
    let summary_md = std::fs::read_to_string(layout.summary_md()).expect("summary.md written");

    insta::with_settings!({ snapshot_path => "snapshots", prepend_module_to_snapshot => false }, {
        insta::assert_snapshot!("finalize__summary_json", summary_json);
        insta::assert_snapshot!("finalize__summary_md", summary_md);
    });
}

#[test]
fn finalize_normal_pr_within_noise_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Tiny improvement well below noise → should reject "within noise floor".
    // baseline mean = 1_000_000; experiment mean = 999_500 → ~0.05%.
    let mut canned = HashMap::new();
    canned.insert(100i64, 1_000_000i64);
    canned.insert(101, 1_000_000);
    canned.insert(500, 999_500);
    canned.insert(501, 999_500);
    let bench = FakeBench(canned);

    let summary = finalize(&FinalizeInputs { layout: &layout, bench: &bench }).unwrap();
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert!(
        normal_pr
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("within noise floor")),
        "expected within-noise rejection; got reason={:?}",
        normal_pr.reason
    );
}

#[test]
fn finalize_normal_pr_regression_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Worse: experiment 5% slower than baseline → "regression".
    let mut canned = HashMap::new();
    canned.insert(100i64, 1_000_000i64);
    canned.insert(101, 1_000_000);
    canned.insert(500, 1_050_000);
    canned.insert(501, 1_050_000);
    let bench = FakeBench(canned);

    let summary = finalize(&FinalizeInputs { layout: &layout, bench: &bench }).unwrap();
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert!(
        normal_pr
            .reason
            .as_deref()
            .is_some_and(|r| r.starts_with("regression")),
        "expected regression rejection; got reason={:?}",
        normal_pr.reason
    );
}
