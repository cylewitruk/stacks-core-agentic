//! End-to-end test for `sbagent session finalize`: runs the finalize logic
//! against the fixture session with a canned `BenchClient`, then snapshots
//! `summary.json` and `summary.md` via insta.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use stacks_bench_agent::models::ToJson;
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

/// Pass 1c invariant: when `optimize/<target>/coordinator-provenance.json`
/// exists, finalize must propagate its `base_sha` / `head_sha` into the
/// corresponding [`Experiment`] row, populating the audit-trail surface
/// Phase 5 PR-writer + downstream review depend on.
#[test]
fn finalize_propagates_coordinator_provenance_into_experiment() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Drop a provenance sidecar for the lone normal_pr target.
    let provenance = serde_json::json!({
        "schema_version": 1,
        "session_id": id.as_str(),
        "target_id": "marf-read-cache-rollback-wrapper",
        "delivery_mode": "normal_pr",
        "base_sha": "0ad33704c259da4102b5f195617760003ac89c18",
        "head_sha": "f994e6ef03002fb7b1acdc1b5018da40e73b105b",
        "commit_message": "perf: optimize marf-read-cache-rollback-wrapper",
    });
    std::fs::write(
        layout
            .results_dir
            .join("optimize")
            .join("marf-read-cache-rollback-wrapper")
            .join("coordinator-provenance.json"),
        provenance
            .to_json_pretty()
            .unwrap(),
    )
    .unwrap();

    let mut canned = HashMap::new();
    canned.insert(100i64, 1_000_000i64);
    canned.insert(101, 1_000_000);
    canned.insert(500, 940_000);
    canned.insert(501, 950_000);
    let bench = FakeBench(canned);

    let summary = finalize(&FinalizeInputs { layout: &layout, bench: &bench }).unwrap();
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert_eq!(
        normal_pr.base_sha.as_deref(),
        Some("0ad33704c259da4102b5f195617760003ac89c18"),
        "base_sha must flow from sidecar into Experiment",
    );
    assert_eq!(
        normal_pr.head_sha.as_deref(),
        Some("f994e6ef03002fb7b1acdc1b5018da40e73b105b"),
        "head_sha must flow from sidecar into Experiment",
    );
}

/// Targets without a `coordinator-provenance.json` (aborted experiments,
/// consensus_issue rows, sessions that predate the sidecar contract) must
/// land in finalize with both SHA fields absent — finalize must not
/// invent SHAs and must not error.
#[test]
fn finalize_leaves_sha_fields_none_when_provenance_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // No provenance sidecar staged.
    let mut canned = HashMap::new();
    canned.insert(100i64, 1_000_000i64);
    canned.insert(101, 1_000_000);
    canned.insert(500, 940_000);
    canned.insert(501, 950_000);
    let bench = FakeBench(canned);

    let summary = finalize(&FinalizeInputs { layout: &layout, bench: &bench }).unwrap();
    for exp in &summary.experiments {
        assert_eq!(exp.base_sha, None, "{}", exp.target_id);
        assert_eq!(exp.head_sha, None, "{}", exp.target_id);
    }
}

/// Pass 1a invariant: when every normal_pr target has a per-target
/// `verify/<target>/baseline-run-ids.json`, finalize must NOT consult the
/// session-level `baseline_run_id` / `baseline_rerun_id` against the DB.
/// Operators can wipe the session-level baseline run row without breaking
/// finalize, as long as per-target denominators cover the whole target set.
/// Regression test for the failure surfaced 2026-05-21 where the eager
/// session-level lookup blocked finalize even though no target consumed it.
#[test]
fn finalize_skips_session_baseline_when_all_targets_have_per_target_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Stage a per-target baseline file for the lone normal_pr target.
    // The IDs (200, 201) will be looked up against the FakeBench; the
    // session-level baseline run_ids (100, 101 from the fixture) are
    // INTENTIONALLY absent from the canned table to prove the eager
    // session-level lookup isn't happening.
    let verify_dir = layout
        .results_dir
        .join("verify")
        .join("marf-read-cache-rollback-wrapper");
    std::fs::create_dir_all(&verify_dir).unwrap();
    std::fs::write(
        verify_dir.join("baseline-run-ids.json"),
        r#"{"txid_run_ids":[200,201],"block_run_ids":[]}"#,
    )
    .unwrap();

    let mut canned = HashMap::new();
    // No 100 / 101 — would have errored under the eager-lookup behavior.
    canned.insert(200i64, 1_000_000i64);
    canned.insert(201, 1_000_000);
    canned.insert(500, 940_000);
    canned.insert(501, 950_000);
    let bench = FakeBench(canned);

    let summary = finalize(&FinalizeInputs { layout: &layout, bench: &bench })
        .expect("finalize must succeed when per-target ids cover all normal_pr targets");

    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert_eq!(
        normal_pr.baseline_run_ids,
        Some(vec![200, 201]),
        "per-target ids must flow into the experiment record",
    );
    // ~5.5% improvement against the 1_000_000 per-target denominator.
    assert!(
        normal_pr
            .improvement_pct
            .is_some_and(|p| p > 5.0 && p < 6.0),
        "improvement_pct must derive from the per-target denominator (200/201, not 100/101); got \
         {:?}",
        normal_pr.improvement_pct,
    );
}
