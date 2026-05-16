//! `sbagent session triage ...` — Phase 1 triage subcommands. Stubs.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::types::SessionId;

pub mod clean;
pub mod run;

/// `sbagent session triage ...`.
#[derive(Debug, Args)]
pub struct TriageArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: TriageCommand,
}

/// Triage subcommands.
#[derive(Debug, Subcommand)]
pub enum TriageCommand {
    /// Render the triage prompt and run a Codex agent against it. Stub for
    /// `run-triage.sh`.
    Run(run::TriageRunArgs),
    /// Clear stale triage artifacts (`candidates.*`, `triage-*`) so the next
    /// run starts clean.
    Clean(clean::TriageCleanArgs),
}

/// Dispatch.
pub async fn run(args: TriageArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    match args.command {
        TriageCommand::Run(a) => run::run(a, ctx, session_id).await,
        TriageCommand::Clean(a) => clean::run(a, ctx, session_id).await,
    }
}
