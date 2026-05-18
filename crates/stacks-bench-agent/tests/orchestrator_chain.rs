//! Smoke test for the post-merge half of the orchestrator chain. Exercises
//! Phase 2 (optimizers) → Phase 4 (finalize) → Phase 5 (publish::generate +
//! publish::push) with fakes, starting from the staged fixture session.
//!
//! Out of scope: Phase 0 (baseline run/import) and Phase 3 (bench experiments)
//! both need either a real `stacks-bench` DB or substantial cargo-runner +
//! per-target bench-client mocks. Triage / analyzer / merge are covered by
//! per-phase tests; chaining them in fakes requires producing fully v2-valid
//! candidates / analyses / merged-targets which would balloon this test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use stacks_bench_agent::harnesses::{AgentHarness, InvokeInputs, InvokeOutputs};
use stacks_bench_agent::layout::{FrameworkDir, Layout};
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::bench::BenchClient;
use stacks_bench_agent::session::finalize::{FinalizeInputs, finalize};
use stacks_bench_agent::session::optimizers::{self, GitCheckoutManager};
use stacks_bench_agent::session::publish::{
    self, CreatePrArgs, GenerateInputs, GhClient, PublishConfig, PushInputs,
};
use stacks_bench_agent::settings::Settings;
use stacks_bench_agent::types::SessionId;

const FULL_PR_BODY: &str =
    "## Summary\n\ns\n\n## What changed\n\nw\n\n## Benchmark result\n\nb\n\n## Validation\n\nv\n";
const POC_PR_BODY: &str = "## Summary\n\ns\n\n## What changed\n\nw\n\n## Benchmark \
                           result\n\nb\n\n## Validation\n\nv\n\n## Consensus / HIP \
                           coordination\n\nc\n";
const ISSUE_BODY: &str = "## Summary\n\ns\n\n## Breakage class\n\nbc\n\n## Proposed \
                          change\n\npc\n\n## Expected impact\n\nei\n\n## HIP / coordination \
                          concerns\n\nh\n\n## Why an issue, not a PR\n\nw\n\n## Reference: target \
                          id\n\nr\n";

/// Routes `invoke` calls based on which template the prompt was rendered
/// from. Identifies optimizer/pr-writer/issue-writer by markers we plant in
/// the staged template files.
struct ChainHarness {
    invocations: Mutex<Vec<String>>,
}

impl ChainHarness {
    fn new() -> Self {
        Self {
            invocations: Mutex::new(Vec::new()),
        }
    }
}

