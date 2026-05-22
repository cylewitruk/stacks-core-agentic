//! `sbagent session run` — full-pipeline orchestrator.
//!
//! Chains the per-phase commands (baseline → triage → analysis → merge →
//! optimize → bench → finalize → optional publish) into one invocation.
//! Each phase is a direct in-process call into the typed module that
//! already implements it. Phase 5 (`publish`) used to fork through
//! `sudo` to a separate publisher user; that split is gone — the token
//! is read into `sbagent`'s memory at Phase 5 startup and never leaves.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::session::bench_range::BenchRangeArgs;
use crate::cli::{CliContext, preflight};
use crate::harnesses::codex::CodexHarness;
use crate::session::bench::StacksBenchCli;
use crate::session::cargo::StdCargoRunner;
use crate::session::finalize::{self, FinalizeInputs};
use crate::session::{
    SessionLayout, analyzers, baseline, bench_experiments, merge, optimizers, publish, triage,
};
use crate::types::SessionId;

/// Args for `sbagent session run`. Per-invocation overrides; long-lived
/// configuration lives in `config.toml`.
#[derive(Debug, Args)]
pub struct RunSessionArgs {
    /// Existing baseline run id to import instead of running a fresh
    /// baseline benchmark.
    #[clap(long)]
    pub import_baseline_run_id: Option<i64>,
    /// Optional companion rerun id.
    #[clap(long)]
    pub import_baseline_rerun_id: Option<i64>,
    /// Skip the per-worktree `cargo clean` after building.
    #[clap(long)]
    pub skip_cargo_clean: bool,
    /// Concurrency cap for analyzer fan-out.
    #[clap(long)]
    pub parallel_analyzers: Option<usize>,
    /// Concurrency cap for optimizer fan-out. Will be clamped to `1` for
    /// sessions with normal_pr targets (Layer 1B v1 constraint:
    /// inner-loop benches share the stacks-bench DB).
    #[clap(long)]
    pub parallel_agents: Option<usize>,
    /// Override `settings.optimizer_attempts` (Layer 1B inner-loop
    /// attempt cap). Defaults to settings → `5`.
    #[clap(long)]
    pub optimizer_attempts: Option<u32>,
    /// Override `settings.optimizer_budget_minutes` (Layer 1B
    /// inner-loop wall-clock budget). Defaults to settings → `60`.
    #[clap(long)]
    pub optimizer_budget_minutes: Option<u32>,
    /// Operator weights for the triage selection lenses (e.g. `0.4,0.4,0.2`).
    /// Defaults from `settings.stacks_bench_axis_weights`, then `0.4,0.4,0.2`.
    #[clap(long)]
    pub axis_weights: Option<String>,
    /// Base branch for optimizer worktrees. Defaults from
    /// `settings.publish_base_branch`, then `feat/stacks-bench`.
    #[clap(long)]
    pub base_branch: Option<String>,
    /// Enable Phase 5 publish (generate + push).
    #[clap(long)]
    pub publish_accepted_prs: bool,

    /// Run `session archive` at the end of the pipeline: commit
    /// `sessions/<id>/` to a `session/<id>` write-once branch and
    /// append one line to `sessions.jsonl` on the tracking branch.
    /// Without `--archive`, the session's bulk stays in the operator's
    /// local working tree and is never committed.
    #[clap(long)]
    pub archive: bool,

    /// Skip Phase 2 optimizer re-runs for targets that already carry a
    /// valid typed `optimizer-report.json` for this session id. Only
    /// targets with missing, corrupt, or context-mismatched reports
    /// re-run. Pair with `--session-id <ID>` to resume a partially-
    /// failed session.
    #[clap(long)]
    pub resume: bool,

    /// Skip the session-start preflight (installed-binary drift,
    /// load-bearing prompt drift, submodule reachability). UNSAFE —
    /// preflight catches the drift modes that have historically wasted
    /// the most operator time mid-session. Reserve for cases where
    /// you've inspected the drift and consciously accept it.
    #[clap(long)]
    pub skip_preflight: bool,

    /// Block-range overrides applied to Phase 0 (baseline + rerun)
    /// AND Phase 3 (per-target experiment benches). The two phases
    /// share one resolved range — candidate selection during
    /// triage / analysis must talk about the same blocks the
    /// experiments end up measuring.
    #[clap(flatten)]
    pub range: BenchRangeArgs,
}

