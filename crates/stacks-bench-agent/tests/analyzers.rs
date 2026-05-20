//! Port verification for `session::analyzers::run`. Tests use a fake
//! harness that writes a canned `analysis.json` per family so the
//! post-invocation tally can roll up.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use stacks_bench_agent::harnesses::{AgentHarness, InvokeInputs, InvokeOutputs};
use stacks_bench_agent::layout::{FrameworkDir, Layout};
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::analyzers::{self, Inputs};
use stacks_bench_agent::settings::Settings;
use stacks_bench_agent::types::SessionId;

/// Fake harness: parses the rendered prompt to discover the family id (the
/// templates we stage embed it as `family=<id>`), then writes one
/// accepted-or-rejected analysis JSON per family using a recorder-supplied
/// status table.
struct FakeHarness {
    /// family_id → (status: "accepted" or "rejected"). Pre-populated by tests.
    statuses: Mutex<std::collections::BTreeMap<String, &'static str>>,
    /// Counter so we can assert how many invocations happened.
    invocations: Mutex<usize>,
}

impl FakeHarness {
    fn new(statuses: std::collections::BTreeMap<String, &'static str>) -> Self {
        Self {
            statuses: Mutex::new(statuses),
            invocations: Mutex::new(0),
        }
    }
    fn invocations(&self) -> usize {
        *self
            .invocations
            .lock()
            .unwrap()
    }
}

impl AgentHarness for FakeHarness {
    async fn check(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn invoke<'a>(&'a self, inputs: &'a InvokeInputs<'a>) -> anyhow::Result<InvokeOutputs> {
        // Recover family id from cwd — the analyzer fan-out sets cwd to
        // `analyses/<family-id>/`, so the last path segment is the id.
        let family = inputs
            .cwd
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("test cwd missing family-id segment"))?
            .to_owned();
        *self
            .invocations
            .lock()
            .unwrap() += 1;

        let status = self
            .statuses
            .lock()
            .unwrap()
            .get(&family)
            .copied()
            .unwrap_or("accepted");

        // Write events + last message + analyzer-conversation-id back-channel.
        std::fs::write(
            inputs.events_jsonl,
            serde_json::json!({"conversation_id": format!("conv-{family}")}).to_string() + "\n",
        )?;
        std::fs::write(inputs.stderr_log, b"")?;
        std::fs::write(inputs.last_message, b"# done\n")?;

        // Write a minimal analysis.json next to the cwd. The cwd is the
        // analyses/<family-id>/ dir.
        let analysis = if status == "accepted" {
            serde_json::json!({
                "schema_version": 2,
                "family_id": family,
                "status": "accepted",
                "selection_lens": "tx_latency",
                "lens_disposition": { "lens": "tx_latency", "status": "addressed" },
                "targets": [{
                    "target_span": "x::y",
                    "bucket": "block_processing",
                    "fix_signature": "x-fix",
                    "hotspot": {
                        "span": "x::y", "self_wall_us": 1, "total_wall_us": 1,
                        "calls": 1, "location": "x.rs:1"
                    },
                    "files": ["x.rs"],
                    "evidence": "e",
                    "proposed_change": "p",
                    "expected_improvement": {
                        "tx_latency": 1.0, "tenure_throughput": 0.0, "commit_time": 0.0
                    },
                    "risk": "low",
                    "verification_plan": "v",
                    "consensus_breaking": false
                }]
            })
        } else {
            serde_json::json!({
                "schema_version": 2,
                "family_id": family,
                "status": "rejected",
                "reason": "test reject"
            })
        };
        std::fs::write(
            inputs
                .cwd
                .join("analysis.json"),
            analysis.to_string(),
        )?;

        Ok(InvokeOutputs {
            conversation_id: Some(format!("conv-{family}")),
        })
    }
}

