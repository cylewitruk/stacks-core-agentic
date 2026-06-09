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
/// `status --porcelain`. On non-zero exit, trimmed stderr is folded
/// into the error so operators see git's own diagnostic (auth failure,
/// missing remote, etc.) rather than an opaque exit code.
pub fn run_git_output(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("spawn git {args:?} in {}", dir.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr_trimmed = stderr.trim();
        if stderr_trimmed.is_empty() {
            bail!("git {args:?} exited {} in {}", out.status, dir.display());
        }
        bail!("git {args:?} exited {} in {}: {stderr_trimmed}", out.status, dir.display(),);
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim()
        .to_owned())
}

/// Validate that `url` will actually receive the PAT-via-extraheader
/// auth callers rely on. With a non-empty `auth_url_prefix`, the URL
/// must start with that prefix verbatim — so a
/// `git.auth_url_prefix = "https://gitlab.com/"` config rejects
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
             attaches a Basic credential over HTTPS. (`git.auth_url_prefix = \"\"` is set, so any \
             HTTPS host is accepted — fix the URL to use HTTPS or switch to SSH and authenticate \
             out-of-band.)",
        );
    }
    bail!(
        "{label} must start with `{auth_url_prefix}` (got `{url}`); the PAT-via-env auth \
         mechanism uses `http.{auth_url_prefix}.extraheader` and git won't apply it to URLs \
         outside that prefix. Either change the URL to match `git.auth_url_prefix`, change \
         `git.auth_url_prefix` in config to your forge's HTTPS root, or authenticate manually.",
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

/// `stage_and_commit` variant that uses `git add --force`. Two
/// behaviors differ from the gitignore-respecting helper above:
///
/// 1. Paths matching `.gitignore` are still staged — required for the archive
///    flow on `session/<id>` branches, where `main`'s `.gitignore` excludes
///    `/sessions/` but the bulk is exactly what must be committed.
/// 2. Paths are NOT existence-filtered (the caller passes whole directories
///    whose contents are too numerous to enumerate; a missing entry should
///    surface as a `git add` error rather than silently no-op).
///
/// Same `CommitOutcome` semantics as [`stage_and_commit`].
pub fn stage_force_and_commit(
    dir: &Path,
    paths: &[&str],
    commit_msg: &str,
    env: &[(String, String)],
) -> Result<CommitOutcome> {
    if !paths.is_empty() {
        let mut args_v: Vec<&str> = vec!["add", "-f", "--"];
        args_v.extend(paths.iter().copied());
        run_git_envs(dir, &args_v, env).with_context(|| format!("git add -f -- {paths:?}"))?;
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

/// `git fetch <remote> <branch>` followed by `git rebase
/// <remote>/<branch>`. Used by the archive flow's main-branch path:
/// before appending to `sessions.jsonl`, pull-rebase to absorb any
/// concurrent writes from a peer.
///
/// Unauthenticated — suitable for public remotes, `file://` remotes
/// in tests, or when the operator's git already has credentials
/// cached out-of-band. For private HTTPS / non-public-fork
/// operators, use [`pull_rebase_with_auth`] which threads a PAT
/// extraheader through the `fetch`.
pub fn pull_rebase(dir: &Path, remote: &str, branch: &str) -> Result<()> {
    run_git(dir, &["fetch", remote, branch])
        .with_context(|| format!("git fetch {remote} {branch}"))?;
    let upstream = format!("{remote}/{branch}");
    run_git(dir, &["rebase", &upstream]).with_context(|| format!("git rebase {upstream}"))
}

/// Authenticated counterpart to [`pull_rebase`]. Same shape, but the
/// `git fetch` step rides on the PAT-via-extraheader env mechanism
/// so private remotes work. The `rebase` step is purely local and
/// doesn't need auth.
///
/// Caller is responsible for gating against
/// [`validate_auth_url`] on the resolved remote URL before invoking.
#[allow(clippy::too_many_arguments)]
pub fn pull_rebase_with_auth(
    dir: &Path,
    remote: &str,
    branch: &str,
    token: &str,
    base_env: &[(String, String)],
    auth_username: &str,
    auth_url_prefix: &str,
) -> Result<()> {
    let (key, value) = auth_header_config_entries(token, auth_username, auth_url_prefix);
    let env = merge_git_config_entry(base_env, &key, &value);
    run_git_envs(dir, &["fetch", remote, branch], &env)
        .with_context(|| format!("git fetch {remote} {branch} (authenticated)"))?;
    let upstream = format!("{remote}/{branch}");
    run_git(dir, &["rebase", &upstream]).with_context(|| format!("git rebase {upstream}"))
}

/// `git push -u <remote> <branch>` with PAT injection, and on
/// `non-fast-forward` rejection, pull-rebase + retry up to
/// `max_retries` times. Used by archive to land both the
/// `session/<id>` branch (push, idempotent) and `main` (append +
/// push, race-tolerant). Returns the underlying push error after
/// exhausting retries.
///
/// When `max_retries == 0`, behaves identically to [`push_with_pat`]
/// — single attempt, no rebase loop. Useful for callers (e.g.
/// `session/<id>` archive branches, which are write-once) that
/// shouldn't ever experience a push race.
#[allow(clippy::too_many_arguments)]
pub fn push_or_retry(
    target_dir: &Path,
    remote: &str,
    branch: &str,
    token: &str,
    base_env: &[(String, String)],
    auth_username: &str,
    auth_url_prefix: &str,
    max_retries: usize,
) -> Result<()> {
    let mut attempt = 0_usize;
    loop {
        match push_with_pat(
            target_dir,
            remote,
            branch,
            token,
            base_env,
            auth_username,
            auth_url_prefix,
        ) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if attempt >= max_retries {
                    return Err(e);
                }
                attempt += 1;
                // The retry fetch needs the SAME PAT as the push —
                // otherwise the fetch step in pull_rebase would fail
                // on any private operator repo.
                pull_rebase_with_auth(
                    target_dir,
                    remote,
                    branch,
                    token,
                    base_env,
                    auth_username,
                    auth_url_prefix,
                )
                .with_context(|| format!("retry attempt {attempt}: pull --rebase"))?;
            }
        }
    }
}

/// Check whether `branch` exists locally (i.e. `refs/heads/<branch>`
/// resolves). Cheap idempotency check used by the archive flow before
/// creating the `session/<id>` branch.
pub fn branch_exists(dir: &Path, branch: &str) -> bool {
    run_git_check(dir, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")])
}

/// Create `branch` pointing at `base_ref` and check it out. When
/// `branch` already exists, just check it out — supports idempotent
/// re-runs of the archive flow without requiring callers to do their
/// own branch-existence dance.
pub fn checkout_or_create_branch(dir: &Path, branch: &str, base_ref: &str) -> Result<()> {
    if branch_exists(dir, branch) {
        run_git(dir, &["checkout", branch])
    } else {
        run_git(dir, &["checkout", "-b", branch, base_ref])
    }
}

/// Return the currently checked-out branch name. Errors when HEAD is
/// detached (no symbolic ref). Used by archive to save the
/// operator's branch context before switching, and restore it after.
pub fn current_branch(dir: &Path) -> Result<String> {
    run_git_output(dir, &["symbolic-ref", "--short", "HEAD"])
        .context("git symbolic-ref --short HEAD (operator repo has detached HEAD?)")
}

// ── Higher-level wrappers ─────────────────────────────────────────────
//
// The wrappers below encapsulate operations that previously appeared as
// raw `Command::new("git")` calls scattered across `session/`, `cli/`,
// and tests. Goal: every git invocation in the codebase goes through
// `git.rs` so error context, env handling, and lock semantics are
// consistent.

/// `git rev-parse HEAD` in `dir`. Returns the 40-char SHA on
/// success. Used as a "what sha am I about to record?" probe by
/// Phase 0a archival, the optimizer post-commit observer, and the
/// analyzer's seed for prompt context.
pub fn rev_parse_head(dir: &Path) -> Result<String> {
    let sha = run_git_output(dir, &["rev-parse", "HEAD"])?;
    if sha.len() != 40
        || !sha
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        bail!("git rev-parse HEAD returned non-40-char-hex value: {sha:?}");
    }
    Ok(sha)
}

/// Best-effort variant of [`rev_parse_head`]. Returns `None` when
/// anything goes wrong (not a repo, no commits, IO failure). Used by
/// callers that treat the sha as decorative rather than load-bearing.
pub fn rev_parse_head_optional(dir: &Path) -> Option<String> {
    rev_parse_head(dir).ok()
}

/// `git rev-parse HEAD^` — the parent of HEAD. Returns the 40-char
/// SHA on success. Used by the coordinator-provenance sidecar to
/// record the base commit the agent's change was applied on top of.
/// Fails on root commits (no parent) and on detached HEADs at the
/// initial commit — both indicate the per-target clone is in an
/// unexpected state, so a hard error here is the right surface.
pub fn rev_parse_head_parent(dir: &Path) -> Result<String> {
    let sha = run_git_output(dir, &["rev-parse", "HEAD^"])?;
    if sha.len() != 40
        || !sha
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        bail!("git rev-parse HEAD^ returned non-40-char-hex value: {sha:?}");
    }
    Ok(sha)
}

