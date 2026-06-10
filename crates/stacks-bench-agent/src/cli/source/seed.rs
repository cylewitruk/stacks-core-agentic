//! `sbagent source seed` — bare-clone a branch from a source URL and
//! push it to a destination URL.
//!
//! Replaces the pre-v3 `sbagent init --seed-from` flow: operators
//! bootstrapping a brand-new bot fork still need to seed
//! `[source].branch` onto it before the first session can fetch.
//! Splitting the concern out of `init` keeps it scoped to the bot-fork
//! seeding case rather than mixing into operator-dir bootstrap.
//!
//! ## Auth & URL handling — differs from `init --push`
//!
//! `init --push` is opinionated about the operator's `origin` URL
//! (always HTTPS via PAT) because the bootstrap contract requires
//! it. `source seed` is a one-shot operator-driven push to a URL
//! they typed on the command line; SSH is a legitimate operator
//! choice:
//!
//! - `--to` starts with `https://`: validate against `git.auth_url_prefix`
//!   (matches `init --push`'s strictness) and attach the PAT via
//!   `http.<prefix>.extraheader` env. Requires `publish.token_file`.
//! - `--to` starts with `git@` / `ssh://` / `file://`: skip the
//!   `validate_auth_url` gate; do not require a PAT; the operator's SSH agent /
//!   local fs handles auth. The auth mode is logged so the operator can
//!   confirm.
//! - Plain `http://` is rejected — the auth helper requires HTTPS to attach a PAT,
//!   and silent-fallback-to-no-auth on `http://` would defeat the contract.

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::cli::CliContext;
use crate::git::{build_auth_header_env, clone_bare_branch, push_url_refspec, validate_auth_url};
use crate::session::publish::read_publish_token;

/// Args for `sbagent source seed`.
#[derive(Debug, Clone, Args)]
pub struct SeedArgs {
    /// Source URL to clone the branch from. Typically a fork that
    /// already carries the branch (e.g. the human operator's
    /// personal fork during pilot).
    #[clap(long)]
    pub from: String,

    /// Destination URL to push to. Typically the bot's writable
    /// fork (`[source].url` post-cutover). Required.
    ///
    /// **Accepted URL forms:** `https://...` (PAT injected via
    /// `http.<prefix>.extraheader`), `git@host:owner/repo` /
    /// `ssh://...` (operator's SSH agent handles auth, no PAT),
    /// `file://...` (local push, no PAT).
    ///
    /// Plain `http://` is rejected — the PAT-via-env mechanism
    /// requires HTTPS; silently falling back to no-auth on `http://`
    /// would defeat the contract.
    #[clap(long)]
    pub to: String,

    /// Branch to seed. Defaults to `[source].branch` from settings;
    /// required when `[source].branch` is unset.
    #[clap(long)]
    pub branch: Option<String>,
}

/// Auth mode chosen for the `--to` URL. Reported via info log so the
/// operator can confirm which credential path will be used.
#[derive(Debug, PartialEq, Eq)]
enum AuthMode {
    /// HTTPS with PAT-via-env injection.
    HttpsPat,
    /// SSH (`git@` / `ssh://`) — operator's SSH agent handles auth.
    Ssh,
    /// `file://` local push — no auth needed.
    LocalFile,
}

impl AuthMode {
    fn describe(&self) -> &'static str {
        match self {
            Self::HttpsPat => "HTTPS with PAT via http.extraheader",
            Self::Ssh => "SSH (operator's agent / key)",
            Self::LocalFile => "file:// (no auth)",
        }
    }
}

/// Classify the `--to` URL into one of the three accepted auth
/// modes, rejecting `http://` and unknown schemes.
fn classify_dest_url(url: &str) -> Result<AuthMode> {
    if url.starts_with("https://") {
        Ok(AuthMode::HttpsPat)
    } else if url.starts_with("git@") || url.starts_with("ssh://") {
        Ok(AuthMode::Ssh)
    } else if url.starts_with("file://") {
        Ok(AuthMode::LocalFile)
    } else if url.starts_with("http://") {
        bail!(
            "`--to` URL `{url}` uses plain HTTP; HTTPS is required for PAT injection. Use \
             `https://...`, `git@...`, `ssh://...`, or `file://...`."
        )
    } else {
        bail!(
            "`--to` URL `{url}` has unsupported scheme; expected one of `https://`, `git@`, \
             `ssh://`, `file://`."
        )
    }
}

