//! `sbagent session publish` — Phase 5: generate per-target publish
//! artifacts and (unless `--dry-run`) push branches + open PRs/issues.
//!
//! Mirrors the Phase-5 block inside `session run` so it can be exercised
//! standalone — useful as a rehearsal after `finalize` without re-running
//! the whole pipeline. With `--dry-run`, only `publish::generate` runs:
//! no preflight, no token, no network, no git push. The agent still
//! writes `pr-title.txt` / `pr-body.md` / `issue-title.txt` /
//! `issue-body.md` under each target's `optimize/<id>/` so the operator
//! can review them.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::{CliContext, preflight};
use crate::harnesses::codex::CodexHarness;
use crate::session::{SessionLayout, publish};
use crate::types::SessionId;

/// Args for `sbagent session publish`.
#[derive(Debug, Args)]
pub struct PublishArgs {
    /// Skip the push step: generate per-target publish artifacts but
    /// don't push branches, open PRs, or open issues. Skips the
    /// publish-wiring preflight too (token / `publish.base_repo` /
    /// remote auth are only needed by the push path).
    #[clap(long)]
    pub dry_run: bool,
}

/// Run `sbagent session publish`.
pub async fn run(args: PublishArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());

    if !args.dry_run {
        preflight::ensure_publish_wiring(ctx)
            .await
            .context("preflight: publish wiring")?;
    }

    let harness = Arc::new(CodexHarness::new());

    let gen_outputs = publish::generate(&publish::GenerateInputs {
        layout: &layout,
        framework: &ctx.layout,
        settings: &ctx.settings,
        harness: harness.as_ref(),
    })
    .await?;
    println!(
        "publish generate: pr={} issue={} skipped={}",
        gen_outputs.pr_count, gen_outputs.issue_count, gen_outputs.skip_count
    );

    if args.dry_run {
        println!("publish: --dry-run set; skipping push.");
        return Ok(());
    }

    let publish_config = publish::PublishConfig::from_settings(&ctx.settings);
    publish::ensure_token_outside_framework(
        &publish_config.publish_token_file,
        ctx.layout
            .framework
            .as_deref()
            .map(|p| p as &std::path::Path),
    )?;
    let token = publish::read_publish_token(&publish_config.publish_token_file)?;
    let gh = publish::StdGhClient::from_token(&token)?;
    let push_outputs = publish::push(&publish::PushInputs {
        layout: &layout,
        framework: &ctx.layout,
        config: &publish_config,
        gh: &gh,
    })
    .await
    .context("publish push")?;
    println!(
        "publish push: pr={} issue={} skipped={}",
        push_outputs.pr_count, push_outputs.issue_count, push_outputs.skip_count
    );

    Ok(())
}
