//! `sbagent publish ...` — Phase 5 publishing commands.
//!
//! Two subcommands:
//! - [`generate`] invokes the pr-writer / issue-writer prompts and writes
//!   per-target title/body files.
//! - [`push`] reads the GitHub token from `publish_token_file` in-process, uses
//!   `octocrab` for PR/issue creation, and shells `git` for worktree → branch →
//!   push. No sudo, no separate publisher user.

use anyhow::{Context as _, Result};
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::harnesses::codex::CodexHarness;
use crate::session::publish::{
    self, GenerateInputs, PUBLISH_ARTIFACT_FILE_NAMES, PublishConfig, PushInputs, StdGhClient,
};
use crate::session::{SessionLayout, clean};
use crate::types::SessionId;

/// `sbagent publish ...`.
#[derive(Debug, Args)]
pub struct PublishArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: PublishCommand,
}

/// Publish subcommands.
#[derive(Debug, Subcommand)]
pub enum PublishCommand {
    /// Generate per-target PR/issue text (agent-side; no GitHub access).
    Generate(GenerateCommandArgs),
    /// Push generated PRs/issues to GitHub. Reads the token from
    /// `publish_token_file`.
    Push(PushCommandArgs),
    /// Clear per-target PR/issue artifacts so the next `publish generate`
    /// re-renders from scratch. Does NOT touch already-pushed PRs/issues
    /// on GitHub.
    Clean(CleanCommandArgs),
}

/// Args for `sbagent publish generate`.
#[derive(Debug, Args)]
pub struct GenerateCommandArgs {
    /// Session id.
    #[clap(long, env = "SBAGENT_SESSION_ID")]
    pub session_id: SessionId,
}

/// Args for `sbagent publish push`.
#[derive(Debug, Args)]
pub struct PushCommandArgs {
    /// Session id.
    #[clap(long, env = "SBAGENT_SESSION_ID")]
    pub session_id: SessionId,
}

/// Args for `sbagent publish clean`.
#[derive(Debug, Args)]
pub struct CleanCommandArgs {
    /// Session id.
    #[clap(long, env = "SBAGENT_SESSION_ID")]
    pub session_id: SessionId,
}

/// Dispatch.
pub async fn run(args: PublishArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        PublishCommand::Generate(a) => run_generate(a, ctx).await,
        PublishCommand::Push(a) => run_push(a, ctx).await,
        PublishCommand::Clean(a) => run_clean(a, ctx).await,
    }
}

async fn run_generate(args: GenerateCommandArgs, ctx: &CliContext) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, args.session_id.clone());
    let harness = CodexHarness::new();

    let outputs = publish::generate(&GenerateInputs {
        layout: &layout,
        framework: &ctx.layout,
        settings: &ctx.settings,
        harness: &harness,
    })
    .await?;
    println!(
        "Generated artifacts for {} PR target(s), {} issue target(s); {} skipped.",
        outputs.pr_count, outputs.issue_count, outputs.skip_count
    );
    Ok(())
}

async fn run_clean(args: CleanCommandArgs, ctx: &CliContext) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, args.session_id.clone());
    let mut report = clean::CleanReport::default();

    // Walk optimize/<target>/ and remove every per-target publish
    // artifact (pr-title.txt, pr-body.md, pr-writer-* / issue-writer-*).
    // Optimizer-side artifacts (implementation/abort markers) under
    // the same per-target dir are intentionally left intact —
    // publish clean exists to re-render PR/issue text without
    // re-running the optimizer.
    if let Ok(rd) = std::fs::read_dir(layout.optimize_dir()) {
        for entry in rd.flatten() {
            if !entry
                .file_type()
                .map(|t| t.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let target_dir = entry.path();
            for name in PUBLISH_ARTIFACT_FILE_NAMES {
                report.merge(clean::remove_one(&target_dir.join(name))?);
            }
        }
    }
    clean::print_report("publish clean", &report);
    Ok(())
}

async fn run_push(args: PushCommandArgs, ctx: &CliContext) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, args.session_id.clone());
    let config = PublishConfig::from_settings(&ctx.settings);
    publish::ensure_token_outside_framework(
        &config.publish_token_file,
        ctx.layout
            .framework
            .as_deref()
            .map(|p| p as &std::path::Path),
    )?;
    let token = publish::read_publish_token(&config.publish_token_file)?;
    let gh = StdGhClient::from_token(&token)?;
    let outputs = publish::push(&PushInputs {
        layout: &layout,
        framework: &ctx.layout,
        config: &config,
        gh: &gh,
    })
    .await
    .context("publish push failed")?;
    println!(
        "publish-accepted: {} PR(s), {} issue(s); {} skipped.",
        outputs.pr_count, outputs.issue_count, outputs.skip_count
    );
    Ok(())
}
