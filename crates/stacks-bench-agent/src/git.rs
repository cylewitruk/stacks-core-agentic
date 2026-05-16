//! Shared git subprocess + PAT-via-env auth helpers.
//!
//! Both `sbagent init` (operator-dir bootstrap) and `sbagent sync`
//! (bundle refresh) need the same git primitives:
//!
//! - thin wrappers over `git -C <dir> ...` with consistent error handling and
//!   env-var injection support;
//! - a `validate_auth_url` gate that catches non-HTTPS / wrong-prefix origins
//!   before they silently bypass the PAT header;
//! - a `push_with_pat` that injects the `http.<prefix>.extraheader` config
//!   override via `GIT_CONFIG_COUNT` env-vars so the token never enters argv,
//!   `.git/config`, or shell history;
//! - a `stage_and_commit` helper that mirrors what init's commit-step does
//!   (stage explicit pathspecs, skip if porcelain shows nothing staged, commit
//!   with caller-supplied env for identity overrides).
//!
//! We deliberately stay on subprocess invocations rather than the
//! `git2` crate. `libgit2` would marginally clean up the auth header
//! dance (`RemoteCallbacks::credentials`), but it still can't drive
//! `git submodule add` completely and pulls in libssh2, openssl-sys,
//! and a meaningful build cost. Subprocess + tight wrappers stays
//! lower-blast-radius for the autonomous-bot use case.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;

/// Run `git -C <dir> <args>` and bail on non-zero exit. The
/// load-bearing wrapper — every callsite that must abort on failure
/// goes through this.
pub fn run_git(dir: &Path, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .with_context(|| format!("spawn git {args:?} in {}", dir.display()))?;
    if !status.success() {
        bail!("git {args:?} exited {status} in {}", dir.display());
    }
    Ok(())
}

/// `run_git` variant that injects env-vars. Used for identity
/// overrides on `git commit` and for the PAT-via-env config override
/// on `git push`. Other behavior identical to [`run_git`].
pub fn run_git_envs(dir: &Path, args: &[&str], env: &[(String, String)]) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(
            env.iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .status()
        .with_context(|| format!("spawn git {args:?} in {}", dir.display()))?;
    if !status.success() {
        bail!("git {args:?} exited {status} in {}", dir.display());
    }
    Ok(())
}

/// Boolean probe — `git -C <dir> <args>` succeeded or not. Suppresses
/// both stdout and stderr (the failure case is "thing is absent", not
/// "something broke") so callers can use it for cheap existence checks
/// like `remote get-url <name>`.
pub fn run_git_check(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `git -C <dir> <args>` and return trimmed stdout. Used for
/// reading single-value outputs like `remote get-url <name>` or
/// `status --porcelain`.
pub fn run_git_output(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("spawn git {args:?} in {}", dir.display()))?;
    if !out.status.success() {
        bail!("git {args:?} exited {} in {}", out.status, dir.display());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim()
        .to_owned())
}

/// Validate that `url` will actually receive the PAT-via-extraheader
/// auth callers rely on. With a non-empty `auth_url_prefix`, the URL
/// must start with that prefix verbatim — so a
/// `git_auth_url_prefix = "https://gitlab.com/"` config rejects
/// `git@github.com:...`, `https://github.com/...`, AND
/// `https://gitlab.com.evil.example/...`. In expert / empty-prefix
/// mode, we still demand `https://` (git won't send
/// `http.extraheader` over SSH / file://; failing fast beats a
/// confusing "no credentials" git error).
pub fn validate_auth_url(url: &str, auth_url_prefix: &str, label: &str) -> Result<()> {
    let ok = if auth_url_prefix.is_empty() {
        url.starts_with("https://")
    } else {
        url.starts_with(auth_url_prefix)
    };
    if ok {
        return Ok(());
    }
    if auth_url_prefix.is_empty() {
        bail!(
            "{label} must be an `https://` URL (got `{url}`); the PAT-via-env auth mechanism only \
             attaches a Basic credential over HTTPS. (`git_auth_url_prefix = \"\"` is set, so any \
             HTTPS host is accepted — fix the URL to use HTTPS or switch to SSH and authenticate \
             out-of-band.)",
        );
    }
    bail!(
        "{label} must start with `{auth_url_prefix}` (got `{url}`); the PAT-via-env auth \
         mechanism uses `http.{auth_url_prefix}.extraheader` and git won't apply it to URLs \
         outside that prefix. Either change the URL to match `git_auth_url_prefix`, change \
         `git_auth_url_prefix` in config to your forge's HTTPS root, or authenticate manually.",
    )
}

