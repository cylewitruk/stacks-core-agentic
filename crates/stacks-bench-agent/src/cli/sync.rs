//! `sbagent sync` — refresh the operator's on-disk bundles
//! (`.sbagent/{schemas,queries,prompts,context}/`) from the binary's
//! embedded defaults and optionally commit + push the result.
//!
//! The operator's `.sbagent/` tree is a **mirror** of the binary's
//! embedded bundle. After an `sbagent` upgrade, run `sbagent sync` to
//! pull the new version's bundles onto disk.
//!
//! Write semantics (default-flipped 2026-05-21):
//! - **Schemas + queries** always overwrite. They are versioned contract, not a
//!   tuning surface; the operator's on-disk copy must always reflect the
//!   running binary.
//! - **Prompts + context docs** also overwrite by default. The bundled
//!   templates are the contract surface; silent operator edits drifted
//!   load-bearing prompts (e.g. `optimizer.md`) past what the orchestrator's
//!   typed-contract gate expects and burned operator time mid-session. Pass
//!   `--keep-tunables` to preserve operator edits while still refreshing
//!   schemas + queries. The legacy `--force-tunables` / `--force-prompts` flags
//!   are accepted as no-op aliases for one release.
//!
//! `--commit` produces a single bot-authored commit covering whatever
//! `.sbagent/` paths actually changed. `--push` implies `--commit`
//! and pushes the resulting branch using the same PAT-via-env auth
//! mechanism `sbagent init --push` uses (token never enters argv,
//! `.git/config`, or shell history). The flags exist so a scheduled
//! CI workflow can run `sbagent sync --push` after each binary
//! upgrade without spelling out the git + auth dance in shell.
//!
//! `sbagent check` reports schema / query drift as a failure and
//! prompt drift as a failure on the load-bearing `optimizer.md`, so
//! an operator who upgrades the binary without running sync sees a
//! loud "run sbagent sync" message rather than silently broken
//! validation. Session-start preflight ([`crate::session::preflight`])
//! runs the same prompt-drift check before any heavy phase.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::cli::CliContext;
use crate::session::optimizers::optimizer_git_env;
use crate::session::publish;
use crate::{context, git, prompts, queries, schemas};

/// Args for `sbagent sync`.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Preserve the operator's on-disk prompt templates and context
    /// docs from being overwritten with the binary's bundled defaults.
    /// By default `sync` ALWAYS refreshes both tunable bundles (along
    /// with schemas + queries) — the bundled templates are the
    /// contract surface, and silent operator edits have repeatedly
    /// drifted load-bearing prompts (e.g. `optimizer.md`) past what
    /// the orchestrator's typed-contract gate expects. Pass
    /// `--keep-tunables` only when you've consciously edited prompts /
    /// context docs and want to merge bundled changes manually.
    ///
    /// The legacy `--force-tunables` and `--force-prompts` flags are
    /// accepted as deprecated no-op aliases for one release (the
    /// behavior they used to opt into is now the default). Both emit
    /// a deprecation notice and resolve to `keep_tunables = false`.
    #[clap(long = "keep-tunables")]
    pub keep_tunables: bool,

    /// Deprecated: pre-flip-of-defaults muscle memory. Sync now
    /// refreshes tunables by default; this flag was the opt-in for
    /// that behavior. Now a no-op. Pass `--keep-tunables` to preserve
    /// operator edits.
    #[clap(long = "force-tunables", alias = "force-prompts", hide = true)]
    pub force_tunables_deprecated: bool,

    /// After writing the bundles, stage the changed `.sbagent/` paths
    /// and produce a single commit authored as the bot (identity from
    /// `git.author_name` / `git.author_email`). Skipped silently when
    /// nothing changed. Implied by `--push`.
    #[clap(long)]
    pub commit: bool,

    /// Implies `--commit`. After committing, push the current branch
    /// to `origin` using the bot's PAT (read from `publish.token_file`).
    /// Origin must match `git.auth_url_prefix` (default
    /// `https://github.com/`) — same validation as `sbagent init --push`.
    #[clap(long)]
    pub push: bool,
}

