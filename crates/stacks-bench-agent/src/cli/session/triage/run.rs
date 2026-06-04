//! `sbagent session triage run` — port of `scripts/run-triage.sh`.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::harnesses::codex::CodexHarness;
use crate::session::SessionLayout;
use crate::session::triage::{self, Inputs};
use crate::types::SessionId;

/// Args for `sbagent session triage run`.
#[derive(Debug, Args)]
pub struct TriageRunArgs {
    /// Operator weights for the three triage selection lenses, comma-separated
    /// `tx_latency,tenure_throughput,commit_time`. Defaults to
    /// `settings.triage.axis_weights` from `config.toml`, then
    /// `0.4,0.4,0.2`.
    #[clap(long)]
    pub axis_weights: Option<String>,
}

/// Run the triage agent.
pub async fn run(args: TriageRunArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let harness = CodexHarness::new();
    let axis_weights = args
        .axis_weights
        .clone()
        .unwrap_or_else(|| {
            ctx.settings
                .triage
                .effective_axis_weights()
                .to_owned()
        });

    let outputs = triage::run(&Inputs {
        layout: &layout,
        framework: &ctx.layout,
        settings: &ctx.settings,
        axis_weights: &axis_weights,
        harness: &harness,
    })
    .await?;

    println!("candidates: {}", outputs.candidate_count);
    if outputs.candidate_count == 0 {
        eprintln!("Triage returned zero candidates. Downstream phases will no-op.");
    }
    Ok(())
}
