//! Port verification for `session::optimizers::run`. Uses fakes for both
//! the agent harness and the git worktree manager, so the test never
//! touches a real git repo or codex CLI.

use std::path::Path;
use std::sync::{Arc, Mutex};

use stacks_bench_agent::harnesses::{AgentHarness, InvokeInputs, InvokeOutputs};
use stacks_bench_agent::layout::{FrameworkDir, Layout};
use stacks_bench_agent::models::common::{
    BreakageClass, Bucket, DeliveryMode, Hotspot, ImprovementVector, Risk, SchemaVersionV2,
};
use stacks_bench_agent::models::targets::{
    MergeMethod, MergedFrom, MergedTarget, OptimizationTargets,
};
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::optimizers::{self, GitCheckoutManager, Inputs};
use stacks_bench_agent::settings::Settings;
use stacks_bench_agent::types::SessionId;

/// Per-target outcome the FakeHarness should emit.
#[derive(Debug, Clone, Copy)]
enum FakeDecision {
    /// Emit `outcome=implemented` with the given delivery_mode. The
    /// harness also drops a `fake-edit.txt` inside the checkout so the
    /// coordinator's `git status --porcelain` step sees a dirty tree.
    Implemented(DeliveryMode),
    /// Emit `outcome=aborted` with the given delivery_mode.
    Aborted(DeliveryMode),
}

/// Fake harness that writes a typed `optimizer-report.json` per target
/// based on the test-provided decision map. Replaces the marker-file
/// contract (`implementation.md` / `abort.md`) the agent used to emit
/// directly. Real ports never invoke this fake against `consensus_issue`
/// targets — those skip the harness entirely.
struct FakeHarness {
    /// target_id → typed outcome the fake will emit.
    decisions: Mutex<std::collections::BTreeMap<String, FakeDecision>>,
    /// Rendered prompt captured per target id so tests can assert which
    /// substitutions MiniJinja produced (e.g., the joined POC scope).
    prompts: Mutex<std::collections::BTreeMap<String, String>>,
    /// Session id stamped into every emitted report so the coordinator's
    /// context-checking loader accepts it (mismatch with the test
    /// fixture's session id would be flagged as a misbehaving agent).
    session_id: String,
}

impl FakeHarness {
    fn new(
        decisions: std::collections::BTreeMap<String, FakeDecision>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            decisions: Mutex::new(decisions),
            prompts: Mutex::new(Default::default()),
            session_id: session_id.into(),
        }
    }
    fn prompt_for(&self, target_id: &str) -> Option<String> {
        self.prompts
            .lock()
            .unwrap()
            .get(target_id)
            .cloned()
    }
}

/// Construct a valid `optimizer-report.json` body for the FakeHarness,
/// keyed off the supplied decision. Matches the model's `validate()`
/// invariants so the coordinator's parse-and-validate gate passes. The
/// `session_id` MUST match what the test layout uses, or the
/// coordinator's context-check will reject the report.
fn fake_report_body(target_id: &str, session_id: &str, decision: FakeDecision) -> String {
    use stacks_bench_agent::models::common::SchemaVersionV2;
    use stacks_bench_agent::models::optimizer_report::{
        AbortedOutcomeTag, AbortedReport, FailedGate, ImplementedOutcomeTag, ImplementedReport,
        OptimizerReport, ParityReport, TestFramework, TestSummary,
    };
    let report = match decision {
        FakeDecision::Implemented(mode) => OptimizerReport::Implemented(ImplementedReport {
            schema_version: SchemaVersionV2,
            session_id: session_id.to_owned(),
            target_id: target_id.to_owned(),
            outcome: ImplementedOutcomeTag::Implemented,
            delivery_mode: mode,
            implementation_summary: format!("fake implementation for {target_id}"),
            deviation_from_proposed_change: None,
            dependency_changes: None,
            test_summary: TestSummary {
                framework: TestFramework::Nextest,
                passed: 1,
                failed: 0,
                duration_secs: 1.0,
                log_path: "nextest.log".to_owned(),
            },
            // normal_pr requires Some(true); consensus_poc_pr accepts
            // any value — emit Some(true) uniformly for simplicity.
            clippy_clean: Some(true),
            pr_title: format!("perf: fake {target_id}"),
            parity: ParityReport {
                consensus_sensitive: false,
                evidence: vec![],
                tests: vec![],
                unproven_risk: None,
            },
            hard_fork_followup: None,
        }),
        FakeDecision::Aborted(mode) => OptimizerReport::Aborted(AbortedReport {
            schema_version: SchemaVersionV2,
            session_id: session_id.to_owned(),
            target_id: target_id.to_owned(),
            outcome: AbortedOutcomeTag::Aborted,
            delivery_mode: mode,
            reason: format!("fake abort for {target_id}"),
            failed_gate: Some(FailedGate::NoImplementationFound),
            failing_tests: None,
        }),
    };
    serde_json::to_string_pretty(&report).expect("fake report serializable")
}

