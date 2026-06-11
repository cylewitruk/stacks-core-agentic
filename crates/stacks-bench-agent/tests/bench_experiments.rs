//! Port verification for `session::bench_experiments`.
//!
//! Tests use:
//! - a fake `CargoRunner` that creates a stub binary file at the worktree's
//!   `target/release/stacks-bench` path (mirroring what cargo build would do,
//!   without invoking cargo);
//! - the same recording `BenchClient` shape used by the baseline tests, so we
//!   can assert the exact `bench run` invocations land for each target.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use stacks_bench_agent::models::ToJson;
use stacks_bench_agent::models::common::{
    BenchInvocation, BenchSamples, Bucket, DeliveryMode, EvidenceQuery, ExpectedSignal, Hotspot,
    ImprovementVector, ProfilerMode, Risk, SchemaVersionV4, SelectionLens, SignalDirection,
    VerificationReplay,
};
use stacks_bench_agent::models::targets::{
    MergeMethod, MergedFrom, MergedTarget, OptimizationTargets,
};
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::bench::{BenchClient, InvokeOptions};
use stacks_bench_agent::session::bench_experiments::{self, BenchEnv, Inputs, TargetOutcome};
use stacks_bench_agent::session::cargo::CargoRunner;
use stacks_bench_agent::types::SessionId;

/// Fake cargo: writes empty stdout/stderr logs and creates a stub binary.
/// `clean()` recursively removes `<worktree>/target/` so tests that exercise
/// the default cargo-clean path can assert real disk reclamation, not just
/// log-file presence.
struct StubCargo;

impl CargoRunner for StubCargo {
    fn build_release(&self, worktree: &Path, stdout: &Path, stderr: &Path) -> anyhow::Result<()> {
        std::fs::write(stdout, b"")?;
        std::fs::write(stderr, b"")?;
        let bin = worktree
            .join("target")
            .join("release")
            .join("stacks-bench");
        std::fs::create_dir_all(bin.parent().unwrap())?;
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = std::fs::metadata(&bin)?.permissions();
            perms.set_mode(perms.mode() | 0o111);
            std::fs::set_permissions(&bin, perms)?;
        }
        Ok(())
    }
    fn clean(&self, worktree: &Path, stdout: &Path, stderr: &Path) -> anyhow::Result<()> {
        std::fs::write(stdout, b"")?;
        std::fs::write(stderr, b"")?;
        let target_dir = worktree.join("target");
        if target_dir.exists() {
            std::fs::remove_dir_all(&target_dir)?;
        }
        Ok(())
    }
}

