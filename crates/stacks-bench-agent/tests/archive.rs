//! Integration test for `crate::session::archive` against a real
//! tempdir-backed git repo. Uses `dry_run: true` so no PAT is needed
//! and the test stays hermetic (no network).
//!
//! What's exercised end-to-end:
//! - operator-repo state validation (clean worktree, non-archive branch)
//! - the force-add path that bypasses main's `/sessions/` gitignore
//! - typed ledger line append + commit
//! - idempotency on re-run

use std::path::Path;
use std::process::Command;

use stacks_bench_agent::layout::Layout;
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::archive::{ArchiveInputs, archive};
use stacks_bench_agent::settings::Settings;
use stacks_bench_agent::types::SessionId;

fn fixture_session_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session"))
}

/// Init a git repo at `dir` with bot identity + signing off + an
/// initial commit so the working tree has a valid HEAD.
fn init_operator_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "bot@test"]);
    git(dir, &["config", "user.name", "bot"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join(".gitignore"), "/sessions/\n").unwrap();
    git(dir, &["add", ".gitignore"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn git_output(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?} failed in {}", dir.display());
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .to_owned()
}

/// Stage the fixture session bulk into the WORKSPACE path. Returns
/// `(session_layout, workspace)` so callers can pass both into the
/// Layout builder. The workspace sits outside the operator tree, as
/// the archive flow now requires (the legacy `<operator>/sessions/`
/// layout is rejected by `archive_worktree_path` to prevent the
/// branch-switch wipe hazard).
fn stage_session_in_workspace(workspace: &Path, id: &SessionId) -> SessionLayout {
    let sessions_root = workspace.join("sessions");
    let layout = SessionLayout::new(&sessions_root, id.clone());
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

fn build_layout(operator: &Path, workspace: &Path) -> Layout {
    Layout {
        framework: None,
        schemas_dir: operator.join(".schemas"),
        queries_dir: operator.join(".queries"),
        context_dir: operator.join(".context"),
        memory_dir: operator.join("memory"),
        sessions_root: workspace.join("sessions"),
        stacks_bench_data_dir: operator.join("data"),
        bench_lock: operator.join(".lock-bench"),
        test_lock: operator.join(".lock-test"),
        base: None,
        stacks_bench_shadow_dir: None,
        agent_workspace_root: Some(workspace.to_path_buf()),
        operator_repo_root: Some(operator.to_path_buf()),
    }
}

#[test]
fn archive_dry_run_creates_branch_and_ledger() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);

    let id: SessionId = "20260518-190321-test"
        .to_owned()
        .try_into()
        .unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_layout = stage_session_in_workspace(&workspace, &id);
    let layout = build_layout(&operator, &workspace);

    let outputs = archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .expect("archive should succeed");

    assert!(!outputs.already_archived);
    assert!(outputs.ledger_appended);
    assert_eq!(outputs.branch, "session/20260518-190321-test");

    // session/<id> branch exists.
    let branches = git_output(&operator, &["branch", "--list"]);
    assert!(branches.contains("session/20260518-190321-test"), "branches: {branches}");

    // The branch contains the bulk under sessions/<id>/.
    let session_files =
        git_output(&operator, &["ls-tree", "-r", "--name-only", "session/20260518-190321-test"]);
    assert!(
        session_files.contains("sessions/20260518-190321-test/"),
        "branch ls-tree did not include session bulk: {session_files}"
    );

    // main has the ledger commit.
    let ledger = std::fs::read_to_string(operator.join("sessions.jsonl")).unwrap();
    let lines: Vec<&str> = ledger
        .lines()
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "expected exactly one ledger line, got {lines:?}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["id"], "20260518-190321-test");
    assert_eq!(v["kind"], "session_completed");
    assert_eq!(v["schema_version"], 1);
}

#[test]
fn archive_is_idempotent_on_rerun() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);

    let id: SessionId = "20260518-200000-test"
        .to_owned()
        .try_into()
        .unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_layout = stage_session_in_workspace(&workspace, &id);
    let layout = build_layout(&operator, &workspace);

    archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .unwrap();

    // Second call should short-circuit as already_archived; ledger
    // length stays at 1.
    let second = archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .unwrap();
    assert!(second.already_archived);
    assert!(!second.ledger_appended);

    let ledger = std::fs::read_to_string(operator.join("sessions.jsonl")).unwrap();
    let lines: Vec<&str> = ledger
        .lines()
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "rerun should not add a second line: {lines:?}");
}

