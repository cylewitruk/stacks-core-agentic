//! Post-v3-cutover `sbagent init` regression tests.
//!
//! Contract:
//! - `init` no longer adds a submodule, no longer writes `.gitmodules`, and no
//!   longer carries `<base>` in the initial commit.
//! - The initial commit covers only the operator-owned `.sbagent/` bundle
//!   mirrors plus `.gitignore`.
//! - Pre-existing untracked files in the target dir are NOT swept into the
//!   initial commit (`git add -A` discipline).

use std::path::{Path, PathBuf};

use stacks_bench_agent::cli::init::{InitArgs, run as init_run};
use stacks_bench_agent::settings::Settings;

fn run_git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {:?} failed in {}", args, dir.display());
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {:?} failed", args);
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .to_owned()
}

/// Minimal settings for init: prompt_overrides_dir + git identity.
/// `[source]` is NOT required by init (it's read at session-start
/// preflight); leaving it unset confirms init no longer depends on
/// upstream source config.
fn settings_for(target: &Path) -> Settings {
    let prompts = target
        .join(".sbagent")
        .join("prompts");
    let config_path = target.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[layout]\nprompt_overrides_dir = \"{}\"\n\n[git]\nauthor_name = \"Test \
             Bot\"\nauthor_email = \"bot@example.com\"\n",
            prompts.display(),
        ),
    )
    .unwrap();
    Settings::load(Some(&config_path)).expect("load settings")
}

#[tokio::test]
async fn init_writes_no_gitmodules_and_no_submodule_pointer() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path();
    let settings = settings_for(target);

    init_run(
        InitArgs {
            dir: Some(target.to_path_buf()),
            push: false,
            push_branch: "main".into(),
        },
        &settings,
    )
    .await
    .expect("init");

    // No .gitmodules at the operator root.
    assert!(
        !target
            .join(".gitmodules")
            .exists(),
        ".gitmodules must not be written post-cutover"
    );

    // No `repos/` subtree (the legacy submodule lived at `repos/stacks-core`).
    assert!(!target.join("repos").exists(), "`repos/` subtree must not be created post-cutover",);

    // The initial commit must not carry a submodule entry. `git
    // ls-tree HEAD` would show a `160000` mode line for any submodule
    // pointer; assert none are present.
    let tree = git_stdout(target, &["ls-tree", "-r", "HEAD"]);
    assert!(!tree.contains("160000"), "initial commit carries a submodule pointer:\n{tree}",);
}

#[tokio::test]
async fn init_commits_only_sbagent_bundle_and_gitignore() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path();
    let settings = settings_for(target);

    init_run(
        InitArgs {
            dir: Some(target.to_path_buf()),
            push: false,
            push_branch: "main".into(),
        },
        &settings,
    )
    .await
    .expect("init");

    let tree = git_stdout(target, &["ls-tree", "-r", "--name-only", "HEAD"]);
    let mut tracked: Vec<&str> = tree.lines().collect();
    tracked.sort_unstable();

    // Every tracked file should live under `.sbagent/` or be the
    // operator-owned `.gitignore`. Anything else (e.g. `repos/`,
    // `.gitmodules`) signals a regression.
    for path in &tracked {
        assert!(
            *path == ".gitignore" || path.starts_with(".sbagent/"),
            "unexpected file in initial commit: {path}\nfull tree:\n{tree}",
        );
    }

    // Sanity: the four bundle subdirs all materialized.
    for sub in ["prompts", "schemas", "queries", "context"] {
        assert!(
            tracked
                .iter()
                .any(|p| p.starts_with(&format!(".sbagent/{sub}/"))),
            "`.sbagent/{sub}/` must be in the initial commit; tree:\n{tree}",
        );
    }
}

#[tokio::test]
async fn init_does_not_sweep_pre_existing_untracked_files() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path();

    // Drop a stray file BEFORE init runs. `git add -A` would commit
    // it; the explicit pathspec list must not.
    std::fs::write(target.join("README.md"), "operator's own readme").unwrap();
    let stray = target.join("notes.txt");
    std::fs::write(&stray, "private operator notes").unwrap();

    let settings = settings_for(target);
    init_run(
        InitArgs {
            dir: Some(target.to_path_buf()),
            push: false,
            push_branch: "main".into(),
        },
        &settings,
    )
    .await
    .expect("init");

    let tree = git_stdout(target, &["ls-tree", "-r", "--name-only", "HEAD"]);
    // Substring check is unsafe: the bundled queries dir contains its
    // own `README.md`. Verify by line equality at the operator root.
    let tracked: Vec<&str> = tree.lines().collect();
    assert!(
        !tracked.contains(&"README.md"),
        "operator-root README.md must remain untracked, tree:\n{tree}",
    );
    assert!(
        !tracked.contains(&"notes.txt"),
        "operator-root notes.txt must remain untracked, tree:\n{tree}",
    );

    // Both files still exist on disk (init didn't delete them).
    assert!(
        target
            .join("README.md")
            .is_file()
    );
    assert!(stray.is_file());
}