/// `git status --porcelain` in `dir`, trimmed (empty string = clean
/// tree). Suitable for the "is anything dirty?" check used by Phase
/// 0a + the optimizer's commit gate. Not suitable for parsing the
/// status-column bytes, since the leading whitespace that encodes
/// staged/unstaged state is stripped — add a raw-output sibling
/// helper here in `git.rs` if a caller ever needs that.
pub fn status_porcelain(dir: &Path) -> Result<String> {
    run_git_output(dir, &["status", "--porcelain"])
        .with_context(|| format!("git status --porcelain in {}", dir.display()))
}

/// True iff the worktree has ANY uncommitted change (tracked or
/// untracked). Returns `false` on error — defensive fallback. Used
/// by Phase 0a to surface a `dirty: true` field in the binary
/// manifest.
pub fn is_worktree_dirty(dir: &Path) -> bool {
    status_porcelain(dir)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// `git -C <dir> remote` — list configured remote names. Trimmed +
/// empty-lines-dropped.
pub fn list_remotes(dir: &Path) -> Result<Vec<String>> {
    let raw = run_git_output(dir, &["remote"])
        .with_context(|| format!("git remote in {}", dir.display()))?;
    Ok(raw
        .lines()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect())
}

/// `git -C <dir> remote get-url <name>` — return the configured URL
/// for `name`. Bails when the remote doesn't exist.
pub fn get_remote_url(dir: &Path, name: &str) -> Result<String> {
    run_git_output(dir, &["remote", "get-url", name])
        .with_context(|| format!("git -C {} remote get-url {name}", dir.display()))
}

/// Add OR update a remote's URL in `dir`. When `name` is already
/// configured, runs `git remote set-url`; otherwise `git remote add`.
/// Used by per-target clone setup (mirrors the base's remote map
/// into the new clone, idempotently).
pub fn add_or_set_remote(dir: &Path, name: &str, url: &str) -> Result<()> {
    let existing = list_remotes(dir)?;
    let subcmd = if existing
        .iter()
        .any(|n| n == name)
    {
        "set-url"
    } else {
        "add"
    };
    run_git(dir, &["remote", subcmd, name, url])
        .with_context(|| format!("git remote {subcmd} {name} {url} in {}", dir.display()))
}

/// `git clone --reference <base> --branch <branch> --local <base>
/// <dest>` — used by the optimizer to fan out per-target clones
/// that share the base's object store via `--reference --local`.
/// `branch` is checked out at the new clone's HEAD.
pub fn clone_with_reference(base: &Path, branch: &str, dest: &Path) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("clone")
        .arg("--reference")
        .arg(base)
        .arg("--branch")
        .arg(branch)
        .arg("--local")
        .arg(base)
        .arg(dest)
        .status()
        .with_context(|| format!("git clone {} -> {}", base.display(), dest.display()))?;
    if !status.success() {
        bail!(
            "git clone --reference {} --branch {branch} --local {} {} exited {status}",
            base.display(),
            base.display(),
            dest.display(),
        );
    }
    Ok(())
}

