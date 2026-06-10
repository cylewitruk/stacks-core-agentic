//! `sbagent source seed` integration tests.
//!
//! Exercise the subcommand end-to-end against `file://` fixtures —
//! no real GitHub. Covers:
//!
//! - Successful seed of a branch onto an empty bare destination.
//! - Re-running the same command is a no-op (fast-forward push).
//! - Plain `http://` `--to` errors with the HTTPS-required message.
//! - Unknown `--to` schemes (e.g. `ftp://`) error with the accepted-schemes
//!   list.
//! - HTTPS `--to` outside `git.auth_url_prefix` errors at `validate_auth_url`
//!   BEFORE any token-file read — pins the HTTPS-with-PAT branch without
//!   needing real credentials.
//! - Explicit `--branch` overrides the `[source].branch` default.
//! - `--help` text describes both `--from`/`--to` and the accepted URL forms.

use std::path::{Path, PathBuf};

/// Shared git args overlaid on every invocation: signing off (no key
/// in the test fixture) + bare-repo safety override (the operator's
/// local config may set `safe.bareRepository=explicit`, which would
/// otherwise reject reading from the fixture bare repos).
const COMMON_GIT_ARGS: &[&str] =
    &["-c", "commit.gpgsign=false", "-c", "tag.gpgSign=false", "-c", "safe.bareRepository=all"];

fn run_git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(COMMON_GIT_ARGS)
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("spawning git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn run_git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(COMMON_GIT_ARGS)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("spawning git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .to_owned()
}

/// Seed a fixture upstream bare repo with one branch + one commit.
/// Returns the bare-repo path. The branch is `feat/stacks-bench`.
fn make_source_bare(tmp_root: &Path) -> PathBuf {
    let bare = tmp_root.join("source.git");
    let seed = tmp_root.join("source-seed");
    std::fs::create_dir_all(&seed).unwrap();
    run_git(&seed, &["init", "-b", "feat/stacks-bench"]);
    run_git(&seed, &["config", "user.name", "Upstream Author"]);
    run_git(&seed, &["config", "user.email", "upstream@example.com"]);
    std::fs::write(seed.join("README.md"), "stacks-core fixture\n").unwrap();
    run_git(&seed, &["add", "README.md"]);
    run_git(&seed, &["commit", "-m", "seed"]);
    run_git(tmp_root, &["init", "--bare", "source.git"]);
    let url = format!("file://{}", bare.display());
    run_git(&seed, &["push", &url, "feat/stacks-bench"]);
    bare
}

/// Empty bare destination — the target the operator wants to seed.
fn make_dest_bare(tmp_root: &Path) -> PathBuf {
    let bare = tmp_root.join("dest.git");
    run_git(tmp_root, &["init", "--bare", "dest.git"]);
    bare
}

/// Write a minimal config.toml with `[source].branch` set. `--branch`
/// defaults pull from this.
fn write_config(target: &Path) -> PathBuf {
    let prompts = target
        .join(".sbagent")
        .join("prompts");
    let config_path = target.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[layout]\nprompt_overrides_dir = \"{}\"\n\n[source]\nurl = \
             \"https://example.com/x.git\"\nbranch = \"feat/stacks-bench\"\n",
            prompts.display(),
        ),
    )
    .unwrap();
    config_path
}

fn exec_seed(config_path: &Path, args: &[&str]) -> std::process::Output {
    let mut cli_args = vec!["-c", config_path.to_str().unwrap(), "source", "seed"];
    cli_args.extend_from_slice(args);
    std::process::Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(&cli_args)
        .output()
        .expect("spawn sbagent source seed")
}

#[test]
fn source_seed_pushes_branch_to_empty_dest_via_file_urls() {
    let tmp = tempfile::tempdir().unwrap();
    let source_bare = make_source_bare(tmp.path());
    let dest_bare = make_dest_bare(tmp.path());
    let config_path = write_config(tmp.path());

    let from = format!("file://{}", source_bare.display());
    let to = format!("file://{}", dest_bare.display());
    let out = exec_seed(&config_path, &["--from", &from, "--to", &to]);
    assert!(
        out.status.success(),
        "seed failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // Destination now carries `refs/heads/feat/stacks-bench`.
    let refs = run_git_stdout(&dest_bare, &["for-each-ref", "--format=%(refname)"]);
    assert!(
        refs.lines()
            .any(|r| r == "refs/heads/feat/stacks-bench"),
        "dest should have refs/heads/feat/stacks-bench; got:\n{refs}",
    );

    // The seeded ref points at the source's tip.
    let source_tip = run_git_stdout(&source_bare, &["rev-parse", "refs/heads/feat/stacks-bench"]);
    let dest_tip = run_git_stdout(&dest_bare, &["rev-parse", "refs/heads/feat/stacks-bench"]);
    assert_eq!(source_tip, dest_tip, "seeded SHA must match source");
}

#[test]
fn source_seed_is_idempotent_on_rerun() {
    let tmp = tempfile::tempdir().unwrap();
    let source_bare = make_source_bare(tmp.path());
    let dest_bare = make_dest_bare(tmp.path());
    let config_path = write_config(tmp.path());

    let from = format!("file://{}", source_bare.display());
    let to = format!("file://{}", dest_bare.display());
    // First seed.
    let first = exec_seed(&config_path, &["--from", &from, "--to", &to]);
    assert!(first.status.success(), "first seed failed");
    let tip_after_first =
        run_git_stdout(&dest_bare, &["rev-parse", "refs/heads/feat/stacks-bench"]);

    // Second seed (no upstream advance) — must succeed as fast-forward no-op.
    let second = exec_seed(&config_path, &["--from", &from, "--to", &to]);
    assert!(
        second.status.success(),
        "second seed (no-op) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr),
    );
    let tip_after_second =
        run_git_stdout(&dest_bare, &["rev-parse", "refs/heads/feat/stacks-bench"]);
    assert_eq!(tip_after_first, tip_after_second, "re-run must not move the dest ref");
}

#[test]
fn source_seed_rejects_plain_http_to_url() {
    let tmp = tempfile::tempdir().unwrap();
    let source_bare = make_source_bare(tmp.path());
    let config_path = write_config(tmp.path());
    let from = format!("file://{}", source_bare.display());

    let out = exec_seed(&config_path, &["--from", &from, "--to", "http://example.com/repo.git"]);
    assert!(!out.status.success(), "plain http:// --to must error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("plain HTTP"),
        "stderr should explain why HTTPS is required:\n{stderr}"
    );
}

#[test]
fn source_seed_rejects_unknown_scheme_to_url() {
    let tmp = tempfile::tempdir().unwrap();
    let source_bare = make_source_bare(tmp.path());
    let config_path = write_config(tmp.path());
    let from = format!("file://{}", source_bare.display());

    let out = exec_seed(&config_path, &["--from", &from, "--to", "ftp://example.com/repo.git"]);
    assert!(!out.status.success(), "ftp:// --to must error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsupported scheme"),
        "stderr should list the accepted schemes:\n{stderr}",
    );
}

