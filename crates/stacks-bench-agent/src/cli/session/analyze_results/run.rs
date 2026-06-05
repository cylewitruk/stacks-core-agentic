//! `sbagent session analyze-results run` — Phase 3.5 fan-out.

use std::sync::Arc;

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::harnesses::codex::CodexHarness;
use crate::session::SessionLayout;
use crate::session::results_analyzer::{self, Inputs, TargetOutcome};
use crate::types::SessionId;

/// Args for `sbagent session analyze-results run`.
#[derive(Debug, Args)]
pub struct ResultsAnalyzerRunArgs {
    /// Concurrency cap for the results-analyzer fan-out. Defaults to
    /// `analyzer.concurrency_cap`.
    #[clap(long)]
    pub parallel: Option<usize>,
}

/// Fan out one results-analyzer per `bench_eligible` target.
pub async fn run(
    args: ResultsAnalyzerRunArgs,
    ctx: &CliContext,
    session_id: &SessionId,
) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let harness = Arc::new(CodexHarness::new());

    let outputs = results_analyzer::run(Inputs {
        layout,
        framework: ctx.layout.clone(),
        settings: ctx.settings.clone(),
        parallel: args.parallel,
        harness,
    })
    .await?;

    let (produced, skipped, failed) = outputs.tally();
    println!("results-analyses: {produced} produced, {skipped} skipped, {failed} failed");
    for (id, outcome) in &outputs.per_target {
        match outcome {
            TargetOutcome::Produced(ra) => {
                let headline = ra
                    .headline_improvement_pct
                    .map(|p| format!("{p:.2}%"))
                    .unwrap_or_else(|| "—".to_owned());
                println!(
                    "{id}: verdict={:?} confidence={:?} headline={headline}",
                    ra.verdict, ra.confidence,
                );
            }
            TargetOutcome::Skipped { reason } => {
                println!("{id}: skipped ({reason})");
            }
            TargetOutcome::Failed { reason } => {
                println!("{id}: FAILED ({reason})");
            }
        }
    }
    Ok(())
}
