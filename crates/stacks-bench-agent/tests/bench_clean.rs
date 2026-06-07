//! Port verification for `cli::session::bench::clean`.
//!
//! Pinned behavior: `sbagent session bench clean` drops BOTH the Phase 3
//! candidate side (`optimize/<target>/{candidate-run-ids.json,
//! <invocation-id>/...}`) AND the Phase 1.8 baseline-calibration side
//! (`verify/<target>/{baseline-run-ids.json, <invocation-id>/...}`) for
//! every target in `optimization-targets.json`, plus a wholesale sweep
//! of `verify/` so the command also works when targets have already
//! been wiped by `analysis clean`. The Phase 0 `baseline/bin/` archive
//! is untouched.

use stacks_bench_agent::cli::session::bench::clean::clean_with_layout;
use stacks_bench_agent::models::ToJson;
use stacks_bench_agent::models::common::{
    BenchInvocation, BenchSamples, Bucket, DeliveryMode, ExpectedSignal, Hotspot,
    ImprovementVector, ProfilerMode, Risk, SchemaVersionV3, SelectionLens, SignalDirection,
    VerificationReplay,
};
use stacks_bench_agent::models::targets::{
    MergeMethod, MergedFrom, MergedTarget, OptimizationTargets,
};
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::types::SessionId;

fn make_layout(tmp: &tempfile::TempDir) -> SessionLayout {
    let id: SessionId = "20260606-104400"
        .to_owned()
        .try_into()
        .unwrap();
    SessionLayout::new(tmp.path(), id)
}

fn hex64(b: u8) -> String {
    format!("0x{}", std::iter::repeat_n(format!("{:02x}", b), 32).collect::<String>())
}

fn invocation(id: &str) -> BenchInvocation {
    BenchInvocation {
        id: id.to_owned(),
        label: format!("label-{id}"),
        purpose: "smoke".to_owned(),
        samples: BenchSamples::Blocks { blocks: vec![hex64(0x11)] },
        warmup: 5,
        repetitions: 10,
        profiler: ProfilerMode::Rich,
        expected_signal: ExpectedSignal {
            axis: SelectionLens::TxLatency,
            direction: SignalDirection::Improves,
            estimate_pct: Some(4.0),
            tolerance_pct: Some(2.0),
        },
    }
}

fn target(id: &str, invocations: Vec<BenchInvocation>) -> MergedTarget {
    MergedTarget {
        id: id.to_owned(),
        merged_from: vec![MergedFrom {
            family_id: "fam".to_owned(),
            target_index: 0,
        }],
        convergence_count: 1,
        rank: None,
        target_span: "x::y".to_owned(),
        bucket: Bucket::BlockProcessing,
        hotspot: Hotspot {
            span: "x::y".to_owned(),
            self_wall_us: 1,
            total_wall_us: 1,
            calls: 1,
            location: "x.rs:1".to_owned(),
        },
        files: vec!["x.rs".to_owned()],
        evidence: "e".to_owned(),
        proposed_change: "p".to_owned(),
        expected_improvement: ImprovementVector {
            tx_latency: 1.0,
            tenure_throughput: 0.0,
            commit_time: 0.0,
        },
        risk: Risk::Low,
        verification_plan: "v".to_owned(),
        verification_replay: Some(VerificationReplay {
            rationale: "test".to_owned(),
            invocations,
            suspected_spans: None,
        }),
        merge_notes: None,
        contributor_differences: None,
        consensus_breaking: false,
        breakage_class: None,
        poc_implementable: None,
        poc_test_scope: None,
        consensus_writeup: None,
        delivery_mode: DeliveryMode::NormalPr,
        bench_eligible: true,
    }
}

