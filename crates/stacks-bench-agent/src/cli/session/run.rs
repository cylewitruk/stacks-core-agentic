//! `sbagent session run` — full-pipeline orchestrator.
//!
//! Chains the per-phase commands (baseline → triage → analysis → merge →
//! optimize → bench → finalize → optional publish) into one invocation.
//! Each phase is a direct in-process call into the typed module that
//! already implements it. Phase 5 (`publish`) used to fork through
//! `sudo` to a separate publisher user; that split is gone — the token
//! is read into `sbagent`'s memory at Phase 5 startup and never leaves.
//!
//! Internally each phase is its own `fn phase_*` so `run()` reads as a
//! linear phase list. Cross-phase state (resolved layout, harness, bench
//! range, etc.) is bundled in [`PhaseEnv`] and passed by `&` to every
//! phase. The only inter-phase value that isn't in `PhaseEnv` is the
//! [`baseline::ArchiveBinaryOutputs`] Phase 0a produces and Phase 0
//! consumes.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::session::bench_range::{BenchRangeArgs, ResolvedBenchRange};
use crate::cli::{CliContext, preflight};
use crate::harnesses::codex::CodexHarness;
use crate::session::bench::StacksBenchCli;
use crate::session::cargo::StdCargoRunner;
use crate::session::finalize::{self, FinalizeInputs};
use crate::session::{
    SessionLayout, analyzers, baseline, bench_experiments, db_consistency, loader, merge,
    optimizers, publish, triage,
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
    /// Override `settings.optimizer.attempts` (Layer 1B inner-loop
    /// attempt cap). Defaults to settings → `5`.
    #[clap(long)]
    pub optimizer_attempts: Option<u32>,
    /// Override `settings.optimizer.budget_minutes` (Layer 1B
    /// inner-loop wall-clock budget). Defaults to settings → `60`.
    #[clap(long)]
    pub optimizer_budget_minutes: Option<u32>,
    /// Operator weights for the triage selection lenses (e.g. `0.4,0.4,0.2`).
    /// Defaults from `settings.triage.axis_weights`, then `0.4,0.4,0.2`.
    #[clap(long)]
    pub axis_weights: Option<String>,
    /// Base branch for optimizer worktrees. Defaults from
    /// `settings.publish.base_branch`, then `feat/stacks-bench`.
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

/// Shared cross-phase state. Built once at the top of [`run`] and passed
/// by `&` to every `phase_*` function so each phase is a self-contained
/// unit of work with explicit inputs.
struct PhaseEnv<'a> {
    args: &'a RunSessionArgs,
    ctx: &'a CliContext,
    session_id: &'a SessionId,
    layout: SessionLayout,
    harness: Arc<CodexHarness>,
    range: ResolvedBenchRange,
    axis_weights: String,
    base_branch: String,
    /// Resolved per-session source state from the v3 materialization
    /// step. Every phase that previously consumed
    /// `<operator>/repos/<base>/` now reads
    /// `source.session_checkout` instead.
    source: crate::source::ResolvedSource,
}

impl<'a> PhaseEnv<'a> {
    /// Resolve every cross-phase value (block range, axis weights, base
    /// branch) once, applying CLI → settings → default precedence.
    fn resolve(
        args: &'a RunSessionArgs,
        ctx: &'a CliContext,
        session_id: &'a SessionId,
        source: crate::source::ResolvedSource,
    ) -> Result<Self> {
        // Phase 0 (baseline) AND Phase 3 (per-target experiments) must
        // replay the same blocks — candidate selection during triage /
        // analysis talks about the baseline run, so the experiments
        // need to match. Even with `--import-baseline-run-id` set we
        // still need the range for Phase 3.
        let range = args
            .range
            .resolve(&ctx.settings)?;
        let axis_weights = args
            .axis_weights
            .clone()
            .unwrap_or_else(|| {
                ctx.settings
                    .triage
                    .effective_axis_weights()
                    .to_owned()
            });
        let base_branch = args
            .base_branch
            .clone()
            .or_else(|| {
                ctx.settings
                    .publish
                    .base_branch
                    .clone()
            })
            .unwrap_or_else(|| "feat/stacks-bench".to_owned());
        Ok(Self {
            args,
            ctx,
            session_id,
            layout: SessionLayout::from_layout(&ctx.layout, session_id.clone()),
            harness: Arc::new(CodexHarness::new()),
            range,
            axis_weights,
            base_branch,
            source,
        })
    }
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