/// Stage a framework that has the analyzer template + a candidates.json
/// referencing the supplied family ids.
fn stage_framework_and_session(
    tmp: &tempfile::TempDir,
    families: &[(&str, &str)],
) -> (Layout, SessionLayout) {
    let framework = tmp.path().join("framework");
    std::fs::create_dir_all(framework.join("prompts")).unwrap();
    std::fs::create_dir_all(framework.join("schemas")).unwrap();
    std::fs::create_dir_all(framework.join("queries")).unwrap();
    // Seed the bundled context docs — the orchestrator's required-doc
    // startup check now fails when any of these are missing.
    stacks_bench_agent::context::seed_to(&framework.join("context")).unwrap();
    std::fs::create_dir_all(
        framework
            .join("repos")
            .join("stacks-core"),
    )
    .unwrap();
    let layout = Layout {
        framework: Some(FrameworkDir::new(framework.clone())),
        schemas_dir: framework
            .clone()
            .join("schemas"),
        queries_dir: framework
            .clone()
            .join("queries"),
        context_dir: framework
            .clone()
            .join("context"),
        memory_dir: framework
            .clone()
            .join("memory"),
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
        operator_repo_root: None,
    };

    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    let session = SessionLayout::from_layout(&layout, id);
    session
        .create_all_phase_dirs()
        .unwrap();

    // Build a candidates.json with the requested family ids.
    let candidates: Vec<serde_json::Value> = families
        .iter()
        .map(|(id, _)| {
            serde_json::json!({
                "id": id,
                "kind": "tx_family",
                "selection_lens": "tx_latency",
                "representative_ids": { "stacks_tx_hashes": ["0x0000000000000000000000000000000000000000000000000000000000000001"] },
                "rationale": "fake"
            })
        })
        .collect();
    let n_candidates = candidates.len() as u32;
    let cand_doc = serde_json::json!({
        "schema_version": 2,
        "session_id": "20260507-104400",
        "baseline_run_id": 100,
        "baseline_rerun_id": 101,
        "noise_floor_pct": 0.8,
        "candidates": candidates,
        "rejected_families": [],
        "lens_coverage": {
            // The fixture candidates above all use selection_lens = "tx_latency".
            "tx_latency": n_candidates,
            "tenure_throughput": 0,
            "commit_time": 0,
            "weights_applied": "0.4,0.4,0.2",
        },
    });
    std::fs::write(session.candidates_json(), cand_doc.to_string()).unwrap();

    (layout, session)
}

#[tokio::test]
async fn analyzers_fan_out_writes_analysis_per_family_and_tallies() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, session) = stage_framework_and_session(
        &tmp,
        &[("fam-a", "accepted"), ("fam-b", "rejected"), ("fam-c", "accepted")],
    );

    let mut statuses = std::collections::BTreeMap::new();
    statuses.insert("fam-a".to_owned(), "accepted");
    statuses.insert("fam-b".to_owned(), "rejected");
    statuses.insert("fam-c".to_owned(), "accepted");
    let harness = Arc::new(FakeHarness::new(statuses));

    let prompts_dir = tmp.path().join("prompts");
    stacks_bench_agent::prompts::seed_to(&prompts_dir).expect("seed prompts");
    let settings = Settings {
        prompt_overrides_dir: Some(prompts_dir),
        ..Settings::default()
    };
    let outputs = analyzers::run(Inputs {
        layout: session.clone(),
        framework: layout.clone(),
        settings,
        parallel: Some(2),
        harness: harness.clone(),
    })
    .await
    .expect("analyzers::run");

    assert_eq!(harness.invocations(), 3);
    assert_eq!(outputs.total, 3);
    assert_eq!(outputs.accepted, 2);
    assert_eq!(outputs.rejected, 1);

    // Per-family conversation id files written.
    for fam in ["fam-a", "fam-b", "fam-c"] {
        let conv_path: PathBuf = session
            .analysis_family_dir(fam)
            .join("conversation-id");
        assert_eq!(
            std::fs::read_to_string(&conv_path)
                .unwrap()
                .trim(),
            format!("conv-{fam}")
        );
    }
}

#[tokio::test]
async fn analyzers_no_op_when_no_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let (layout, session) = stage_framework_and_session(&tmp, &[]);
    let harness = Arc::new(FakeHarness::new(std::collections::BTreeMap::new()));

    let outputs = analyzers::run(Inputs {
        layout: session,
        framework: layout,
        settings: Settings::default(),
        parallel: None,
        harness: harness.clone(),
    })
    .await
    .expect("analyzers::run");

    assert_eq!(harness.invocations(), 0);
    assert_eq!(outputs.total, 0);
    assert_eq!(outputs.accepted, 0);
    assert_eq!(outputs.rejected, 0);
}