/// Run the full pipeline.
pub async fn run(args: RunSessionArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    // Session-start preflight runs FIRST so drift modes that would
    // waste hours mid-session (stale prompts breaking the orchestrator
    // contract, submodule not on the publish branch, installed binary
    // older than the workspace build) abort before any heavy phase
    // touches disk or the bench DB.
    if !args.skip_preflight {
        let findings =
            crate::session::preflight::collect_findings(ctx).context("session-start preflight")?;
        crate::session::preflight::report(&findings)?;
    }

    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let harness = Arc::new(CodexHarness::new());

    // Resolve the block range once. Phase 0 (baseline) AND Phase 3
    // (per-target experiments) must replay the same blocks —
    // candidate selection during triage / analysis talks about the
    // baseline run, so the experiments need to match. Even with
    // `--import-baseline-run-id` set we still need the range for
    // Phase 3; the operator is responsible for setting it to match
    // whatever the imported baseline used.
    let range = args
        .range
        .resolve(&ctx.settings)?;

    let axis_weights = args
        .axis_weights
        .clone()
        .or_else(|| {
            ctx.settings
                .stacks_bench_axis_weights
                .clone()
        })
        .unwrap_or_else(|| "0.4,0.4,0.2".to_owned());
    let base_branch = args
        .base_branch
        .clone()
        .or_else(|| {
            ctx.settings
                .publish_base_branch
                .clone()
        })
        .unwrap_or_else(|| "feat/stacks-bench".to_owned());
    println!("session: {}", layout.results_dir.display());

    // When Phase 5 is requested, validate publish wiring NOW — before
    // Phases 0-4 burn an hour of compute. Catches an unreadable/empty
    // token file and an unauthorized or wrong `publish_base_repo`.
    if args.publish_accepted_prs {
        preflight::ensure_publish_wiring(ctx)
            .await
            .context("preflight: publish wiring (--publish-accepted-prs)")?;
    }

    // Phase 0a: build + archive baseline binary BEFORE either
    // baseline branch (fresh run OR import). Phase 1.8 calibration
    // and Phase 3 full-range fallback both depend on
    // `baseline/bin/stacks-bench` existing — including in
    // imported-baseline sessions where Phase 0b is skipped.
    let stacks_core_base = ctx
        .layout
        .require_base()?
        .to_path_buf();
    let archive_outputs = baseline::archive_baseline_binary(&baseline::ArchiveBinaryInputs {
        layout: &layout,
        stacks_core_base: &stacks_core_base,
    })
    .context("Phase 0a: archive baseline binary")?;
    eprintln!(
        "Phase 0a: archived baseline stacks-bench binary at {} (source_sha={})",
        archive_outputs
            .archived_path
            .display(),
        archive_outputs.source_sha,
    );

    // Phase 0
    if let Some(run_id) = args.import_baseline_run_id {
        // Even though Phase 0b is skipped under import, we still use
        // the strict archived binary for any DB lookups (bench show,
        // bench list) so the import path doesn't silently fall back
        // to a stale `target/release/stacks-bench`.
        let bench = StacksBenchCli::strict_archived(
            archive_outputs
                .archived_path
                .clone(),
            ctx.layout
                .stacks_bench_data_dir
                .clone(),
            stacks_core_base.clone(),
        );
        let inputs = baseline::ImportInputs::from_settings(
            &layout,
            &bench,
            run_id,
            args.import_baseline_rerun_id,
            &ctx.settings,
            &ctx.layout.bench_lock,
        );
        baseline::import(&inputs).context("Phase 0: baseline import")?;
    } else {
        let source_dir = ctx
            .settings
            .source_dir
            .as_deref()
            .context("settings.source_dir is required for `baseline run`")?;
        let network = ctx
            .settings
            .stacks_bench_network
            .as_deref()
            .unwrap_or("mainnet");
        let bench = StacksBenchCli::strict_archived(
            archive_outputs
                .archived_path
                .clone(),
            ctx.layout
                .stacks_bench_data_dir
                .clone(),
            stacks_core_base.clone(),
        );
        baseline::run(&baseline::RunInputs {
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
            single_run_noise_floor_pct: ctx
                .settings
                .single_run_noise_floor_pct
                .unwrap_or(1.0),
            bench_lock: &ctx.layout.bench_lock,
        })
        .context("Phase 0: baseline run")?;
    }

    // Phase 1: triage
    triage::run(&triage::Inputs {
        layout: &layout,
        framework: &ctx.layout,
        settings: &ctx.settings,
        axis_weights: &axis_weights,
        harness: harness.as_ref(),
    })
    .await
    .context("Phase 1: triage")?;

    // Phase 1.5: analyzer fan-out
    analyzers::run(analyzers::Inputs {
        layout: layout.clone(),
        framework: ctx.layout.clone(),
        settings: ctx.settings.clone(),
        parallel: args.parallel_analyzers,
        harness: harness.clone(),
    })
    .await
    .context("Phase 1.5: analyzers")?;

    // Phase 1.7: merge
    merge::run(&merge::Inputs {
        layout: &layout,
        framework: &ctx.layout,
        settings: &ctx.settings,
        harness: harness.as_ref(),
    })
    .await
    .context("Phase 1.7: merge")?;

    // Phase 1.8: per-target targeted baseline calibration. For each
    // normal_pr target with verification_replay, run one bench
    // invocation per replay phase against the strict archived
    // baseline binary. The resulting baseline-run-ids feed Phase 4
    // finalize's apples-to-apples improvement_pct comparison.
    {
        let targets = crate::session::loader::read_optimization_targets(&layout)
            .context("Phase 1.8: loading optimization-targets.json")?;
        let stacks_core_base = ctx
            .layout
            .require_base()?
            .to_path_buf();
        let archived_path = layout.baseline_bin_path();
        let bench = crate::session::bench::StacksBenchCli::strict_archived(
            archived_path,
            ctx.layout
                .stacks_bench_data_dir
                .clone(),
            stacks_core_base,
        );
        let source_dir = ctx
            .settings
            .source_dir
            .as_deref()
            .context("settings.source_dir is required for Phase 1.8 calibration")?;
        let network = ctx
            .settings
            .stacks_bench_network
            .as_deref()
            .unwrap_or("mainnet");
        crate::session::calibration::run(&crate::session::calibration::Inputs {
            layout: &layout,
            bench: &bench,
            source_dir,
            network,
            shadow_dir_root: ctx
                .layout
                .stacks_bench_shadow_dir
                .as_deref(),
            bench_lock: &ctx.layout.bench_lock,
            targets: &targets,
        })
        .context("Phase 1.8: targeted baseline calibration")?;
    }

    // Phase 2: optimizer fan-out. CLI flags override settings, then
    // optimizers::run applies a final clamp to enforce the inner-loop
    // bench's single-writer constraint.
    let mut settings_for_optimize = ctx.settings.clone();
    if let Some(n) = args.optimizer_attempts {
        settings_for_optimize.optimizer_attempts = Some(n);
    }
    if let Some(m) = args.optimizer_budget_minutes {
        settings_for_optimize.optimizer_budget_minutes = Some(m);
    }
    optimizers::run(optimizers::Inputs {
        layout: layout.clone(),
        framework: ctx.layout.clone(),
        settings: settings_for_optimize,
        parallel: args.parallel_agents,
        base_branch: base_branch.clone(),
        harness: harness.clone(),
        git: Arc::new(optimizers::StdGitCheckoutManager),
        resume: args.resume,
    })
    .await
    .context("Phase 2: optimizers")?;

    // Phase 3: bench experiments
    {
        let data_dir = ctx
            .layout
            .stacks_bench_data_dir
            .clone();
        let cargo_cwd = ctx
            .layout
            .require_base()?
            .to_path_buf();
        let bench_for_target = move |bin: &std::path::Path| {
            bench_experiments::stacks_bench_for(bin, &data_dir, &cargo_cwd)
        };
        let bench_for_target_ref: &dyn Fn(
            &std::path::Path,
        ) -> Box<dyn crate::session::bench::BenchClient> = &bench_for_target;
        let source_dir = ctx
            .settings
            .source_dir
            .as_deref()
            .context("settings.source_dir is required for `bench`")?;
        let network = ctx
            .settings
            .stacks_bench_network
            .as_deref()
            .unwrap_or("mainnet");
        let worktrees_root = ctx
            .layout
            .session_optimizer_checkouts_dir(session_id);
        let targets = crate::session::loader::read_optimization_targets(&layout)
            .context("Phase 3: loading optimization-targets.json")?;
        bench_experiments::run(&bench_experiments::Inputs {
            layout: &layout,
            worktrees_root: &worktrees_root,
            targets: &targets,
            range: bench_experiments::BenchRange {
                source_dir,
                network,
                // The full-pipeline orchestrator always runs baseline,
                // so the resolver has already produced concrete values
                // here. Wrap as Some for BenchRange's Option-typed slot
                // (which is None-able only on the `session bench run`
                // standalone path with all-recipe targets).
                start_at: Some(range.start_at),
                count: Some(range.count),
                warmup: range.warmup,
                filter: range.filter.as_deref(),
                shadow_dir_root: ctx
                    .layout
                    .stacks_bench_shadow_dir
                    .as_deref(),
            },
            bench_lock: &ctx.layout.bench_lock,
            skip_cargo_clean: args.skip_cargo_clean,
            cargo: &StdCargoRunner,
            bench_for_target: bench_for_target_ref,
        })
        .context("Phase 3: bench-experiments")?;
    }

    // Phase 4: finalize
    {
        let bench = StacksBenchCli::from_layout(&ctx.layout)?;
        // Warn on dangling artifact ↔ DB run-id refs before finalize
        // bakes them into summary.json.
        crate::session::db_consistency::warn_dangling_refs(&layout, &bench)
            .context("Phase 4: DB consistency check")?;
        finalize::finalize(&FinalizeInputs { layout: &layout, bench: &bench })
            .context("Phase 4: finalize")?;
    }

    // Phase 5 (optional)
    if args.publish_accepted_prs {
        publish::generate(&publish::GenerateInputs {
            layout: &layout,
            framework: &ctx.layout,
            settings: &ctx.settings,
            harness: harness.as_ref(),
        })
        .await
        .context("Phase 5: publish generate")?;

        // Push in-process. The token never leaves sbagent's address space.
        let publish_config = publish::PublishConfig::from_settings(&ctx.settings);
        publish::ensure_token_outside_framework(
            &publish_config.publish_token_file,
            ctx.layout
                .framework
                .as_deref()
                .map(|p| p as &std::path::Path),
        )?;
        let token = publish::read_publish_token(&publish_config.publish_token_file)
            .context("Phase 5: reading publish_token_file")?;
        let gh = publish::StdGhClient::from_token(&token)
            .context("Phase 5: building octocrab client")?;
        publish::push(&publish::PushInputs {
            layout: &layout,
            framework: &ctx.layout,
            config: &publish_config,
            gh: &gh,
        })
        .await
        .context("Phase 5: publish push")?;
    }

    // Phase 6 (optional): archive the session.
    if args.archive {
        // Warn on dangling artifact ↔ DB refs before the write-once
        // session/<id> branch + ledger append bakes them in.
        let bench = StacksBenchCli::from_layout(&ctx.layout)?;
        crate::session::db_consistency::warn_dangling_refs(&layout, &bench)
            .context("Phase 6: pre-archive DB consistency check")?;
        let outputs = crate::session::archive::archive(&crate::session::archive::ArchiveInputs {
            layout: &layout,
            framework: &ctx.layout,
            settings: &ctx.settings,
            dry_run: false,
        })
        .context("Phase 6: archive")?;
        crate::session::archive::print_outputs(&outputs);
    }

    // Session-end sweep: tear down per-target clones for aborted
    // experiments so they don't accumulate in the operator's tree.
    // Kept (implementation.md-marked) checkouts survive — Phase 5
    // publish pushed from them, and operators may inspect them. With
    // the clone-based model, teardown is just `rm -rf <clone>` — the
    // `agent/<session>/<target>` branch lives inside the clone and
    // goes away with it.
    let git = optimizers::StdGitCheckoutManager;
    let checkouts_root = ctx
        .layout
        .session_optimizer_checkouts_dir(session_id);
    let dropped = optimizers::prune_aborted_experiments(&git, &checkouts_root, &layout)
        .context("session-end experiment cleanup")?;
    if dropped > 0 {
        println!("session-end cleanup: dropped {dropped} aborted experiment(s)");
    }

    println!();
    println!("session: {}", layout.results_dir.display());
    println!("summary: {}", layout.summary_md().display());
    Ok(())
}
