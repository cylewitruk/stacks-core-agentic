//! `sbagent session analysis merge` — port of `scripts/merge-analyses.sh`.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::harnesses::codex::CodexHarness;
use crate::session::SessionLayout;
use crate::session::merge::{self, Inputs};
use crate::types::SessionId;

/// Args for `sbagent session analysis merge`.
#[derive(Debug, Args)]
pub struct AnalysisMergeArgs {}

/// Run the LLM merge consolidation pass.
pub async fn run(_args: AnalysisMergeArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let harness = CodexHarness::new();

    let outputs = merge::run(&Inputs {
        layout: &layout,
        framework: &ctx.layout,
        settings: &ctx.settings,
        harness: &harness,
    })
    .await?;

    if outputs.empty_input_shortcut {
        println!("merge-analyses: 0 accepted analyses; emitted empty targets list.");
    } else {
        println!(
            "merge-analyses: LLM merge succeeded ({} input(s) → {} target(s))",
            outputs.accepted_input_count, outputs.merged_target_count
        );
    }
    Ok(())
}
