//! Tests for `publish::push`. Drive the orchestrator with a `FakeGh` that
//! records calls; verify per-mode dispatch (`normal_pr`, `consensus_poc_pr`,
//! `consensus_issue`), idempotent skip, and the issue-body trace tag.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use stacks_bench_agent::layout::{FrameworkDir, Layout};
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::publish::{
    CreatePrArgs, GhClient, PublishConfig, PushInputs, push,
};
use stacks_bench_agent::types::SessionId;

/// One recorded call against the fake gh client. Strings only, so assertions
/// are easy.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    PrExists {
        repo: String,
        head_owner: String,
        branch: String,
        base: String,
    },
    IssueExists {
        repo: String,
        trace_tag: String,
    },
    SwitchBranch {
        worktree: PathBuf,
        branch: String,
    },
    AddModified {
        worktree: PathBuf,
    },
    Commit {
        worktree: PathBuf,
        message: String,
    },
    Push {
        worktree: PathBuf,
        remote: String,
        branch: String,
    },
    CreatePr {
        repo: String,
        base: String,
        head: String,
        draft: bool,
        labels: Vec<String>,
        title: String,
    },
    CreateIssue {
        repo: String,
        labels: Vec<String>,
        title: String,
        body: String,
    },
}

#[derive(Default)]
struct FakeGh {
    calls: Mutex<Vec<Call>>,
    /// When set, [`pr_exists`] returns `true` for any call.
    pr_exists_returns: bool,
    /// When set, [`issue_exists`] returns `true` for any call.
    issue_exists_returns: bool,
    remote_url: String,
}

impl GhClient for FakeGh {
    fn worktree_remote_url(&self, _worktree: &Path, _remote: &str) -> Result<String> {
        Ok(self.remote_url.clone())
    }
    fn switch_branch(&self, worktree: &Path, branch: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::SwitchBranch {
                worktree: worktree.to_path_buf(),
                branch: branch.to_owned(),
            });
        Ok(())
    }
    fn add_modified(&self, worktree: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::AddModified {
                worktree: worktree.to_path_buf(),
            });
        Ok(())
    }
    fn commit_if_staged(&self, worktree: &Path, message: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Commit {
                worktree: worktree.to_path_buf(),
                message: message.to_owned(),
            });
        Ok(())
    }
    fn push_branch(&self, worktree: &Path, remote: &str, branch: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Push {
                worktree: worktree.to_path_buf(),
                remote: remote.to_owned(),
                branch: branch.to_owned(),
            });
        Ok(())
    }
    async fn pr_exists(
        &self,
        repo: &str,
        head_owner: &str,
        branch: &str,
        base: &str,
    ) -> Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::PrExists {
                repo: repo.to_owned(),
                head_owner: head_owner.to_owned(),
                branch: branch.to_owned(),
                base: base.to_owned(),
            });
        Ok(self.pr_exists_returns)
    }
    async fn issue_exists(&self, repo: &str, trace_tag: &str) -> Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::IssueExists {
                repo: repo.to_owned(),
                trace_tag: trace_tag.to_owned(),
            });
        Ok(self.issue_exists_returns)
    }
    async fn create_pr<'a>(&'a self, args: CreatePrArgs<'a>) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::CreatePr {
                repo: args.repo.to_owned(),
                base: args.base.to_owned(),
                head: args.head.to_owned(),
                draft: args.draft,
                labels: args.labels.to_vec(),
                title: args.title.to_owned(),
            });
        Ok(())
    }
    async fn create_issue<'a>(
        &'a self,
        repo: &'a str,
        labels: &'a [String],
        title: &'a str,
        body: &'a str,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::CreateIssue {
                repo: repo.to_owned(),
                labels: labels.to_vec(),
                title: title.to_owned(),
                body: body.to_owned(),
            });
        Ok(())
    }
}

fn fixture_session_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session"))
}

