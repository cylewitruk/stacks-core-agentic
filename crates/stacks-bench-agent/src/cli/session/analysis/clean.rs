//! `sbagent session analysis clean` — clear Phase 1.5 (analyzer fan-out)
//! and Phase 1.7 (merge) artifacts in one shot.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::session::SessionLayout;
use crate::session::clean::{self, CleanReport};
use crate::types::SessionId;

/// Args for `sbagent session analysis clean`.
#[derive(Debug, Args)]
pub struct AnalysisCleanArgs {}

/// Clear analyzer + merge artifacts. Idempotent.
pub async fn run(_args: AnalysisCleanArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let mut report = CleanReport::default();

    // Drop the entire analysis tree (per-family analyzer prompts, events,
    // stderr, conversation ids, and the analysis.json each family wrote)
    // plus the merge phase's outputs. Both are namespaced under
    // results/{analysis,merge}/ so removing the dirs wholesale is safe.
    report.merge(clean::remove_one(&layout.analysis_dir())?);
    report.merge(clean::remove_one(&layout.merge_dir())?);

    clean::print_report("analysis clean", &report);
    Ok(())
}