impl AgentHarness for ChainHarness {
    async fn check(&self) -> Result<()> {
        Ok(())
    }
    async fn invoke<'a>(&'a self, inputs: &'a InvokeInputs<'a>) -> Result<InvokeOutputs> {
        // Always write events + last-message so post-invoke validation passes.
        std::fs::write(
            inputs.events_jsonl,
            serde_json::json!({"conversation_id": "conv-fake"}).to_string() + "\n",
        )?;
        std::fs::write(inputs.stderr_log, b"")?;
        std::fs::write(inputs.last_message, b"# done\n")?;

        // events_jsonl is `<session>/results/optimize/<target>/<file>`.
        // After the layout restructure, the optimizer drops its phase
        // prefix (writes plain `events.jsonl`), while pr-writer and
        // issue-writer keep theirs to avoid colliding in the shared
        // per-target dir. Classify the call from those names.
        let events_name = inputs
            .events_jsonl
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let kind = if events_name == "events.jsonl" {
            "optimizer"
        } else if events_name == "pr-writer-events.jsonl" {
            "pr-writer"
        } else if events_name == "issue-writer-events.jsonl" {
            "issue-writer"
        } else {
            "unknown"
        };
        let output_dir: PathBuf = inputs
            .events_jsonl
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| inputs.cwd.to_path_buf());
        let target = output_dir
            .file_name()
            .map(|s| {
                s.to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();
        self.invocations
            .lock()
            .unwrap()
            .push(format!("{kind}:{target}"));

        // Resolve delivery_mode by reading the session's
        // merged optimization-targets.json. The fake doesn't have
        // access to the typed targets doc directly, so it reaches
        // up through the per-target dir to find it under merge/.
        let delivery: String = output_dir
            .parent() // optimize/
            .and_then(|p| p.parent()) // results/
            .map(|p| {
                p.join("merge")
                    .join("optimization-targets.json")
            })
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("targets")
                    .and_then(|a| a.as_array())
                    .and_then(|arr| {
                        arr.iter()
                            .find(|t| {
                                t.get("id")
                                    .and_then(|i| i.as_str())
                                    == Some(&target)
                            })
                            .and_then(|t| {
                                t.get("delivery_mode")
                                    .and_then(|d| d.as_str())
                            })
                            .map(str::to_owned)
                    })
            })
            .unwrap_or_else(|| "normal_pr".to_owned());
        let delivery = delivery.as_str();

        match kind {
            "optimizer" => {
                // Typed optimizer-report.json written by the agent. For
                // consensus_issue, the optimizer phase is skipped entirely
                // by the orchestrator; this branch only handles normal_pr
                // and consensus_poc_pr.
                use stacks_bench_agent::models::common::{DeliveryMode, SchemaVersionV2};
                use stacks_bench_agent::models::optimizer_report::{
                    ImplementedOutcomeTag, ImplementedReport, OptimizerReport, ParityReport,
                    TestFramework, TestSummary,
                };
                let mode = match delivery {
                    "consensus_poc_pr" => DeliveryMode::ConsensusPocPr,
                    _ => DeliveryMode::NormalPr,
                };
                let report = OptimizerReport::Implemented(ImplementedReport {
                    schema_version: SchemaVersionV2,
                    // Must match the session id the test layout uses or
                    // the coordinator's context-checking loader rejects
                    // the report.
                    session_id: "20260507-104400".to_owned(),
                    target_id: target.clone(),
                    outcome: ImplementedOutcomeTag::Implemented,
                    delivery_mode: mode,
                    implementation_summary: format!("chain harness implementation for {target}"),
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
                    pr_title: format!("perf: chain {target}"),
                    parity: ParityReport {
                        consensus_sensitive: false,
                        evidence: vec![],
                        tests: vec![],
                        unproven_risk: None,
                    },
                    hard_fork_followup: None,
                });
                std::fs::write(
                    output_dir.join("optimizer-report.json"),
                    serde_json::to_string_pretty(&report)?,
                )?;
                // Coordinator commits after the agent exits, requiring
                // `git status --porcelain` to show changes. Simulate
                // the agent's source edit so the coordinator commit
                // contract is satisfied.
                std::fs::write(
                    inputs
                        .cwd
                        .join("fake-edit.txt"),
                    format!("synthesized by ChainHarness for {target}\n"),
                )?;
            }
            "pr-writer" => {
                std::fs::write(
                    output_dir.join("pr-title.txt"),
                    format!("perf: optimize {target}\n"),
                )?;
                let body = match delivery {
                    "consensus_poc_pr" => POC_PR_BODY,
                    _ => FULL_PR_BODY,
                };
                std::fs::write(output_dir.join("pr-body.md"), body)?;
            }
            "issue-writer" => {
                std::fs::write(
                    output_dir.join("issue-title.txt"),
                    format!("consensus: {target}\n"),
                )?;
                std::fs::write(output_dir.join("issue-body.md"), ISSUE_BODY)?;
            }
            _ => anyhow::bail!("unknown prompt kind"),
        }

        Ok(InvokeOutputs {
            conversation_id: Some("conv-fake".to_owned()),
        })
    }
}

struct ChainGit;
impl GitCheckoutManager for ChainGit {
    fn recreate_checkout(
        &self,
        _base: &Path,
        checkout: &Path,
        _branch: &str,
        _base_branch: &str,
    ) -> Result<()> {
        let _ = std::fs::remove_dir_all(checkout);
        std::fs::create_dir_all(checkout)?;
        // Pass-b.1: the coordinator commits inside the checkout. The
        // chain test needs a real `.git/` so `git status --porcelain`
        // / `git commit` work. Init + one initial commit, signing
        // disabled.
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "fake@t"],
            vec!["config", "user.name", "fake"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(checkout)
                .args(&args)
                .status()?;
            anyhow::ensure!(status.success(), "git {args:?} failed: {status}");
        }
        std::fs::write(checkout.join(".gitignore"), "target/\n")?;
        for args in [vec!["add", ".gitignore"], vec!["commit", "-q", "-m", "init"]] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(checkout)
                .args(&args)
                .status()?;
            anyhow::ensure!(status.success(), "git {args:?} failed: {status}");
        }
        Ok(())
    }
    fn remove_checkout(&self, checkout: &Path) -> Result<bool> {
        if !checkout.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(checkout)?;
        Ok(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GhCall {
    PrExists,
    IssueExists,
    SwitchBranch(String),
    AddModified,
    Commit,
    Push(String),
    CreatePr { head: String, draft: bool, label_count: usize },
    CreateIssue { title: String, body_has_trace: bool },
}

#[derive(Default)]
struct ChainGh {
    calls: Mutex<Vec<GhCall>>,
}

impl GhClient for ChainGh {
    fn worktree_remote_url(&self, _w: &Path, _r: &str) -> Result<String> {
        Ok("git@github.com:cylewitruk/stacks-core.git".to_owned())
    }
    fn switch_branch(&self, _: &Path, branch: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(GhCall::SwitchBranch(branch.to_owned()));
        Ok(())
    }
    fn add_modified(&self, _: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(GhCall::AddModified);
        Ok(())
    }
    fn commit_if_staged(&self, _: &Path, _: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(GhCall::Commit);
        Ok(())
    }
    fn push_branch(&self, _: &Path, _: &str, branch: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(GhCall::Push(branch.to_owned()));
        Ok(())
    }
    async fn pr_exists(&self, _: &str, _: &str, _: &str, _: &str) -> Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(GhCall::PrExists);
        Ok(false)
    }
    async fn issue_exists(&self, _: &str, _: &str) -> Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(GhCall::IssueExists);
        Ok(false)
    }
    async fn create_pr<'a>(&'a self, args: CreatePrArgs<'a>) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(GhCall::CreatePr {
                head: args.head.to_owned(),
                draft: args.draft,
                label_count: args.labels.len(),
            });
        Ok(())
    }
    async fn create_issue<'a>(
        &'a self,
        _: &'a str,
        _: &'a [String],
        title: &'a str,
        body: &'a str,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(GhCall::CreateIssue {
                title: title.to_owned(),
                body_has_trace: body.contains("<!-- agentic-"),
            });
        Ok(())
    }
}