    // v3 Phase 3: materialize the per-session source checkout +
    // write `source.json` BEFORE constructing PhaseEnv. Every
    // downstream phase that previously consumed
    // `<operator>/repos/<base>/` now reads
    // `env.source.session_checkout`. Order: preflight (which
    // validates [source] config) -> .run.pid install -> source
    // materialization -> PhaseEnv -> Phase 0a.
    let session_layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    println!(
        "session: {}",
        session_layout
            .results_dir
            .display()
    );

    // Drop guard for `.run.pid`: written after preflight passes (so a
    // failing preflight never leaves a marker behind), cleared on
    // every exit path the runtime unwinds through — normal return,
    // `?` bail, and unwinding panics. SIGINT/SIGKILL terminate
    // without unwinding and leave the file behind; `workspace prune`'s
    // liveness check handles that stale-PID case.
    let _pid_guard = crate::session::run_pid::RunPidGuard::install(session_layout.session_dir())
        .context(".run.pid install")?;

    // Source materialization: writes `<session>/results/source.json`
    // on fresh runs (write-once); reads + reuses it on resume. The
    // `results/` dir must exist before SourceJson::write — create it
    // here so source.json lands in the canonical spot.
    std::fs::create_dir_all(&session_layout.results_dir).with_context(|| {
        format!(
            "creating session results dir {}",
            session_layout
                .results_dir
                .display(),
        )
    })?;
    let workspace_root = ctx
        .layout
        .require_agent_workspace_root()?
        .to_path_buf();
    let source = crate::source::materialize_session_source(
        &crate::source::StdSourceRepo,
        &workspace_root,
        session_id.as_str(),
        &ctx.settings.source,
        &session_layout.source_json(),
    )
    .context("v3 Phase 3: per-session source materialization")?;
    println!(
        "source: {} ({}@{})",
        source
            .session_checkout
            .display(),
        source.source.branch,
        &source.source.sha[..source
            .source
            .sha
            .len()
            .min(10)],
    );

    let env = PhaseEnv::resolve(&args, ctx, session_id, source)?;

    // When Phase 5 is requested, validate publish wiring NOW — before
    // Phases 0-4 burn an hour of compute. Catches an unreadable/empty
    // token file and an unauthorized or wrong `publish.base_repo`.
    if env.args.publish_accepted_prs {
        preflight::ensure_publish_wiring(env.ctx).await?;
    }

    let archive_outputs = phase_0a_archive_baseline(&env)?;
    phase_0_baseline(&env, &archive_outputs)?;
    phase_1_triage(&env).await?;
    phase_1_5_analyzers(&env).await?;
    phase_1_7_merge(&env).await?;
    phase_1_8_calibration(&env)?;
    phase_2_optimizers(&env).await?;
    phase_3_bench_experiments(&env)?;
    phase_3_5_results_analyzer(&env).await?;
    phase_4_finalize(&env)?;
    if env.args.publish_accepted_prs {
        phase_5_publish(&env).await?;
    }
    if env.args.archive {
        phase_6_archive(&env)?;
    }
    session_end_cleanup(&env)?;

    println!();
    println!(
        "session: {}",
        env.layout
            .results_dir
            .display()
    );
    println!(
        "summary: {}",
        env.layout
            .summary_md()
            .display()
    );
    Ok(())
}

/// Phase 0a: build + archive the baseline `stacks-bench` binary BEFORE
/// either Phase 0 branch (fresh run OR import). Phase 1.8 calibration
/// depends on `baseline/bin/stacks-bench` existing — including in
/// imported-baseline sessions where Phase 0b is skipped.
fn phase_0a_archive_baseline(env: &PhaseEnv<'_>) -> Result<baseline::ArchiveBinaryOutputs> {
    // v3 Phase 3 cutover: `cargo build` runs inside the per-session
    // source checkout, not inside `<operator>/repos/<base>/`. This
    // eliminates the cross-session `<base>/target/` pollution drift
    // mode named in Decision 0003 (concurrent sessions can't share
    // the operator submodule's `target/` cache anymore — each session
    // has its own `target/` under the per-session checkout).
    let stacks_core_base = env
        .source
        .session_checkout
        .clone();
    let outputs = baseline::archive_baseline_binary(&baseline::ArchiveBinaryInputs {
        layout: &env.layout,
        stacks_core_base: &stacks_core_base,
    })
    .context("Phase 0a")?;
    eprintln!(
        "Phase 0a: archived baseline stacks-bench binary at {} (source_sha={})",
        outputs
            .archived_path
            .display(),
        outputs.source_sha,
    );
    Ok(outputs)
}

