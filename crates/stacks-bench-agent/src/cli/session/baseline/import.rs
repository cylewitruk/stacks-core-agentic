//! `sbagent session baseline import` — port of `scripts/import-baseline.sh`.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::session::SessionLayout;
use crate::session::baseline::{self, ImportInputs};
use crate::session::bench::StacksBenchCli;
use crate::types::SessionId;

/// Args for `sbagent session baseline import`.
#[derive(Debug, Args)]
pub struct BaselineImportArgs {
    /// Existing run id in the stacks-bench db to import as the baseline.
    #[clap(long)]
    pub run_id: i64,
    /// Optional companion rerun id. When omitted, the baseline run id is
    /// used for both — a single-run import — and a fallback noise floor is
    /// written to `<results>/baseline-noise-floor-pct`.
    #[clap(long)]
    pub rerun_id: Option<i64>,
}

/// Import a baseline run id.
pub async fn run(args: BaselineImportArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());

    let bench = StacksBenchCli {
        release_bin: Some(
            ctx.layout
                .require_base()?
                .join("target")
                .join("release")
                .join("stacks-bench"),
        ),
        data_dir: ctx
            .layout
            .stacks_bench_data_dir
            .clone(),
        cargo_cwd: ctx
            .layout
            .require_base()?
            .to_path_buf(),
    };

    let inputs = ImportInputs::from_settings(
        &layout,
        &bench,
        args.run_id,
        args.rerun_id,
        &ctx.settings,
        &ctx.layout.bench_lock,
    );
    let outputs = baseline::import(&inputs)?;

    println!("imported baseline-run-id   : {}", outputs.baseline_run_id);
    println!("imported baseline-rerun-id : {}", outputs.baseline_rerun_id);
    if outputs.single_run_fallback {
        let pct = ctx
            .settings
            .single_run_noise_floor_pct
            .unwrap_or(1.0);
        eprintln!("WARNING: imported a single run only; using fallback noise floor {pct}%.");
    }
    Ok(())
}
