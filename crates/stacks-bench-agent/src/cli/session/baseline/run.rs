//! `sbagent session baseline run` — Phase 0 (baseline + rerun) entry
//! point. The block range `stacks-bench` replays is overridable per
//! invocation via flags / env vars (see
//! [`crate::cli::session::bench_range::BenchRangeArgs`]) so the
//! closed-loop can sample different windows without mutating config.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::cli::session::bench_range::BenchRangeArgs;
use crate::session::SessionLayout;
use crate::session::baseline::{self, ArchiveBinaryInputs, RunInputs};
use crate::session::bench::StacksBenchCli;
use crate::types::SessionId;

/// Args for `sbagent session baseline run`. Long-lived configuration
/// lives in `config.toml`; the flattened [`BenchRangeArgs`] lets an
/// operator override the block range per-invocation (CLI > env >
/// config precedence).
#[derive(Debug, Args)]
pub struct BaselineRunArgs {
    /// Block-range overrides — see [`BenchRangeArgs`] for the four
    /// flags + matching `SBAGENT_BENCH_*` env vars.
    #[clap(flatten)]
    pub range: BenchRangeArgs,
}

/// Run a fresh baseline benchmark + rerun.
pub async fn run(args: BaselineRunArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());

    let source_dir = ctx
        .settings
        .stacks_bench
        .source_dir_required()?;
    let network = ctx
        .settings
        .stacks_bench
        .effective_network();
    let range = args
        .range
        .resolve(&ctx.settings)?;

    // Phase 0a: build + archive the baseline binary BEFORE Phase 0b
    // bench runs. From this point on, baseline / calibration /
    // full-range fallback all read from the archived path. Strict
    // contract: missing archived binary later = hard error.
    let stacks_core_base = ctx
        .layout
        .require_base()?
        .to_path_buf();
    let archive_outputs = baseline::archive_baseline_binary(&ArchiveBinaryInputs {
        layout: &layout,
        stacks_core_base: &stacks_core_base,
    })?;
    eprintln!(
        "Phase 0a: archived baseline stacks-bench binary at {} (source_sha={})",
        archive_outputs
            .archived_path
            .display(),
        archive_outputs.source_sha,
    );

    // Phase 0b uses the strict archived binary from Phase 0a.
    // Missing archived binary → hard error, no `cargo stacks-bench`
    // fallback.
    let bench = StacksBenchCli::strict_archived(
        archive_outputs
            .archived_path
            .clone(),
        ctx.layout
            .stacks_bench_data_dir
            .clone(),
        stacks_core_base.clone(),
    );

    let outputs = baseline::run(&RunInputs {
        layout: &layout,
        bench: &bench,
        source_dir,
        network,
        start_at: range.start_at,
        count: range.count,
        warmup: range.warmup,
        filter: range.filter.as_deref(),
        shadow_dir_root: ctx
            .layout
            .stacks_bench_shadow_dir
            .as_deref(),
        bench_lock: &ctx.layout.bench_lock,
        single_run_noise_floor_pct: ctx
            .settings
            .triage
            .single_run_noise_floor_pct
            .unwrap_or(1.0),
    })?;

    println!("baseline-run-id   : {}", outputs.baseline_run_id);
    println!("baseline-rerun-id : {}", outputs.baseline_rerun_id);
    Ok(())
}