/// Phase 0: import an existing baseline run id, or run a fresh
/// baseline benchmark. Always reads the archived binary from Phase 0a
/// (no fallback to `target/release/stacks-bench`).
fn phase_0_baseline(
    env: &PhaseEnv<'_>,
    archive_outputs: &baseline::ArchiveBinaryOutputs,
) -> Result<()> {
    // v3 Phase 3 cutover: bench cargo cwd is the per-session source
    // checkout (matches the cwd Phase 0a built against).
    let stacks_core_base = env
        .source
        .session_checkout
        .clone();
    let bench = StacksBenchCli::strict_archived(
        archive_outputs
            .archived_path
            .clone(),
        env.ctx
            .layout
            .stacks_bench_data_dir
            .clone(),
        stacks_core_base,
    );
    if let Some(run_id) = env
        .args
        .import_baseline_run_id
    {
        let inputs = baseline::ImportInputs::from_settings(
            &env.layout,
            &bench,
            run_id,
            env.args
                .import_baseline_rerun_id,
            &env.ctx.settings,
            &env.ctx.layout.bench_lock,
        );
        baseline::import(&inputs).context("Phase 0: baseline import")?;
    } else {
        let source_dir = env
            .ctx
            .settings
            .stacks_bench
            .source_dir_required()
            .context("Phase 0: baseline run")?;
        baseline::run(&baseline::RunInputs {
            layout: &env.layout,
            bench: &bench,
            source_dir,
            network: env
                .ctx
                .settings
                .stacks_bench
                .effective_network(),
            start_at: env.range.start_at,
            count: env.range.count,
            warmup: env.range.warmup,
            filter: env.range.filter.as_deref(),
            shadow_dir_root: env
                .ctx
                .layout
                .stacks_bench_shadow_dir
                .as_deref(),
            single_run_noise_floor_pct: env
                .ctx
                .settings
                .triage
                .effective_single_run_noise_floor_pct(),
            bench_lock: &env.ctx.layout.bench_lock,
        })
        .context("Phase 0: baseline run")?;
    }
    Ok(())
}

/// Phase 1: triage. Produces `candidates.json` listing the workload
/// families worth optimizing.
async fn phase_1_triage(env: &PhaseEnv<'_>) -> Result<()> {
    triage::run(&triage::Inputs {
        layout: &env.layout,
        framework: &env.ctx.layout,
        settings: &env.ctx.settings,
        axis_weights: &env.axis_weights,
        harness: env.harness.as_ref(),
        source_checkout: &env.source.session_checkout,
    })
    .await
    .context("Phase 1: triage")?;
    Ok(())
}

/// Phase 1.5: one analyzer subagent per triage candidate. Per-family
/// `analysis.json` files land under `analysis/<family-id>/`.
async fn phase_1_5_analyzers(env: &PhaseEnv<'_>) -> Result<()> {
    analyzers::run(analyzers::Inputs {
        layout: env.layout.clone(),
        framework: env.ctx.layout.clone(),
        settings: env.ctx.settings.clone(),
        parallel: env.args.parallel_analyzers,
        harness: env.harness.clone(),
        source_checkout: env
            .source
            .session_checkout
            .clone(),
    })
    .await
    .context("Phase 1.5: analyzers")?;
    Ok(())
}

/// Phase 1.7: LLM merge of accepted analyses → `optimization-targets.json`.
async fn phase_1_7_merge(env: &PhaseEnv<'_>) -> Result<()> {
    merge::run(&merge::Inputs {
        layout: &env.layout,
        framework: &env.ctx.layout,
        settings: &env.ctx.settings,
        harness: env.harness.as_ref(),
    })
    .await
    .context("Phase 1.7: merge")?;
    Ok(())
}

/// Phase 1.8: per-target targeted baseline calibration. For each
/// normal_pr target with verification_replay, run one bench invocation
/// per replay phase against the strict archived baseline binary. The
/// resulting baseline-run-ids feed Phase 4 finalize's apples-to-apples
/// improvement_pct comparison.
fn phase_1_8_calibration(env: &PhaseEnv<'_>) -> Result<()> {
    let targets = loader::read_optimization_targets(&env.layout).context("Phase 1.8")?;
    // v3 Phase 3 cutover: same per-session checkout as Phase 0a/0.
    let stacks_core_base = env
        .source
        .session_checkout
        .clone();
    let bench = StacksBenchCli::strict_archived(
        env.layout.baseline_bin_path(),
        env.ctx
            .layout
            .stacks_bench_data_dir
            .clone(),
        stacks_core_base,
    );
    let source_dir = env
        .ctx
        .settings
        .stacks_bench
        .source_dir_required()
        .context("Phase 1.8: targeted baseline calibration")?;
    crate::session::calibration::run(&crate::session::calibration::Inputs {
        layout: &env.layout,
        bench: &bench,
        source_dir,
        network: env
            .ctx
            .settings
            .stacks_bench
            .effective_network(),
        shadow_dir_root: env
            .ctx
            .layout
            .stacks_bench_shadow_dir
            .as_deref(),
        bench_lock: &env.ctx.layout.bench_lock,
        targets: &targets,
    })
    .context("Phase 1.8: targeted baseline calibration")?;
    Ok(())
}