fn write_targets(layout: &SessionLayout, targets: Vec<MergedTarget>) {
    let doc = OptimizationTargets {
        schema_version: SchemaVersionV3,
        session_id: "20260606-104400".to_owned(),
        baseline_run_id: 100,
        baseline_rerun_id: 101,
        noise_floor_pct: 0.8,
        merge_method: MergeMethod::Llm,
        merge_model: "gpt-test".to_owned(),
        targets,
        rejected_by_merge: vec![],
        lens_dispositions: vec![],
    };
    std::fs::create_dir_all(layout.merge_dir()).unwrap();
    std::fs::write(layout.optimization_targets_json(), doc.to_json_pretty().unwrap()).unwrap();
}

/// Seed Phase 3 + Phase 1.8 artifacts for `(target, invocation)`. Also
/// seeds the Phase 0 baseline binary archive so the test can confirm it
/// is left intact across cleans.
fn seed_artifacts(layout: &SessionLayout, target_id: &str, invocations: &[&str]) {
    // Phase 0 archive — bench clean must NEVER touch this.
    let bin_path = layout.baseline_bin_path();
    std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
    std::fs::write(&bin_path, b"baseline-binary").unwrap();

    // Phase 3 candidate side.
    let exp = layout.experiment_dir(target_id);
    std::fs::create_dir_all(&exp).unwrap();
    std::fs::write(exp.join("candidate-run-ids.json"), b"{}").unwrap();
    for inv in invocations {
        let inv_dir = layout.experiment_candidate_invocation_dir(target_id, inv);
        std::fs::create_dir_all(&inv_dir).unwrap();
        std::fs::write(inv_dir.join("bench-run.json"), b"{}").unwrap();
        std::fs::write(inv_dir.join("bench-run.stderr.log"), b"").unwrap();
    }

    // Phase 1.8 baseline-calibration side.
    let verify_target = layout.verify_target_dir(target_id);
    std::fs::create_dir_all(&verify_target).unwrap();
    std::fs::write(layout.verify_baseline_run_ids_json(target_id), b"{}").unwrap();
    for inv in invocations {
        let inv_dir = layout.verify_baseline_invocation_dir(target_id, inv);
        std::fs::create_dir_all(&inv_dir).unwrap();
        std::fs::write(layout.verify_baseline_bench_run_json(target_id, inv), b"{}").unwrap();
    }
}

#[test]
fn bench_clean_drops_phase_3_and_phase_1_8_for_every_target() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);

    write_targets(
        &layout,
        vec![
            target("target-a", vec![invocation("warm-steady"), invocation("cold-start")]),
            target("target-b", vec![invocation("solo")]),
        ],
    );
    seed_artifacts(&layout, "target-a", &["warm-steady", "cold-start"]);
    seed_artifacts(&layout, "target-b", &["solo"]);

    let report = clean_with_layout(&layout).expect("clean_with_layout");

    // Phase 3 — every target's candidate side gone.
    for (tgt, invs) in [("target-a", &["warm-steady", "cold-start"][..]), ("target-b", &["solo"])] {
        assert!(
            !layout
                .experiment_dir(tgt)
                .join("candidate-run-ids.json")
                .exists(),
            "{tgt} candidate-run-ids.json should be gone"
        );
        for inv in invs {
            assert!(
                !layout
                    .experiment_candidate_invocation_dir(tgt, inv)
                    .exists(),
                "{tgt}/{inv} candidate dir should be gone"
            );
        }
    }

    // Phase 1.8 — every target's verify side gone, and the wholesale
    // verify_dir was also removed.
    assert!(!layout.verify_dir().exists(), "verify/ root should be removed after wholesale sweep");

    // Phase 0 archive untouched.
    assert!(
        layout
            .baseline_bin_path()
            .exists(),
        "baseline/bin/stacks-bench must survive bench clean"
    );

    assert!(report.total_removed() > 0, "expected some removals: {report:?}");
}

