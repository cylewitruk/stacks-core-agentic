//! Phase 3.5: post-bench results-analyzer fan-out.
//!
//! One Codex agent invocation per `bench_eligible` target whose Phase 3
//! verification bench actually produced run-ids. Each agent reads the
//! target's `verification_replay` (analyzer hypothesis), the
//! `optimizer-report.json` (claim + diff), and the per-invocation
//! target calibration baseline + verification bench `bench-run.json` files;
//! writes a typed
//! [`ResultsAnalysis`](crate::models::results_analysis::ResultsAnalysis)
//! verdict to `results/analyze/<target>/results-analysis.json`.
//!
//! Phase 4 finalize then sources `Experiment.improvement_pct` +
//! `Experiment.status` from this verdict verbatim. A target whose
//! results-analyzer fails (timeout, parse error, validation error) lands
//! at finalize with the file absent and surfaces as an `Aborted`
//! experiment with `reason = "results-analyzer did not produce a
//! verdict: ..."` — the rest of the session ships unaffected.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::harnesses::{AgentHarness, InvokeInputs};
use crate::layout::Layout;
use crate::models::common::DeliveryMode;
use crate::models::optimizer_report::OptimizerReport;
use crate::models::results_analysis::ResultsAnalysis;
use crate::models::targets::MergedTarget;
use crate::models::{FromJsonValidated, ToJson, ValidateModel};
use crate::prompts;
use crate::session::{SessionLayout, loader};
use crate::settings::Settings;

/// Inputs to the results-analyzer fan-out.
pub struct Inputs<H: AgentHarness + 'static> {
    /// Resolved per-session layout.
    pub layout: SessionLayout,
    /// Resolved framework + data layout.
    pub framework: Layout,
    /// Settings (codex model + reasoning effort + timeout + parallel cap).
    pub settings: Settings,
    /// Concurrency cap. `None` defaults to
    /// `analyzer.concurrency_cap` (the results-analyzer reuses the
    /// analyzer's parallel knob — the per-target workload is shaped the
    /// same way).
    pub parallel: Option<usize>,
    /// Agent harness, shared across spawned tasks via `Arc`.
    pub harness: Arc<H>,
    /// Per-session source checkout passed to every results-analyzer
    /// prompt as `base` and granted to Codex via `add_dirs`.
    pub source_checkout: std::path::PathBuf,
}

/// Per-target outcome of [`run`].
#[derive(Debug)]
pub enum TargetOutcome {
    /// Verdict produced and validated.
    Produced(ResultsAnalysis),
    /// Target was skipped without invoking the agent (consensus mode,
    /// aborted optimizer report, missing candidate run-ids, etc.).
    Skipped { reason: String },
    /// Agent ran but the verdict didn't materialize cleanly. The reason
    /// flows into Phase 4's `Experiment.reason` so finalize can decide.
    Failed { reason: String },
}

/// Outputs of [`run`].
#[derive(Debug, Default)]
pub struct Outputs {
    /// One entry per `MergedTarget` in `optimization-targets.json`,
    /// in source order.
    pub per_target: Vec<(String, TargetOutcome)>,
}

impl Outputs {
    /// Headline counts for CLI summaries.
    pub fn tally(&self) -> (usize, usize, usize) {
        let mut produced = 0;
        let mut skipped = 0;
        let mut failed = 0;
        for (_, o) in &self.per_target {
            match o {
                TargetOutcome::Produced(_) => produced += 1,
                TargetOutcome::Skipped { .. } => skipped += 1,
                TargetOutcome::Failed { .. } => failed += 1,
            }
        }
        (produced, skipped, failed)
    }
}