/// Run `sbagent sync`. Always rewrites schemas + queries; rewrites
/// prompts + context UNLESS `--keep-tunables` is set (default-flipped
/// from the prior behavior where prompts were left alone — see
/// `SyncArgs::keep_tunables` for rationale).
pub async fn run(args: SyncArgs, ctx: &CliContext) -> Result<()> {
    if args.force_tunables_deprecated {
        eprintln!(
            "note: `--force-tunables` / `--force-prompts` is now a no-op (sync refreshes tunables \
             by default); this flag will be removed in a future release. Drop it from your \
             scripts.",
        );
    }
    let refresh_tunables = !args.keep_tunables;

    // ── 1. Refresh bundles on disk ────────────────────────────────
    // Legacy context-doc migration runs at startup (in
    // `CliContext::from_args`, before `context::seed_to` would
    // populate the destination), so by the time sync runs, the
    // context bundle is already in the right place.
    let schemas_written = schemas::sync(&ctx.layout.schemas_dir).with_context(|| {
        format!(
            "syncing schemas into {}",
            ctx.layout
                .schemas_dir
                .display()
        )
    })?;
    println!(
        "sync: rewrote {} schema(s) in {}",
        schemas_written.len(),
        ctx.layout
            .schemas_dir
            .display(),
    );
    for name in &schemas_written {
        println!("  - {name}");
    }

    let queries_written = queries::sync(&ctx.layout.queries_dir).with_context(|| {
        format!(
            "syncing queries into {}",
            ctx.layout
                .queries_dir
                .display()
        )
    })?;
    println!(
        "sync: rewrote {} query/-ies in {}",
        queries_written.len(),
        ctx.layout
            .queries_dir
            .display(),
    );
    for name in &queries_written {
        println!("  - {name}");
    }

    if refresh_tunables {
        let prompts_dir = ctx
            .settings
            .require_prompt_overrides_dir()
            .context(
                "sync requires `prompt_overrides_dir` in config to refresh tunables; pass \
                 `--keep-tunables` to skip the tunable refresh if you can't set it",
            )?;
        let written = prompts::sync_force(prompts_dir)
            .with_context(|| format!("refreshing prompts into {}", prompts_dir.display()))?;
        println!("sync: rewrote {} prompt(s) in {}", written.len(), prompts_dir.display(),);
        for name in &written {
            println!("  - {name}");
        }
        let written = context::sync_force(&ctx.layout.context_dir).with_context(|| {
            format!(
                "refreshing context docs into {}",
                ctx.layout
                    .context_dir
                    .display()
            )
        })?;
        println!(
            "sync: rewrote {} context file(s) in {}",
            written.len(),
            ctx.layout
                .context_dir
                .display(),
        );
        for name in &written {
            println!("  - {name}");
        }
    } else {
        println!(
            "sync: prompts + context left alone (--keep-tunables); operator edits preserved. Be \
             aware that load-bearing prompt drift (esp. optimizer.md) can break the \
             orchestrator's typed-report gate at session-start preflight.",
        );
    }

    // ── 2. Optionally commit ──────────────────────────────────────
    // Both `--commit` alone and `--push` (which implies --commit)
    // route through here. The git ops run from cwd, which by
    // convention is the operator dir; bundles are absolutized in
    // Layout so paths are valid regardless.
    if args.commit || args.push {
        commit_bundles(ctx, refresh_tunables).context("`sbagent sync --commit`")?;
    }

    // ── 3. Optionally push ────────────────────────────────────────
    if args.push {
        push_current_branch(ctx).context("`sbagent sync --push`")?;
    }

    Ok(())
}

