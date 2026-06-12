//! `sbagent session baseline ...` — Phase 0 discovery-pass subcommands.
//! The CLI retains the legacy `baseline` name.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::types::SessionId;

pub mod clean;
pub mod import;
pub mod run;

/// `sbagent session baseline ...`.
#[derive(Debug, Args)]
pub struct BaselineArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: BaselineCommand,
}

/// Baseline subcommands.
#[derive(Debug, Subcommand)]
pub enum BaselineCommand {
    /// Run a fresh discovery-pass benchmark.
    Run(run::BaselineRunArgs),
    /// Import an existing discovery-pass run id from the persistent
    /// stacks-bench db.
    Import(import::BaselineImportArgs),
    /// Clear discovery-pass artifacts (`baseline-*`, `bench-list.json`,
    /// profiler hotspots) so the next run/import starts clean.
    Clean(clean::BaselineCleanArgs),
}

/// Dispatch.
pub async fn run(args: BaselineArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    match args.command {
        BaselineCommand::Run(a) => run::run(a, ctx, session_id).await,
        BaselineCommand::Import(a) => import::run(a, ctx, session_id).await,
        BaselineCommand::Clean(a) => clean::run(a, ctx, session_id).await,
    }
}