/// Stage the fixture session under `<tmp>/sessions/<id>/results` and return
/// (Layout, SessionLayout). Also stages publish artifacts (`pr-*`, `issue-*`)
/// for each target so push() finds them.
fn stage(tmp: &tempfile::TempDir, id: &SessionId) -> (Layout, SessionLayout) {
    // sessions root
    let sessions_root = tmp.path().join("sessions");
    let session_layout = SessionLayout::new(&sessions_root, id.clone());
    session_layout
        .create_all_phase_dirs()
        .unwrap();
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", fixture_session_dir().display()))
        .arg(&session_layout.results_dir)
        .status()
        .unwrap();
    assert!(status.success());

    // Stage publish artifacts per target
    for (id, mode) in [
        ("marf-read-cache-rollback-wrapper", "pr"),
        ("clarity-cost-recalibration", "pr"),
        ("marf-layout-redesign", "issue"),
    ] {
        let exp = session_layout.experiment_dir(id);
        match mode {
            "pr" => {
                std::fs::write(exp.join("pr-title.txt"), "perf: optimize foo\n").unwrap();
                std::fs::write(exp.join("pr-body.md"), "## Summary\n\nbody\n").unwrap();
            }
            "issue" => {
                std::fs::write(exp.join("issue-title.txt"), "consensus: marf layout\n").unwrap();
                std::fs::write(exp.join("issue-body.md"), "## Summary\n\nissue body\n").unwrap();
            }
            _ => unreachable!(),
        }
    }

    // Stage worktrees for the PR targets (push() asserts they're dirs).
    let worktrees = session_layout
        .worktrees_dir
        .clone();
    std::fs::create_dir_all(worktrees.join("marf-read-cache-rollback-wrapper")).unwrap();
    std::fs::create_dir_all(worktrees.join("clarity-cost-recalibration")).unwrap();

    // Build a Layout pointing at the staged tree.
    let framework = tmp.path().join("framework");
    std::fs::create_dir_all(framework.join("prompts")).unwrap();
    std::fs::create_dir_all(framework.join("schemas")).unwrap();
    let base = tmp.path().join("base");
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
        operator_repo_root: None,
    };
    (layout, session_layout)
}

fn config_with(token_file: PathBuf) -> PublishConfig {
    PublishConfig {
        publish_remote: "origin".to_owned(),
        publish_base_repo: "cylewitruk/stacks-core".to_owned(),
        publish_base_branch: "feat/stacks-bench".to_owned(),
        publish_draft_prs: true,
        publish_pr_labels: vec![],
        publish_branch_prefix: "agentic".to_owned(),
        publish_token_file: token_file,
        publish_head_owner: Some("cylewitruk".to_owned()),
    }
}

fn write_token(tmp: &tempfile::TempDir) -> PathBuf {
    let p = tmp.path().join("token");
    std::fs::write(&p, "secret").unwrap();
    p
}

fn id() -> SessionId {
    "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap()
}