impl AgentHarness for FakeHarness {
    async fn check(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn invoke<'a>(&'a self, inputs: &'a InvokeInputs<'a>) -> anyhow::Result<InvokeOutputs> {
        // The optimizer fan-out puts events_jsonl under
        // `experiments/<target-id>/`, so target id == that dir's name.
        let output_dir = inputs
            .events_jsonl
            .parent()
            .ok_or_else(|| anyhow::anyhow!("events_jsonl has no parent"))?;
        let target = output_dir
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("output_dir has no terminal segment"))?
            .to_string_lossy()
            .into_owned();
        self.prompts
            .lock()
            .unwrap()
            .insert(
                target.clone(),
                inputs
                    .rendered_prompt
                    .to_owned(),
            );

        std::fs::write(
            inputs.events_jsonl,
            serde_json::json!({"conversation_id": format!("conv-{target}")}).to_string() + "\n",
        )?;
        std::fs::write(inputs.stderr_log, b"")?;
        std::fs::write(inputs.last_message, b"# done\n")?;

        let decision = self
            .decisions
            .lock()
            .unwrap()
            .get(&target)
            .copied()
            .unwrap_or(FakeDecision::Implemented(DeliveryMode::NormalPr));
        std::fs::write(
            output_dir.join("optimizer-report.json"),
            fake_report_body(&target, &self.session_id, decision),
        )?;
        // For implemented outcomes the coordinator runs `git status
        // --porcelain` after we exit and demotes if the tree is clean,
        // so synthesize a real edit inside the checkout.
        if matches!(decision, FakeDecision::Implemented(_)) {
            std::fs::write(
                inputs
                    .cwd
                    .join("fake-edit.txt"),
                format!("synthesized by FakeHarness for {target}\n"),
            )?;
        }

        Ok(InvokeOutputs {
            conversation_id: Some(format!("conv-{target}")),
        })
    }
}

/// Fake git: initializes a minimal real git repo at the checkout so
/// `optimizers::run_one`'s coordinator-commit step (pass-b.1) has
/// something it can actually `git status` / `git commit` against.
/// Stops short of cloning from a base — there's no `base` to clone
/// from in these tests — but the checkout has its own `.git/`, one
/// initial commit, and `commit.gpgsign=false` so the coordinator's
/// commit doesn't trigger signing.
struct FakeGit;
impl GitCheckoutManager for FakeGit {
    fn recreate_checkout(
        &self,
        _base: &Path,
        checkout: &Path,
        _branch_name: &str,
        _base_branch: &str,
    ) -> anyhow::Result<()> {
        let _ = std::fs::remove_dir_all(checkout);
        std::fs::create_dir_all(checkout)?;
        stacks_bench_agent::git::init_test_repo(checkout)?;
        // Seed an initial commit so HEAD resolves + the coordinator's
        // post-edit commit advances HEAD past a real baseline.
        std::fs::write(checkout.join(".gitignore"), "target/\n")?;
        for args in [&["add", ".gitignore"][..], &["commit", "-q", "-m", "init"][..]] {
            stacks_bench_agent::git::run_git(checkout, args)?;
        }
        Ok(())
    }
    fn remove_checkout(&self, checkout: &Path) -> anyhow::Result<bool> {
        if !checkout.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(checkout)?;
        Ok(true)
    }
}

