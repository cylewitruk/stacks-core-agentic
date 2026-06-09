//! `sbagent session optimize run` — port of `scripts/run-optimizers.sh`.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::harnesses::codex::CodexHarness;
use crate::session::SessionLayout;
use crate::session::optimizers::{self, Inputs, StdGitCheckoutManager};
use crate::types::SessionId;

/// Args for `sbagent session optimize run`.
#[derive(Debug, Args)]
pub struct OptimizeRunArgs {
    /// Concurrency cap. Defaults to one task per merged target. Clamped
    /// to `1` for sessions with normal_pr targets (Layer 1B v1
    /// constraint).
    #[clap(long)]
    pub parallel: Option<usize>,
    /// Base branch the per-target worktrees check out from.
    #[clap(long, default_value = "feat/stacks-bench")]
    pub base_branch: String,
    /// Override `settings.optimizer.attempts` (Layer 1B inner-loop
    /// attempt cap). Defaults to settings → `5`.
    #[clap(long)]
    pub optimizer_attempts: Option<u32>,
    /// Override `settings.optimizer.budget_minutes` (Layer 1B
    /// inner-loop wall-clock budget). Defaults to settings → `60`.
    #[clap(long)]
    pub optimizer_budget_minutes: Option<u32>,
    /// Skip targets that already carry a valid typed
    /// `optimizer-report.json` for this session; only re-run targets
    /// whose report is missing, corrupt, or context-mismatched. Used
    /// to recover a partially-failed optimizer phase without redoing
    /// the targets that already succeeded.
    #[clap(long)]
    pub resume: bool,
    /// Skip the session-start preflight. See `session run --help` for
    /// the same flag's full rationale.
    #[clap(long)]
    pub skip_preflight: bool,
}

/// Fan out one optimizer per merged target.
pub async fn run(args: OptimizeRunArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    if !args.skip_preflight {
        let findings =
            crate::session::preflight::collect_findings(ctx).context("session-start preflight")?;
        crate::session::preflight::report(&findings)?;
    }

    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let harness = Arc::new(CodexHarness::new());

    let mut settings = ctx.settings.clone();
    if let Some(n) = args.optimizer_attempts {
        settings.optimizer.attempts = Some(n);
    }
    if let Some(m) = args.optimizer_budget_minutes {
        settings
            .optimizer
            .budget_minutes = Some(m);
    }

    // v3 Phase 3: the standalone `session optimize run` consumes the
    // per-session source checkout previously materialized by
    // `session run`. Read source.json — it's the truth — and derive
    // the per-session checkout path from the RECORDED cache_id, NOT
    // from current settings (an operator who changes/removes
    // `[source].id` between invocations must still find the original
    // checkout). Bails with a clear pointer if no prior `session run`
    // has materialized the source for this session.
    let source =
        crate::models::source::SourceJson::read(&layout.source_json()).with_context(|| {
            "v3 Phase 3: per-session source.json missing — run `sbagent session run` first to \
             materialize the source checkout (or remove the partial session dir to start over)"
        })?;
    let workspace_root = ctx
        .layout
        .require_agent_workspace_root()?;
    let source_checkout = crate::source::repo::session_repo_dir_for(
        workspace_root,
        session_id.as_str(),
        &source.cache_id,
    );

    let outputs = optimizers::run(Inputs {
        layout,
        framework: ctx.layout.clone(),
        settings,
        parallel: args.parallel,
        base_branch: args.base_branch,
        harness,
        git: Arc::new(StdGitCheckoutManager),
        resume: args.resume,
        source_checkout,
    })
    .await?;

    println!(
        "optimizers: {} landed, {} aborted, {} routed to issue (of {} targets)",
        outputs.landed, outputs.aborted, outputs.routed_to_issue, outputs.total
    );
    Ok(())
}