/// Build the `http.<prefix>.extraheader` config key + Basic header
/// value as `(key, value)`. When `auth_url_prefix` is empty (expert
/// mode), the key is the unqualified `http.extraheader` (the header
/// will then apply to every HTTPS remote git contacts during the
/// invocation — caller is responsible for auditing the URL set).
pub fn auth_header_config_entries(
    token: &str,
    auth_username: &str,
    auth_url_prefix: &str,
) -> (String, String) {
    let cred = format!("{auth_username}:{token}");
    let b64 = base64::engine::general_purpose::STANDARD.encode(cred);
    let header = format!("AUTHORIZATION: basic {b64}");
    let key = if auth_url_prefix.is_empty() {
        "http.extraheader".to_owned()
    } else {
        format!("http.{auth_url_prefix}.extraheader")
    };
    (key, header)
}

/// Build a one-shot env-var set carrying the PAT auth header. Returns
/// an empty vec when the dest URL isn't `https://...` — git silently
/// ignores the extraheader for `file://` / `ssh://` dests, but we
/// suppress injection entirely so tests against local URLs don't need
/// a real PAT and the env shape stays minimal in non-HTTPS code paths.
/// Used by callers that need a stand-alone env vec (e.g. init's
/// `seed_branch` bare-clone push).
pub fn build_auth_header_env(
    dest_url: &str,
    token: &str,
    auth_username: &str,
    auth_url_prefix: &str,
) -> Vec<(String, String)> {
    if !dest_url.starts_with("https://") {
        return Vec::new();
    }
    let (key, value) = auth_header_config_entries(token, auth_username, auth_url_prefix);
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), key),
        ("GIT_CONFIG_VALUE_0".into(), value),
    ]
}

/// `git push -u <remote> <branch>` in `target_dir` with the bot PAT
/// injected via an `http.<auth_url_prefix>.extraheader` config
/// override (or unqualified `http.extraheader` when `auth_url_prefix`
/// is empty — expert mode). The override rides on
/// `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_N`/`GIT_CONFIG_VALUE_N` env vars
/// so the token never enters argv, `.git/config`, or the remote URL.
///
/// Caller is responsible for gating the push behind
/// [`validate_auth_url`] on the resolved remote URL — git would
/// silently fall back to SSH / prompted auth on a mismatched remote
/// and `push_with_pat` doesn't itself check.
///
/// `base_env` is overlaid: existing `GIT_CONFIG_COUNT` entries are
/// extended (so an identity-override env can already carry config
/// entries and this just adds one more for the auth header).
pub fn push_with_pat(
    target_dir: &Path,
    remote: &str,
    branch: &str,
    token: &str,
    base_env: &[(String, String)],
    auth_username: &str,
    auth_url_prefix: &str,
) -> Result<()> {
    let auth_entries = auth_header_config_entries(token, auth_username, auth_url_prefix);
    let env = merge_git_config_entry(base_env, &auth_entries.0, &auth_entries.1);

    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(target_dir)
        .arg("push")
        .arg("-u")
        .arg(remote)
        .arg(branch)
        .envs(
            env.iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .status()
        .with_context(|| format!("spawn git push -u {remote} {branch}"))?;
    if !status.success() {
        bail!("git push -u {remote} {branch} exited {status}");
    }
    Ok(())
}

/// Merge a single `GIT_CONFIG_KEY_<N>` / `GIT_CONFIG_VALUE_<N>` pair
/// onto `base_env`, extending `GIT_CONFIG_COUNT` by 1. Lets callers
/// stack multiple config overrides (e.g. identity entries from
/// `optimizer_git_env` + an auth header for a single push) without
/// each layer having to track the slot count.
fn merge_git_config_entry(
    base_env: &[(String, String)],
    key: &str,
    value: &str,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = base_env.to_vec();
    let prev_count: usize = env
        .iter()
        .find(|(k, _)| k == "GIT_CONFIG_COUNT")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let new_count = prev_count + 1;
    if let Some(slot) = env
        .iter_mut()
        .find(|(k, _)| k == "GIT_CONFIG_COUNT")
    {
        slot.1 = new_count.to_string();
    } else {
        env.push(("GIT_CONFIG_COUNT".into(), new_count.to_string()));
    }
    env.push((format!("GIT_CONFIG_KEY_{prev_count}"), key.to_owned()));
    env.push((format!("GIT_CONFIG_VALUE_{prev_count}"), value.to_owned()));
    env
}

/// Outcome of [`stage_and_commit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// One commit was produced with the supplied message.
    Committed,
    /// Nothing was staged after `git add -- <paths>` — re-running the
    /// caller on a clean tree leaves no work to do. Caller decides
    /// whether to print a notice or move on silently.
    NothingToCommit,
}