struct ChainBench(HashMap<i64, i64>);
impl BenchClient for ChainBench {
    fn total_duration_us(&self, run_id: i64) -> Result<Option<i64>> {
        Ok(self.0.get(&run_id).copied())
    }
    fn invoke(&self, _o: stacks_bench_agent::session::bench::InvokeOptions<'_>) -> Result<()> {
        unimplemented!("chain test does not drive bench invoke")
    }
}

fn fixture_session_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session"))
}

fn id() -> SessionId {
    "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap()
}

/// Stage a writable copy of the fixture session + a synthetic framework dir
/// containing the prompt templates each phase needs. Returns the layout, the
/// session layout, and the temp dir guard.
fn stage(tmp: &tempfile::TempDir) -> (Layout, SessionLayout) {
    let id = id();
    let sessions_root = tmp.path().join("sessions");
    let session = SessionLayout::new(&sessions_root, id.clone());
    session
        .create_all_phase_dirs()
        .unwrap();
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", fixture_session_dir().display()))
        .arg(&session.results_dir)
        .status()
        .unwrap();
    assert!(status.success());

    // Wipe per-target outputs to force the chain to regenerate them.
    // (Phase 2 writes here; the fixture seeded a few targets with
    // implementation.md/abort.md/consensus-issue.md to exercise the
    // post-merge routing, but the actual run must produce its own.)
    let exp_dir = session.optimize_dir();
    let _ = std::fs::remove_dir_all(&exp_dir);
    std::fs::create_dir_all(&exp_dir).unwrap();

    // Synthetic framework. Templates ship inside the binary now (Askama),
    // so this dir only needs prompts/+schemas/ to exist for the layout
    // validator and operator-editable reference docs to live in (the
    // tests don't touch those references).
    let framework = tmp.path().join("framework");
    let prompts = framework.join("prompts");
    let schemas = framework.join("schemas");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::create_dir_all(&schemas).unwrap();
    // Seed the bundled context docs — the orchestrator's required-doc
    // startup check (added in Codex-review pass) fails when these are
    // missing from disk.
    stacks_bench_agent::context::seed_to(&framework.join("context")).unwrap();
    let base = framework
        .join("repos")
        .join("stacks-core");
    std::fs::create_dir_all(&base).unwrap();

    let layout = Layout {
        framework: Some(FrameworkDir::new(framework.clone())),
        schemas_dir: framework.join("schemas"),
        queries_dir: framework.join("queries"),
        context_dir: framework.join("context"),
        memory_dir: framework.join("memory"),
        sessions_root,
        stacks_bench_data_dir: tmp.path().join("data"),
        bench_lock: tmp.path().join("bench.lock"),
        test_lock: tmp.path().join("test.lock"),
        base: Some(base),
        stacks_bench_shadow_dir: None,
        agent_workspace_root: None,
    };
    (layout, session)
}