/// Run the results-analyzer fan-out. One async task per
/// `bench_eligible` target; concurrency capped via
/// [`Inputs::parallel`] (falls back to `analyzer.concurrency_cap`).
///
/// Per-target failure is non-fatal: the offending target lands in
/// [`Outputs`] as [`TargetOutcome::Failed`] and the rest continue.
pub async fn run<H>(inputs: Inputs<H>) -> Result<Outputs>
where
    H: AgentHarness + 'static,
{
    let targets = loader::read_optimization_targets(&inputs.layout)
        .context("loading optimization-targets")?;

    // Decide each target's eligibility up-front. The skip reasons are
    // recorded synchronously so we never spawn a task that would just
    // skip — and the outcome ordering matches `targets.targets`.
    let mut spawn_specs: Vec<(usize, MergedTarget)> = Vec::new();
    let mut outcomes: Vec<(String, TargetOutcome)> = Vec::with_capacity(targets.targets.len());
    for (i, t) in targets
        .targets
        .iter()
        .enumerate()
    {
        match eligibility(&inputs.layout, t)? {
            Eligibility::Eligible => {
                spawn_specs.push((i, t.clone()));
                outcomes
                    .push((t.id.clone(), TargetOutcome::Skipped { reason: "scheduled".into() }));
            }
            Eligibility::Skip(reason) => {
                outcomes.push((t.id.clone(), TargetOutcome::Skipped { reason }));
            }
        }
    }

    if spawn_specs.is_empty() {
        return Ok(Outputs { per_target: outcomes });
    }

    let configured = inputs
        .parallel
        .unwrap_or_else(|| {
            inputs
                .settings
                .analyzer
                .effective_concurrency_cap()
        })
        .max(1);
    let parallel = configured.min(spawn_specs.len());
    let semaphore = Arc::new(Semaphore::new(parallel));

    let mut set: JoinSet<(usize, String, Result<ResultsAnalysis>)> = JoinSet::new();
    for (idx, target) in spawn_specs {
        let task_inputs = TaskInputs {
            target,
            session_id: inputs
                .layout
                .id
                .as_str()
                .to_owned(),
            layout: inputs.layout.clone(),
            framework: inputs.framework.clone(),
            settings: inputs.settings.clone(),
            sem: semaphore.clone(),
            harness: inputs.harness.clone(),
            source_checkout: inputs.source_checkout.clone(),
        };
        let target_id = task_inputs.target.id.clone();
        set.spawn(async move {
            let res = run_one(&task_inputs).await;
            (idx, target_id, res)
        });
    }

    let mut panic_count = 0u32;
    while let Some(joined) = set.join_next().await {
        let (idx, target_id, res) = match joined {
            Ok(triple) => triple,
            Err(e) => {
                // Task panic — JoinError carries no target context, so
                // we can't pin it to a specific slot. Bump a counter
                // and patch the remaining `scheduled` placeholders to
                // `Failed { reason: panic }` once the loop drains.
                tracing::error!(?e, "results-analyzer task panicked");
                panic_count += 1;
                continue;
            }
        };
        let outcome = match res {
            Ok(ra) => TargetOutcome::Produced(ra),
            Err(e) => TargetOutcome::Failed { reason: format!("{e:#}") },
        };
        debug_assert_eq!(outcomes[idx].0, target_id);
        outcomes[idx] = (target_id, outcome);
    }

    // Any slot still showing the placeholder `scheduled` reason after
    // the join loop must be a slot whose task panicked (otherwise the
    // join branch above would have replaced it). Patch them all to
    // `Failed { reason: panic }` so the tally + finalize see a clean
    // failure signal rather than a "scheduled" outcome surviving.
    if panic_count > 0 {
        let mut patched = 0u32;
        for (_, outcome) in outcomes.iter_mut() {
            if let TargetOutcome::Skipped { reason } = outcome
                && reason == "scheduled"
            {
                *outcome = TargetOutcome::Failed {
                    reason: "results-analyzer task panicked (panic not pinned to a specific \
                             target by tokio)"
                        .to_owned(),
                };
                patched += 1;
            }
        }
        if patched != panic_count {
            tracing::warn!(
                panic_count,
                patched,
                "results-analyzer panic count does not match the number of unfilled scheduled \
                 slots; tally may understate failures",
            );
        }
    }

    Ok(Outputs { per_target: outcomes })
}

