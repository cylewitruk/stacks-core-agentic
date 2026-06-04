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

    let outputs = optimizers::run(Inputs {
        layout,
        framework: ctx.layout.clone(),
        settings,
        parallel: args.parallel,
        base_branch: args.base_branch,
        harness,
        git: Arc::new(StdGitCheckoutManager),
        resume: args.resume,
    })
    .await?;

    println!(
        "optimizers: {} landed, {} aborted, {} routed to issue (of {} targets)",
        outputs.landed, outputs.aborted, outputs.routed_to_issue, outputs.total
    );
    Ok(())
}
