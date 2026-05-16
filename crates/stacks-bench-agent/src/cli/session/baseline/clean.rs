//! `sbagent session baseline clean` — clear Phase 0 artifacts so the
//! next baseline run/import starts from a known-empty state.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::session::{SessionLayout, clean};
use crate::types::SessionId;

/// Args for `sbagent session baseline clean`.
#[derive(Debug, Args)]
pub struct BaselineCleanArgs {}

/// Clear stale baseline artifacts. Idempotent.
pub async fn run(_args: BaselineCleanArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    // Every Phase 0 artifact lives under `results/baseline/`, so a
    // wholesale remove is safe (no need to enumerate per-file).
    let report = clean::remove_one(&layout.baseline_dir())?;
    clean::print_report("baseline clean", &report);
    Ok(())
}