/// Per-target spawn state. Owned because tokio tasks are `'static`.
struct TaskInputs<H: AgentHarness + 'static> {
    target: MergedTarget,
    session_id: String,
    layout: SessionLayout,
    framework: Layout,
    settings: Settings,
    sem: Arc<Semaphore>,
    harness: Arc<H>,
    /// Per-session source checkout (carried from
    /// [`Inputs::source_checkout`]).
    source_checkout: std::path::PathBuf,
}

/// Eligibility verdict for one target. Eligible targets get an agent
/// task; everything else lands in the skip column with a
/// human-readable reason.
enum Eligibility {
    Eligible,
    Skip(String),
}

fn eligibility(layout: &SessionLayout, target: &MergedTarget) -> Result<Eligibility> {
    if !matches!(target.delivery_mode, DeliveryMode::NormalPr) {
        return Ok(Eligibility::Skip(format!(
            "not bench_eligible (delivery_mode={:?})",
            target.delivery_mode
        )));
    }
    // Phase 2 must have committed an Implemented report. Aborted /
    // missing reports leave nothing to judge.
    match loader::read_optimizer_report_for_target(layout, &target.id, target.delivery_mode)? {
        Some(OptimizerReport::Implemented(_)) => {}
        Some(OptimizerReport::Aborted(_)) => {
            return Ok(Eligibility::Skip("optimizer report outcome=aborted".into()));
        }
        None => {
            return Ok(Eligibility::Skip("no optimizer-report.json on disk".into()));
        }
    }
    // Phase 3 must have produced verification-bench run-ids. The file name is
    // the legacy `candidate-run-ids` contract.
    let cand_path = layout.experiment_candidate_run_ids_json(&target.id);
    if !cand_path.is_file() {
        return Ok(Eligibility::Skip(format!(
            "no candidate-run-ids.json at {}",
            cand_path.display()
        )));
    }
    // Symmetrically, Phase 1.8 must have produced target-calibration-baseline
    // run-ids. The file name is the legacy `baseline-run-ids` contract.
    let base_path = layout.verify_baseline_run_ids_json(&target.id);
    if !base_path.is_file() {
        return Ok(Eligibility::Skip(format!(
            "no baseline-run-ids.json at {}",
            base_path.display()
        )));
    }
    Ok(Eligibility::Eligible)
}

