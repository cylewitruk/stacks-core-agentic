//! `sbagent session finalize run` — port of `scripts/finalize-session.sh`.
//!
//! The finalize logic itself lives in
//! [`crate::session::finalize::finalize`]; this module is the thin CLI
//! wrapper. The default [`BenchClient`](crate::session::bench::BenchClient)
//! is [`StacksBenchCli`](crate::session::bench::StacksBenchCli), which
//! shells out to `cargo stacks-bench` (or the prebuilt release binary).

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::bench::StacksBenchCli;
use crate::session::finalize::{FinalizeInputs, finalize};
use crate::session::{SessionLayout, db_consistency};
use crate::types::SessionId;

/// Args for `sbagent session finalize run`.
#[derive(Debug, Args)]
pub struct FinalizeRunArgs {}

/// Finalize one session: emit `summary.json` + `summary.md`.
#[allow(unused_variables)]
pub async fn run(args: FinalizeRunArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());

    let bench = StacksBenchCli::from_layout(&ctx.layout)?;

    // DB ↔ artifact run-id consistency check: surface dangling refs
    // BEFORE finalize bakes them into `summary.json`. Same helper the
    // full-pipeline `session run` invokes at Phase 4.
    db_consistency::warn_dangling_refs(&layout, &bench).context("DB consistency check")?;

    let summary = finalize(&FinalizeInputs { layout: &layout, bench: &bench })?;
    println!(
        "wrote {} and {}",
        layout
            .summary_json()
            .display(),
        layout.summary_md().display()
    );
    println!(
        "experiments: {} | accepted={} rejected={} aborted={} poc_landed={} routed_to_issue={}",
        summary.experiments.len(),
        summary
            .outcome_counts
            .normal_pr
            .accepted,
        summary
            .outcome_counts
            .normal_pr
            .rejected,
        summary
            .outcome_counts
            .normal_pr
            .aborted
            + summary
                .outcome_counts
                .consensus_poc_pr
                .aborted
            + summary
                .outcome_counts
                .consensus_issue
                .aborted,
        summary
            .outcome_counts
            .consensus_poc_pr
            .poc_landed,
        summary
            .outcome_counts
            .consensus_issue
            .routed_to_issue,
    );
    Ok(())
}
