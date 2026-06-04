//! `sbagent session archive` — commit a completed session's evidence
//! bundle to a write-once `session/<id>` branch and append one line to
//! `sessions.jsonl` on the operator's tracking branch.
//!
//! See [`crate::session::archive`] for the full contract.

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::archive::{ArchiveInputs, archive, print_outputs};
use crate::session::bench::StacksBenchCli;
use crate::session::{SessionLayout, db_consistency};
use crate::types::SessionId;

/// Args for `sbagent session archive`.
#[derive(Debug, Args)]
pub struct ArchiveArgs {
    /// Don't push to the configured remote: commit `session/<id>` and
    /// the `sessions.jsonl` append locally, then stop. Useful as a
    /// rehearsal on a machine without bot PAT setup, or before
    /// shipping the first real archive of a session.
    #[clap(long)]
    pub dry_run: bool,
}

/// Dispatch `sbagent session archive`.
pub async fn run(args: ArchiveArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());

    // DB ↔ artifact run-id consistency check: archive bakes the
    // session's `summary.json` + per-target run-ids into the
    // write-once `session/<id>` branch + the `sessions.jsonl` append.
    // Dangling references here become permanent audit-trail poison;
    // warn before the immutable write. Same helper the full-pipeline
    // `session run` invokes at Phase 6. Skipped silently when
    // `base` isn't set (archive will fail with a clearer error
    // anyway).
    if ctx.layout.base.is_some() {
        let bench = StacksBenchCli::from_layout(&ctx.layout)?;
        db_consistency::warn_dangling_refs(&layout, &bench)?;
    }

    let outputs = archive(&ArchiveInputs {
        layout: &layout,
        framework: &ctx.layout,
        settings: &ctx.settings,
        dry_run: args.dry_run,
    })
    .context("session archive")?;
    print_outputs(&outputs);
    Ok(())
}
