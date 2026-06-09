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
use crate::source::read_session_source;
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
    // `session run` invokes at Phase 6.
    //
    // v3 Phase 3 cutover: use the archived Phase 0a baseline binary
    // with the per-session source checkout (looked up via
    // `source.json` — archive is strictly downstream of session start),
    // mirroring standalone finalize.
    let workspace_root = ctx
        .layout
        .require_agent_workspace_root()?;
    let resolved = read_session_source(workspace_root, session_id.as_str(), &layout.source_json())
        .context("v3 Phase 3: per-session source.json required for archive")?;
    let bench = StacksBenchCli::strict_archived(
        layout.baseline_bin_path(),
        ctx.layout
            .stacks_bench_data_dir
            .clone(),
        resolved
            .session_checkout
            .clone(),
    );
    db_consistency::warn_dangling_refs(&layout, &bench)?;

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