// Stages a context dir with the bundled docs seeded so the
// orchestrator's required-doc startup check passes.
fn stage_context_seeds(framework: &std::path::Path) {
    stacks_bench_agent::context::seed_to(&framework.join("context")).unwrap();
}

fn stage_framework_and_session(
    tmp: &tempfile::TempDir,
    targets: Vec<MergedTarget>,
) -> (Layout, SessionLayout) {
    let framework = tmp.path().join("framework");
    std::fs::create_dir_all(framework.join("prompts")).unwrap();
    std::fs::create_dir_all(framework.join("schemas")).unwrap();
    stage_context_seeds(&framework);
    std::fs::create_dir_all(
        framework
            .join("repos")
            .join("stacks-core"),
    )
    .unwrap();
    let layout = Layout {
        framework: Some(FrameworkDir::new(framework.clone())),
        schemas_dir: framework
            .clone()
            .join("schemas"),
        queries_dir: framework
            .clone()
            .join("queries"),
        context_dir: framework
            .clone()
            .join("context"),
        memory_dir: framework
            .clone()
            .join("memory"),
        sessions_root: tmp.path().join("sessions"),
        stacks_bench_data_dir: tmp
            .path()
            .join("data")
            .join("stacks-bench"),
        bench_lock: tmp
            .path()
            .join("benchmark.lock"),
        test_lock: tmp.path().join("test.lock"),
        base: Some(
            framework
                .join("repos")
                .join("stacks-core"),
        ),
        stacks_bench_shadow_dir: None,
        agent_workspace_root: None,
        operator_repo_root: None,
    };

    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let session = SessionLayout::from_layout(&layout, id);
    session
        .create_all_phase_dirs()
        .unwrap();

    let targets_doc = OptimizationTargets {
        schema_version: SchemaVersionV2,
        session_id: "20260507-104400".to_owned(),
        baseline_run_id: 100,
        baseline_rerun_id: 101,
        noise_floor_pct: 0.8,
        merge_method: MergeMethod::Llm,
        merge_model: "test".to_owned(),
        targets,
        rejected_by_merge: vec![],
        lens_dispositions: vec![],
    };
    std::fs::write(
        session.optimization_targets_json(),
        serde_json::to_string_pretty(&targets_doc).unwrap(),
    )
    .unwrap();

    (layout, session)
}

fn target(id: &str, delivery_mode: DeliveryMode, poc_scope: Option<Vec<&str>>) -> MergedTarget {
    let consensus_breaking = !matches!(delivery_mode, DeliveryMode::NormalPr);
    let breakage_class =
        if consensus_breaking { Some(BreakageClass::ClarityCostWeight) } else { None };
    let poc_implementable = match delivery_mode {
        DeliveryMode::NormalPr => None,
        DeliveryMode::ConsensusPocPr => Some(true),
        DeliveryMode::ConsensusIssue => Some(false),
    };
    let poc_test_scope = poc_scope.map(|v| {
        v.into_iter()
            .map(str::to_owned)
            .collect()
    });
    let consensus_writeup = consensus_breaking.then(|| "writeup".to_owned());
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
        consensus_breaking,
        breakage_class,
        poc_implementable,
        poc_test_scope,
        consensus_writeup,
        delivery_mode,
        bench_eligible: matches!(delivery_mode, DeliveryMode::NormalPr),
    }
}