#[tokio::test]
async fn push_creates_pr_for_normal_pr_and_consensus_poc_pr() {
    let tmp = tempfile::tempdir().unwrap();
    let id = id();
    let (layout, sl) = stage(&tmp, &id);
    let token = write_token(&tmp);
    let cfg = config_with(token);
    let gh = FakeGh::default();
    let outputs = push(&PushInputs {
        layout: &sl,
        framework: &layout,
        config: &cfg,
        gh: &gh,
    })
    .await
    .expect("push");

    assert_eq!(outputs.pr_count, 2);
    assert_eq!(outputs.issue_count, 1);
    assert_eq!(outputs.skip_count, 0);

    let calls = gh.calls.lock().unwrap();

    // Both PR targets should reach create_pr.
    let create_prs: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            Call::CreatePr { head, draft, labels, .. } => {
                Some((head.clone(), *draft, labels.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(create_prs.len(), 2);

    // consensus_poc_pr is forced draft + carries the safety labels.
    let poc_pr = create_prs
        .iter()
        .find(|(h, ..)| h.contains("clarity-cost-recalibration"))
        .expect("consensus_poc_pr CreatePr call missing");
    assert!(poc_pr.1, "consensus_poc_pr must be draft");
    let labels: Vec<_> = poc_pr
        .2
        .iter()
        .map(String::as_str)
        .collect();
    assert!(labels.contains(&"consensus-change"));
    assert!(labels.contains(&"needs-HIP"));
    assert!(labels.contains(&"do-not-merge"));

    // Branch follows the `<prefix>/<session>/<target>` shape.
    let pushed: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            Call::Push { branch, .. } => Some(branch.clone()),
            _ => None,
        })
        .collect();
    assert!(
        pushed
            .iter()
            .any(|b| b == "agentic/20260507-104400/marf-read-cache-rollback-wrapper")
    );
    assert!(
        pushed
            .iter()
            .any(|b| b == "agentic/20260507-104400/clarity-cost-recalibration")
    );
}

#[tokio::test]
async fn push_creates_issue_with_trace_tag_in_body() {
    let tmp = tempfile::tempdir().unwrap();
    let id = id();
    let (layout, sl) = stage(&tmp, &id);
    let token = write_token(&tmp);
    let cfg = config_with(token);
    let gh = FakeGh::default();
    push(&PushInputs {
        layout: &sl,
        framework: &layout,
        config: &cfg,
        gh: &gh,
    })
    .await
    .unwrap();

    let calls = gh.calls.lock().unwrap();
    let issue = calls
        .iter()
        .find_map(|c| match c {
            Call::CreateIssue { title, body, labels, .. } => {
                Some((title.clone(), body.clone(), labels.clone()))
            }
            _ => None,
        })
        .expect("create_issue call missing");
    assert!(issue.0.contains("consensus"));
    assert!(
        issue
            .1
            .contains("<!-- agentic-20260507-104400-marf-layout-redesign -->"),
        "body must carry the hidden trace tag"
    );
    assert!(
        issue
            .2
            .contains(&"consensus-change".to_owned())
    );
    assert!(
        issue
            .2
            .contains(&"needs-HIP".to_owned())
    );
}

#[tokio::test]
async fn push_skips_git_ops_when_pr_already_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let id = id();
    let (layout, sl) = stage(&tmp, &id);
    let token = write_token(&tmp);
    let cfg = config_with(token);
    let gh = FakeGh {
        pr_exists_returns: true,
        ..Default::default()
    };
    let outputs = push(&PushInputs {
        layout: &sl,
        framework: &layout,
        config: &cfg,
        gh: &gh,
    })
    .await
    .expect("push");

    // Issue path still runs (not gated on pr_exists). Only PR git ops are skipped.
    assert_eq!(outputs.issue_count, 1);
    assert_eq!(outputs.pr_count, 2, "skip is silent: pr_count still increments");

    let calls = gh.calls.lock().unwrap();
    assert!(
        !calls.iter().any(|c| matches!(
            c,
            Call::SwitchBranch { .. } | Call::Push { .. } | Call::CreatePr { .. }
        )),
        "no git/PR ops should occur when pr_exists returned true; calls={calls:#?}"
    );
}

#[tokio::test]
async fn push_skips_issue_when_issue_already_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let id = id();
    let (layout, sl) = stage(&tmp, &id);
    let token = write_token(&tmp);
    let cfg = config_with(token);
    let gh = FakeGh {
        issue_exists_returns: true,
        ..Default::default()
    };
    push(&PushInputs {
        layout: &sl,
        framework: &layout,
        config: &cfg,
        gh: &gh,
    })
    .await
    .unwrap();

    let calls = gh.calls.lock().unwrap();
    assert!(
        !calls
            .iter()
            .any(|c| matches!(c, Call::CreateIssue { .. })),
        "no issue should be created when issue_exists returned true"
    );
}

#[test]
fn read_publish_token_fails_clearly_when_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("does-not-exist");
    let err = stacks_bench_agent::session::publish::read_publish_token(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("reading token file"), "msg={msg}");
}

#[test]
fn read_publish_token_rejects_empty_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("empty");
    std::fs::write(&path, "").unwrap();
    let err = stacks_bench_agent::session::publish::read_publish_token(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("is empty"), "msg={msg}");
}

