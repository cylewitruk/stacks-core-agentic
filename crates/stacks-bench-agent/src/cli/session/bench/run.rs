//! `sbagent session bench run` — Phase 3 candidate bench.
//!
//! Per Pass 1c, every `bench_eligible` target carries its own
//! `verification_replay.invocations[]`. No session-level full-range
//! fallback exists; the merge schema rejects `bench_eligible` targets
//! without `verification_replay`.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::bench_experiments::{self, BenchEnv, Inputs, TargetOutcome};
use crate::session::cargo::StdCargoRunner;
use crate::session::{SessionLayout, loader};
use crate::source::read_session_source;
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
    let targets = loader::read_optimization_targets(&layout)?;
    if targets.targets.is_empty() {
        println!("No targets to benchmark; phase is a no-op.");
        return Ok(());
    }

    let source_dir = ctx
        .settings
        .stacks_bench
        .source_dir_required()?;
    let network = ctx
        .settings
        .stacks_bench
        .effective_network();

    let worktrees_root = ctx
        .layout
        .session_optimizer_checkouts_dir(session_id);
    let data_dir = ctx
        .layout
        .stacks_bench_data_dir
        .clone();
    // v3 Phase 3 cutover: Phase 3 candidate bench cargo cwd is the
    // per-session source checkout. Bench run is downstream of session
    // start, so source.json MUST already exist — `read_session_source`
    // bails loudly if it doesn't.
    let workspace_root = ctx
        .layout
        .require_agent_workspace_root()?;
    let resolved = read_session_source(workspace_root, session_id.as_str(), &layout.source_json())
        .context("v3 Phase 3: per-session source.json required for bench run")?;
    let cargo_cwd = resolved
        .session_checkout
        .clone();
    let bench_for_target =
        move |bin: &std::path::Path| -> Box<dyn crate::session::bench::BenchClient> {
            bench_experiments::stacks_bench_for(bin, &data_dir, &cargo_cwd)
        };
    let bench_for_target_ref: &dyn Fn(
        &std::path::Path,
    ) -> Box<dyn crate::session::bench::BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets,
        env: BenchEnv {
            source_dir,
            network,
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