/// Stage explicit `paths` under `dir` (never `git add -A`), then
/// produce a commit if the resulting index differs from HEAD. Mirrors
/// what `sbagent init` does for its initial-commit step; reused by
/// `sbagent sync --commit` for the bundle-refresh case.
///
/// `paths` are filtered through `dir.join(p).exists()` so callers can
/// pass an aspirational list without worrying about unmaterialized
/// entries (e.g. an init re-run where the operator deleted one of the
/// init-owned paths between calls).
///
/// Staging-only entries (X-column in porcelain) drive the commit
/// decision — untracked files (`??`) and unmodified entries don't
/// count. This protects against the "operator has stray uncommitted
/// files in the operator dir" case from accidentally producing a
/// commit-on-noop.
pub fn stage_and_commit(
    dir: &Path,
    paths: &[&str],
    commit_msg: &str,
    env: &[(String, String)],
) -> Result<CommitOutcome> {
    let existing: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|p| dir.join(p).exists())
        .collect();
    if !existing.is_empty() {
        let mut args_v: Vec<&str> = vec!["add", "--"];
        args_v.extend(existing.iter().copied());
        run_git_envs(dir, &args_v, env).with_context(|| format!("git add -- {existing:?}"))?;
    }
    let porcelain =
        run_git_output(dir, &["status", "--porcelain"]).context("git status --porcelain")?;
    let any_staged = porcelain.lines().any(|line| {
        let bytes = line.as_bytes();
        bytes.len() >= 2 && bytes[0] != b' ' && bytes[0] != b'?'
    });
    if !any_staged {
        return Ok(CommitOutcome::NothingToCommit);
    }
    run_git_envs(dir, &["commit", "-m", commit_msg], env)
        .with_context(|| format!("git commit -m {commit_msg:?}"))?;
    Ok(CommitOutcome::Committed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_auth_url_accepts_matching_prefix() {
        assert!(
            validate_auth_url("https://github.com/bot/repo.git", "https://github.com/", "test",)
                .is_ok()
        );
    }

    #[test]
    fn validate_auth_url_rejects_non_prefix_https() {
        let err =
            validate_auth_url("https://gitlab.com/bot/repo.git", "https://github.com/", "test url")
                .expect_err("non-prefix HTTPS must be rejected");
        let msg = format!("{err:#}");
        assert!(msg.contains("must start with `https://github.com/`"), "{msg}");
        assert!(msg.contains("https://gitlab.com/bot/repo.git"), "{msg}");
    }

    /// In expert mode (empty prefix), only the protocol gate
    /// applies: HTTPS accepted, SSH/file:// rejected.
    #[test]
    fn validate_auth_url_expert_mode_demands_https() {
        assert!(validate_auth_url("https://any.host/x.git", "", "test").is_ok());
        let err = validate_auth_url("git@github.com:bot/repo.git", "", "test").expect_err("ssh");
        let msg = format!("{err:#}");
        assert!(msg.contains("must be an `https://` URL"), "{msg}");
    }

    /// Auth header builder produces the expected `(key, value)` for
    /// both prefixed + expert-mode cases.
    #[test]
    fn auth_header_config_entries_round_trip() {
        let (k, v) = auth_header_config_entries("tok", "x-access-token", "https://github.com/");
        assert_eq!(k, "http.https://github.com/.extraheader");
        assert!(v.starts_with("AUTHORIZATION: basic "));
        // Value is base64("x-access-token:tok") — decode + spot-check.
        let b64 = v
            .strip_prefix("AUTHORIZATION: basic ")
            .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(decoded, b"x-access-token:tok");

        // Expert mode drops the URL qualifier.
        let (k, _) = auth_header_config_entries("tok", "x-access-token", "");
        assert_eq!(k, "http.extraheader");
    }

    /// `build_auth_header_env` returns empty for non-HTTPS dests so
    /// file:// pushes in tests don't carry a meaningless header.
    #[test]
    fn build_auth_header_env_empty_for_non_https() {
        assert!(
            build_auth_header_env("file:///tmp/dest.git", "tok", "u", "https://github.com/")
                .is_empty()
        );
        assert!(
            build_auth_header_env("ssh://git@host/x.git", "tok", "u", "https://github.com/")
                .is_empty()
        );
        // HTTPS dest → 3 env entries.
        assert_eq!(
            build_auth_header_env(
                "https://github.com/bot/x.git",
                "tok",
                "u",
                "https://github.com/"
            )
            .len(),
            3,
        );
    }

    /// Merging an auth entry onto a base env that already carries
    /// identity overrides extends `GIT_CONFIG_COUNT` rather than
    /// resetting it.
    #[test]
    fn merge_git_config_entry_extends_existing_count() {
        let base = vec![
            ("GIT_AUTHOR_NAME".into(), "bot".into()),
            ("GIT_CONFIG_COUNT".into(), "2".into()),
            ("GIT_CONFIG_KEY_0".into(), "user.name".into()),
            ("GIT_CONFIG_VALUE_0".into(), "bot".into()),
            ("GIT_CONFIG_KEY_1".into(), "user.email".into()),
            ("GIT_CONFIG_VALUE_1".into(), "bot@".into()),
        ];
        let merged = merge_git_config_entry(&base, "http.x.extraheader", "AUTHORIZATION: basic x");
        let count: usize = merged
            .iter()
            .find(|(k, _)| k == "GIT_CONFIG_COUNT")
            .map(|(_, v)| v.parse().unwrap())
            .unwrap();
        assert_eq!(count, 3, "count must extend from 2 → 3, got {count}");
        assert!(
            merged
                .iter()
                .any(|(k, _)| k == "GIT_CONFIG_KEY_2")
        );
    }

    /// `stage_and_commit` on a clean tree (or a re-run after all
    /// paths were already committed) returns `NothingToCommit`.
    #[test]
    fn stage_and_commit_returns_nothing_when_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        run_git(dir, &["init", "-q", "-b", "main"]).unwrap();
        run_git(dir, &["config", "user.email", "test@t"]).unwrap();
        run_git(dir, &["config", "user.name", "test"]).unwrap();
        run_git(dir, &["config", "commit.gpgsign", "false"]).unwrap();
        std::fs::write(dir.join("hello.txt"), "hi\n").unwrap();
        run_git(dir, &["add", "hello.txt"]).unwrap();
        run_git(dir, &["commit", "-q", "-m", "init"]).unwrap();

        let outcome = stage_and_commit(dir, &["hello.txt"], "noop", &[]).unwrap();
        assert_eq!(outcome, CommitOutcome::NothingToCommit);
    }

    /// `stage_and_commit` on a dirty tree (new files matching the
    /// pathspec) produces exactly one commit with the supplied message.
    #[test]
    fn stage_and_commit_produces_commit_when_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        run_git(dir, &["init", "-q", "-b", "main"]).unwrap();
        run_git(dir, &["config", "user.email", "test@t"]).unwrap();
        run_git(dir, &["config", "user.name", "test"]).unwrap();
        run_git(dir, &["config", "commit.gpgsign", "false"]).unwrap();
        // Empty tree won't accept a commit; seed one.
        std::fs::write(dir.join("seed.txt"), "s\n").unwrap();
        run_git(dir, &["add", "seed.txt"]).unwrap();
        run_git(dir, &["commit", "-q", "-m", "seed"]).unwrap();

        std::fs::write(dir.join("new.txt"), "new\n").unwrap();
        std::fs::write(dir.join("stray.txt"), "stray\n").unwrap();

        let outcome = stage_and_commit(dir, &["new.txt"], "feat: add new", &[]).unwrap();
        assert_eq!(outcome, CommitOutcome::Committed);

        // Only `new.txt` should be in HEAD — `stray.txt` must NOT
        // have been swept in (mirrors init's explicit-pathspec stance).
        let head_files =
            run_git_output(dir, &["show", "--name-only", "--pretty=format:", "HEAD"]).unwrap();
        let names: Vec<&str> = head_files
            .lines()
            .filter(|l| !l.is_empty())
            .collect();
        assert!(names.contains(&"new.txt"), "{names:?}");
        assert!(!names.contains(&"stray.txt"), "{names:?}");
    }
}