#[tokio::test]
async fn optimizers_routes_three_delivery_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, session) = stage_framework_and_session(
        &tmp,
        vec![
            target("normal-tgt", DeliveryMode::NormalPr, None),
            target(
                "poc-tgt",
                DeliveryMode::ConsensusPocPr,
                Some(vec!["package(x)::test::a", "package(x)::test::b"]),
            ),
            target("issue-tgt", DeliveryMode::ConsensusIssue, None),
        ],
    );

    let mut decisions = std::collections::BTreeMap::new();
    decisions.insert("normal-tgt".to_owned(), FakeDecision::Implemented(DeliveryMode::NormalPr));
    decisions.insert("poc-tgt".to_owned(), FakeDecision::Implemented(DeliveryMode::ConsensusPocPr));
    let harness = Arc::new(FakeHarness::new(decisions, "20260507-104400"));

    let prompts_dir = tmp.path().join("prompts");
    stacks_bench_agent::prompts::seed_to(&prompts_dir).expect("seed prompts");
    let settings = Settings {
        prompt_overrides_dir: Some(prompts_dir),
        ..Settings::default()
    };
    let outputs = optimizers::run(Inputs {
        layout: session.clone(),
        framework: layout.clone(),
        settings,
        parallel: Some(2),
        base_branch: "feat/test".to_owned(),
        harness: harness.clone(),
        git: Arc::new(FakeGit),
        resume: false,
    })
    .await
    .expect("optimizers::run");

    assert_eq!(outputs.total, 3);
    assert_eq!(outputs.landed, 2);
    assert_eq!(outputs.routed_to_issue, 1);
    assert_eq!(outputs.aborted, 0);

    // consensus_issue target got a marker, no optimizer prompt.
    assert!(
        session
            .experiment_dir("issue-tgt")
            .join("consensus-issue.md")
            .is_file()
    );
    assert!(
        !session
            .experiment_dir("issue-tgt")
            .join("optimizer-prompt.md")
            .is_file()
    );

    // PoC target's POC_TEST_SCOPE_EXPR is the bash `join " | "` form,
    // and that string lands in the rendered prompt verbatim.
    let poc_prompt = harness
        .prompt_for("poc-tgt")
        .unwrap();
    assert!(
        poc_prompt.contains("package(x)::test::a | package(x)::test::b"),
        "rendered poc-tgt prompt missing the joined scope expression"
    );

    // normal_pr target gets empty POC_TEST_SCOPE_EXPR; the only place
    // that would carry the substring above is the `${POC_TEST_SCOPE_EXPR}`
    // slot, so its absence is the assertion.
    let normal_prompt = harness
        .prompt_for("normal-tgt")
        .unwrap();
    assert!(
        !normal_prompt.contains("package(x)::test::"),
        "normal-tgt prompt unexpectedly contains a poc test scope"
    );

    // optimizer-report.json landed for normal + poc, and the
    // coordinator rendered the implementation.md companion view.
    for tid in ["normal-tgt", "poc-tgt"] {
        assert!(
            session
                .experiment_dir(tid)
                .join("optimizer-report.json")
                .is_file(),
            "{tid}: optimizer-report.json missing"
        );
        assert!(
            session
                .experiment_dir(tid)
                .join("implementation.md")
                .is_file(),
            "{tid}: coordinator-rendered implementation.md missing"
        );
    }
}

