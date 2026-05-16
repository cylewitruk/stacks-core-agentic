//! `sbagent session finalize ...` — Phase 4 finalize subcommands.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::types::SessionId;

pub mod clean;
pub mod render;
pub mod run;

/// `sbagent session finalize ...`.
#[derive(Debug, Args)]
pub struct FinalizeArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: FinalizeCommand,
}

/// Finalize subcommands.
#[derive(Debug, Subcommand)]
pub enum FinalizeCommand {
    /// Emit `summary.json` + `summary.md` from the session's accepted
    /// artifacts.
    Run(run::FinalizeRunArgs),
    /// Re-render `summary.md` + `targets.md` from the on-disk JSON
    /// without re-querying the stacks-bench database.
    Render(render::FinalizeRenderArgs),
    /// Drop the previously-emitted `summary.json` / `summary.md` so
    /// `finalize run` re-derives them from scratch.
    Clean(clean::FinalizeCleanArgs),
}

/// Dispatch.
pub async fn run(args: FinalizeArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    match args.command {
        FinalizeCommand::Run(a) => run::run(a, ctx, session_id).await,
        FinalizeCommand::Render(a) => render::run(a, ctx, session_id).await,
        FinalizeCommand::Clean(a) => clean::run(a, ctx, session_id).await,
    }
}