/// Stage the changed `.sbagent/` paths (always schemas + queries; also
/// prompts + context unless `--keep-tunables` was set) and produce
/// one bot-authored commit. Skipped silently when nothing changed.
fn commit_bundles(ctx: &CliContext, refresh_tunables: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("getting cwd for `sbagent sync --commit`")?;
    if !is_git_repo(&cwd) {
        bail!(
            "`--commit` requires the operator dir to be a git repo. cwd ({}) has no .git/. Run \
             `sbagent init` first, or `cd` into the operator dir before invoking sync.",
            cwd.display(),
        );
    }

    // Build the pathspec relative to cwd. `Layout` absolutizes the
    // bundle dirs, so we strip the cwd prefix to get a git-friendly
    // relative path. Falls back to the absolute path when the bundle
    // lives outside the operator dir — that's a non-standard layout
    // but git handles absolute pathspecs fine.
    let mut pathspecs: Vec<String> = Vec::new();
    let schemas_rel = rel_to(&ctx.layout.schemas_dir, &cwd);
    let queries_rel = rel_to(&ctx.layout.queries_dir, &cwd);
    pathspecs.push(schemas_rel);
    pathspecs.push(queries_rel);
    if refresh_tunables {
        if let Some(dir) = ctx
            .settings
            .layout
            .prompt_overrides_dir
            .as_deref()
        {
            pathspecs.push(rel_to(dir, &cwd));
        }
        pathspecs.push(rel_to(&ctx.layout.context_dir, &cwd));
    }
    let pathspec_refs: Vec<&str> = pathspecs
        .iter()
        .map(String::as_str)
        .collect();

    let version = env!("CARGO_PKG_VERSION");
    let msg = format!("chore: sync sbagent bundles ({version})");
    let env = optimizer_git_env(&ctx.settings);
    match git::stage_and_commit(&cwd, &pathspec_refs, &msg, &env)? {
        git::CommitOutcome::Committed => {
            println!("sync: committed `{msg}`");
        }
        git::CommitOutcome::NothingToCommit => {
            println!("sync: commit skipped (no changes after seeding)");
        }
    }
    Ok(())
}

/// Push the current branch to `origin` via PAT-via-env. Mirrors the
/// `--push` flow in `sbagent init` (including the auth-url-prefix
/// origin gate); see [`crate::cli::init::run`] for the full mechanism.
fn push_current_branch(ctx: &CliContext) -> Result<()> {
    let cwd = std::env::current_dir().context("getting cwd for `sbagent sync --push`")?;

    // Same precondition order as `init --push`: `publish.token_file`
    // configured + readable, origin present + HTTPS-matching.
    let token_path = ctx
        .settings
        .publish
        .token_file_required()
        .context("`sbagent sync --push`")?;
    let token = publish::read_publish_token(token_path)?;

    if !git::run_git_check(&cwd, &["remote", "get-url", "origin"]) {
        bail!(
            "`--push` requires `origin` configured in {}. Run:\n  git -C {} remote add origin \
             <operator-repo-url>\nthen re-run with `--push`.",
            cwd.display(),
            cwd.display(),
        );
    }
    let origin_url = git::run_git_output(&cwd, &["remote", "get-url", "origin"])
        .context("reading origin URL")?;
    let auth_username = ctx
        .settings
        .git
        .effective_auth_username();
    let auth_url_prefix = ctx
        .settings
        .git
        .effective_auth_url_prefix()?;
    git::validate_auth_url(&origin_url, &auth_url_prefix, "`--push`'s `origin`")?;

    let branch = git::run_git_output(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .context("reading current branch")?;
    if branch == "HEAD" {
        bail!(
            "`--push` requires a checked-out branch; HEAD is detached in {}. Check out a branch \
             before re-running.",
            cwd.display(),
        );
    }

    let env = optimizer_git_env(&ctx.settings);
    git::push_with_pat(&cwd, "origin", &branch, &token, &env, auth_username, &auth_url_prefix)
        .context("git push -u origin via PAT header")?;
    println!("sync: pushed origin {branch}");
    Ok(())
}

/// Cheap "is this a git repo?" check — `.git` exists (either dir or
/// file, the latter for linked worktrees).
fn is_git_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Return `path` relative to `base` when possible, else the absolute
/// form. `git add` accepts both; the relative form is just easier to
/// read in commit metadata.
fn rel_to(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .map(|p| {
            p.to_string_lossy()
                .into_owned()
        })
        .unwrap_or_else(|_| {
            path.to_string_lossy()
                .into_owned()
        })
}
