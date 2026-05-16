//! `sbagent session finalize clean` — drop the previously-emitted
//! `summary.json` / `summary.md` so `finalize run` re-derives them from
//! the session's accepted artifacts. Useful after re-running `bench` or
//! patching analyzer output by hand.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::session::{SessionLayout, clean};
use crate::types::SessionId;

/// Args for `sbagent session finalize clean`.
#[derive(Debug, Args)]
pub struct FinalizeCleanArgs {}

/// Clear summary artifacts. Idempotent.
pub async fn run(_args: FinalizeCleanArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let report = clean::remove_paths([&layout.summary_json(), &layout.summary_md()])?;
    clean::print_report("finalize clean", &report);
    Ok(())
}
