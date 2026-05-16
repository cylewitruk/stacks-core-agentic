//! Phase 1.5: analyzer fan-out.
//!
//! One Codex agent invocation per family from `candidates.json`, run in
//! parallel up to a configurable concurrency cap. Each analyzer receives a
//! single-family slice of `candidates.json` (`FAMILY_JSON`) and writes its
//! result to `analysis/<family-id>/analysis.json`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::harnesses::{AgentHarness, InvokeInputs};
use crate::layout::Layout;
use crate::models::analyze::Analysis;
use crate::prompts;
use crate::session::{SessionLayout, loader};
use crate::settings::Settings;

/// Inputs to an analyzer fan-out.
pub struct Inputs<H: AgentHarness + 'static> {
    /// Resolved per-session layout.
    pub layout: SessionLayout,
    /// Resolved framework + data layout.
    pub framework: Layout,
    /// Settings (codex model + reasoning effort + timeout + parallel cap).
    pub settings: Settings,
    /// Concurrency cap. `None` defaults to "one task per family".
    pub parallel: Option<usize>,
    /// Agent harness, shared across spawned tasks via `Arc`.
    pub harness: Arc<H>,
}

/// Outputs of an analyzer fan-out.
#[derive(Debug, Default)]
pub struct Outputs {
    /// Number of family analyses written.
    pub total: usize,
    /// Of `total`, how many are `accepted`.
    pub accepted: usize,
    /// Of `total`, how many are `rejected`.
    pub rejected: usize,
}

/// Run analyzer fan-out. Mirrors the bash script's `xargs -P` loop:
/// `parallel` tasks at a time, each invoking the harness for one family.
pub async fn run<H>(inputs: Inputs<H>) -> Result<Outputs>
where
    H: AgentHarness + 'static,
{
    let candidates = loader::read_candidates(&inputs.layout).context("loading candidates.json")?;
    if candidates
        .candidates
        .is_empty()
    {
        return Ok(Outputs::default());
    }

    let parallel = inputs
        .parallel
        .unwrap_or(candidates.candidates.len())
        .max(1);
    let semaphore = Arc::new(Semaphore::new(parallel));

    let mut set: JoinSet<Result<()>> = JoinSet::new();
    for candidate in &candidates.candidates {
        let task_inputs = AnalyzerTaskInputs {
            family_id: candidate.id.clone(),
            family_json: serde_json::to_string(candidate)
                .context("serializing single-family slice for the analyzer prompt")?,
            baseline_run_id: candidates.baseline_run_id,
            framework: inputs.framework.clone(),
            settings: inputs.settings.clone(),
            session_results_dir: inputs
                .layout
                .results_dir
                .clone(),
            sem: semaphore.clone(),
            harness: inputs.harness.clone(),
        };
        set.spawn(run_one(task_inputs));
    }

    let mut total = 0usize;
    let mut errors: Vec<anyhow::Error> = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => total += 1,
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(anyhow::Error::new(e).context("analyzer task panicked")),
        }
    }
    if !errors.is_empty() {
        // Surface the first error; the rest are likely cascade failures from
        // the same root cause (e.g. shared codex CLI breakage).
        let mut iter = errors.into_iter();
        let first = iter.next().unwrap();
        for e in iter {
            tracing::error!(?e, "additional analyzer task error");
        }
        return Err(first);
    }

    // Tally accepted vs rejected from the on-disk analyses dir.
    let analyses = loader::read_all_analyses(&inputs.layout)?;
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    for a in analyses.values() {
        match a {
            Analysis::Accepted(_) => accepted += 1,
            Analysis::Rejected(_) => rejected += 1,
        }
    }

    Ok(Outputs { total, accepted, rejected })
}

/// Per-task owned state. Spawned futures need to be `'static`; cloning the
/// per-task data here is the cost of spawning vs. using `futures::stream`.
struct AnalyzerTaskInputs<H: AgentHarness + 'static> {
    family_id: String,
    family_json: String,
    baseline_run_id: i64,
    framework: Layout,
    settings: Settings,
    session_results_dir: PathBuf,
    sem: Arc<Semaphore>,
    harness: Arc<H>,
}

