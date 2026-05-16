//! `sbagent session triage clean` — clear stale Phase 1 artifacts so the
//! next triage starts from a known-empty state.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::session::clean::CleanReport;
use crate::session::{SessionLayout, clean};
use crate::types::SessionId;

/// Args for `sbagent session triage clean`.
#[derive(Debug, Args)]
pub struct TriageCleanArgs {}

/// Clear stale triage artifacts. Idempotent: missing files are no-ops.
pub async fn run(_args: TriageCleanArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let mut report = CleanReport::default();
    // Every Phase 1 artifact (candidates, prompts, events, prerendered
    // queries, agent drilldowns) lives under `results/triage/`, so the
    // wholesale remove is safe.
    report.merge(clean::remove_one(&layout.triage_dir())?);
    clean::print_report("triage clean", &report);
    Ok(())
}
