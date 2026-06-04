//! Shared preflight checks.
//!
//! `cli::check` runs these on demand under `sbagent check --with-publish`.
//! `cli::session::run` runs them upfront when invoked with
//! `--publish-accepted-prs`, so a misconfigured Phase 5 fails before
//! Phases 0-4 burn an hour of compute.
//!
//! With Phase 5 now in-process (no sudo, no separate publisher user),
//! the publish preflight checks the token file (location, readability),
//! API access for `publish.base_repo`, and the git-side push path
//! (remote configured, github.com URL, auth works via `ls-remote`).

use anyhow::{Result, bail};

use super::CliContext;
use crate::session::publish::{self, PublishConfig, StdGhClient};

/// Append publish-wiring findings to `findings`. Probes:
///
/// 1. `<publish.token_file>` lives outside the framework root (Codex can read
///    anything inside it during `publish generate`).
/// 2. `<publish.token_file>` exists, is readable by `sbagent`, and is
///    non-empty.
/// 3. `octocrab.repos(owner, repo).get()` succeeds against `publish.base_repo`
///    with the token (catches a wrong repo or an unauthorized token before any
///    PR is opened).
/// 4. `git -C <base> remote get-url <publish.remote>` succeeds (catches a
///    missing remote or a path-not-a-checkout case before Phase 5 attempts to
///    push).
/// 5. The remote URL parses to a github.com `owner/repo`.
/// 6. `git -C <base> ls-remote <publish.remote>` succeeds — validates the SSH
///    key / HTTPS credentials the eventual `git push` will use, so fresh VMs
///    with a correctly-named remote but missing auth fail at preflight rather
///    than late in Phase 5.
pub async fn collect_publish_findings(ctx: &CliContext, findings: &mut Vec<String>) {
    let cfg = PublishConfig::from_settings(&ctx.settings);

    if let Err(e) = publish::ensure_token_outside_framework(
        &cfg.publish_token_file,
        ctx.layout
            .framework
            .as_deref()
            .map(|p| p as &std::path::Path),
    ) {
        findings.push(format!("{e:#}"));
        return;
    }

    let token = match publish::read_publish_token(&cfg.publish_token_file) {
        Ok(t) => t,
        Err(e) => {
            findings.push(format!(
                "publish.token_file at {} is unreadable or empty: {e:#}",
                cfg.publish_token_file
                    .display()
            ));
            return;
        }
    };
    let client = match StdGhClient::from_token(&token) {
        Ok(c) => c,
        Err(e) => {
            findings.push(format!("constructing octocrab client: {e:#}"));
            return;
        }
    };
    let Some((owner, repo)) = cfg
        .publish_base_repo
        .split_once('/')
    else {
        findings.push(format!(
            "publish.base_repo `{}` is not in `owner/repo` form",
            cfg.publish_base_repo
        ));
        return;
    };
    if let Err(e) = client
        .api
        .repos(owner, repo)
        .get()
        .await
    {
        findings.push(format!(
            "GET repos/{}/{} via octocrab failed: {e:#}; confirm `publish.base_repo` is correct \
             and the token has access",
            owner, repo
        ));
    }

    // git side: `publish push` shells `git push -u <remote> <branch>` and
    // derives `head_owner` from `git remote get-url <remote>`. Probe
    // both upfront so a fresh VM with a missing/typoed remote fails
    // here, not after Phase 4.
    let base = match ctx.layout.base.as_deref() {
        Some(p) => p,
        None => {
            findings.push(
                "`stacks_core.base` (stacks-core checkout path) not set in settings; cannot probe \
                 git remote / ls-remote. Configure `stacks_core.base` in config.toml and re-run."
                    .to_owned(),
            );
            return;
        }
    };
    match crate::git::get_remote_url(base, &cfg.publish_remote) {
        Err(e) => findings.push(format!(
            "{e:#}; configure the remote or set `publish.remote` to the correct name",
        )),
        Ok(url) if !is_github_url(&url) => findings.push(format!(
            "remote `{}` resolves to {url}, which is not a github.com URL; the publish path can \
             only push to github.com",
            cfg.publish_remote
        )),
        Ok(_) => {}
    }

    // Validate that `git push` will actually authenticate. `ls-remote`
    // hits the same auth path (SSH agent / HTTPS credential helper) as
    // push but is read-only, so a misconfigured fresh VM fails here
    // instead of after Phase 4. Force non-interactive: a missing
    // credential / unknown SSH host / passphrase-protected key would
    // otherwise prompt and block preflight forever.
    //
    // - `GIT_TERMINAL_PROMPT=0` disables git's own credential prompts.
    // - `GIT_SSH_COMMAND=ssh -oBatchMode=yes` disables ssh's prompts (host trust,
    //   passphrase, etc.) so failures surface immediately as non-zero exits.
    if let Err(e) = crate::git::ls_remote_no_prompt(base, &cfg.publish_remote) {
        findings.push(format!(
            "{e:#}; verify the SSH key / HTTPS credentials this user has for `git push <remote>`",
        ));
    }
}

/// True when `url` is one of the github.com URL forms `derive_head_owner`
/// recognises (`git@github.com:owner/repo.git` /
/// `https://github.com/owner/repo`).
fn is_github_url(url: &str) -> bool {
    url.starts_with("git@github.com:") || url.starts_with("https://github.com/")
}

/// Run `collect_publish_findings` and bail with the aggregated list on
/// any failure. Used by `session run --publish-accepted-prs` to fail
/// fast before Phase 0 instead of after Phase 4.
pub async fn ensure_publish_wiring(ctx: &CliContext) -> Result<()> {
    let mut findings = Vec::new();
    collect_publish_findings(ctx, &mut findings).await;
    if findings.is_empty() {
        return Ok(());
    }
    for f in &findings {
        eprintln!("FAIL: {f}");
    }
    bail!(
        "{} publish-wiring check(s) failed; re-run `sbagent check --with-publish` after fixing, \
         or omit `--publish-accepted-prs` to skip Phase 5",
        findings.len()
    )
}