/// A corrupt `summary.json` (file present but unparseable) must
/// abort archive rather than silently fall through to an empty
/// targets array. Without this gate, a malformed source artifact
/// would mint a permanent `status=succeeded` ledger row with no
/// targets — pure audit-trail poison.
#[test]
fn archive_aborts_on_corrupt_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);

    let id: SessionId = "20260518-230000-test"
        .to_owned()
        .try_into()
        .unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_layout = stage_session_in_workspace(&workspace, &id);
    // Overwrite the staged summary.json with non-JSON content.
    std::fs::write(session_layout.summary_json(), "this is not json").unwrap();
    let layout = build_layout(&operator, &workspace);

    let err = archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .expect_err("corrupt summary.json must abort");
    let msg = format!("{err:#}");
    assert!(msg.contains("summary.json"), "error must mention summary.json: {msg}");

    // Critically: the operator repo must NOT have a half-finished
    // archive. No session/<id> branch, no sessions.jsonl.
    let branches = git_output(&operator, &["branch", "--list"]);
    assert!(
        !branches.contains("session/20260518-230000-test"),
        "no archive branch should have been created on failure: {branches}"
    );
    assert!(
        !operator
            .join("sessions.jsonl")
            .exists(),
        "no ledger file should have been created on failure"
    );
}

/// The `artifact_url` (when derivable) must be written onto the
/// `sessions.jsonl` line, not just shown in the CLI output. Without
/// this, the durable record claims `null` while the operator's
/// terminal flashes a URL — a confusing audit trail.
#[test]
fn archive_writes_artifact_url_into_ledger() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);
    // Configure an `origin` pointing at an HTTPS GitHub URL so
    // derive_artifact_url returns Some(...). The remote doesn't need
    // to actually resolve — dry_run skips the push.
    git(&operator, &["remote", "add", "origin", "https://github.com/owner/repo.git"]);

    let id: SessionId = "20260519-000000-test"
        .to_owned()
        .try_into()
        .unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_layout = stage_session_in_workspace(&workspace, &id);
    let layout = build_layout(&operator, &workspace);

    let outputs = archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .unwrap();

    assert_eq!(
        outputs
            .artifact_url
            .as_deref(),
        Some("https://github.com/owner/repo/tree/session/20260519-000000-test"),
        "outputs should carry the URL"
    );

    let ledger = std::fs::read_to_string(operator.join("sessions.jsonl")).unwrap();
    let line = ledger.lines().next().unwrap();
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        v["artifact_url"].as_str(),
        Some("https://github.com/owner/repo/tree/session/20260519-000000-test"),
        "ledger line must carry the SAME URL the outputs do"
    );
}

/// Load-bearing invariant from the workspace refactor: after
/// archive, the operator's MAIN worktree must NOT contain
/// `sessions/<id>/`. The archive branch ops happen in a separate
/// `git worktree`, so switching branches in the main worktree can't
/// wipe the bulk anymore (that's the bug this refactor exists to fix).
#[test]
fn archive_leaves_operator_main_worktree_clean_of_session_bulk() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);

    let id: SessionId = "20260519-100000-test"
        .to_owned()
        .try_into()
        .unwrap();
    // Bulk lives OUTSIDE the operator under a workspace path, as it
    // does under the new agent_workspace_root layout.
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_layout = stage_session_in_workspace(&workspace, &id);
    let layout = build_layout(&operator, &workspace);

    archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .unwrap();

    // Operator main worktree: sessions/ must not exist (or must be
    // empty). The archive branch exists with the bulk, but main is
    // pristine.
    let operator_sessions = operator.join("sessions");
    if operator_sessions.exists() {
        let entries = std::fs::read_dir(&operator_sessions)
            .unwrap()
            .count();
        assert_eq!(
            entries, 0,
            "operator/sessions/ must be empty after archive; found {entries} entries"
        );
    }
    // The bulk in workspace stays put — archive copies, doesn't move.
    // (The fixture session lacks a finalize/ output by design — it's a
    // pre-finalize snapshot — so we pin on optimization-targets.json,
    // which the fixture does carry.)
    let probe = session_layout.optimization_targets_json();
    assert!(probe.exists(), "workspace bulk should survive archive; missing {probe:?}");
    // And the archive branch picks up the bulk under the in-tree
    // path sessions/<id>/.
    let session_files =
        git_output(&operator, &["ls-tree", "-r", "--name-only", "session/20260519-100000-test"]);
    assert!(
        session_files.contains("sessions/20260519-100000-test/"),
        "branch tree should still contain the in-tree bulk path: {session_files}"
    );
}

