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
    Bucket, DeliveryMode, Hotspot, ImprovementVector, Risk, SchemaVersionV2, VerificationReplay,
};
use stacks_bench_agent::models::targets::{
    MergeMethod, MergedFrom, MergedTarget, OptimizationTargets,
};
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::bench::{BenchClient, InvokeOptions};
use stacks_bench_agent::session::bench_experiments::{self, BenchRange, Inputs, TargetOutcome};
use stacks_bench_agent::session::cargo::CargoRunner;
use stacks_bench_agent::types::SessionId;

/// Fake cargo: writes empty stdout/stderr logs and creates a stub binary.
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
    fn clean(&self, _worktree: &Path, stdout: &Path, stderr: &Path) -> anyhow::Result<()> {
        std::fs::write(stdout, b"")?;
        std::fs::write(stderr, b"")?;
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
/// `target_id` in the experiment dir. Tests that exercise the bench
/// path need this so the typed-report gate in `static_skip_reason`
/// permits the target. The `session_id` MUST match `make_layout`'s id
/// or the context-checking loader rejects.
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

fn target(id: &str, mode: DeliveryMode) -> MergedTarget {
    let bench_eligible = matches!(mode, DeliveryMode::NormalPr);
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
        proposed_change: "p".to_owned(),
        expected_improvement: ImprovementVector {
            tx_latency: 1.0,
            tenure_throughput: 0.0,
            commit_time: 0.0,
        },
        risk: Risk::Low,
        verification_plan: "v".to_owned(),
        verification_replay: None,
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
        schema_version: SchemaVersionV2,
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

#[test]
fn bench_experiments_normal_pr_target_runs_two_invocations() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");
    // Create the worktree directory the build step expects to find.
    std::fs::create_dir_all(worktrees_root.join("target-a")).unwrap();
    // Seed a typed `outcome=implemented` report so the bench gate
    // doesn't skip the target.
    write_implemented_report(&layout, "target-a");

    let targets_doc = make_targets(vec![target("target-a", DeliveryMode::NormalPr)]);
    let bench = RecordingBench::new(1000);

    let bench_for_target = |bin_path: &Path| -> Box<dyn BenchClient> {
        // Wrap the *single* recording fake; the bin_path is unused for the test.
        let _ = bin_path;
        Box::new(SharedBenchHandle(&bench))
    };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        range: BenchRange {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            start_at: Some(5_000_000),
            count: Some(200_000),
            warmup: None,
            filter: None,
            shadow_dir_root: None,
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: true,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    // Outcome reflects the two recorded run ids.
    assert_eq!(outcomes.len(), 1);
    let (id, outcome) = &outcomes[0];
    assert_eq!(id, "target-a");
    let TargetOutcome::Benched { run_ids } = outcome else {
        panic!("expected Benched, got {outcome:?}");
    };
    assert_eq!(run_ids.len(), 2);

    // Both invocations were `bench run --source ... --name target-a-run-{1,2}`,
    // and NEITHER carries minimal-profiler flags. Pass 1c flag-symmetry
    // invariant: candidate must use the same profile flags as the Phase
    // 1.8 baseline calibration; baseline runs rich (no --bench-spans-only,
    // no --no-profiler-kv), so candidate must match.
    let calls = bench.calls();
    assert_eq!(calls.len(), 2);
    for (i, call) in calls.iter().enumerate() {
        assert_eq!(call[0], "bench");
        assert_eq!(call[1], "run");
        let expected_name = format!("target-a-run-{}", i + 1);
        assert!(
            call.iter()
                .any(|a| a == &expected_name),
            "call[{i}] missing --name {expected_name}: {call:?}"
        );
        assert!(
            !call
                .iter()
                .any(|a| a == "--bench-spans-only"),
            "candidate call[{i}] must NOT carry --bench-spans-only (flag-symmetry with rich \
             baseline); got: {call:?}"
        );
        assert!(
            !call
                .iter()
                .any(|a| a == "--no-profiler-kv"),
            "candidate call[{i}] must NOT carry --no-profiler-kv (flag-symmetry with rich \
             baseline); got: {call:?}"
        );
    }

    // run-ids file has both ids on separate lines.
    let run_ids_file = layout.experiment_run_ids_path("target-a");
    let body = std::fs::read_to_string(&run_ids_file).unwrap();
    let ids: Vec<&str> = body.lines().collect();
    assert_eq!(ids.len(), 2);

    // The per-target binary got copied out.
    assert!(
        layout
            .experiment_dir("target-a")
            .join("bin")
            .join("stacks-bench")
            .is_file()
    );
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
        target("poc", DeliveryMode::ConsensusPocPr),
        target("issue", DeliveryMode::ConsensusIssue),
    ]);
    let bench = RecordingBench::new(2000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        range: BenchRange {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            start_at: Some(5_000_000),
            count: Some(200_000),
            warmup: None,
            filter: None,
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
    // No bench invocations recorded.
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
    // Pre-create the experiment dir + a typed `outcome=aborted`
    // report so the bench gate skips this target. session_id must
    // match `make_layout`'s id or the context-checking loader rejects.
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

    let targets_doc = make_targets(vec![target("aborted", DeliveryMode::NormalPr)]);
    let bench = RecordingBench::new(3000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        range: BenchRange {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            start_at: Some(5_000_000),
            count: Some(200_000),
            warmup: None,
            filter: None,
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

/// When a `normal_pr` target carries a `verification_replay` recipe, Phase 3
/// should swap the full-range `--start-at`/`--count` args for the
/// targeted-replay shape (`--repetitions` + repeated `--txid` and/or
/// `--block` flags). One phase per non-empty recipe section; both sections
/// present → two phases (txids and blocks can't share a single bench
/// invocation because the flags conflict).
#[test]
fn bench_experiments_uses_verification_replay_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = make_layout(&tmp);
    let worktrees_root = tmp.path().join("worktrees");
    std::fs::create_dir_all(worktrees_root.join("repl-tgt")).unwrap();
    // Seed a typed `outcome=implemented` report so the bench gate
    // doesn't skip the target.
    write_implemented_report(&layout, "repl-tgt");
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");

    let mut tgt = target("repl-tgt", DeliveryMode::NormalPr);
    // The recipe stores hashes in artifact form (with `0x` prefix);
    // `bench_experiments` strips them before passing to stacks-bench
    // (whose `--txid` / `--block` reject the prefix).
    let txid_a = "0x".to_owned() + &"a".repeat(64);
    let txid_b = "0x".to_owned() + &"b".repeat(64);
    let block_c = "0x".to_owned() + &"c".repeat(64);
    let txid_a_stripped = &txid_a[2..];
    let txid_b_stripped = &txid_b[2..];
    let block_c_stripped = &block_c[2..];
    tgt.verification_replay = Some(VerificationReplay {
        txids: Some(vec![txid_a.clone(), txid_b.clone()]),
        blocks: Some(vec![block_c.clone()]),
        repetitions: 20,
        // Recipe omits warmup → bench_experiments should default to 10
        // and emit `--warmup 10` on both txid and block phases.
        warmup: None,
        rationale: "snapshot-test".to_owned(),
    });
    let targets_doc = make_targets(vec![tgt]);
    let bench = RecordingBench::new(7000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        range: BenchRange {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            start_at: Some(5_000_000),
            count: Some(200_000),
            warmup: None,
            filter: None,
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
    assert_eq!(run_ids.len(), 2, "txids + blocks → two phases → two run-ids");

    let calls = bench.calls();
    assert_eq!(calls.len(), 2, "two bench invocations (txids, blocks)");

    // Flag-symmetry invariant (Pass 1c): targeted-replay candidate runs
    // must NOT carry --bench-spans-only / --no-profiler-kv because the
    // Phase 1.8 baseline they're compared against runs rich. Asymmetric
    // profile overhead biases improvement_pct on profile-heavy
    // workloads. If lean profiles are reintroduced later, baseline +
    // candidate must match within a profile.
    for (i, call) in calls.iter().enumerate() {
        assert!(
            !call
                .iter()
                .any(|a| a == "--bench-spans-only"),
            "targeted-replay call[{i}] must NOT carry --bench-spans-only: {call:?}"
        );
        assert!(
            !call
                .iter()
                .any(|a| a == "--no-profiler-kv"),
            "targeted-replay call[{i}] must NOT carry --no-profiler-kv: {call:?}"
        );
    }

    // Phase 1: txids — `--repetitions 20 --txid <a> --txid <b>` + name suffix
    // `txids`.
    let txid_call = &calls[0];
    assert!(
        txid_call
            .iter()
            .any(|a| a == "repl-tgt-txids"),
        "txid call missing --name suffix `txids`: {txid_call:?}"
    );
    let reps_idx = txid_call
        .iter()
        .position(|a| a == "--repetitions")
        .expect("--repetitions present in txid call");
    assert_eq!(txid_call[reps_idx + 1], "20");
    // recipe omitted warmup → default 10 emitted.
    let warmup_idx = txid_call
        .iter()
        .position(|a| a == "--warmup")
        .expect("--warmup present in txid call (default applied)");
    assert_eq!(txid_call[warmup_idx + 1], "10");
    let txid_flags: Vec<&String> = txid_call
        .iter()
        .filter(|a| *a == "--txid")
        .collect();
    assert_eq!(txid_flags.len(), 2, "two --txid flags expected");
    // bench_experiments strips the `0x` prefix; assert the raw hex
    // appears (no `0x`) AND the prefixed form does NOT.
    assert!(
        txid_call
            .iter()
            .any(|a| a == txid_a_stripped),
        "txid call missing stripped {txid_a_stripped} in {txid_call:?}"
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
        "txid call must NOT carry the 0x-prefixed form (stacks-bench rejects it)"
    );
    assert!(
        !txid_call
            .iter()
            .any(|a| a == "--start-at"),
        "txid call must NOT carry --start-at"
    );
    assert!(
        !txid_call
            .iter()
            .any(|a| a == "--block"),
        "txid call must NOT carry --block (CLI conflict)"
    );

    // Phase 2: blocks — `--repetitions 20 --block <c>` + name suffix `blocks`.
    let block_call = &calls[1];
    assert!(
        block_call
            .iter()
            .any(|a| a == "repl-tgt-blocks"),
        "block call missing --name suffix `blocks`: {block_call:?}"
    );
    assert!(
        block_call
            .iter()
            .any(|a| a == block_c_stripped),
        "block call missing stripped {block_c_stripped} in {block_call:?}"
    );
    assert!(
        !block_call
            .iter()
            .any(|a| a == &block_c),
        "block call must NOT carry the 0x-prefixed form"
    );
    let block_flags: Vec<&String> = block_call
        .iter()
        .filter(|a| *a == "--block")
        .collect();
    assert_eq!(block_flags.len(), 1);
    assert!(
        !block_call
            .iter()
            .any(|a| a == "--txid")
    );
}

/// When `BenchRange::shadow_dir_root` is set, every `bench run` invocation
/// emits `--shadow-dir-root <path>`. This is the flag operators set when the
/// source dir's parent isn't writable from the codex sandbox (e.g.
/// `/Volumes/Extern`); stacks-bench then creates its reflink shadow under
/// the override path instead. When unset, the flag is omitted entirely.
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

    let tgt = target("tgt-A", DeliveryMode::NormalPr);
    let targets_doc = make_targets(vec![tgt]);
    let bench = RecordingBench::new(8000);
    let bench_for_target =
        |_: &Path| -> Box<dyn BenchClient> { Box::new(SharedBenchHandle(&bench)) };
    let bench_for_target_ref: &dyn Fn(&Path) -> Box<dyn BenchClient> = &bench_for_target;

    bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets_doc,
        range: BenchRange {
            source_dir: &PathBuf::from("/mnt/chainstate"),
            network: "mainnet",
            start_at: Some(5_000_000),
            count: Some(200_000),
            warmup: None,
            filter: None,
            shadow_dir_root: Some(&shadow_root),
        },
        bench_lock: &bench_lock,
        skip_cargo_clean: true,
        cargo: &StubCargo,
        bench_for_target: bench_for_target_ref,
    })
    .expect("bench_experiments::run");

    let calls = bench.calls();
    assert_eq!(calls.len(), 2, "full-range default = two invocations");
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