/// Phase 2: optimizer fan-out. CLI flags override `settings.optimizer.*`;
/// `optimizers::run` applies a final clamp to enforce the inner-loop
/// bench's single-writer constraint.
async fn phase_2_optimizers(env: &PhaseEnv<'_>) -> Result<()> {
    let mut settings = env.ctx.settings.clone();
    if let Some(n) = env.args.optimizer_attempts {
        settings.optimizer.attempts = Some(n);
    }
    if let Some(m) = env
        .args
        .optimizer_budget_minutes
    {
        settings
            .optimizer
            .budget_minutes = Some(m);
    }
    optimizers::run(optimizers::Inputs {
        layout: env.layout.clone(),
        framework: env.ctx.layout.clone(),
        settings,
        parallel: env.args.parallel_agents,
        base_branch: env.base_branch.clone(),
        harness: env.harness.clone(),
        git: Arc::new(optimizers::StdGitCheckoutManager),
        resume: env.args.resume,
        source_checkout: env
            .source
            .session_checkout
            .clone(),
    })
    .await
    .context("Phase 2: optimizers")?;
    Ok(())
}

/// Phase 3: bench experiments. One bench invocation per merged target;
/// shared `stacks-bench` DB (single-writer constraint preserved by the
/// `bench_lock`).
fn phase_3_bench_experiments(env: &PhaseEnv<'_>) -> Result<()> {
    let data_dir = env
        .ctx
        .layout
        .stacks_bench_data_dir
        .clone();
    // v3 Phase 3 cutover: Phase 3 candidate bench cargo cwd is the
    // per-session source checkout, matching every other build step.
    let cargo_cwd = env
        .source
        .session_checkout
        .clone();
    let bench_for_target = move |bin: &std::path::Path| {
        bench_experiments::stacks_bench_for(bin, &data_dir, &cargo_cwd)
    };
    let bench_for_target_ref: &dyn Fn(
        &std::path::Path,
    ) -> Box<dyn crate::session::bench::BenchClient> = &bench_for_target;
    let source_dir = env
        .ctx
        .settings
        .stacks_bench
        .source_dir_required()
        .context("Phase 3")?;
    let worktrees_root = env
        .ctx
        .layout
        .session_optimizer_checkouts_dir(env.session_id);
    let targets = loader::read_optimization_targets(&env.layout).context("Phase 3")?;
    bench_experiments::run(&bench_experiments::Inputs {
        layout: &env.layout,
        worktrees_root: &worktrees_root,
        targets: &targets,
        env: bench_experiments::BenchEnv {
            source_dir,
            network: env
                .ctx
                .settings
                .stacks_bench
                .effective_network(),
            shadow_dir_root: env
                .ctx
                .layout
                .stacks_bench_shadow_dir
                .as_deref(),
        },
        bench_lock: &env.ctx.layout.bench_lock,
        skip_cargo_clean: env.args.skip_cargo_clean,
        cargo: &StdCargoRunner,
        bench_for_target: bench_for_target_ref,
    })
    .context("Phase 3")?;
    Ok(())
}

/// Phase 3.5: results-analyzer fan-out. One agent per
/// `bench_eligible` target with an Implemented optimizer report +
/// matching baseline/candidate run-ids — judges measured vs
/// `expected_signal` and writes a typed verdict that Phase 4 finalize
/// sources `improvement_pct` + `status` from. Per-target failure is
/// non-fatal: the offending target lands at finalize as an Aborted
/// experiment with `reason = "results-analyzer did not produce a
/// verdict: ..."`. Concurrency: `analyzer.concurrency_cap`.
async fn phase_3_5_results_analyzer(env: &PhaseEnv<'_>) -> Result<()> {
    use std::sync::Arc;

    use crate::harnesses::codex::CodexHarness;
    use crate::session::results_analyzer;

    let harness = Arc::new(CodexHarness::new());
    let outputs = results_analyzer::run(results_analyzer::Inputs {
        layout: env.layout.clone(),
        framework: env.ctx.layout.clone(),
        settings: env.ctx.settings.clone(),
        parallel: None,
        harness,
        source_checkout: env
            .source
            .session_checkout
            .clone(),
    })
    .await
    .context("Phase 3.5: results-analyzer")?;
    let (produced, skipped, failed) = outputs.tally();
    println!(
        "Phase 3.5 results-analyzers: {produced} produced, {skipped} skipped, {failed} failed"
    );
    Ok(())
}