async fn run_one<H: AgentHarness + 'static>(state: AnalyzerTaskInputs<H>) -> Result<()> {
    let _permit = state
        .sem
        .acquire_owned()
        .await
        .context("acquiring analyzer semaphore permit")?;

    let out_dir = state
        .session_results_dir
        .join("analysis")
        .join(&state.family_id);
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let prompts_dir = state
        .settings
        .require_prompt_overrides_dir()?;
    let rendered = prompts::render(
        "analyzer",
        &prompts::AnalyzerPrompt {
            family_id: state.family_id.clone(),
            family_json: state.family_json.clone(),
            output_dir: out_dir
                .to_string_lossy()
                .into_owned(),
            base: state
                .framework
                .require_base()?
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
            triage_queries_dir: state
                .session_results_dir
                .join("triage")
                .join("queries")
                .to_string_lossy()
                .into_owned(),
            baseline_run_id: state
                .baseline_run_id
                .to_string(),
            non_targets_path: prompts_dir
                .join("non-targets.md")
                .to_string_lossy()
                .into_owned(),
            bucket_anchors_path: prompts_dir
                .join("bucket-anchors.md")
                .to_string_lossy()
                .into_owned(),
            analysis_schema_path: state
                .framework
                .schemas_dir
                .join("analysis.schema.json")
                .to_string_lossy()
                .into_owned(),
        },
        prompts_dir,
    )?;
    std::fs::write(out_dir.join("prompt.md"), &rendered)?;

    let timeout = state
        .settings
        .codex_exec_timeout_sec
        .filter(|n| *n > 0)
        .map(Duration::from_secs);
    let model = state
        .settings
        .codex_model
        .as_deref()
        .unwrap_or("gpt-5.5");
    let reasoning_effort = state
        .settings
        .codex_reasoning_effort
        .as_deref();
    let dangerous = state
        .settings
        .codex_dangerously_bypass_sandbox
        .unwrap_or(false);
    let add_dirs: Vec<PathBuf> = vec![
        state
            .framework
            .require_base()?
            .to_path_buf(),
        state
            .framework
            .stacks_bench_data_dir
            .clone(),
        // Operator-side bundles (rendered prompt references files
        // inside each by absolute path: bucket-anchors.md / non-targets.md
        // in prompts, *.sql in queries, analysis.schema.json in schemas,
        // plus the session's pre-rendered triage CSVs).
        state
            .framework
            .queries_dir
            .clone(),
        state
            .framework
            .schemas_dir
            .clone(),
        prompts_dir.to_path_buf(),
        state
            .session_results_dir
            .join("triage")
            .join("queries"),
    ];

    let invoke_outputs = state
        .harness
        .invoke(&InvokeInputs {
            rendered_prompt: &rendered,
            cwd: out_dir.as_ref(),
            add_dirs: &add_dirs,
            events_jsonl: &out_dir.join("events.jsonl"),
            stderr_log: &out_dir.join("stderr.log"),
            last_message: &out_dir.join("final-message.md"),
            timeout,
            model,
            reasoning_effort,
            skip_git_repo_check: true,
            dangerously_bypass_sandbox: dangerous,
            // Analyzers may need code lookups; keep search enabled (matches the bash flag set).
            enable_web_search: false,
            extra_env: &[],
        })
        .await
        .with_context(|| format!("invoking codex for analyzer {}", state.family_id))?;

    if let Some(id) = &invoke_outputs.conversation_id {
        std::fs::write(out_dir.join("conversation-id"), format!("{id}\n"))?;
    }

    // Verify the analyzer wrote a parseable analysis.json. Cross-field
    // validation runs on demand later (in merge / validate); we just check
    // the structural shape here.
    let analysis_path = out_dir.join("analysis.json");
    if !is_non_empty_file(&analysis_path) {
        bail!(
            "analyzer for {} did not emit analysis.json (see {})",
            state.family_id,
            out_dir
                .join("final-message.md")
                .display()
        );
    }
    let raw = std::fs::read_to_string(&analysis_path)?;
    let _: Analysis = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {}", analysis_path.display()))?;
    Ok(())
}

fn is_non_empty_file(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}