async fn run_one<H: AgentHarness + 'static>(state: &TaskInputs<H>) -> Result<ResultsAnalysis> {
    let sem = state.sem.clone();
    let _permit = sem
        .acquire_owned()
        .await
        .context("acquiring results-analyzer semaphore permit")?;

    let target_id = state.target.id.as_str();
    let out_dir = state
        .layout
        .analyze_target_dir(target_id);
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let prompts_dir = state
        .settings
        .require_prompt_overrides_dir()?;

    // Load the optimizer report so we can hand it to the agent (it's
    // small enough to inline). `eligibility` already verified the
    // Implemented variant; assert here so a race between phases turns
    // into a clear error rather than a silent string of `null`.
    let optimizer_report = match loader::read_optimizer_report_for_target(
        &state.layout,
        target_id,
        state.target.delivery_mode,
    )? {
        Some(OptimizerReport::Implemented(r)) => OptimizerReport::Implemented(r),
        Some(OptimizerReport::Aborted(_)) | None => {
            anyhow::bail!(
                "internal: target `{target_id}` was eligible at scheduling but its \
                 optimizer-report is no longer Implemented",
            );
        }
    };
    let optimizer_report_json = optimizer_report
        .to_json_pretty()
        .context("serializing optimizer-report for prompt")?;
    let target_json = state
        .target
        .to_json_pretty()
        .context("serializing merged target for prompt")?;

    let rendered = prompts::render(
        "results-analyzer",
        &prompts::ResultsAnalyzerPrompt {
            session_id: state.session_id.clone(),
            target_id: target_id.to_owned(),
            output_dir: out_dir
                .to_string_lossy()
                .into_owned(),
            // Prompt `base` = per-session source checkout.
            base: state
                .source_checkout
                .to_string_lossy()
                .into_owned(),
            stacks_bench_data_dir: state
                .framework
                .stacks_bench_data_dir
                .to_string_lossy()
                .into_owned(),
            queries_dir: state
                .framework
                .queries_dir
                .to_string_lossy()
                .into_owned(),
            target_json,
            optimizer_report_json,
            candidate_invocations_dir: state
                .layout
                .experiment_dir(target_id)
                .to_string_lossy()
                .into_owned(),
            baseline_invocations_dir: state
                .layout
                .verify_target_dir(target_id)
                .to_string_lossy()
                .into_owned(),
            candidate_run_ids_path: state
                .layout
                .experiment_candidate_run_ids_json(target_id)
                .to_string_lossy()
                .into_owned(),
            baseline_run_ids_path: state
                .layout
                .verify_baseline_run_ids_json(target_id)
                .to_string_lossy()
                .into_owned(),
            results_analysis_schema_path: state
                .framework
                .schemas_dir
                .join("results-analysis.schema.json")
                .to_string_lossy()
                .into_owned(),
        },
        prompts_dir,
    )?;
    std::fs::write(
        state
            .layout
            .analyze_prompt(target_id),
        &rendered,
    )?;

    let timeout = state
        .settings
        .codex
        .effective_exec_timeout();
    let model = state
        .settings
        .codex
        .effective_model();
    let reasoning_effort = state
        .settings
        .codex
        .reasoning_effort
        .as_deref();
    let dangerous = state
        .settings
        .codex
        .dangerously_bypass_sandbox
        .unwrap_or(false);

    // The agent reads bench-run.json from verify/<target>/ and
    // optimize/<target>/, reads the schema from the schemas dir, and
    // writes outputs into analyze/<target>/. Hand it write access to
    // its output dir and read access to everything else.
    let mut add_dirs: Vec<PathBuf> = vec![
        out_dir.clone(),
        state
            .layout
            .verify_target_dir(target_id),
        state
            .layout
            .experiment_dir(target_id),
        // Grant the per-session source checkout.
        state.source_checkout.clone(),
        state
            .framework
            .stacks_bench_data_dir
            .clone(),
        state
            .framework
            .queries_dir
            .clone(),
        state
            .framework
            .schemas_dir
            .clone(),
        prompts_dir.to_path_buf(),
    ];
    add_dirs.extend(
        state
            .settings
            .codex
            .extra_writable_roots
            .iter()
            .cloned(),
    );

    let _invoke_outputs = state
        .harness
        .invoke(&InvokeInputs {
            rendered_prompt: &rendered,
            cwd: out_dir.as_ref(),
            add_dirs: &add_dirs,
            events_jsonl: &state
                .layout
                .analyze_events_jsonl(target_id),
            stderr_log: &state
                .layout
                .analyze_stderr(target_id),
            last_message: &state
                .layout
                .analyze_final_message(target_id),
            timeout,
            model,
            reasoning_effort,
            skip_git_repo_check: true,
            dangerously_bypass_sandbox: dangerous,
            enable_web_search: false,
            extra_env: &[],
        })
        .await
        .with_context(|| format!("invoking codex for results-analyzer `{target_id}`"))?;

    // Load + validate the agent's output. Schema/parse/validation
    // failures bubble up as `TargetOutcome::Failed` rather than
    // killing the phase — finalize will close the experiment as
    // Aborted with this reason.
    let ra_path = state
        .layout
        .analyze_results_analysis_json(target_id);
    let raw = std::fs::read_to_string(&ra_path)
        .with_context(|| format!("reading {}", ra_path.display()))?;
    let ra = ResultsAnalysis::from_json_validated(&raw)
        .with_context(|| format!("parsing/validating {}", ra_path.display()))?;
    ra.validate_model()
        .with_context(|| format!("cross-field validation of {}", ra_path.display()))?;
    // Context check: the agent must echo our session + target ids.
    if ra.session_id != state.session_id {
        anyhow::bail!(
            "results-analysis.session_id = {:?} does not match the session this phase ran in \
             ({:?})",
            ra.session_id,
            state.session_id,
        );
    }
    if ra.target_id != target_id {
        anyhow::bail!(
            "results-analysis.target_id = {:?} does not match the target the agent was given \
             ({:?})",
            ra.target_id,
            target_id,
        );
    }
    Ok(ra)
}
