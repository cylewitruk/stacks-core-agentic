//! `sbagent session analyze-results ...` — Phase 3.5 results-analyzer
//! subcommands.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::types::SessionId;

pub mod clean;
pub mod run;

/// `sbagent session analyze-results ...`.
#[derive(Debug, Args)]
pub struct AnalyzeResultsArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: AnalyzeResultsCommand,
}

/// Results-analyzer subcommands.
#[derive(Debug, Subcommand)]
pub enum AnalyzeResultsCommand {
    /// Fan out one results-analyzer agent per `bench_eligible` target.
    /// Each agent reads the target's `verification_replay` (analyzer
    /// hypothesis), the `optimizer-report.json` claim + diff, and the
    /// per-invocation baseline + candidate `bench-run.json` files;
    /// writes a typed verdict to
    /// `analyze/<target>/results-analysis.json`. Phase 4 finalize
    /// sources `improvement_pct` + `status` from this verdict.
    Run(run::ResultsAnalyzerRunArgs),
    /// Remove every `analyze/<target>/` dir for the session so the next
    /// `analyze-results run` re-judges from scratch.
    Clean(clean::ResultsAnalyzerCleanArgs),
}

/// Dispatch.
pub async fn run(args: AnalyzeResultsArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    match args.command {
        AnalyzeResultsCommand::Run(a) => run::run(a, ctx, session_id).await,
        AnalyzeResultsCommand::Clean(a) => clean::run(a, ctx, session_id).await,
    }
}
