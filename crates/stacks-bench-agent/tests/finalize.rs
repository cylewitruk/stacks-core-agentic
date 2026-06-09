//! End-to-end test for `sbagent session finalize`: runs the finalize logic
//! against the fixture session and snapshots `summary.json` / `summary.md`.
//!
//! After the Pass 1c results-analyzer cutover, finalize no longer reaches
//! the stacks-bench DB on its own — it sources each `Experiment`'s
//! `improvement_pct` and `status` fields from
//! `analyze/<target>/results-analysis.json`. These tests stage that
//! verdict file (plus the run-id files it cross-checks against) to
//! drive expected outcomes.

use std::path::Path;
use std::process::Command;

use stacks_bench_agent::models::ToJson;
use stacks_bench_agent::session::SessionLayout;
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

/// Overwrite the fixture's `results-analysis.json` with a custom verdict
/// so individual tests can drive Mixed / Rejected / Aborted outcomes.
fn write_results_analysis(layout: &SessionLayout, target_id: &str, body: serde_json::Value) {
    let path = layout.analyze_results_analysis_json(target_id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body.to_string()).unwrap();
}

#[test]
fn finalize_snapshot_against_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    let summary = finalize(&FinalizeInputs { layout: &layout }).expect("finalize");

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

    // The fixture's verdict reports headline_improvement_pct = 4.2.
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert_eq!(normal_pr.improvement_pct, Some(4.2));

    // Snapshot the on-disk artifacts so reviewers can see the full output.
    let summary_json =
        std::fs::read_to_string(layout.summary_json()).expect("summary.json written");
    let summary_md = std::fs::read_to_string(layout.summary_md()).expect("summary.md written");

    insta::with_settings!({ snapshot_path => "snapshots", prepend_module_to_snapshot => false }, {
        insta::assert_snapshot!("finalize__summary_json", summary_json);
        insta::assert_snapshot!("finalize__summary_md", summary_md);
    });
}

/// Pass 1c invariant: when the agent emits `verdict = rejected`, the
/// `Experiment.status` is `Rejected` and the `reason` carries the
/// agent's headline_rationale.
#[test]
fn finalize_sources_rejected_verdict_from_results_analysis() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);
    write_results_analysis(
        &layout,
        "marf-read-cache-rollback-wrapper",
        serde_json::json!({
            "schema_version": 1,
            "session_id": id.as_str(),
            "target_id": "marf-read-cache-rollback-wrapper",
            "axis": "tx_latency",
            "verdict": "rejected",
            "confidence": "high",
            "headline_rationale": "Warm steady-state regressed 3% — cache eviction the analyzer missed.",
            "per_invocation": [
                {
                    "invocation_id": "cold-first-touch",
                    "label": "cold first-touch",
                    "baseline_run_id": 200,
                    "candidate_run_id": 500,
                    "measured_pct": -0.5,
                    "matches_expected_signal": false,
                    "observations": []
                },
                {
                    "invocation_id": "warm-steady",
                    "label": "warm steady-state",
                    "baseline_run_id": 201,
                    "candidate_run_id": 501,
                    "measured_pct": -3.1,
                    "matches_expected_signal": false,
                    "observations": ["warm regressed; cache eviction"]
                }
            ],
            "caveats": [],
            "db_queries": []
        }),
    );

    let summary = finalize(&FinalizeInputs { layout: &layout }).expect("finalize");
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert_eq!(normal_pr.status, stacks_bench_agent::models::summary::ExperimentStatus::Rejected);
    assert!(
        normal_pr
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("cache eviction")),
        "expected agent's rationale in reason; got {:?}",
        normal_pr.reason
    );
    assert_eq!(normal_pr.improvement_pct, None);
}

/// Pass 1c invariant: when `analyze/<target>/results-analysis.json` is
/// missing for a normal_pr target, finalize lands the experiment as
/// `Aborted` (results-analyzer didn't produce a verdict). The rest of
/// the session continues unaffected.
#[test]
fn finalize_aborts_when_results_analysis_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);
    // Remove the fixture's results-analysis.json to simulate Phase 3.5 failure.
    std::fs::remove_file(layout.analyze_results_analysis_json("marf-read-cache-rollback-wrapper"))
        .unwrap();

    let summary = finalize(&FinalizeInputs { layout: &layout }).expect("finalize");
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert_eq!(normal_pr.status, stacks_bench_agent::models::summary::ExperimentStatus::Aborted);
    assert!(
        normal_pr
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("results-analyzer did not produce a verdict")),
        "expected results-analyzer absence reason; got {:?}",
        normal_pr.reason
    );
    assert_eq!(normal_pr.improvement_pct, None);
}

/// Pass 1c invariant: when `verify/<target>/baseline-run-ids.json` is
/// missing, finalize hard-fails. The results-analyzer cannot have
/// produced a sound verdict without a baseline; surfacing the absence
/// at finalize stops a half-judged session from publishing.
#[test]
fn finalize_hard_fails_when_per_target_baseline_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);
    std::fs::remove_file(
        layout
            .results_dir
            .join("verify")
            .join("marf-read-cache-rollback-wrapper")
            .join("baseline-run-ids.json"),
    )
    .unwrap();

    let err = finalize(&FinalizeInputs { layout: &layout }).expect_err(
        "finalize must hard-fail when bench_eligible target lacks baseline-run-ids.json",
    );
    let msg = format!("{err:#}");
    assert!(msg.contains("baseline-run-ids.json"), "{msg}");
    assert!(msg.contains("Pass 1c invariant"), "{msg}");
}

