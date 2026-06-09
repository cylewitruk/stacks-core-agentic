//! `sbagent session baseline import` — port of `scripts/import-baseline.sh`.

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::SessionLayout;
use crate::session::baseline::{self, ArchiveBinaryInputs, ImportInputs};
use crate::session::bench::StacksBenchCli;
use crate::source::{StdSourceRepo, materialize_session_source};
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

    // v3 Phase 3 cutover: Phase 0a's cargo build runs in the
    // per-session source checkout. Materialize fresh (or reuse on
    // resume) — same contract as `session run` + `session baseline
    // run`.
    std::fs::create_dir_all(&layout.results_dir).with_context(|| {
        format!("creating session results dir {}", layout.results_dir.display())
    })?;
    let workspace_root = ctx
        .layout
        .require_agent_workspace_root()?
        .to_path_buf();
    let resolved = materialize_session_source(
        &StdSourceRepo,
        &workspace_root,
        session_id.as_str(),
        &ctx.settings.source,
        &layout.source_json(),
    )
    .context("v3 Phase 3: per-session source materialization")?;
    let stacks_core_base = resolved
        .session_checkout
        .clone();

    // Phase 0a archival also runs for imported-baseline sessions —
    // downstream Phase 1.8 + Phase 3 paths need
    // `baseline/bin/stacks-bench` regardless of how Phase 0b was
    // resolved.
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

    let bench = StacksBenchCli::strict_archived(
        archive_outputs
            .archived_path
            .clone(),
        ctx.layout
            .stacks_bench_data_dir
            .clone(),
        stacks_core_base,
    );

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
            .triage
            .effective_single_run_noise_floor_pct();
        eprintln!("WARNING: imported a single run only; using fallback noise floor {pct}%.");
    }
    Ok(())
}