/// Recording fake bench: writes a canned envelope per `bench run` call so
/// `extract_run_id` can succeed.
struct RecordingBench {
    next: Mutex<i64>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl RecordingBench {
    fn new(start: i64) -> Self {
        Self {
            next: Mutex::new(start),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().clone()
    }
}

impl BenchClient for RecordingBench {
    fn total_duration_us(&self, _: i64) -> anyhow::Result<Option<i64>> {
        Ok(None)
    }
    fn invoke(&self, opts: InvokeOptions<'_>) -> anyhow::Result<()> {
        let argv: Vec<String> = opts
            .args
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        self.calls.lock().push(argv);
        if let Some(stdout_path) = opts.stdout {
            let mut id = self.next.lock();
            *id += 1;
            let value = *id;
            if let Some(parent) = stdout_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(
                stdout_path,
                serde_json::json!({ "data": { "run_id": value } }).to_string(),
            )?;
        }
        if let Some(stderr_path) = opts.stderr
            && let Some(parent) = stderr_path.parent()
        {
            std::fs::create_dir_all(parent)?;
            std::fs::write(stderr_path, b"")?;
        }
        Ok(())
    }
}

/// Seed a typed `optimizer-report.json` with `outcome=implemented` for
/// `target_id` in the experiment dir.
fn write_implemented_report(layout: &SessionLayout, target_id: &str) {
    use stacks_bench_agent::models::common::SchemaVersionV2;
    use stacks_bench_agent::models::optimizer_report::{
        ImplementedOutcomeTag, ImplementedReport, OptimizerReport, ParityReport, TestFramework,
        TestSummary,
    };
    let exp = layout.experiment_dir(target_id);
    std::fs::create_dir_all(&exp).unwrap();
    let report = OptimizerReport::Implemented(ImplementedReport {
        schema_version: SchemaVersionV2,
        session_id: "20260507-104400".to_owned(),
        target_id: target_id.to_owned(),
        outcome: ImplementedOutcomeTag::Implemented,
        delivery_mode: DeliveryMode::NormalPr,
        implementation_summary: "test impl".to_owned(),
        deviation_from_proposed_change: None,
        dependency_changes: None,
        test_summary: TestSummary {
            framework: TestFramework::Nextest,
            passed: 1,
            failed: 0,
            duration_secs: 1.0,
            log_path: "nextest.log".to_owned(),
        },
        clippy_clean: Some(true),
        pr_title: "perf: test".to_owned(),
        parity: ParityReport {
            consensus_sensitive: false,
            evidence: vec![],
            tests: vec![],
            unproven_risk: None,
        },
        hard_fork_followup: None,
    });
    std::fs::write(
        exp.join("optimizer-report.json"),
        report
            .to_json_pretty()
            .unwrap(),
    )
    .unwrap();
}

fn make_layout(tmp: &tempfile::TempDir) -> SessionLayout {
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    SessionLayout::new(tmp.path(), id)
}

fn hex64(b: u8) -> String {
    format!("0x{}", std::iter::repeat_n(format!("{:02x}", b), 32).collect::<String>())
}

fn default_invocation(id: &str) -> BenchInvocation {
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

fn default_vr() -> VerificationReplay {
    VerificationReplay {
        rationale: "test".to_owned(),
        invocations: vec![default_invocation("warm-steady")],
        suspected_spans: None,
    }
}

fn target(id: &str, mode: DeliveryMode, vr: Option<VerificationReplay>) -> MergedTarget {
    let bench_eligible = matches!(mode, DeliveryMode::NormalPr);
    let evidence_queries = if let Some(vr) = &vr {
        vec![EvidenceQuery {
            purpose: "prove span movement".to_owned(),
            sql_path: "queries/span_run_drift.sql".into(),
            params: Default::default(),
            output_path: "queries/span-run-drift.csv".to_owned(),
            key_observation: "baseline p95 self_wall_us = 1000".to_owned(),
            supports_invocations: vec![vr.invocations[0].id.clone()],
        }]
    } else {
        vec![]
    };
    MergedTarget {
        id: id.to_owned(),
        merged_from: vec![MergedFrom {
            family_id: "x-fam".to_owned(),
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
        evidence_queries,
        proposed_change: "p".to_owned(),
        expected_improvement: ImprovementVector {
            tx_latency: 1.0,
            tenure_throughput: 0.0,
            commit_time: 0.0,
        },
        risk: Risk::Low,
        verification_plan: "v".to_owned(),
        verification_replay: vr,
        merge_notes: None,
        contributor_differences: None,
        consensus_breaking: !bench_eligible,
        breakage_class: None,
        poc_implementable: None,
        poc_test_scope: None,
        consensus_writeup: None,
        delivery_mode: mode,
        bench_eligible,
    }
}

fn make_targets(targets: Vec<MergedTarget>) -> OptimizationTargets {
    OptimizationTargets {
        schema_version: SchemaVersionV4,
        session_id: "20260507-104400".to_owned(),
        baseline_run_id: 100,
        baseline_rerun_id: 101,
        noise_floor_pct: 0.8,
        merge_method: MergeMethod::Llm,
        merge_model: "gpt-test".to_owned(),
        targets,
        rejected_by_merge: vec![],
        lens_dispositions: vec![],
    }
}

/// Helper: forward `BenchClient` calls to a borrowed reference. Used only
/// to thread the recorder into the factory closure.
struct SharedBenchHandle<'a>(&'a RecordingBench);
impl BenchClient for SharedBenchHandle<'_> {
    fn total_duration_us(&self, run_id: i64) -> anyhow::Result<Option<i64>> {
        self.0
            .total_duration_us(run_id)
    }
    fn invoke(&self, opts: InvokeOptions<'_>) -> anyhow::Result<()> {
        self.0.invoke(opts)
    }
}

#[test]
fn bench_experiments_normal_pr_target_runs_one_invocation() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");
    std::fs::create_dir_all(worktrees_root.join("target-a")).unwrap();
    write_implemented_report(&layout, "target-a");

    let targets_doc =
        make_targets(vec![target("target-a", DeliveryMode::NormalPr, Some(default_vr()))]);
    let bench = RecordingBench::new(1000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        env: BenchEnv {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            shadow_dir_root: None,
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: true,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    assert_eq!(outcomes.len(), 1);
    let (id, outcome) = &outcomes[0];
    assert_eq!(id, "target-a");
    let TargetOutcome::Benched { run_ids } = outcome else {
        panic!("expected Benched, got {outcome:?}");
    };
    assert_eq!(run_ids.len(), 1, "one invocation → one run-id");

    let calls = bench.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call[0], "bench");
    assert_eq!(call[1], "run");
    assert!(
        call.iter()
            .any(|a| a == "candidate-target-a-warm-steady"),
        "missing --name candidate-target-a-warm-steady: {call:?}"
    );
    // Pass 1c flag-symmetry: rich profiler emits no minimization flags.
    assert!(
        !call
            .iter()
            .any(|a| a == "--bench-spans-only")
    );
    assert!(
        !call
            .iter()
            .any(|a| a == "--no-profiler-kv")
    );

    // candidate-run-ids.json carries one entry.
    let ids_file = layout.experiment_candidate_run_ids_json("target-a");
    let raw = std::fs::read_to_string(&ids_file).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        parsed["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(parsed["entries"][0]["invocation_id"], "warm-steady");

    assert!(
        layout
            .experiment_dir("target-a")
            .join("bin")
            .join("stacks-bench")
            .is_file()
    );
}

#[test]
fn bench_experiments_skips_consensus_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    std::fs::create_dir_all(worktrees_root.join("poc")).unwrap();
    std::fs::create_dir_all(worktrees_root.join("issue")).unwrap();

    let bench_lock = tmp
        .path()
        .join("benchmark.lock");
    let targets_doc = make_targets(vec![
        target("poc", DeliveryMode::ConsensusPocPr, None),
        target("issue", DeliveryMode::ConsensusIssue, None),
    ]);
    let bench = RecordingBench::new(2000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        env: BenchEnv {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            shadow_dir_root: None,
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: true,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    for (_, outcome) in &outcomes {
        match outcome {
            TargetOutcome::Skipped { reason } => {
                assert!(reason.contains("not bench_eligible"), "unexpected skip reason: {reason}")
            }
            TargetOutcome::Benched { .. } => panic!("consensus targets should be skipped"),
        }
    }
    assert!(bench.calls().is_empty());
}

#[test]
fn bench_experiments_skips_targets_with_aborted_optimizer_report() {
    use stacks_bench_agent::models::common::SchemaVersionV2;
    use stacks_bench_agent::models::optimizer_report::{
        AbortedOutcomeTag, AbortedReport, FailedGate, OptimizerReport,
    };
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    std::fs::create_dir_all(worktrees_root.join("aborted")).unwrap();
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");
    let exp = layout.experiment_dir("aborted");
    std::fs::create_dir_all(&exp).unwrap();
    let report = OptimizerReport::Aborted(AbortedReport {
        schema_version: SchemaVersionV2,
        session_id: "20260507-104400".to_owned(),
        target_id: "aborted".to_owned(),
        outcome: AbortedOutcomeTag::Aborted,
        delivery_mode: DeliveryMode::NormalPr,
        reason: "test abort".to_owned(),
        failed_gate: Some(FailedGate::NoImplementationFound),
        failing_tests: None,
    });
    std::fs::write(
        exp.join("optimizer-report.json"),
        report
            .to_json_pretty()
            .unwrap(),
    )
    .unwrap();

    let targets_doc =
        make_targets(vec![target("aborted", DeliveryMode::NormalPr, Some(default_vr()))]);
    let bench = RecordingBench::new(3000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        env: BenchEnv {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            shadow_dir_root: None,
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: true,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    let (_, outcome) = &outcomes[0];
    let TargetOutcome::Skipped { reason } = outcome else {
        panic!("expected skipped, got {outcome:?}");
    };
    assert!(
        reason.contains("optimizer report outcome=aborted"),
        "unexpected skip reason: {reason}",
    );
    assert!(bench.calls().is_empty());
}

/// Multi-invocation targets emit one `bench run` per invocation, with
/// args derived from the `BenchSamples` variant + per-invocation
/// repetitions/warmup. The candidate-run-ids.json carries one entry per
/// invocation, in the order they appear on the target.
#[test]
fn bench_experiments_iterates_invocations_with_correct_args() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    std::fs::create_dir_all(worktrees_root.join("repl-tgt")).unwrap();
    write_implemented_report(&layout, "repl-tgt");
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");

    let txid_a = hex64(0xaa);
    let txid_b = hex64(0xbb);
    let block_c = hex64(0xcc);
    let txid_a_stripped = &txid_a[2..];
    let txid_b_stripped = &txid_b[2..];
    let block_c_stripped = &block_c[2..];

    let vr = VerificationReplay {
        rationale: "exercise both sample modes".to_owned(),
        invocations: vec![
            BenchInvocation {
                id: "cold-first-touch".to_owned(),
                label: "cold first touch".to_owned(),
                purpose: "tx-shape".to_owned(),
                samples: BenchSamples::Txids {
                    txids: vec![txid_a.clone(), txid_b.clone()],
                },
                warmup: 0,
                repetitions: 20,
                profiler: ProfilerMode::Rich,
                expected_signal: ExpectedSignal {
                    axis: SelectionLens::TxLatency,
                    direction: SignalDirection::Improves,
                    estimate_pct: Some(8.0),
                    tolerance_pct: Some(3.0),
                },
            },
            BenchInvocation {
                id: "warm-steady".to_owned(),
                label: "warm steady".to_owned(),
                purpose: "block-context".to_owned(),
                samples: BenchSamples::Blocks { blocks: vec![block_c.clone()] },
                warmup: 10,
                repetitions: 20,
                profiler: ProfilerMode::Rich,
                expected_signal: ExpectedSignal {
                    axis: SelectionLens::TxLatency,
                    direction: SignalDirection::Improves,
                    estimate_pct: Some(4.0),
                    tolerance_pct: Some(2.0),
                },
            },
        ],
        suspected_spans: None,
    };
    let tgt = target("repl-tgt", DeliveryMode::NormalPr, Some(vr));
    let targets_doc = make_targets(vec![tgt]);
    let bench = RecordingBench::new(7000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        env: BenchEnv {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            shadow_dir_root: None,
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: true,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    assert_eq!(outcomes.len(), 1);
    let (_, outcome) = &outcomes[0];
    let TargetOutcome::Benched { run_ids } = outcome else {
        panic!("expected Benched, got {outcome:?}");
    };
    assert_eq!(run_ids.len(), 2, "two invocations → two run-ids");

    let calls = bench.calls();
    assert_eq!(calls.len(), 2);

    // Invocation 1: cold-first-touch — txids.
    let txid_call = &calls[0];
    assert!(
        txid_call
            .iter()
            .any(|a| a == "candidate-repl-tgt-cold-first-touch")
    );
    let reps_idx = txid_call
        .iter()
        .position(|a| a == "--repetitions")
        .expect("--repetitions in txid call");
    assert_eq!(txid_call[reps_idx + 1], "20");
    let warmup_idx = txid_call
        .iter()
        .position(|a| a == "--warmup")
        .expect("--warmup in txid call");
    assert_eq!(txid_call[warmup_idx + 1], "0");
    assert_eq!(
        txid_call
            .iter()
            .filter(|a| *a == "--txid")
            .count(),
        2
    );
    assert!(
        txid_call
            .iter()
            .any(|a| a == txid_a_stripped)
    );
    assert!(
        txid_call
            .iter()
            .any(|a| a == txid_b_stripped)
    );
    assert!(
        !txid_call
            .iter()
            .any(|a| a == &txid_a),
        "0x-prefixed form must NOT reach stacks-bench"
    );
    assert!(
        !txid_call
            .iter()
            .any(|a| a == "--block")
    );
    assert!(
        !txid_call
            .iter()
            .any(|a| a == "--start-at")
    );

    // Invocation 2: warm-steady — blocks.
    let block_call = &calls[1];
    assert!(
        block_call
            .iter()
            .any(|a| a == "candidate-repl-tgt-warm-steady")
    );
    let warmup_idx = block_call
        .iter()
        .position(|a| a == "--warmup")
        .unwrap();
    assert_eq!(block_call[warmup_idx + 1], "10");
    assert!(
        block_call
            .iter()
            .any(|a| a == block_c_stripped)
    );
    assert_eq!(
        block_call
            .iter()
            .filter(|a| *a == "--block")
            .count(),
        1
    );
    assert!(
        !block_call
            .iter()
            .any(|a| a == "--txid")
    );

    // candidate-run-ids.json carries both entries in invocation order.
    let ids_file = layout.experiment_candidate_run_ids_json("repl-tgt");
    let raw = std::fs::read_to_string(&ids_file).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let entries = parsed["entries"]
        .as_array()
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["invocation_id"], "cold-first-touch");
    assert_eq!(entries[1]["invocation_id"], "warm-steady");
}

/// When `BenchEnv::shadow_dir_root` is set, every `bench run` invocation
/// emits `--shadow-dir-root <path>`.
#[test]
fn bench_experiments_emits_shadow_dir_root_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    std::fs::create_dir_all(worktrees_root.join("tgt-A")).unwrap();
    write_implemented_report(&layout, "tgt-A");
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");
    let shadow_root = tmp.path().join("shadows");

    let tgt = target("tgt-A", DeliveryMode::NormalPr, Some(default_vr()));
    let targets_doc = make_targets(vec![tgt]);
    let bench = RecordingBench::new(8000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        env: BenchEnv {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            shadow_dir_root: Some(&shadow_root),
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: true,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    let calls = bench.calls();
    assert!(!calls.is_empty());
    let shadow_str = shadow_root
        .to_string_lossy()
        .into_owned();
    for call in &calls {
        let idx = call
            .iter()
            .position(|a| a == "--shadow-dir-root")
            .unwrap_or_else(|| panic!("--shadow-dir-root missing in {call:?}"));
        assert_eq!(call[idx + 1], shadow_str, "--shadow-dir-root value mismatch in {call:?}");
    }
}

// ───────────────────────────────────────────────────────────────────
// Phase 3 of v2-cleanup-and-workspace-hygiene: pin the existing
// per-worktree `cargo clean` reclamation contract so it cannot
// regress unnoticed. The clean runs in `build_one` between binary
// copy and bench invocations — bench invocations use the copied
// binary at `exp_dir/bin/stacks-bench`, so the worktree's `target/`
// is genuinely disposable from that point onward.
// ───────────────────────────────────────────────────────────────────

/// Default path: `skip_cargo_clean = false` → `target/` is reclaimed in
/// each per-target worktree, and the `cargo-clean.log` /
/// `cargo-clean.stderr.log` fingerprint lands under
/// `optimize/<target>/`. The copied binary at `optimize/<target>/bin/`
/// must survive (the bench step depends on it).
#[test]
fn bench_experiments_reclaims_target_dir_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    let worktree = worktrees_root.join("target-a");
    std::fs::create_dir_all(&worktree).unwrap();
    write_implemented_report(&layout, "target-a");

    let targets_doc =
        make_targets(vec![target("target-a", DeliveryMode::NormalPr, Some(default_vr()))]);
    let bench = RecordingBench::new(2000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");

    bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        env: BenchEnv {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            shadow_dir_root: None,
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: false,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    // `target/` is gone — `cargo clean` ran.
    assert!(
        !worktree
            .join("target")
            .exists(),
        "worktree's target/ must be reclaimed by default",
    );

    // Fingerprint logs exist under optimize/<target>/.
    let exp = layout.experiment_dir("target-a");
    assert!(
        exp.join("cargo-clean.log")
            .exists(),
        "cargo-clean.log must be written when clean runs",
    );
    assert!(
        exp.join("cargo-clean.stderr.log")
            .exists(),
        "cargo-clean.stderr.log must be written when clean runs",
    );

    // Copied binary survives — bench invocations point at this.
    assert!(
        exp.join("bin")
            .join("stacks-bench")
            .is_file(),
        "copied stacks-bench binary must survive cargo clean (lives outside the worktree's \
         target/)",
    );
}

/// Opt-out path: `skip_cargo_clean = true` → worktree's `target/`
/// survives in full, and the `cargo-clean.log` fingerprint is absent
/// (proves the gate is honored, not a silent log-without-clean).
#[test]
fn bench_experiments_skip_cargo_clean_preserves_target_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    let worktree = worktrees_root.join("target-a");
    std::fs::create_dir_all(&worktree).unwrap();
    write_implemented_report(&layout, "target-a");

    let targets_doc =
        make_targets(vec![target("target-a", DeliveryMode::NormalPr, Some(default_vr()))]);
    let bench = RecordingBench::new(3000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");

    bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        env: BenchEnv {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            shadow_dir_root: None,
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: true,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    // `target/` survives — `cargo clean` was suppressed.
    assert!(
        worktree
            .join("target")
            .join("release")
            .join("stacks-bench")
            .exists(),
        "worktree's target/release/stacks-bench must survive --skip-cargo-clean",
    );

    // No cargo-clean fingerprint under optimize/<target>/ — the gate
    // suppressed the call entirely, not just the disk wipe.
    let exp = layout.experiment_dir("target-a");
    assert!(
        !exp.join("cargo-clean.log")
            .exists(),
        "cargo-clean.log must NOT exist when --skip-cargo-clean is set",
    );
    assert!(
        !exp.join("cargo-clean.stderr.log")
            .exists(),
        "cargo-clean.stderr.log must NOT exist when --skip-cargo-clean is set",
    );
}
