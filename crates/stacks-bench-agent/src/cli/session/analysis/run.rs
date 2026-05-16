//! `sbagent session analysis run` — port of `scripts/run-analyzers.sh`.

use std::sync::Arc;

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::harnesses::codex::CodexHarness;
use crate::session::SessionLayout;
use crate::session::analyzers::{self, Inputs};
use crate::types::SessionId;

/// Args for `sbagent session analysis run`.
#[derive(Debug, Args)]
pub struct AnalysisRunArgs {
    /// Concurrency cap for the analyzer fan-out. Defaults to one task per
    /// candidate family.
    #[clap(long)]
    pub parallel: Option<usize>,
}

/// Fan out one analyzer per candidate family.
pub async fn run(args: AnalysisRunArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let harness = Arc::new(CodexHarness::new());

    let outputs = analyzers::run(Inputs {
        layout,
        framework: ctx.layout.clone(),
        settings: ctx.settings.clone(),
        parallel: args.parallel,
        harness,
    })
    .await?;

    println!(
        "analyses: {} accepted, {} rejected (of {} total)",
        outputs.accepted, outputs.rejected, outputs.total
    );
    Ok(())
}