/// Resolve the branch argument: explicit `--branch` wins; otherwise
/// `[source].branch`; otherwise hard fail with a clear pointer.
fn resolve_branch(args: &SeedArgs, ctx: &CliContext) -> Result<String> {
    if let Some(b) = &args.branch {
        return Ok(b.clone());
    }
    ctx.settings
        .source
        .branch
        .clone()
        .context(
            "no --branch given and `[source].branch` is unset; pass `--branch <name>` or set \
             `[source].branch` in config.toml",
        )
}

/// Run `sbagent source seed`.
pub fn run(args: SeedArgs, ctx: &CliContext) -> Result<()> {
    let branch = resolve_branch(&args, ctx)?;
    let auth_mode = classify_dest_url(&args.to)?;
    tracing::info!(
        from = %args.from,
        to = %args.to,
        branch = %branch,
        auth = auth_mode.describe(),
        "seeding branch",
    );

    // HTTPS-only knobs: validate against `git.auth_url_prefix` +
    // read the PAT. Skipped entirely for SSH/file:// dests so the
    // operator doesn't need a `publish.token_file` configured to
    // seed a personal fork via their SSH key.
    let push_env = if auth_mode == AuthMode::HttpsPat {
        let auth_url_prefix = ctx
            .settings
            .git
            .effective_auth_url_prefix()?;
        validate_auth_url(&args.to, &auth_url_prefix, "`--to`")?;
        let auth_username = ctx
            .settings
            .git
            .effective_auth_username();
        let token_file = ctx
            .settings
            .publish
            .token_file_required()
            .context("`sbagent source seed` against an HTTPS destination")?;
        let token = read_publish_token(token_file)?;
        build_auth_header_env(&args.to, &token, auth_username, &auth_url_prefix)
    } else {
        Vec::new()
    };

    // Stage the push from a throwaway bare clone of `--from`. The
    // tempdir is dropped on return (RAII), so the staging area
    // doesn't pollute the workspace.
    let staging = tempfile::tempdir().context("creating tempdir for seed bare clone")?;
    let bare = staging
        .path()
        .join("seed.git");
    clone_bare_branch(&args.from, &branch, &bare)?;

    let refspec = format!("{branch}:refs/heads/{branch}");
    push_url_refspec(&bare, &args.to, &refspec, &push_env).with_context(|| match auth_mode {
        AuthMode::HttpsPat => format!(
            "push to {} failed; either the PAT lacks Contents:write on that repo, or the \
             destination branch has diverged and a non-fast-forward push was rejected",
            args.to,
        ),
        _ => format!(
            "push to {} failed; verify the operator's SSH/local credentials for that destination, \
             or that the destination branch hasn't diverged",
            args.to,
        ),
    })?;

    eprintln!("seeded {branch} on {} from {}", args.to, args.from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_https_url() {
        assert_eq!(
            classify_dest_url("https://github.com/owner/repo.git").unwrap(),
            AuthMode::HttpsPat,
        );
    }

    #[test]
    fn classify_ssh_forms() {
        assert_eq!(classify_dest_url("git@github.com:owner/repo.git").unwrap(), AuthMode::Ssh,);
        assert_eq!(
            classify_dest_url("ssh://git@github.com/owner/repo.git").unwrap(),
            AuthMode::Ssh,
        );
    }

    #[test]
    fn classify_file_url() {
        assert_eq!(classify_dest_url("file:///tmp/repo.git").unwrap(), AuthMode::LocalFile);
    }

    #[test]
    fn classify_rejects_plain_http_with_https_remediation() {
        let err = classify_dest_url("http://github.com/owner/repo.git").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("plain HTTP"), "{msg}");
        assert!(msg.contains("HTTPS"), "{msg}");
    }

    #[test]
    fn classify_rejects_unknown_scheme() {
        let err = classify_dest_url("ftp://example.com/repo.git").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unsupported scheme"), "{msg}");
    }
}
