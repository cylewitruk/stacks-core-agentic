//! Phase 1: triage agent.
//!
//! Reads the Phase 0 baseline artifacts, renders the triage prompt via
//! [`crate::prompts::TriagePrompt`] (Askama), invokes the agent harness,
//! captures stdout/stderr to disk, validates the produced
//! `candidates.json` against the typed model, and surfaces the count.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

use crate::harnesses::{AgentHarness, InvokeInputs};
use crate::layout::Layout;
use crate::models::ValidateModel;
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
    // Resolve per-phase context-doc paths from the bundle manifest.
    // Replaces the prior hard-coded `prompts_dir.join("<doc>.md")` —
    // which docs apply to which phase is now declared in the sidecars
    // under `<context_dir>/<id>.toml`, and the orchestrator just
    // surfaces the paths the prompt body references by name.
    // Hard-fail BEFORE rendering the prompt when any context doc
    // declared as required for this phase is missing or empty on
    // disk. The prompt would otherwise embed the missing absolute
    // path and the agent would discover the broken read inside its
    // own reasoning chain, surfacing as a confusing tool-call error.
    let missing = crate::context::required_missing_for_phase(
        &inputs.framework.context_dir,
        crate::context::Phase::Triage,
    )?;
    if !missing.is_empty() {
        let summary = missing
            .iter()
            .map(|(id, p)| format!("  - `{id}` → expected at {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "required context docs missing or empty for the triage phase:\n{summary}\n\nRun \
             `sbagent sync` to restore from the binary's bundled defaults, or `sbagent check` to \
             see the full drift report.",
        );
    }
    let ctx_paths = crate::context::paths_for_phase(
        &inputs.framework.context_dir,
        crate::context::Phase::Triage,
    )?;
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
            non_targets_path: crate::context::ctx_path(&ctx_paths, "non-targets")?,
            bucket_anchors_path: crate::context::ctx_path(&ctx_paths, "bucket-anchors")?,
            domain_context_path: crate::context::ctx_path(&ctx_paths, "stacks-domain-context")?,
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
            memory_dir: inputs
                .framework
                .memory_dir
                .to_string_lossy()
                .into_owned(),
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
        .codex
        .effective_exec_timeout();
    let model = inputs
        .settings
        .codex
        .effective_model();
    let reasoning_effort = inputs
        .settings
        .codex
        .reasoning_effort
        .as_deref();
    let dangerous = inputs
        .settings
        .codex
        .dangerously_bypass_sandbox
        .unwrap_or(false);

    // Ensure the operator memory dir exists BEFORE handing it to
    // codex via --add-dir. On a fresh operator the dir may not have
    // been created yet (the first `sbagent rejections probe` would
    // otherwise create it via the lockfile open). Codex / the macOS
    // sandbox treats a non-existent --add-dir target as "no write
    // root granted", which would silently break the probe.
    fs::create_dir_all(&inputs.framework.memory_dir).with_context(|| {
        format!(
            "creating operator memory dir at {}",
            inputs
                .framework
                .memory_dir
                .display(),
        )
    })?;
    let mut add_dirs: Vec<PathBuf> = vec![
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
        // inside each by absolute path: *.sql in queries,
        // candidates.schema.json in schemas, reference docs in context).
        inputs
            .framework
            .queries_dir
            .clone(),
        inputs
            .framework
            .schemas_dir
            .clone(),
        inputs
            .framework
            .context_dir
            .clone(),
        prompts_dir.to_path_buf(),
        // Operator memory dir. The agent's per-candidate
        // `sbagent rejections probe --memory-dir <...>` invocations
        // need this readable AND writable (probe's lock-acquire path
        // creates the `.locks/memory.lock` file on first use).
        inputs
            .framework
            .memory_dir
            .clone(),
    ];
    add_dirs.extend(
        inputs
            .settings
            .codex
            .extra_writable_roots
            .iter()
            .cloned(),
    );
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
        .validate_model()
        .context("candidates.json failed cross-field validation")?;

    // 6. Generate the human-readable `candidates.md` view from the validated JSON.
    //    The agent no longer writes this file; the JSON is the contract, markdown
    //    is orchestrator-derived sugar.
    let rendered_md = render_candidates_md(&candidates);
    fs::write(layout.candidates_md(), rendered_md).with_context(|| {
        format!(
            "writing derived {}",
            layout
                .candidates_md()
                .display()
        )
    })?;

    let candidate_count = candidates.candidates.len();
    // Soft cap warning. The triage-only-rejects-on-quality-grounds
    // architecture (no fixability gates) can produce 10-15+
    // candidates per session early on, before the ledger has
    // absorbed plausible-but-dead-end families. The soft cap exists
    // so a degenerate run that dumps every workload-entry pattern
    // surfaces an operator-visible warning rather than silently
    // spawning N analyzer subagents.
    let soft_cap = inputs
        .settings
        .triage
        .effective_candidate_soft_cap();
    if candidate_count > soft_cap {
        tracing::warn!(
            count = candidate_count,
            soft_cap,
            "triage emitted {candidate_count} candidates, exceeding soft cap of {soft_cap}; \
             analyzer phase may be slow. Tune `triage.candidate_soft_cap` to silence, or expect \
             the ledger to converge over the next few sessions.",
        );
    }
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

/// Render a human-readable `candidates.md` view from a validated
/// [`crate::models::candidates::Candidates`]. Includes the candidate
/// slate, the per-lens coverage tally, and the counter-search audit —
/// everything the prior "agent-written final-message.md" was supposed
/// to carry, now derived from typed fields so the audit content
/// can't drift from the JSON.
fn render_candidates_md(c: &crate::models::candidates::Candidates) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(2 * 1024);
    let _ = writeln!(s, "# Triage candidates — session {}", c.session_id);
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "- baseline run id: `{}`  ·  rerun id: `{}`  ·  noise floor: `{:.4}%`",
        c.baseline_run_id, c.baseline_rerun_id, c.noise_floor_pct,
    );
    let _ = writeln!(
        s,
        "- lens coverage: `tx_latency`={}, `tenure_throughput`={}, `commit_time`={}  ·  weights \
         applied: `{}`",
        c.lens_coverage.tx_latency,
        c.lens_coverage
            .tenure_throughput,
        c.lens_coverage.commit_time,
        c.lens_coverage
            .weights_applied,
    );
    if let Some(notes) = &c
        .lens_coverage
        .redistribution_notes
    {
        let _ = writeln!(s, "- redistribution notes: {notes}");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "## Promoted candidates ({})", c.candidates.len());
    let _ = writeln!(s);
    if c.candidates.is_empty() {
        let _ = writeln!(
            s,
            "_None._ See **Rejected alternative families** below for what was investigated."
        );
    } else {
        for cand in &c.candidates {
            let _ = writeln!(s, "### `{}`", cand.id);
            let _ = writeln!(s);
            let _ = writeln!(s, "- **kind**: `{:?}`", cand.kind);
            let _ = writeln!(s, "- **selection lens**: `{:?}`", cand.selection_lens);
            if let Some(b) = &cand.bucket {
                let _ = writeln!(s, "- **bucket**: `{b:?}`");
            }
            let _ = writeln!(s, "- **rationale**: {}", cand.rationale);
            if let Some(spans) = &cand.suspected_spans {
                let _ = writeln!(s, "- **suspected spans**: `{}`", spans.join("`, `"));
            }
            if let Some(gm) = &cand.global_materiality {
                let _ = writeln!(
                    s,
                    "- **global materiality**: pct_blocks={:?}, self_wall_ms={:?}",
                    gm.pct_blocks, gm.self_wall_ms,
                );
            }
            let _ = writeln!(s);
        }
    }
    let _ = writeln!(s, "## Rejected alternative families ({})", c.rejected_families.len());
    let _ = writeln!(s);
    if c.rejected_families.is_empty() {
        let _ = writeln!(
            s,
            "_None._ (Agent reported the slate was dominated by clear winners; no counter-search \
             alternatives needed rejecting.)",
        );
    } else {
        for r in &c.rejected_families {
            let _ = writeln!(s, "- `{}` (lens `{:?}`): {}", r.family_id, r.lens, r.reason,);
        }
    }
    s
}