/// Pass 1c invariant: candidate run-ids written in a different order
/// than `verification_replay.invocations[]` are canonicalized to
/// VR order before landing on the Experiment row.
#[test]
fn finalize_canonicalizes_candidate_run_ids_order() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);
    // Fixture VR order: cold-first-touch, warm-steady. Reverse the
    // candidate file's entries on disk.
    std::fs::write(
        layout
            .results_dir
            .join("optimize")
            .join("marf-read-cache-rollback-wrapper")
            .join("candidate-run-ids.json"),
        r#"{"entries":[{"invocation_id":"warm-steady","run_id":501},{"invocation_id":"cold-first-touch","run_id":500}]}"#,
    )
    .unwrap();

    let summary = finalize(&FinalizeInputs { layout: &layout }).expect("finalize");
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert_eq!(
        normal_pr.run_ids,
        Some(vec![500, 501]),
        "run_ids must be canonicalized to VR.invocations[] order",
    );
}

/// Pass 1c invariant: a candidate-run-ids set that doesn't match the
/// target's VR (extra/missing invocations) surfaces as Aborted with
/// the mismatch detail in the reason.
#[test]
fn finalize_aborts_on_candidate_invocation_id_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);
    std::fs::write(
        layout
            .results_dir
            .join("optimize")
            .join("marf-read-cache-rollback-wrapper")
            .join("candidate-run-ids.json"),
        r#"{"entries":[{"invocation_id":"warm-steady","run_id":501}]}"#,
    )
    .unwrap();

    let summary = finalize(&FinalizeInputs { layout: &layout }).expect("finalize");
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert!(
        normal_pr
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("invocation set mismatch")),
        "expected invocation-set-mismatch reason; got {:?}",
        normal_pr.reason
    );
    assert_eq!(normal_pr.improvement_pct, None);
}

/// Pass 1c invariant: when `optimize/<target>/coordinator-provenance.json`
/// exists, finalize propagates `base_sha` / `head_sha` into the
/// corresponding [`Experiment`] row.
#[test]
fn finalize_propagates_coordinator_provenance_into_experiment() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);
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

    let summary = finalize(&FinalizeInputs { layout: &layout }).unwrap();
    let normal_pr = summary
        .experiments
        .iter()
        .find(|e| e.target_id == "marf-read-cache-rollback-wrapper")
        .unwrap();
    assert_eq!(normal_pr.base_sha.as_deref(), Some("0ad33704c259da4102b5f195617760003ac89c18"),);
    assert_eq!(normal_pr.head_sha.as_deref(), Some("f994e6ef03002fb7b1acdc1b5018da40e73b105b"),);
}

/// Pass 1c invariant: when no provenance sidecar is present, both SHA
/// fields must be absent (finalize must not invent SHAs).
#[test]
fn finalize_leaves_sha_fields_none_when_provenance_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    let summary = finalize(&FinalizeInputs { layout: &layout }).unwrap();
    for exp in &summary.experiments {
        assert_eq!(exp.base_sha, None, "{}", exp.target_id);
        assert_eq!(exp.head_sha, None, "{}", exp.target_id);
    }
}

/// v3 Phase 3 cutover: when `<session>/results/source.json` exists,
/// finalize copies its four fields into `Summary.source_*`. When it
/// doesn't exist (pre-cutover sessions), the fields stay `None` so
/// the legacy path still flows through cleanly.
#[test]
fn finalize_populates_summary_source_fields_from_source_json() {
    use stacks_bench_agent::models::common::SchemaVersionV1;
    use stacks_bench_agent::models::source::SourceJson;

    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    let source = SourceJson {
        schema_version: SchemaVersionV1,
        url: "https://github.com/stacks-network/stacks-core.git".to_owned(),
        branch: "feat/stacks-bench".to_owned(),
        sha: "0ad33704c259da4102b5f195617760003ac89c18".to_owned(),
        fetched_at: "2026-06-07T12:00:00Z".to_owned(),
        cache_id: "stacks-core-feat-stacks-bench".to_owned(),
    };
    source
        .write(&layout.source_json())
        .expect("write source.json");

    let summary = finalize(&FinalizeInputs { layout: &layout }).expect("finalize with source");
    assert_eq!(
        summary.source_url.as_deref(),
        Some("https://github.com/stacks-network/stacks-core.git"),
    );
    assert_eq!(
        summary
            .source_branch
            .as_deref(),
        Some("feat/stacks-bench")
    );
    assert_eq!(summary.source_sha.as_deref(), Some("0ad33704c259da4102b5f195617760003ac89c18"),);
    assert_eq!(
        summary
            .source_fetched_at
            .as_deref(),
        Some("2026-06-07T12:00:00Z")
    );
}

#[test]
fn finalize_leaves_source_fields_none_when_source_json_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let layout = stage_fixture(&tmp, &id);

    // Confirm the fixture has no source.json (pre-cutover shape).
    assert!(!layout.source_json().exists());

    let summary = finalize(&FinalizeInputs { layout: &layout }).expect("finalize");
    assert_eq!(summary.source_url, None);
    assert_eq!(summary.source_branch, None);
    assert_eq!(summary.source_sha, None);
    assert_eq!(summary.source_fetched_at, None);
}