#[tokio::test]
async fn post_merge_chain_optimizers_finalize_publish() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, session) = stage(&tmp);

    // Phase 2: optimizers fan out across all 3 fixture targets.
    let harness = Arc::new(ChainHarness::new());
    let prompts_dir = tmp.path().join("prompts");
    stacks_bench_agent::prompts::seed_to(&prompts_dir).expect("seed prompts");
    let settings = Settings {
        prompt_overrides_dir: Some(prompts_dir),
        ..Settings::default()
    };
    let optimizers_outputs = optimizers::run(optimizers::Inputs {
        layout: session.clone(),
        framework: layout.clone(),
        settings: settings.clone(),
        parallel: Some(2),
        base_branch: "feat/stacks-bench".to_owned(),
        harness: harness.clone(),
        git: Arc::new(ChainGit),
    })
    .await
    .expect("optimizers::run");
    assert_eq!(optimizers_outputs.total, 3);
    // marf-read-cache + clarity-cost → optimizer-driven impl.md;
    // marf-layout-redesign → consensus_issue (orchestrator-emitted marker).
    assert_eq!(optimizers_outputs.landed, 2, "optimizers={optimizers_outputs:?}");
    assert_eq!(optimizers_outputs.routed_to_issue, 1);

    // Substitute Phase 3: stage run-ids + canned bench data so finalize can
    // compute durations. Only normal_pr targets need run-ids in the fixture.
    let pr_target = "marf-read-cache-rollback-wrapper";
    let exp = session.experiment_dir(pr_target);
    std::fs::write(exp.join("run-ids"), "500\n501\n").unwrap();

    // Phase 4: finalize.
    let mut canned = HashMap::new();
    canned.insert(100i64, 1_010_000i64);
    canned.insert(101, 990_000);
    canned.insert(500, 940_000);
    canned.insert(501, 950_000);
    let bench = ChainBench(canned);
    let summary = finalize(&FinalizeInputs {
        layout: &session,
        bench: &bench,
    })
    .expect("finalize");
    assert_eq!(
        summary
            .outcome_counts
            .normal_pr
            .accepted,
        1
    );
    assert!(
        session
            .summary_json()
            .is_file()
    );
    assert!(session.summary_md().is_file());

    // Phase 5a: publish::generate. Drives pr-writer for the 2 PR targets and
    // issue-writer for the 1 consensus_issue target.
    let gen_outputs = publish::generate(&GenerateInputs {
        layout: &session,
        framework: &layout,
        settings: &settings,
        harness: harness.as_ref(),
    })
    .await
    .expect("publish::generate");
    // Both PR targets ship: normal_pr (accepted) + consensus_poc_pr (gated
    // only on implementation.md, which the optimizer fake wrote).
    assert_eq!(gen_outputs.pr_count, 2);
    assert_eq!(gen_outputs.issue_count, 1);
    let kinds_called: Vec<_> = harness
        .invocations
        .lock()
        .unwrap()
        .iter()
        .map(|s| {
            s.split(':')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(
        kinds_called
            .iter()
            .any(|k| k == "pr-writer")
    );
    assert!(
        kinds_called
            .iter()
            .any(|k| k == "issue-writer")
    );

    // Stage worktrees for PR targets so push() can locate them.
    let worktrees = layout.session_worktrees_dir(&id());
    std::fs::create_dir_all(worktrees.join(pr_target)).unwrap();
    std::fs::create_dir_all(worktrees.join("clarity-cost-recalibration")).unwrap();

    // Phase 5b: publish::push.
    let token_path = tmp.path().join("token");
    std::fs::write(&token_path, "secret").unwrap();
    let cfg = PublishConfig {
        publish_remote: "origin".to_owned(),
        publish_base_repo: "cylewitruk/stacks-core".to_owned(),
        publish_base_branch: "feat/stacks-bench".to_owned(),
        publish_draft_prs: true,
        publish_pr_labels: vec![],
        publish_branch_prefix: "agentic".to_owned(),
        publish_token_file: token_path,
        publish_head_owner: Some("cylewitruk".to_owned()),
    };
    let gh = ChainGh::default();
    publish::push(&PushInputs {
        layout: &session,
        framework: &layout,
        config: &cfg,
        gh: &gh,
    })
    .await
    .expect("publish::push");

    let calls = gh.calls.lock().unwrap();
    let issue_calls: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            GhCall::CreateIssue { body_has_trace, title } => Some((*body_has_trace, title.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(issue_calls.len(), 1, "exactly one consensus_issue should ship");
    assert!(issue_calls[0].0, "issue body must carry the agentic trace tag");

    let pushed_branches: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            GhCall::Push(b) => Some(b.clone()),
            _ => None,
        })
        .collect();
    assert!(
        pushed_branches
            .iter()
            .any(|b| b.contains(pr_target))
    );
}