/// `git switch -c <branch>` — create + check out `branch` from
/// HEAD. Used by per-target clones to land on an
/// `agent/<session>/<target>` branch.
pub fn switch_create_branch(dir: &Path, branch: &str) -> Result<()> {
    run_git(dir, &["switch", "-c", branch])
        .with_context(|| format!("git switch -c {branch} in {}", dir.display()))
}

/// Best-effort `git worktree remove --force <path>` against `repo`.
/// Returns the underlying status; callers that don't care about the
/// outcome can discard it. Used by the optimizer's clone teardown
/// to clear stale linked-worktree registry entries.
pub fn worktree_remove_force_quiet(repo: &Path, path: &Path) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Best-effort `git worktree prune` in `repo`. Pairs with
/// [`worktree_remove_force_quiet`] in the optimizer cleanup path.
pub fn worktree_prune_quiet(repo: &Path) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("worktree")
        .arg("prune")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// `git add -A` with caller-supplied env. Distinct from
/// [`stage_and_commit`] (which takes explicit pathspecs and never
/// stages with `-A`): this is the "stage everything the optimizer
/// agent changed" sweep used by Phase 2's coordinator-side commit.
/// Reserved for callers that own the worktree's mutation contract;
/// avoid in generic code.
pub fn add_all_with_env(dir: &Path, env: &[(String, String)]) -> Result<()> {
    run_git_envs(dir, &["add", "-A"], env)
        .with_context(|| format!("git add -A in {}", dir.display()))
}

