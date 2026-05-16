//! `sbagent session optimize ...` — Phase 2/3 optimizer subcommands. Stubs.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::types::SessionId;

pub mod clean;
pub mod run;

/// `sbagent session optimize ...`.
#[derive(Debug, Args)]
pub struct OptimizeArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: OptimizeCommand,
}

/// Optimize subcommands.
#[derive(Debug, Subcommand)]
pub enum OptimizeCommand {
    /// Fan out optimizers + run benchmarks. Stub for `run-optimizers.sh` +
    /// `bench-experiments.sh`.
    Run(run::OptimizeRunArgs),
    /// Clear experiment dirs and worktrees so the next run starts clean.
    Clean(clean::OptimizeCleanArgs),
}

/// Dispatch.
pub async fn run(args: OptimizeArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    match args.command {
        OptimizeCommand::Run(a) => run::run(a, ctx, session_id).await,
        OptimizeCommand::Clean(a) => clean::run(a, ctx, session_id).await,
    }
}
