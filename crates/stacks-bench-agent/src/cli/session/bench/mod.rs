//! `sbagent session bench ...` — Phase 3 bench-experiment subcommands.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::types::SessionId;

pub mod clean;
pub mod run;

/// `sbagent session bench ...`.
#[derive(Debug, Args)]
pub struct BenchArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: BenchCommand,
}

/// Bench subcommands.
#[derive(Debug, Subcommand)]
pub enum BenchCommand {
    /// Build per-target release binaries, then run one stacks-bench
    /// invocation per `verification_replay.invocations[]` entry under
    /// the bench lock.
    Run(run::BenchRunArgs),
    /// Clear per-target `candidate-run-ids.json` and per-invocation
    /// bench-output dirs so the next `bench run` re-benchmarks every
    /// target from scratch.
    Clean(clean::BenchCleanArgs),
}

/// Dispatch.
pub async fn run(args: BenchArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    match args.command {
        BenchCommand::Run(a) => run::run(a, ctx, session_id).await,
        BenchCommand::Clean(a) => clean::run(a, ctx, session_id).await,
    }
}