/// Phase 4: finalize. Aggregate per-target outcomes into
/// `summary.json` and `summary.md`. Warn on dangling DB-vs-artifact
/// run-id refs first so they don't get baked into the immutable
/// summary.
fn phase_4_finalize(env: &PhaseEnv<'_>) -> Result<()> {
    // v3 Phase 3 cutover: the DB-consistency probe shells out to
    // `stacks-bench bench show` — use the archived Phase 0a baseline
    // binary, not a `cargo stacks-bench` fallback against the
    // soon-to-be-removed operator submodule. Cargo cwd is the
    // per-session source checkout for the (unused, since strict)
    // cargo fallback path.
    let bench = StacksBenchCli::strict_archived(
        env.layout.baseline_bin_path(),
        env.ctx
            .layout
            .stacks_bench_data_dir
            .clone(),
        env.source
            .session_checkout
            .clone(),
    );
    db_consistency::warn_dangling_refs(&env.layout, &bench).context("Phase 4: DB consistency")?;
    finalize::finalize(&FinalizeInputs { layout: &env.layout }).context("Phase 4: finalize")?;
    Ok(())
}

/// Phase 5: publish (generate prompts → push PRs/issues). Token is read
/// into sbagent's process memory at call time; it never leaves.
async fn phase_5_publish(env: &PhaseEnv<'_>) -> Result<()> {
    publish::generate(&publish::GenerateInputs {
        layout: &env.layout,
        framework: &env.ctx.layout,
        settings: &env.ctx.settings,
        harness: env.harness.as_ref(),
    })
    .await
    .context("Phase 5: generate")?;

    let publish_config = publish::PublishConfig::from_settings(&env.ctx.settings);
    publish::ensure_token_outside_framework(
        &publish_config.publish_token_file,
        env.ctx
            .layout
            .framework
            .as_deref()
            .map(|p| p as &std::path::Path),
    )?;
    let token =
        publish::read_publish_token(&publish_config.publish_token_file).context("Phase 5")?;
    let gh = publish::StdGhClient::from_token(&token)?;
    publish::push(&publish::PushInputs {
        layout: &env.layout,
        framework: &env.ctx.layout,
        config: &publish_config,
        gh: &gh,
    })
    .await
    .context("Phase 5: push")?;
    Ok(())
}

/// Phase 6: archive the session into the operator's git repo
/// (`session/<id>` write-once branch + `sessions.jsonl` append). Warn on
/// dangling DB ↔ artifact refs first so the archive doesn't bake them
/// into the immutable branch.
fn phase_6_archive(env: &PhaseEnv<'_>) -> Result<()> {
    // v3 Phase 3 cutover: same archived-binary path as Phase 4
    // finalize. The DB-consistency probe doesn't need cargo; the
    // strict-archived path keeps the contract self-contained.
    let bench = StacksBenchCli::strict_archived(
        env.layout.baseline_bin_path(),
        env.ctx
            .layout
            .stacks_bench_data_dir
            .clone(),
        env.source
            .session_checkout
            .clone(),
    );
    db_consistency::warn_dangling_refs(&env.layout, &bench).context("Phase 6: DB consistency")?;
    let outputs = crate::session::archive::archive(&crate::session::archive::ArchiveInputs {
        layout: &env.layout,
        framework: &env.ctx.layout,
        settings: &env.ctx.settings,
        dry_run: false,
    })
    .context("Phase 6")?;
    crate::session::archive::print_outputs(&outputs);
    Ok(())
}

/// Session-end sweep: tear down per-target clones for aborted
/// experiments so they don't accumulate in the operator's tree. Kept
/// (implementation.md-marked) checkouts survive — Phase 5 publish
/// pushed from them, and operators may inspect them. With the
/// clone-based model, teardown is just `rm -rf <clone>` — the
/// `agent/<session>/<target>` branch lives inside the clone and goes
/// away with it.
fn session_end_cleanup(env: &PhaseEnv<'_>) -> Result<()> {
    let git = optimizers::StdGitCheckoutManager;
    let checkouts_root = env
        .ctx
        .layout
        .session_optimizer_checkouts_dir(env.session_id);
    let dropped = optimizers::prune_aborted_experiments(&git, &checkouts_root, &env.layout)
        .context("session-end experiment cleanup")?;
    if dropped > 0 {
        println!("session-end cleanup: dropped {dropped} aborted experiment(s)");
    }
    Ok(())
}
