//! Shared preflight checks.
//!
//! `cli::check` runs these on demand under `sbagent check --with-publish`.
//! `cli::session::run` runs them upfront when invoked with
//! `--publish-accepted-prs`, so a misconfigured Phase 5 fails before
//! Phases 0-4 burn an hour of compute.
//!
//! With Phase 5 now in-process (no sudo, no separate publisher user),
//! the publish preflight checks the token file (location, readability),
//! API access for `publish.base_repo`, and that `publish.head_owner`
//! is set explicitly. `publish.remote` resolution moves to per-target
//! worktree time (Phase 5), against worktrees that don't exist yet at
//! preflight, so the legacy `<base>` git-side probes are gone.

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
/// 4. `publish.head_owner` is set.
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

    if let Some(msg) = check_publish_remote_is_origin(&cfg.publish_remote) {
        findings.push(msg);
    }

    // `publish.head_owner` names the GitHub owner whose fork holds the
    // bot's `agentic/<session>/<target>` branches. Required: per-session
    // source checkouts inherit `origin` from `[source].url` (rewritten
    // from the local cache path at materialization time), so there is no
    // separate publish remote — Phase 5 pushes `agentic/...` directly to
    // `[source].url`. The operator must set `publish.head_owner`
    // explicitly so the PR head ref (`<head_owner>:<branch>`) matches
    // the owner of the repo Phase 5 actually pushed to.
    match cfg
        .publish_head_owner
        .as_deref()
    {
        None => findings.push(
            "`publish.head_owner` is not set; required. Set it to the GitHub owner whose fork \
             holds the bot's branches (typically the operator's bot account, e.g. `cylewitruk`)."
                .to_owned(),
        ),
        Some(head_owner) => {
            // Cross-check `[source].url` owner against `publish.head_owner`:
            // Phase 5 pushes to `origin` in the per-target clone, which
            // resolves to `[source].url`. If the URL's owner doesn't match
            // `publish.head_owner`, the push will either land somewhere
            // unexpected or fail with permission denied, and the PR head
            // ref `<head_owner>:<branch>` won't resolve. Surface this at
            // preflight rather than at Phase 5 after burning compute.
            if let Some(url) = ctx
                .settings
                .source
                .url
                .as_deref()
                && let Some(url_owner) = github_owner_from_url(url)
                && !url_owner.eq_ignore_ascii_case(head_owner)
            {
                findings.push(format!(
                    "`[source].url` owner `{url_owner}` does not match `publish.head_owner` \
                     `{head_owner}`. Phase 5 pushes to `origin` in per-target clones, which \
                     resolves to `[source].url`; the PR head ref `{head_owner}:<branch>` would \
                     not find a branch at `{url_owner}/...`. Set `[source].url` to a writable \
                     fork at `https://github.com/{head_owner}/...` (or update \
                     `publish.head_owner` to `{url_owner}` if that's the right fork)."
                ));
            }
        }
    }
}

/// Reject any `publish.remote` other than `"origin"` post-v3.
///
/// Per-target clones have exactly one remote (`origin`, pointing at
/// `[source].url`) because they replicate the per-session source
/// checkout's remote map and that checkout has only `origin`. Phase 5
/// runs `git push <publish.remote> <branch>`; with anything but
/// `"origin"` configured the push would fail at Phase 5 with
/// `fatal: '<name>' does not appear to be a git repository`. Reject
/// upfront. A future tunable hook may install a separate publish
/// remote URL into per-target clones; until then, `"origin"` is the
/// only valid value.
fn check_publish_remote_is_origin(remote: &str) -> Option<String> {
    if remote == "origin" {
        return None;
    }
    Some(format!(
        "`publish.remote` = `{remote}` but post-v3-cutover only `origin` is installed in \
         per-target clones (which inherit the single `origin = [source].url` remote from the \
         per-session source checkout). Phase 5's `git push {remote} <branch>` would fail. Set \
         `publish.remote = \"origin\"` (the default — drop the override) until the per-target \
         remote-install hook ships."
    ))
}

/// Best-effort extraction of the GitHub `<owner>` segment from a clone
/// URL. Recognises the three forms `git remote get-url` typically
/// emits:
/// - `https://github.com/<owner>/<repo>(.git)?`
/// - `git@github.com:<owner>/<repo>(.git)?`
/// - `ssh://git@github.com/<owner>/<repo>(.git)?`
///
/// Returns `None` for non-github.com hosts or anything that doesn't
/// parse — the caller treats that as "skip the cross-check" rather
/// than a hard failure (forge-agnostic deployments shouldn't trip).
fn github_owner_from_url(url: &str) -> Option<&str> {
    let path = url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    let owner = path.split('/').next()?;
    if owner.is_empty() { None } else { Some(owner) }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_owner_parses_https() {
        assert_eq!(
            github_owner_from_url("https://github.com/cylewitruk/stacks-core.git"),
            Some("cylewitruk")
        );
        assert_eq!(
            github_owner_from_url("https://github.com/cylewitruk/stacks-core"),
            Some("cylewitruk")
        );
    }

    #[test]
    fn github_owner_parses_ssh_forms() {
        assert_eq!(
            github_owner_from_url("git@github.com:cylewitruk/stacks-core.git"),
            Some("cylewitruk")
        );
        assert_eq!(
            github_owner_from_url("ssh://git@github.com/cylewitruk/stacks-core.git"),
            Some("cylewitruk")
        );
    }

    #[test]
    fn github_owner_returns_none_for_non_github_hosts() {
        // Non-github URLs should not trip the cross-check (forge-agnostic).
        assert_eq!(github_owner_from_url("https://gitlab.example.com/team/repo.git"), None);
        assert_eq!(github_owner_from_url("https://gitea.example.com/owner/repo.git"), None);
    }

    #[test]
    fn github_owner_returns_none_for_garbage() {
        assert_eq!(github_owner_from_url(""), None);
        assert_eq!(github_owner_from_url("not-a-url"), None);
    }

    #[test]
    fn check_publish_remote_accepts_origin() {
        assert!(check_publish_remote_is_origin("origin").is_none());
    }

    #[test]
    fn check_publish_remote_rejects_legacy_bot_name() {
        let msg = check_publish_remote_is_origin("bot")
            .expect("non-origin remote should produce a finding");
        assert!(msg.contains("`bot`"), "finding should quote the configured value: {msg}");
        assert!(msg.contains("`origin`"), "finding should point at the fix: {msg}");
    }

    #[test]
    fn check_publish_remote_rejects_arbitrary_name() {
        assert!(check_publish_remote_is_origin("fork").is_some());
        assert!(check_publish_remote_is_origin("").is_some());
    }
}
