//! Phase 1: triage agent.
//!
//! Reads the Phase 0 baseline artifacts, renders the triage prompt via
//! [`crate::prompts::TriagePrompt`] (Askama), invokes the agent harness,
//! captures stdout/stderr to disk, validates the produced
//! `candidates.json` against the typed model, and surfaces the count.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use crate::harnesses::{AgentHarness, InvokeInputs};
use crate::layout::Layout;
use crate::prompts;
use crate::session::{SessionLayout, loader};
use crate::settings::Settings;

/// Inputs to a triage run.
pub struct Inputs<'a, H: AgentHarness> {
    /// Resolved per-session layout.
    pub layout: &'a SessionLayout,
    /// Resolved framework layout (used for prompt + queries paths).
    pub framework: &'a Layout,
    /// Settings (provides codex model + reasoning effort + timeout).
    pub settings: &'a Settings,
    /// Operator weights for the three triage selection lenses
    /// (comma-separated).
    pub axis_weights: &'a str,
    /// Agent harness (typically [`crate::harnesses::codex::CodexHarness`]).
    pub harness: &'a H,
}

/// Outputs of a triage run.
#[derive(Debug)]
pub struct Outputs {
    /// Number of candidate families emitted.
    pub candidate_count: usize,
    /// Conversation id captured from the agent's JSONL events stream.
    pub conversation_id: Option<String>,
}

/// Run triage end-to-end. Mirrors `scripts/run-triage.sh`.
pub async fn run<H: AgentHarness>(inputs: &Inputs<'_, H>) -> Result<Outputs> {
    let layout = inputs.layout;
    fs::create_dir_all(&layout.results_dir)
        .with_context(|| format!("creating {}", layout.results_dir.display()))?;

    // 1. Load baseline ids.
    let baseline_run_id = loader::read_run_id_file(&layout.baseline_run_id_path())
        .context("reading baseline-run-id")?;
    let baseline_rerun_id = loader::read_run_id_file(&layout.baseline_rerun_id_path())
        .context("reading baseline-rerun-id")?;

    // 2. Optional precomputed noise floor (for single-run imports).
    let noise_floor_path = layout.baseline_noise_floor_path();
    let precomputed_noise_floor_pct = if noise_floor_path.is_file() {
        fs::read_to_string(&noise_floor_path)
            .with_context(|| format!("reading {}", noise_floor_path.display()))?
            .trim()
            .to_owned()
    } else {
        String::new()
    };

    // 3. Pre-render the run-id-scoped triage queries so the agent
    // doesn't have to spawn sqlite3 for the standard orientation +
    // candidate-ranking set. Per-query failures are warned but don't
    // abort — a missing CSV is still useful signal to the agent.
    let triage_queries_dir = layout.triage_queries_dir();
    crate::session::triage_queries::prerender(
        &inputs.framework.queries_dir,
        &triage_queries_dir,
        &inputs
            .framework
            .stacks_bench_db_path(),
        baseline_run_id,
    )
    .context("pre-rendering triage queries")?;

    // 4. Render the prompt.
    let prompts_dir = inputs
        .settings
        .require_prompt_overrides_dir()?;
    let rendered = prompts::render(
        "triage",
        &prompts::TriagePrompt {
            opt_session_id: layout.id.as_str().to_owned(),
            opt_session_dir: layout
                .results_dir
                .to_string_lossy()
                .into_owned(),
            stacks_bench_data_dir: inputs
                .framework
                .stacks_bench_data_dir
                .to_string_lossy()
                .into_owned(),
            base: inputs
                .framework
                .require_base()?
                .to_string_lossy()
                .into_owned(),
            baseline_run_id: baseline_run_id.to_string(),
            baseline_rerun_id: baseline_rerun_id.to_string(),
            precomputed_noise_floor_pct,
            non_targets_path: prompts_dir
                .join("non-targets.md")
                .to_string_lossy()
                .into_owned(),
            bucket_anchors_path: prompts_dir
                .join("bucket-anchors.md")
                .to_string_lossy()
                .into_owned(),
            candidates_schema_path: inputs
                .framework
                .schemas_dir
                .join("candidates.schema.json")
                .to_string_lossy()
                .into_owned(),
            queries_dir: inputs
                .framework
                .queries_dir
                .to_string_lossy()
                .into_owned(),
            triage_queries_dir: triage_queries_dir
                .to_string_lossy()
                .into_owned(),
            stacks_bench_axis_weights: inputs.axis_weights.to_owned(),
        },
        prompts_dir,
    )?;
    let prompt_path = layout.triage_prompt();
    fs::create_dir_all(layout.triage_dir())
        .with_context(|| format!("creating {}", layout.triage_dir().display()))?;
    fs::write(&prompt_path, &rendered)
        .with_context(|| format!("writing {}", prompt_path.display()))?;

    // 4. Invoke harness.
    let timeout = inputs
        .settings
        .codex_exec_timeout_sec
        .filter(|n| *n > 0)
        .map(Duration::from_secs);
    let model = inputs
        .settings
        .codex_model
        .as_deref()
        .unwrap_or("gpt-5.5");
    let reasoning_effort = inputs
        .settings
        .codex_reasoning_effort
        .as_deref();
    let dangerous = inputs
        .settings
        .codex_dangerously_bypass_sandbox
        .unwrap_or(false);

    let add_dirs: Vec<PathBuf> = vec![
        // Persistent stacks-bench db + stacks-core checkout (agent reads
        // these directly).
        inputs
            .framework
            .stacks_bench_data_dir
            .clone(),
        inputs
            .framework
            .require_base()?
            .to_path_buf(),
        // Operator-side bundles (rendered prompt references files
        // inside each by absolute path: bucket-anchors.md / non-targets.md
        // in prompts, *.sql in queries, candidates.schema.json in schemas).
        inputs
            .framework
            .queries_dir
            .clone(),
        inputs
            .framework
            .schemas_dir
            .clone(),
        prompts_dir.to_path_buf(),
    ];
    let triage_dir = layout.triage_dir();
    let invoke_outputs = inputs
        .harness
        .invoke(&InvokeInputs {
            rendered_prompt: &rendered,
            cwd: triage_dir.as_ref(),
            add_dirs: &add_dirs,
            events_jsonl: &layout.triage_events(),
            stderr_log: &layout.triage_stderr(),
            last_message: &layout.triage_final_message(),
            timeout,
            model,
            reasoning_effort,
            skip_git_repo_check: true,
            dangerously_bypass_sandbox: dangerous,
            // Triage doesn't need web search (matches the bash --search drop).
            enable_web_search: false,
            extra_env: &[],
        })
        .await
        .context("invoking codex for triage")?;

    if let Some(id) = &invoke_outputs.conversation_id {
        fs::write(layout.triage_conversation_id(), format!("{id}\n"))
            .context("writing triage-conversation-id")?;
    }

    // 5. Verify + structurally validate candidates.json.
    if !is_non_empty_file(&layout.candidates_json()) {
        bail!(
            "Triage did not emit candidates.json. See {}.",
            layout
                .triage_final_message()
                .display()
        );
    }
    let candidates = loader::read_candidates(layout)
        .context("parsing candidates.json (does it match the v2 schema?)")?;
    candidates
        .validate()
        .map_err(|e| anyhow::anyhow!("candidates.json failed cross-field validation: {e}"))?;

    let candidate_count = candidates.candidates.len();
    Ok(Outputs {
        candidate_count,
        conversation_id: invoke_outputs.conversation_id,
    })
}

fn is_non_empty_file(path: &std::path::Path) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}