#[test]
fn source_seed_explicit_branch_overrides_settings_default() {
    let tmp = tempfile::tempdir().unwrap();
    // Seed a "main" branch upstream, not feat/stacks-bench, and pass
    // --branch main explicitly. The settings default
    // (feat/stacks-bench) shouldn't be touched.
    let bare = tmp.path().join("source.git");
    let seed = tmp.path().join("source-seed");
    std::fs::create_dir_all(&seed).unwrap();
    run_git(&seed, &["init", "-b", "main"]);
    run_git(&seed, &["config", "user.name", "Upstream Author"]);
    run_git(&seed, &["config", "user.email", "upstream@example.com"]);
    std::fs::write(seed.join("README.md"), "x\n").unwrap();
    run_git(&seed, &["add", "README.md"]);
    run_git(&seed, &["commit", "-m", "seed"]);
    run_git(tmp.path(), &["init", "--bare", "source.git"]);
    run_git(&seed, &["push", &format!("file://{}", bare.display()), "main"]);

    let dest_bare = make_dest_bare(tmp.path());
    let config_path = write_config(tmp.path());
    let from = format!("file://{}", bare.display());
    let to = format!("file://{}", dest_bare.display());

    let out = exec_seed(&config_path, &["--from", &from, "--to", &to, "--branch", "main"]);
    assert!(
        out.status.success(),
        "explicit --branch main seed failed:\nstderr={}",
        String::from_utf8_lossy(&out.stderr),
    );
    let refs = run_git_stdout(&dest_bare, &["for-each-ref", "--format=%(refname)"]);
    assert!(
        refs.lines()
            .any(|r| r == "refs/heads/main"),
        "dest should have refs/heads/main, not feat/stacks-bench; got:\n{refs}",
    );
}

/// HTTPS `--to` URLs are gated by `git.auth_url_prefix`. Mismatched
/// host errors at `validate_auth_url` BEFORE the helper attempts to
/// read `publish.token_file` — so the test can pin this branch
/// without ever configuring a token file. The error message must
/// name the prefix so the operator knows what to fix.
#[test]
fn source_seed_rejects_https_to_url_outside_configured_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let source_bare = make_source_bare(tmp.path());
    let prompts = tmp
        .path()
        .join(".sbagent")
        .join("prompts");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[layout]\nprompt_overrides_dir = \"{}\"\n\n\
             [source]\nurl = \"https://example.com/x.git\"\nbranch = \"feat/stacks-bench\"\n\n\
             [git]\nauth_url_prefix = \"https://github.com/\"\n",
            prompts.display(),
        ),
    )
    .unwrap();
    let from = format!("file://{}", source_bare.display());

    let out =
        exec_seed(&config_path, &["--from", &from, "--to", "https://gitlab.com/owner/repo.git"]);
    assert!(!out.status.success(), "mismatched HTTPS prefix must error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("https://github.com/"),
        "error should name the configured `git.auth_url_prefix`:\n{stderr}",
    );
    assert!(
        !stderr.contains("publish.token_file"),
        "validate_auth_url should fail BEFORE the token-file read; got:\n{stderr}",
    );
}

#[test]
fn source_seed_help_documents_use_case() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(["source", "seed", "--help"])
        .output()
        .expect("spawn sbagent source seed --help");
    assert!(out.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The post-v3 use case is "bootstrapping a brand-new bot fork
    // before the first session" — the long flag docs should make
    // that searchable.
    assert!(
        stdout.contains("Source URL") && stdout.contains("Destination URL"),
        "--help should describe both --from and --to clearly:\n{stdout}",
    );
    assert!(
        stdout.contains("https") && stdout.contains("ssh"),
        "--help should mention the accepted URL forms:\n{stdout}",
    );
}
