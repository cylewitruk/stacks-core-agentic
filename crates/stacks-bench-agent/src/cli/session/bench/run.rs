//! `sbagent session bench run` — port of `scripts/bench-experiments.sh`.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::bench_experiments::{self, BenchRange, Inputs, TargetOutcome};
use crate::session::cargo::StdCargoRunner;
use crate::session::{SessionLayout, loader};
use crate::types::SessionId;

/// Args for `sbagent session bench`.
#[derive(Debug, Args)]
pub struct BenchRunArgs {
    /// Skip the per-worktree `cargo clean` after building. Equivalent to
    /// bash `SKIP_CARGO_CLEAN=1`.
    #[clap(long)]
    pub skip_cargo_clean: bool,
}

/// Run the bench-experiments phase.
pub async fn run(args: BenchRunArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let targets =
        loader::read_optimization_targets(&layout).context("loading optimization-targets.json")?;
    if targets.targets.is_empty() {
        println!("No targets to benchmark; phase is a no-op.");
        return Ok(());
    }

    let source_dir = ctx
        .settings
        .source_dir
        .as_deref()
        .context("settings.source_dir is required (or env $SOURCE_DIR)")?;
    let network = ctx
        .settings
        .stacks_bench_network
        .as_deref()
        .unwrap_or("mainnet");
    let start_at = ctx
        .settings
        .stacks_bench_start_at
        .context("settings.stacks_bench_start_at is required (or env $STACKS_BENCH_START_AT)")?;
    let count = ctx
        .settings
        .stacks_bench_count
        .context("settings.stacks_bench_count is required (or env $STACKS_BENCH_COUNT)")?;

    let worktrees_root = ctx
        .layout
        .session_optimizer_checkouts_dir(session_id);
    let data_dir = ctx
        .layout
        .stacks_bench_data_dir
        .clone();
    let cargo_cwd = ctx
        .layout
        .require_base()?
        .to_path_buf();
    let bench_for_target =
        move |bin: &std::path::Path| -> Box<dyn crate::session::bench::BenchClient> {
            bench_experiments::stacks_bench_for(bin, &data_dir, &cargo_cwd)
        };
    // Bind the closure to a local so we can pass it as &dyn Fn(...).
    let bench_for_target_ref: &dyn Fn(
        &std::path::Path,
    ) -> Box<dyn crate::session::bench::BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets,
        range: BenchRange {
            source_dir,
            network,
            start_at,
            count,
            warmup: ctx
                .settings
                .stacks_bench_warmup,
            filter: ctx
                .settings
                .stacks_bench_filter
                .as_deref(),
            shadow_dir_root: ctx
                .layout
                .stacks_bench_shadow_dir
                .as_deref(),
        },
        bench_lock: &ctx.layout.bench_lock,
        skip_cargo_clean: args.skip_cargo_clean,
        cargo: &StdCargoRunner,
        bench_for_target: bench_for_target_ref,
    })?;

    print_summary(&outcomes, &worktrees_root);
    Ok(())
}

fn print_summary(outcomes: &[(String, TargetOutcome)], _worktrees_root: &PathBuf) {
    let mut benched = 0u32;
    let mut skipped = 0u32;
    for (id, outcome) in outcomes {
        match outcome {
            TargetOutcome::Benched { run_ids } => {
                benched += 1;
                println!(
                    "{id}: bench complete; run_ids={}",
                    run_ids
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            TargetOutcome::Skipped { reason } => {
                skipped += 1;
                println!("{id}: skipped ({reason})");
            }
        }
    }
    println!("benched={benched} skipped={skipped}");
}
