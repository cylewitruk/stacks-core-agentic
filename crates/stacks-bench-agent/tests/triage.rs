//! Port verification for `session::triage::run`.
//!
//! Uses a recording fake `AgentHarness` that captures the rendered prompt
//! and writes a canned `candidates.json` to the session results dir
//! (mirroring what Codex would do). Asserts (1) the prompt was rendered
//! with the expected `${VAR}` substitutions, (2) Codex would have been
//! invoked with the right `cwd` + `add_dirs`, and (3) the candidates count
//! reflects what the harness wrote.

use std::path::PathBuf;
use std::sync::Mutex;

use stacks_bench_agent::harnesses::{AgentHarness, InvokeInputs, InvokeOutputs};
use stacks_bench_agent::layout::{FrameworkDir, Layout};
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::triage::{self, Inputs};
use stacks_bench_agent::settings::Settings;
use stacks_bench_agent::types::SessionId;

/// Recording fake. Captures the rendered prompt + cwd + add_dirs the
/// session would have shipped to Codex, and writes a canned candidates.json
/// to the session results dir so the post-invocation validation passes.
struct RecordingHarness {
    state: Mutex<RecordingState>,
}

#[derive(Default)]
struct RecordingState {
    prompts: Vec<String>,
    cwds: Vec<PathBuf>,
    add_dirs: Vec<Vec<PathBuf>>,
}

impl RecordingHarness {
    fn new() -> Self {
        Self {
            state: Mutex::new(RecordingState::default()),
        }
    }
    fn last_prompt(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .prompts
            .last()
            .cloned()
    }
    fn last_cwd(&self) -> Option<PathBuf> {
        self.state
            .lock()
            .unwrap()
            .cwds
            .last()
            .cloned()
    }
    fn last_add_dirs(&self) -> Option<Vec<PathBuf>> {
        self.state
            .lock()
            .unwrap()
            .add_dirs
            .last()
            .cloned()
    }
}

impl AgentHarness for RecordingHarness {
    async fn check(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn invoke<'a>(&'a self, inputs: &'a InvokeInputs<'a>) -> anyhow::Result<InvokeOutputs> {
        // Record the call.
        {
            let mut state = self.state.lock().unwrap();
            state.prompts.push(
                inputs
                    .rendered_prompt
                    .to_owned(),
            );
            state
                .cwds
                .push(inputs.cwd.to_path_buf());
            state
                .add_dirs
                .push(inputs.add_dirs.to_vec());
        }

        // Write canned outputs.
        if let Some(parent) = inputs.events_jsonl.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            inputs.events_jsonl,
            serde_json::json!({"conversation_id": "conv-fake-triage"}).to_string() + "\n",
        )?;
        std::fs::write(inputs.stderr_log, b"")?;
        std::fs::write(inputs.last_message, b"# Triage final\n")?;

        // Drop a candidates.json next to where the bash version would land it.
        let cwd = inputs.cwd;
        let candidates = serde_json::json!({
            "schema_version": 2,
            "session_id": "20260507-104400",
            "baseline_run_id": 100,
            "baseline_rerun_id": 101,
            "noise_floor_pct": 0.8,
            "candidates": [
                {
                    "id": "fake-fam",
                    "kind": "tx_family",
                    "selection_lens": "tx_latency",
                    "representative_ids": { "stacks_tx_hashes": [
                        "0x0000000000000000000000000000000000000000000000000000000000000001",
                        "0x0000000000000000000000000000000000000000000000000000000000000002",
                        "0x0000000000000000000000000000000000000000000000000000000000000003"
                    ] },
                    "rationale": "fake"
                }
            ]
        });
        std::fs::write(cwd.join("candidates.json"), candidates.to_string())?;

        Ok(InvokeOutputs {
            conversation_id: Some("conv-fake-triage".to_owned()),
        })
    }
}

/// Build a fixture layout with the prompts/queries/schemas dirs populated
/// just enough for triage to render its template.
fn stage_framework(tmp: &tempfile::TempDir) -> Layout {
    let framework = tmp.path().join("framework");
    std::fs::create_dir_all(framework.join("prompts")).unwrap();
    std::fs::create_dir_all(framework.join("queries")).unwrap();
    std::fs::create_dir_all(framework.join("schemas")).unwrap();
    std::fs::create_dir_all(
        framework
            .join("repos")
            .join("stacks-core"),
    )
    .unwrap();
    Layout {
        framework: Some(FrameworkDir::new(framework.clone())),
        schemas_dir: framework
            .clone()
            .join("schemas"),
        queries_dir: framework
            .clone()
            .join("queries"),
        sessions_root: tmp.path().join("sessions"),
        stacks_bench_data_dir: tmp
            .path()
            .join("data")
            .join("stacks-bench"),
        bench_lock: tmp
            .path()
            .join("benchmark.lock"),
        test_lock: tmp.path().join("test.lock"),
        base: Some(
            framework
                .join("repos")
                .join("stacks-core"),
        ),
        stacks_bench_shadow_dir: None,
        agent_workspace_root: None,
    }
}

