//! `sbagent session analysis ...` — Phase 1.5 / 1.7 analyzer + merge
//! subcommands. Stubs.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::types::SessionId;

pub mod clean;
pub mod merge;
pub mod run;

/// `sbagent session analysis ...`.
#[derive(Debug, Args)]
pub struct AnalysisArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: AnalysisCommand,
}

/// Analysis subcommands.
#[derive(Debug, Subcommand)]
pub enum AnalysisCommand {
    /// Fan out per-family analyzer agents.
    Run(run::AnalysisRunArgs),
    /// Run the LLM merge consolidation pass.
    Merge(merge::AnalysisMergeArgs),
    /// Clear analyzer + merge artifacts (`analysis/`, `merge-*`,
    /// `optimization-targets.json`) so the next run/merge starts clean.
    Clean(clean::AnalysisCleanArgs),
}

/// Dispatch.
pub async fn run(args: AnalysisArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    match args.command {
        AnalysisCommand::Run(a) => run::run(a, ctx, session_id).await,
        AnalysisCommand::Merge(a) => merge::run(a, ctx, session_id).await,
        AnalysisCommand::Clean(a) => clean::run(a, ctx, session_id).await,
    }
}