#[test]
fn archive_rejects_dirty_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);
    // Introduce a tracked-and-modified file (not ignored).
    std::fs::write(operator.join("README.md"), "before").unwrap();
    git(&operator, &["add", "README.md"]);
    git(&operator, &["commit", "-q", "-m", "seed"]);
    std::fs::write(operator.join("README.md"), "after").unwrap();

    let id: SessionId = "20260518-210000-test"
        .to_owned()
        .try_into()
        .unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_layout = stage_session_in_workspace(&workspace, &id);
    let layout = build_layout(&operator, &workspace);

    let err = archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .expect_err("dirty worktree must be rejected");
    assert!(
        format!("{err:#}").contains("uncommitted changes"),
        "error must mention dirty worktree: {err:#}"
    );
}

/// Codex re-review M1: a non-dry-run archive against an operator
/// repo with NO configured remote must succeed without reading
/// `publish_token_file`. Local-only operators (e.g. fresh
/// `sbagent init` not yet pushed anywhere) should be able to archive
/// without a PAT.
#[test]
fn archive_succeeds_locally_without_remote_or_token() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);
    // Deliberately: NO `git remote add ...`.

    let id: SessionId = "20260520-110000-test"
        .to_owned()
        .try_into()
        .unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_layout = stage_session_in_workspace(&workspace, &id);
    let layout = build_layout(&operator, &workspace);

    // Settings have no publish_token_file configured. Archive must
    // NOT try to read one, since there's nowhere to push.
    archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: false,
    })
    .expect("archive must succeed against a remote-less repo without a token");

    // Branch + ledger should still have been produced locally.
    let branches = git_output(&operator, &["branch", "--list"]);
    assert!(branches.contains("session/20260520-110000-test"), "branches: {branches}");
    assert!(
        operator
            .join("sessions.jsonl")
            .exists(),
        "ledger must be committed locally"
    );
}

/// Codex L1: when the resolved archive-worktree path would land
/// inside the operator repo (the legacy `<operator>/sessions/`
/// layout with no `agent_workspace_root`), archive must refuse with
/// a clear error rather than silently nest the worktree — which
/// re-introduces the branch-switch wipe hazard the worktree
/// refactor exists to prevent.
#[test]
fn archive_rejects_worktree_path_inside_operator() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);

    let id: SessionId = "20260520-090000-test"
        .to_owned()
        .try_into()
        .unwrap();
    // Stage bulk at the legacy path inside the operator. No
    // `agent_workspace_root` is set in the layout, so
    // `archive_worktree_path` falls back to
    // `<sessions_root>/.archive-worktrees/<id>` — which here is
    // inside the operator, the failure mode L1 guards against.
    let legacy_sessions = operator.join("sessions");
    let session_layout = SessionLayout::new(&legacy_sessions, id.clone());
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

    let layout = Layout {
        framework: None,
        schemas_dir: operator.join(".schemas"),
        queries_dir: operator.join(".queries"),
        context_dir: operator.join(".context"),
        memory_dir: operator.join("memory"),
        sessions_root: legacy_sessions,
        stacks_bench_data_dir: operator.join("data"),
        bench_lock: operator.join(".lock-bench"),
        test_lock: operator.join(".lock-test"),
        base: None,
        stacks_bench_shadow_dir: None,
        agent_workspace_root: None,
        operator_repo_root: Some(operator.to_path_buf()),
    };

    let err = archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .expect_err("archive must refuse a worktree path inside the operator");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("agent_workspace_root"),
        "error must direct operator to the agent_workspace_root setting: {msg}"
    );
}

#[test]
fn archive_requires_operator_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    init_operator_repo(&operator);

    let id: SessionId = "20260518-220000-test"
        .to_owned()
        .try_into()
        .unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session_layout = stage_session_in_workspace(&workspace, &id);
    // Build a layout that explicitly DOES NOT have operator_repo_root,
    // and where sessions_root.parent() is also outside any usable
    // repo. The auto-derivation in Layout::from_settings would pick
    // up a parent, so construct the literal here to exercise the
    // require_operator_repo_root error path directly.
    let layout = Layout {
        framework: None,
        schemas_dir: operator.join(".schemas"),
        queries_dir: operator.join(".queries"),
        context_dir: operator.join(".context"),
        memory_dir: operator.join("memory"),
        sessions_root: operator.join("sessions"),
        stacks_bench_data_dir: operator.join("data"),
        bench_lock: operator.join(".lock-bench"),
        test_lock: operator.join(".lock-test"),
        base: None,
        stacks_bench_shadow_dir: None,
        agent_workspace_root: None,
        operator_repo_root: None,
    };

    let err = archive(&ArchiveInputs {
        layout: &session_layout,
        framework: &layout,
        settings: &Settings::default(),
        dry_run: true,
    })
    .expect_err("missing operator_repo_root must be rejected");
    assert!(
        format!("{err:#}").contains("operator_repo_root"),
        "error must mention the missing setting: {err:#}"
    );
}
