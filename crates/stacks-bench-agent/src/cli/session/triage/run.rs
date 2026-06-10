//! `sbagent session triage run` — port of `scripts/run-triage.sh`.

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::harnesses::codex::CodexHarness;
use crate::session::SessionLayout;
use crate::session::triage::{self, Inputs};
use crate::source::read_session_source;
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

    // Standalone triage reads source.json (must already exist from
    // `session run`) to derive the per-session source checkout that
    // the agent prompt + add_dirs reference.
    let workspace_root = ctx
        .layout
        .require_agent_workspace_root()?;
    let resolved = read_session_source(workspace_root, session_id.as_str(), &layout.source_json())
        .context("per-session source.json required")?;

    let outputs = triage::run(&Inputs {
        layout: &layout,
        framework: &ctx.layout,
        settings: &ctx.settings,
        axis_weights: &axis_weights,
        harness: &harness,
        source_checkout: &resolved.session_checkout,
    })
    .await?;

    println!("candidates: {}", outputs.candidate_count);
    if outputs.candidate_count == 0 {
        eprintln!("Triage returned zero candidates. Downstream phases will no-op.");
    }
    Ok(())
}