#[tokio::test]
async fn init_is_idempotent_on_re_run() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path();
    let settings = settings_for(target);

    let args = || InitArgs {
        dir: Some(target.to_path_buf()),
        push: false,
        push_branch: "main".into(),
    };
    init_run(args(), &settings)
        .await
        .expect("first init");
    let first_head = git_stdout(target, &["rev-parse", "HEAD"]);

    // Second run must not commit again (re-run safe: nothing new to stage).
    init_run(args(), &settings)
        .await
        .expect("second init");
    let second_head = git_stdout(target, &["rev-parse", "HEAD"]);

    assert_eq!(first_head, second_head, "re-running init must not create a new commit");
}

#[tokio::test]
async fn init_gitignore_template_no_longer_references_submodule_target() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path();
    let settings = settings_for(target);

    init_run(
        InitArgs {
            dir: Some(target.to_path_buf()),
            push: false,
            push_branch: "main".into(),
        },
        &settings,
    )
    .await
    .expect("init");

    let body = std::fs::read_to_string(target.join(".gitignore")).expect(".gitignore");
    assert!(
        !body.contains("repos/") && !body.contains("/target"),
        "post-cutover .gitignore must not reference the gone submodule's `repos/<base>/target/` \
         path. Body:\n{body}",
    );
}

/// `sbagent source cache-id` matches the resolver's output for a
/// derived id (no `[source].id` pinned).
#[tokio::test]
async fn source_cache_id_matches_resolver_derived() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path();
    let prompts = target
        .join(".sbagent")
        .join("prompts");
    let url = "https://github.com/stacks-network/stacks-core.git";
    let config_path = target.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[layout]\nprompt_overrides_dir = \"{}\"\n\n[source]\nurl = \"{url}\"\nbranch = \
             \"develop\"\n",
            prompts.display(),
        ),
    )
    .unwrap();

    let expected = stacks_bench_agent::source::resolve_cache_id(None, url).expect("resolver");
    let printed = exec_source_cache_id(&config_path);
    assert_eq!(printed, expected, "CLI output must match resolver");
}

/// `[source].id` (when set) is echoed verbatim instead of derived.
#[tokio::test]
async fn source_cache_id_echoes_pinned_id() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path();
    let prompts = target
        .join(".sbagent")
        .join("prompts");
    let config_path = target.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[layout]\nprompt_overrides_dir = \"{}\"\n\n\
             [source]\nurl = \"https://example.com/x.git\"\nbranch = \"main\"\nid = \
             \"stacks-core-upstream\"\n",
            prompts.display(),
        ),
    )
    .unwrap();

    let printed = exec_source_cache_id(&config_path);
    assert_eq!(printed, "stacks-core-upstream", "pinned id should be echoed verbatim");
}

/// `sbagent source cache-id` fails loud when `[source].url` is missing
/// — the migration recipe in setup.md assumes the operator has set
/// `[source]` before running this helper.
#[tokio::test]
async fn source_cache_id_fails_loud_when_source_url_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path();
    let prompts = target
        .join(".sbagent")
        .join("prompts");
    let config_path = target.join("config.toml");
    std::fs::write(
        &config_path,
        format!("[layout]\nprompt_overrides_dir = \"{}\"\n", prompts.display()),
    )
    .unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(["-c", config_path.to_str().unwrap(), "source", "cache-id"])
        .output()
        .expect("spawn sbagent");
    assert!(!out.status.success(), "expected failure when [source].url missing");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[source].url") || stderr.contains("source.url"),
        "stderr should pinpoint missing [source].url; got:\n{stderr}",
    );
}

/// Helper: invoke `sbagent source cache-id` via the test binary
/// fixture and return stdout (trimmed).
fn exec_source_cache_id(config_path: &Path) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(["-c", config_path.to_str().unwrap(), "source", "cache-id"])
        .output()
        .expect("spawn sbagent");
    assert!(
        out.status.success(),
        "sbagent source cache-id failed:\nstderr={}\nstdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .to_owned()
}

#[allow(dead_code)]
fn _unused_imports_helper() {
    // Suppress unused-import warnings for helpers some test bodies don't reach.
    let _ = run_git as fn(&Path, &[&str]);
    let _: PathBuf = PathBuf::new();
}