/// `git commit -m <msg>` with caller-supplied env (typically the
/// optimizer's identity overrides). Returns an error when the
/// commit fails — the caller is responsible for first verifying
/// that something IS staged.
pub fn commit_with_message_and_env(
    dir: &Path,
    message: &str,
    env: &[(String, String)],
) -> Result<()> {
    run_git_envs(dir, &["commit", "-m", message], env)
        .with_context(|| format!("git commit in {}", dir.display()))
}

/// `git diff --cached --quiet` — exit 0 means "nothing staged", exit
/// 1 means "at least one staged change", any other code (typically
/// 128) means git itself failed (not a repo, broken index, ...).
/// Captures stderr so a real git failure surfaces in the error rather
/// than being misread as "has staged changes."
pub fn has_staged_changes(dir: &Path) -> Result<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .output()
        .with_context(|| format!("git diff --cached --quiet in {}", dir.display()))?;
    match out.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr_trimmed = stderr.trim();
            if stderr_trimmed.is_empty() {
                bail!("git diff --cached --quiet exited {} in {}", out.status, dir.display(),);
            }
            bail!(
                "git diff --cached --quiet exited {} in {}: {stderr_trimmed}",
                out.status,
                dir.display(),
            )
        }
    }
}

/// `git ls-remote <remote>` with prompts disabled, against `dir`.
/// Used by the publish-wiring preflight to validate that the
/// configured remote responds without falling back to interactive
/// auth (which would hang in a non-TTY operator session).
pub fn ls_remote_no_prompt(dir: &Path, remote: &str) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("ls-remote")
        .arg(remote)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes")
        .status()
        .with_context(|| format!("git -C {} ls-remote {remote}", dir.display()))?;
    if !status.success() {
        bail!("git -C {} ls-remote {remote} exited {status}", dir.display());
    }
    Ok(())
}

/// Push a refspec to an explicit destination URL with the
/// PAT-via-env auth header overlaid. Used by `init --push` where
/// the destination is a one-shot URL (not a configured remote).
/// `base_env` is overlaid the same way [`push_with_pat`] does.
pub fn push_url_refspec(
    bare_repo: &Path,
    dest_url: &str,
    refspec: &str,
    env: &[(String, String)],
) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(bare_repo)
        .arg("push")
        .arg(dest_url)
        .arg(refspec)
        .envs(
            env.iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .status()
        .with_context(|| format!("spawn git push {dest_url} {refspec}"))?;
    if !status.success() {
        bail!("git push {dest_url} {refspec} exited {status}");
    }
    Ok(())
}

// ── Test-helper wrappers ─────────────────────────────────────────────
//
// These are useful in `#[cfg(test)]` setup code that needs to bootstrap
// throwaway git repos. They live in the production module (not
// `#[cfg(test)]`) so integration tests can call them too.

/// `git init -q -b <initial_branch>` in `dir`. Returns an error if
/// the repo can't be initialized. Used by test fixtures to bootstrap
/// throwaway repos.
pub fn init_repo(dir: &Path, initial_branch: &str) -> Result<()> {
    run_git(dir, &["init", "-q", "-b", initial_branch])
        .with_context(|| format!("git init -b {initial_branch} in {}", dir.display()))
}

/// `git config <key> <value>` in `dir` — local config only, never
/// touches global or system config. Used by test fixtures to set
/// `user.email` / `user.name` / `commit.gpgsign=false` so commits
/// don't depend on the host's signing setup.
pub fn config_set_local(dir: &Path, key: &str, value: &str) -> Result<()> {
    run_git(dir, &["config", key, value])
        .with_context(|| format!("git config {key} {value} in {}", dir.display()))
}