fn stage_session(layout: &Layout, id: &SessionId) -> SessionLayout {
    let session = SessionLayout::from_layout(layout, id.clone());
    session
        .create_all_phase_dirs()
        .unwrap();
    std::fs::write(session.baseline_run_id_path(), b"100\n").unwrap();
    std::fs::write(session.baseline_rerun_id_path(), b"101\n").unwrap();
    session
}

#[tokio::test]
async fn triage_renders_prompt_and_records_candidates_count() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = stage_framework(&tmp);
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let session = stage_session(&layout, &id);

    let harness = RecordingHarness::new();
    let prompts_dir = tmp.path().join("prompts");
    stacks_bench_agent::prompts::seed_to(&prompts_dir).expect("seed prompts");
    let settings = Settings {
        prompt_overrides_dir: Some(prompts_dir),
        ..Settings::default()
    };
    let outputs = triage::run(&Inputs {
        layout: &session,
        framework: &layout,
        settings: &settings,
        axis_weights: "0.4,0.4,0.2",
        harness: &harness,
    })
    .await
    .expect("triage::run");

    // Prompt was rendered with the expected substitutions. Spot-check
    // a few of the values that should appear verbatim in the rendered
    // text (the baseline run id, the session id, and the operator
    // axis weights). Asserts on the actual triage.md that ships in
    // the binary; brittle to template content but cheap to keep
    // accurate as the contract is the typed `TriagePrompt` struct.
    let prompt = harness.last_prompt().unwrap();
    assert!(prompt.contains("100"), "prompt missing baseline_run_id 100");
    assert!(prompt.contains("20260507-104400"), "prompt missing session id");
    assert!(prompt.contains("0.4,0.4,0.2"), "prompt missing axis weights");
    // cwd was the triage dir so the agent's relative writes
    // (candidates.json, candidates.md, drilldown CSVs) land under
    // results/triage/ automatically.
    assert_eq!(harness.last_cwd().unwrap(), session.triage_dir());
    // add_dirs MUST cover: stacks-bench data dir + stacks-core base
    // checkout (agent reads both directly), plus the operator-side
    // bundles (queries_dir for *.sql, schemas_dir for the candidates
    // schema, prompts_dir for non-targets/bucket-anchors). The
    // framework dir itself is NOT in add_dirs anymore — every artifact
    // the agent needs lives under one of the operator-side bundle dirs.
    let add_dirs = harness
        .last_add_dirs()
        .unwrap();
    assert!(
        add_dirs
            .iter()
            .any(|p| p == &layout.stacks_bench_data_dir),
        "stacks_bench_data_dir must be in add_dirs; got: {add_dirs:?}",
    );
    let layout_base = layout
        .base
        .as_ref()
        .expect("test layout has base set");
    assert!(
        add_dirs
            .iter()
            .any(|p| p == layout_base),
        "base must be in add_dirs; got: {add_dirs:?}",
    );
    assert!(
        add_dirs
            .iter()
            .any(|p| p == &layout.queries_dir),
        "queries_dir must be in add_dirs; got: {add_dirs:?}",
    );
    assert!(
        add_dirs
            .iter()
            .any(|p| p == &layout.schemas_dir),
        "schemas_dir must be in add_dirs; got: {add_dirs:?}",
    );

    assert_eq!(outputs.candidate_count, 1);
    assert_eq!(
        outputs
            .conversation_id
            .as_deref(),
        Some("conv-fake-triage")
    );

    // The renderer wrote the prompt + the harness wrote the events stream + msg.
    assert!(
        session
            .triage_prompt()
            .is_file()
    );
    assert!(
        session
            .triage_events()
            .is_file()
    );
    assert!(
        session
            .triage_stderr()
            .is_file()
    );
    assert!(
        session
            .triage_final_message()
            .is_file()
    );
    assert_eq!(
        std::fs::read_to_string(session.triage_conversation_id())
            .unwrap()
            .trim(),
        "conv-fake-triage"
    );
}

#[tokio::test]
async fn triage_fails_when_candidates_missing() {
    /// Fake harness that doesn't produce candidates.json.
    struct EmptyHarness;
    impl AgentHarness for EmptyHarness {
        async fn check(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn invoke<'a>(
            &'a self,
            inputs: &'a InvokeInputs<'a>,
        ) -> anyhow::Result<InvokeOutputs> {
            std::fs::write(inputs.events_jsonl, b"")?;
            std::fs::write(inputs.stderr_log, b"")?;
            std::fs::write(inputs.last_message, b"# nothing produced\n")?;
            Ok(InvokeOutputs { conversation_id: None })
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let layout = stage_framework(&tmp);
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let session = stage_session(&layout, &id);

    let prompts_dir = tmp.path().join("prompts");
    stacks_bench_agent::prompts::seed_to(&prompts_dir).expect("seed prompts");
    let settings = Settings {
        prompt_overrides_dir: Some(prompts_dir),
        ..Settings::default()
    };
    let err = triage::run(&Inputs {
        layout: &session,
        framework: &layout,
        settings: &settings,
        axis_weights: "0.4,0.4,0.2",
        harness: &EmptyHarness,
    })
    .await
    .expect_err("expected missing-candidates error");

    assert!(
        err.to_string()
            .contains("Triage did not emit candidates.json"),
        "unexpected error: {err}"
    );
}