#[tokio::test]
async fn optimizers_aborts_clear_implementation_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, session) =
        stage_framework_and_session(&tmp, vec![target("tgt-a", DeliveryMode::NormalPr, None)]);

    let mut decisions = std::collections::BTreeMap::new();
    decisions.insert("tgt-a".to_owned(), FakeDecision::Aborted(DeliveryMode::NormalPr));
    let harness = Arc::new(FakeHarness::new(decisions, "20260507-104400"));

    let prompts_dir = tmp.path().join("prompts");
    stacks_bench_agent::prompts::seed_to(&prompts_dir).expect("seed prompts");
    let settings = Settings {
        prompt_overrides_dir: Some(prompts_dir),
        ..Settings::default()
    };
    let outputs = optimizers::run(Inputs {
        layout: session.clone(),
        framework: layout.clone(),
        settings,
        parallel: None,
        base_branch: "feat/test".to_owned(),
        harness,
        git: Arc::new(FakeGit),
        resume: false,
    })
    .await
    .unwrap();

    assert_eq!(outputs.aborted, 1);
    assert_eq!(outputs.landed, 0);
    let exp_dir = session.experiment_dir("tgt-a");
    assert!(
        exp_dir
            .join("optimizer-report.json")
            .is_file(),
        "agent's typed aborted report missing"
    );
    assert!(
        exp_dir
            .join("abort.md")
            .is_file(),
        "coordinator-rendered abort.md companion missing"
    );
}

/// `prune_aborted_experiments` walks the experiments tree and tears
/// down the per-target checkout (clone) for every dir whose
/// `optimizer-report.json` is missing OR `outcome=aborted` (crashed
/// mid-run). Dirs with `outcome=implemented` are left alone — their
/// checkouts are what Phase 5 publish reads + pushes from.
///
/// With the clone-based model, teardown is a single `remove_checkout`
/// per aborted target — the `agent/<session>/<target>` branch lives
/// inside the clone and goes away with it. No separate `delete_branch`
/// call (or ordering between worktree and branch teardown) is needed.
#[tokio::test]
async fn prune_aborted_experiments_drops_only_unmarked_checkouts() {
    use std::sync::Mutex;

    struct RecordingGit {
        removed: Mutex<Vec<std::path::PathBuf>>,
    }
    impl GitCheckoutManager for RecordingGit {
        fn recreate_checkout(&self, _: &Path, _: &Path, _: &str, _: &str) -> anyhow::Result<()> {
            unreachable!("not used by this test");
        }
        fn remove_checkout(&self, checkout: &Path) -> anyhow::Result<bool> {
            self.removed
                .lock()
                .unwrap()
                .push(checkout.to_path_buf());
            Ok(true)
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let sessions_root = tmp.path().join("sessions");
    let layout = SessionLayout::new(&sessions_root, id.clone());
    let experiments_dir = layout.optimize_dir();
    // Three experiments:
    //   - tgt-kept   → outcome=implemented  (preserve checkout)
    //   - tgt-abort  → outcome=aborted      (drop checkout)
    //   - tgt-crash  → no report at all     (drop checkout — crash equivalent)
    for (target_id, decision) in [
        ("tgt-kept", Some(FakeDecision::Implemented(DeliveryMode::NormalPr))),
        ("tgt-abort", Some(FakeDecision::Aborted(DeliveryMode::NormalPr))),
        ("tgt-crash", None),
    ] {
        let dir = experiments_dir.join(target_id);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(d) = decision {
            // Use the session id the layout was constructed with so the
            // context-checking loader's session_id check passes.
            std::fs::write(
                dir.join("optimizer-report.json"),
                fake_report_body(target_id, "20260507-104400", d),
            )
            .unwrap();
        }
    }

    let git = RecordingGit {
        removed: Mutex::new(Vec::new()),
    };
    let checkouts_root = tmp.path().join("worktrees");

    let dropped =
        optimizers::prune_aborted_experiments(&git, &checkouts_root, &layout).expect("prune");

    let mut removed: Vec<std::path::PathBuf> = git
        .removed
        .lock()
        .unwrap()
        .clone();
    removed.sort();
    assert_eq!(
        removed,
        vec![checkouts_root.join("tgt-abort"), checkouts_root.join("tgt-crash"),],
        "expected exactly the aborted + crashed checkouts to be removed",
    );

    assert!(
        !removed
            .iter()
            .any(|p| p.ends_with("tgt-kept")),
        "kept checkout must not be removed",
    );

    assert_eq!(dropped, 2);
}