/// Init a throwaway repo on branch `main` with a deterministic test
/// identity (`user.email=t@t`, `user.name=t`, `commit.gpgsign=false`).
/// Saves the 4-line `init + config × 3` dance from every test that
/// needs a clean repo independent of the host's gitconfig.
pub fn init_test_repo(dir: &Path) -> Result<()> {
    init_repo(dir, "main")?;
    config_set_local(dir, "user.email", "t@t")?;
    config_set_local(dir, "user.name", "t")?;
    config_set_local(dir, "commit.gpgsign", "false")?;
    Ok(())
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
        init_test_repo(dir).unwrap();
        std::fs::write(dir.join("hello.txt"), "hi\n").unwrap();
        run_git(dir, &["add", "hello.txt"]).unwrap();
        run_git(dir, &["commit", "-q", "-m", "init"]).unwrap();

        let outcome = stage_and_commit(dir, &["hello.txt"], "noop", &[]).unwrap();
        assert_eq!(outcome, CommitOutcome::NothingToCommit);
    }

    /// `branch_exists` true for the current branch, false for an
    /// unknown name. Establishes baseline for `checkout_or_create_branch`.
    #[test]
    fn branch_exists_resolves_local_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_test_repo(dir).unwrap();
        std::fs::write(dir.join("seed"), "x").unwrap();
        run_git(dir, &["add", "seed"]).unwrap();
        run_git(dir, &["commit", "-q", "-m", "seed"]).unwrap();

        assert!(branch_exists(dir, "main"));
        assert!(!branch_exists(dir, "session/missing"));
    }

    /// `checkout_or_create_branch` creates the branch on first call
    /// and just checks out on the second — idempotent for archive
    /// re-runs.
    #[test]
    fn checkout_or_create_branch_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_test_repo(dir).unwrap();
        std::fs::write(dir.join("seed"), "x").unwrap();
        run_git(dir, &["add", "seed"]).unwrap();
        run_git(dir, &["commit", "-q", "-m", "seed"]).unwrap();

        // First call: branch doesn't exist → created.
        checkout_or_create_branch(dir, "session/20260518", "main").unwrap();
        assert_eq!(current_branch(dir).unwrap(), "session/20260518");

        // Back to main, then second call to the same target: existing → just checkout.
        run_git(dir, &["checkout", "main"]).unwrap();
        checkout_or_create_branch(dir, "session/20260518", "main").unwrap();
        assert_eq!(current_branch(dir).unwrap(), "session/20260518");
    }

    /// `stage_force_and_commit` bypasses `.gitignore`. This is the
    /// load-bearing behavior the archive flow depends on — `main`
    /// ignores `/sessions/`, but the `session/<id>` branch must commit
    /// the bulk anyway.
    #[test]
    fn stage_force_and_commit_bypasses_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_test_repo(dir).unwrap();
        std::fs::write(dir.join(".gitignore"), "/sessions/\n").unwrap();
        run_git(dir, &["add", ".gitignore"]).unwrap();
        run_git(dir, &["commit", "-q", "-m", "ignore sessions"]).unwrap();

        let session_dir = dir
            .join("sessions")
            .join("20260518");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("results.json"), "{}").unwrap();

        // Sanity: ignored under main's gitignore.
        let stat = run_git_output(dir, &["status", "--porcelain"]).unwrap();
        assert!(!stat.contains("sessions/"), "should be ignored: {stat:?}");

        let outcome =
            stage_force_and_commit(dir, &["sessions/20260518/"], "archive smoke", &[]).unwrap();
        assert_eq!(outcome, CommitOutcome::Committed);

        let head_files =
            run_git_output(dir, &["show", "--name-only", "--pretty=format:", "HEAD"]).unwrap();
        assert!(head_files.contains("sessions/20260518/results.json"), "{head_files}");
    }

    /// `pull_rebase_with_auth` against a local file:// remote
    /// fast-forwards just like its unauthenticated counterpart.
    /// The extraheader IS injected into the env here (this path uses
    /// `auth_header_config_entries` + `merge_git_config_entry`, not
    /// the protocol-gated `build_auth_header_env`), but git's URL
    /// scoping (`http.https://github.com/.extraheader`) means the
    /// header never gets applied to the `file://` fetch — git skips
    /// it silently. This test pins that the env-machinery path
    /// works end-to-end without a real PAT or network.
    #[test]
    fn pull_rebase_with_auth_fast_forwards_against_local_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let remote_dir = tmp.path().join("remote");
        std::fs::create_dir_all(&remote_dir).unwrap();
        run_git(&remote_dir, &["init", "-q", "--bare", "-b", "main"]).unwrap();

        let work_a = tmp.path().join("work-a");
        std::fs::create_dir_all(&work_a).unwrap();
        init_test_repo(&work_a).unwrap();
        run_git(&work_a, &["remote", "add", "origin", remote_dir.to_str().unwrap()]).unwrap();
        std::fs::write(work_a.join("a"), "1").unwrap();
        run_git(&work_a, &["add", "a"]).unwrap();
        run_git(&work_a, &["commit", "-q", "-m", "a"]).unwrap();
        run_git(&work_a, &["push", "-u", "origin", "main"]).unwrap();

        let work_b = tmp.path().join("work-b");
        run_git(tmp.path(), &["clone", "-q", remote_dir.to_str().unwrap(), "work-b"]).unwrap();
        config_set_local(&work_b, "user.email", "t@t").unwrap();
        config_set_local(&work_b, "user.name", "t").unwrap();
        config_set_local(&work_b, "commit.gpgsign", "false").unwrap();
        std::fs::write(work_b.join("b"), "2").unwrap();
        run_git(&work_b, &["add", "b"]).unwrap();
        run_git(&work_b, &["commit", "-q", "-m", "b"]).unwrap();
        run_git(&work_b, &["push", "origin", "main"]).unwrap();

        // Authenticated pull-rebase with a fake token: works because
        // file:// is a non-HTTPS protocol and the extraheader simply
        // isn't propagated for those URLs. Without the auth path's
        // env machinery in place, the fetch step would still need to
        // succeed — and this test verifies it does.
        pull_rebase_with_auth(
            &work_a,
            "origin",
            "main",
            "fake-token",
            &[],
            "x-access-token",
            "https://github.com/",
        )
        .unwrap();
        assert!(work_a.join("b").exists(), "rebase should have brought in peer's `b`");
    }

    /// `pull_rebase` against a local file:// remote fast-forwards
    /// when local is behind. Tests the path without involving a
    /// real network or PAT.
    #[test]
    fn pull_rebase_fast_forwards_against_local_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let remote_dir = tmp.path().join("remote");
        run_git(&remote_dir, &[]).ok(); // create_dir_all via git init below
        std::fs::create_dir_all(&remote_dir).unwrap();
        run_git(&remote_dir, &["init", "-q", "--bare", "-b", "main"]).unwrap();

        let work_a = tmp.path().join("work-a");
        std::fs::create_dir_all(&work_a).unwrap();
        init_test_repo(&work_a).unwrap();
        run_git(&work_a, &["remote", "add", "origin", remote_dir.to_str().unwrap()]).unwrap();
        std::fs::write(work_a.join("a"), "1").unwrap();
        run_git(&work_a, &["add", "a"]).unwrap();
        run_git(&work_a, &["commit", "-q", "-m", "a"]).unwrap();
        run_git(&work_a, &["push", "-u", "origin", "main"]).unwrap();

        // Peer adds a second commit.
        let work_b = tmp.path().join("work-b");
        run_git(tmp.path(), &["clone", "-q", remote_dir.to_str().unwrap(), "work-b"]).unwrap();
        config_set_local(&work_b, "user.email", "t@t").unwrap();
        config_set_local(&work_b, "user.name", "t").unwrap();
        config_set_local(&work_b, "commit.gpgsign", "false").unwrap();
        std::fs::write(work_b.join("b"), "2").unwrap();
        run_git(&work_b, &["add", "b"]).unwrap();
        run_git(&work_b, &["commit", "-q", "-m", "b"]).unwrap();
        run_git(&work_b, &["push", "origin", "main"]).unwrap();

        // work-a is now behind. pull_rebase should fast-forward it.
        pull_rebase(&work_a, "origin", "main").unwrap();
        assert!(work_a.join("b").exists(), "rebase should have brought in peer's `b`");
    }

    /// `stage_and_commit` on a dirty tree (new files matching the
    /// pathspec) produces exactly one commit with the supplied message.
    #[test]
    fn stage_and_commit_produces_commit_when_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_test_repo(dir).unwrap();
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