#[test]
fn bench_clean_is_idempotent_on_second_run() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);

    write_targets(&layout, vec![target("target-a", vec![invocation("warm-steady")])]);
    seed_artifacts(&layout, "target-a", &["warm-steady"]);

    let first = clean_with_layout(&layout).expect("first clean");
    assert!(first.total_removed() > 0, "first run removed something");

    let second = clean_with_layout(&layout).expect("second clean");
    assert_eq!(second.total_removed(), 0, "second run must remove nothing: {second:?}");
    // Every per-target path + the wholesale verify_dir → at least four
    // skipped_missing entries on the second run. Don't pin the exact
    // count, just confirm skips dominate.
    assert!(second.skipped_missing >= 1, "second run should record skipped: {second:?}");
}

#[test]
fn bench_clean_wholesale_verify_sweep_runs_without_targets_loaded() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);

    // No optimization-targets.json written. Seed verify/ with a stray
    // per-target tree the way an `analysis clean` followed by an
    // orphaned Phase 1.8 rerun could leave it.
    let verify_target = layout.verify_target_dir("ghost-target");
    std::fs::create_dir_all(verify_target.join("solo-inv")).unwrap();
    std::fs::write(verify_target.join("baseline-run-ids.json"), b"{}").unwrap();
    std::fs::write(
        verify_target
            .join("solo-inv")
            .join("bench-run.json"),
        b"{}",
    )
    .unwrap();

    // Phase 0 archive present — must still survive.
    let bin_path = layout.baseline_bin_path();
    std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
    std::fs::write(&bin_path, b"baseline-binary").unwrap();

    let report = clean_with_layout(&layout).expect("clean without targets");

    assert!(
        !layout.verify_dir().exists(),
        "wholesale verify/ sweep should fire even with no targets loaded"
    );
    assert!(
        layout
            .baseline_bin_path()
            .exists(),
        "Phase 0 archive must survive"
    );
    assert_eq!(report.removed_dirs, 1, "exactly one dir removed (the verify/ root): {report:?}");
}

#[test]
fn bench_clean_leaves_optimizer_owned_artifacts_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);

    write_targets(&layout, vec![target("target-a", vec![invocation("warm-steady")])]);
    seed_artifacts(&layout, "target-a", &["warm-steady"]);

    // optimizer-side files that `optimize clean` owns — bench clean
    // must NOT touch these.
    let exp = layout.experiment_dir("target-a");
    let optimizer_report = exp.join("optimizer-report.json");
    let implementation_md = exp.join("implementation.md");
    std::fs::write(&optimizer_report, b"{}").unwrap();
    std::fs::write(&implementation_md, b"# impl").unwrap();

    clean_with_layout(&layout).expect("clean");

    assert!(optimizer_report.exists(), "optimizer-report.json belongs to optimize clean");
    assert!(implementation_md.exists(), "implementation.md belongs to optimize clean");
}

#[test]
fn bench_clean_propagates_corrupt_targets_instead_of_silently_skipping() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);

    // Seed real Phase 3 artifacts the per-target loop would normally
    // remove. If the corrupt-targets path silently fell through, these
    // would survive and the operator would see "bench clean ✓" while
    // orphans linger.
    seed_artifacts(&layout, "target-a", &["warm-steady"]);

    // Write a structurally invalid optimization-targets.json (not even
    // valid JSON). `loader::read_optimization_targets` should bail; the
    // clean must propagate, not swallow.
    std::fs::create_dir_all(layout.merge_dir()).unwrap();
    std::fs::write(layout.optimization_targets_json(), b"{ this isn't JSON").unwrap();

    let err = clean_with_layout(&layout).expect_err("expected clean to fail on corrupt targets");
    let chain = format!("{err:#}");
    assert!(
        chain.contains("optimization-targets.json") || chain.contains("expected"),
        "error should name the targets file or the parser complaint: {chain}"
    );

    // Phase 3 artifacts must still be there — the failure happened
    // before the per-target loop, so the operator can fix the JSON and
    // rerun cleanly.
    assert!(
        layout
            .experiment_dir("target-a")
            .join("candidate-run-ids.json")
            .exists(),
        "corrupt-targets path must NOT half-clean Phase 3 artifacts"
    );
}