/// Pass-(a.5) regression: when `agent_workspace_root` is set, Phase 5
/// publish must resolve each target's checkout through
/// `Layout::session_optimizer_checkouts_dir` — i.e. read from
/// `<workspace_root>/optimizers/<session>/<target>/`, NOT the legacy
/// `<sessions_root>/<id>/worktrees/<target>/` path. Without this test,
/// a future refactor that re-introduced the legacy path inside
/// `publish::push` (or `push_pr`) would silently pass — the other
/// publish tests stage worktrees at the legacy location and use
/// `agent_workspace_root: None`, so they don't exercise the
/// workspace-root branch at all.
#[tokio::test]
async fn push_resolves_checkouts_through_agent_workspace_root_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    let id = id();

    // Stage the session results dir under <tmp>/sessions/<id>/results
    // (durable artifacts continue to live there regardless of
    // workspace-root setting).
    let sessions_root = tmp.path().join("sessions");
    let session_layout = SessionLayout::new(&sessions_root, id.clone());
    session_layout
        .create_all_phase_dirs()
        .unwrap();
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", fixture_session_dir().display()))
        .arg(&session_layout.results_dir)
        .status()
        .unwrap();
    assert!(status.success());
    // Stage publish artifacts for the two PR targets the fixture
    // session expects.
    for target_id in ["marf-read-cache-rollback-wrapper", "clarity-cost-recalibration"] {
        let exp = session_layout.experiment_dir(target_id);
        std::fs::write(exp.join("pr-title.txt"), "perf: optimize foo\n").unwrap();
        std::fs::write(exp.join("pr-body.md"), "## Summary\n\nbody\n").unwrap();
    }
    // Issue target's artifacts (so the fixture session's third target
    // doesn't trip the missing-artifacts check).
    let issue_exp = session_layout.experiment_dir("marf-layout-redesign");
    std::fs::write(issue_exp.join("issue-title.txt"), "consensus: marf layout\n").unwrap();
    std::fs::write(issue_exp.join("issue-body.md"), "## Summary\n\nbody\n").unwrap();

    // Critically: stage the per-target checkouts UNDER the workspace
    // root, NOT under the session's worktrees_dir.
    let workspace_root = tmp.path().join("ws");
    let external_checkouts = workspace_root
        .join("optimizers")
        .join(id.as_str());
    std::fs::create_dir_all(external_checkouts.join("marf-read-cache-rollback-wrapper")).unwrap();
    std::fs::create_dir_all(external_checkouts.join("clarity-cost-recalibration")).unwrap();
    // And do NOT stage anything under session_layout.worktrees_dir.
    // If publish silently drifts back to that path, it'll fail loudly
    // with "missing worktree" (a stronger signal than the workspace
    // path just happening to exist would give).

    let framework = tmp.path().join("framework");
    std::fs::create_dir_all(framework.join("prompts")).unwrap();
    std::fs::create_dir_all(framework.join("schemas")).unwrap();
    let base = tmp.path().join("base");
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
        agent_workspace_root: Some(workspace_root.clone()),
        operator_repo_root: None,
    };

    let token = write_token(&tmp);
    let cfg = config_with(token);
    let gh = FakeGh::default();
    let outputs = push(&PushInputs {
        layout: &session_layout,
        framework: &layout,
        config: &cfg,
        gh: &gh,
    })
    .await
    .expect("push must succeed when checkouts staged under workspace root");

    assert_eq!(outputs.pr_count, 2, "two PRs expected; got {outputs:?}");

    // Every recorded git op MUST carry a worktree path under the
    // external workspace root, AND none under the legacy
    // sessions/<id>/worktrees/ path.
    let legacy_root = session_layout
        .worktrees_dir
        .clone();
    let calls = gh.calls.lock().unwrap();
    let git_op_paths: Vec<PathBuf> = calls
        .iter()
        .filter_map(|c| match c {
            Call::SwitchBranch { worktree, .. }
            | Call::AddModified { worktree }
            | Call::Commit { worktree, .. }
            | Call::Push { worktree, .. } => Some(worktree.clone()),
            _ => None,
        })
        .collect();
    assert!(!git_op_paths.is_empty(), "push didn't perform any git ops; got {calls:#?}",);
    for path in &git_op_paths {
        assert!(
            path.starts_with(&external_checkouts),
            "git op against {path:?} should be under {external_checkouts:?}; Phase 5 silently \
             drifted back to the legacy path",
        );
        assert!(
            !path.starts_with(&legacy_root),
            "git op against {path:?} resolved to legacy sessions/worktrees path instead of \
             agent_workspace_root",
        );
    }
}
