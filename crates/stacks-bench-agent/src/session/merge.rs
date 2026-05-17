//! Phase 1.7: LLM merge.
//!
//! Reads accepted family analyses, invokes one Codex consolidation pass,
//! and validates the merge output against:
//!
//! - per-target schema invariants (typed model `validate()`);
//! - **target coverage**: every (family_id, target_index) from accepted
//!   analyses appears in EXACTLY ONE of (merged_from, rejected_by_merge);
//! - **lens-disposition coverage**: every accepted family_id appears exactly
//!   once in `lens_dispositions[]`;
//! - **no cross-bucket merges**: every merged_from reference shares the merged
//!   target's bucket;
//! - **no intra-analysis merges**: every merged_from has unique family_ids (two
//!   targets from one analyzer cannot collapse);
//! - **no cross-consensus merges**: every merged_from reference shares
//!   `consensus_breaking` (and, when true, `breakage_class`).
//!
//! Empty-input shortcut: zero accepted analyses → emit valid empty
//! `optimization-targets.json` + skip the LLM call.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use crate::harnesses::{AgentHarness, InvokeInputs};
use crate::layout::Layout;
use crate::models::analyze::{AcceptedAnalysis, Analysis};
use crate::models::common::{BreakageClass, Bucket};
use crate::models::targets::OptimizationTargets;
use crate::prompts;
use crate::session::{SessionLayout, loader};
use crate::settings::Settings;

/// Inputs to a merge run.
pub struct Inputs<'a, H: AgentHarness> {
    /// Resolved per-session layout.
    pub layout: &'a SessionLayout,
    /// Resolved framework + data layout.
    pub framework: &'a Layout,
    /// Settings (codex merge model + timeout, etc.).
    pub settings: &'a Settings,
    /// Agent harness for the LLM consolidation pass.
    pub harness: &'a H,
}

/// Outputs of a merge run.
#[derive(Debug)]
pub struct Outputs {
    /// Number of accepted analyses fed to the merge.
    pub accepted_input_count: usize,
    /// Number of merged targets emitted.
    pub merged_target_count: usize,
    /// True iff the empty-input shortcut fired (no LLM call was made).
    pub empty_input_shortcut: bool,
    /// Conversation id captured from the LLM events stream (None when the
    /// shortcut fires).
    pub conversation_id: Option<String>,
}

/// Run the merge phase end-to-end.
pub async fn run<H: AgentHarness>(inputs: &Inputs<'_, H>) -> Result<Outputs> {
    let layout = inputs.layout;
    let candidates = loader::read_candidates(layout)
        .context("loading candidates.json (required for session-level fields)")?;

    // Load every analysis and pre-validate.
    let analyses_map =
        loader::read_all_analyses(layout).context("loading analysis/*/analysis.json")?;
    for (fid, a) in &analyses_map {
        a.validate()
            .map_err(|e| anyhow::anyhow!("analysis/{fid}/analysis.json failed validation: {e}"))?;
    }

    // Filter to accepted, preserving deterministic order (BTreeMap iteration).
    let accepted: Vec<&AcceptedAnalysis> = analyses_map
        .values()
        .filter_map(|a| match a {
            Analysis::Accepted(acc) => Some(acc),
            Analysis::Rejected(_) => None,
        })
        .collect();
    let accepted_count = accepted.len();

    // Empty-input shortcut: emit valid empty targets, skip LLM, validate.
    if accepted_count == 0 {
        let empty = OptimizationTargets {
            schema_version: crate::models::common::SchemaVersionV2,
            session_id: candidates.session_id.clone(),
            baseline_run_id: candidates.baseline_run_id,
            baseline_rerun_id: candidates.baseline_rerun_id,
            noise_floor_pct: candidates.noise_floor_pct,
            merge_method: crate::models::targets::MergeMethod::Llm,
            merge_model: String::new(),
            targets: vec![],
            rejected_by_merge: vec![],
            lens_dispositions: vec![],
        };
        fs::create_dir_all(layout.merge_dir())
            .with_context(|| format!("creating {}", layout.merge_dir().display()))?;
        fs::write(
            layout.optimization_targets_json(),
            serde_json::to_string_pretty(&empty)? + "\n",
        )?;
        fs::write(
            layout.merge_final_message(),
            "# Merge phase: no-op\n\nNo accepted analyses; emitted empty targets list and empty \
             lens_dispositions.\nCoverage check trivially satisfied (0 inputs, 0 outputs, 0 \
             rejected).\n",
        )?;
        validate_merge_output(&empty, &accepted)?;
        return Ok(Outputs {
            accepted_input_count: 0,
            merged_target_count: 0,
            empty_input_shortcut: true,
            conversation_id: None,
        });
    }

    // Render the merge prompt with the accepted-analyses array inlined.
    let accepted_json = serde_json::to_string_pretty(&accepted)
        .context("serializing accepted analyses for merge prompt")?;
    let merge_model = inputs
        .settings
        .codex_merge_reasoning_effort
        .as_deref()
        .or(inputs
            .settings
            .codex_reasoning_effort
            .as_deref())
        .unwrap_or("");
    let merge_model_id = inputs
        .settings
        .codex_model
        .as_deref()
        .unwrap_or("gpt-5.3-codex-spark");

    let prompts_dir = inputs
        .settings
        .require_prompt_overrides_dir()?;
    let missing = crate::context::required_missing_for_phase(
        &inputs.framework.context_dir,
        crate::context::Phase::Merge,
    )?;
    if !missing.is_empty() {
        let summary = missing
            .iter()
            .map(|(id, p)| format!("  - `{id}` → expected at {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "required context docs missing or empty for the merge phase:\n{summary}\n\nRun \
             `sbagent sync` to restore from the binary's bundled defaults.",
        );
    }
    let ctx_paths = crate::context::paths_for_phase(
        &inputs.framework.context_dir,
        crate::context::Phase::Merge,
    )?;
    let rendered = prompts::render(
        "merge-analyses",
        &prompts::MergePrompt {
            opt_session_id: layout.id.as_str().to_owned(),
            opt_session_dir: layout
                .results_dir
                .to_string_lossy()
                .into_owned(),
            baseline_run_id: candidates
                .baseline_run_id
                .to_string(),
            baseline_rerun_id: candidates
                .baseline_rerun_id
                .to_string(),
            noise_floor_pct: candidates
                .noise_floor_pct
                .to_string(),
            optimization_targets_schema_path: inputs
                .framework
                .schemas_dir
                .join("optimization-targets.schema.json")
                .to_string_lossy()
                .into_owned(),
            bucket_anchors_path: crate::context::ctx_path(&ctx_paths, "bucket-anchors")?,
            codex_merge_model: merge_model_id.to_owned(),
            accepted_analyses_json: accepted_json,
        },
        prompts_dir,
    )?;
    fs::create_dir_all(layout.merge_dir())
        .with_context(|| format!("creating {}", layout.merge_dir().display()))?;
    fs::write(layout.merge_prompt(), &rendered)?;

    // Clear stale events / stderr / conv-id so a re-invocation can't surface
    // stale data via lingering files.
    let _ = fs::remove_file(layout.merge_events());
    let _ = fs::remove_file(layout.merge_stderr());
    let _ = fs::remove_file(layout.merge_conversation_id());

    let timeout = inputs
        .settings
        .codex_exec_timeout_sec
        .filter(|n| *n > 0)
        .map(Duration::from_secs);
    let dangerous = inputs
        .settings
        .codex_dangerously_bypass_sandbox
        .unwrap_or(false);
    // Agent reads the schema file + bucket-anchors.md (both rendered
    // into the prompt as absolute paths). The schema file is under
    // `<operator>/.sbagent/schemas/`; bucket-anchors.md is under
    // `<operator>/.sbagent/context/`.
    let add_dirs: Vec<PathBuf> = vec![
        inputs
            .framework
            .schemas_dir
            .clone(),
        inputs
            .framework
            .context_dir
            .clone(),
        prompts_dir.to_path_buf(),
    ];

    // Stamp pre-invocation modification times so a stale targets file can't
    // pass freshness checks (mirrors the bash `mktemp` marker).
    let pre_targets_mtime = mtime(&layout.optimization_targets_json()).ok();
    let pre_msg_mtime = mtime(&layout.merge_final_message()).ok();

    let merge_dir = layout.merge_dir();
    let invoke_outputs = inputs
        .harness
        .invoke(&InvokeInputs {
            rendered_prompt: &rendered,
            cwd: merge_dir.as_ref(),
            add_dirs: &add_dirs,
            events_jsonl: &layout.merge_events(),
            stderr_log: &layout.merge_stderr(),
            last_message: &layout.merge_final_message(),
            timeout,
            model: merge_model_id,
            reasoning_effort: if merge_model.is_empty() { None } else { Some(merge_model) },
            skip_git_repo_check: true,
            dangerously_bypass_sandbox: dangerous,
            enable_web_search: false,
            extra_env: &[],
        })
        .await
        .context("invoking codex for merge")?;

    if let Some(id) = &invoke_outputs.conversation_id {
        fs::write(layout.merge_conversation_id(), format!("{id}\n"))?;
    }

    // Freshness checks.
    let targets_path = layout.optimization_targets_json();
    let msg_path = layout.merge_final_message();
    if !is_non_empty_file(&targets_path) {
        bail!("merge: optimization-targets.json missing or empty after invocation");
    }
    if !is_non_empty_file(&msg_path) {
        bail!("merge: merge-final-message.md missing or empty after invocation");
    }
    if let (Some(pre), Ok(post)) = (pre_targets_mtime, mtime(&targets_path))
        && post == pre
    {
        bail!(
            "merge: codex exited 0 but did not refresh optimization-targets.json (stale prior \
             file detected)"
        );
    }
    if let (Some(pre), Ok(post)) = (pre_msg_mtime, mtime(&msg_path))
        && post == pre
    {
        bail!(
            "merge: codex exited 0 but did not refresh merge-final-message.md (stale prior file \
             detected)"
        );
    }

    // Parse + validate the LLM output.
    let targets =
        loader::read_optimization_targets(layout).context("parsing optimization-targets.json")?;
    validate_merge_output(&targets, &accepted)?;

    Ok(Outputs {
        accepted_input_count: accepted_count,
        merged_target_count: targets.targets.len(),
        empty_input_shortcut: false,
        conversation_id: invoke_outputs.conversation_id,
    })
}

/// Validate the merged output against (1) the typed model's per-record
/// invariants and (2) the merge-phase-specific cross-record invariants
/// (coverage, no cross-bucket / intra-analysis / cross-consensus merges).
pub fn validate_merge_output(
    targets: &OptimizationTargets,
    accepted: &[&AcceptedAnalysis],
) -> Result<()> {
    targets
        .validate()
        .map_err(|e| anyhow::anyhow!("merge output validation: {e}"))?;

    // Reference set of every analyzer-emitted target by (family_id, target_index).
    let mut expected_pairs: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut accepted_family_ids: BTreeSet<String> = BTreeSet::new();
    let mut bucket_by_pair: BTreeMap<(String, usize), Bucket> = BTreeMap::new();
    let mut consensus_by_pair: BTreeMap<(String, usize), (bool, Option<BreakageClass>)> =
        BTreeMap::new();
    for a in accepted {
        accepted_family_ids.insert(a.family_id.clone());
        for (i, t) in a.targets.iter().enumerate() {
            let key = (a.family_id.clone(), i);
            expected_pairs.insert(key.clone());
            bucket_by_pair.insert(key.clone(), t.bucket);
            consensus_by_pair.insert(key.clone(), (t.consensus_breaking, t.breakage_class));
        }
    }

    // Coverage: every (family_id, target_index) appears once across merged_from +
    // rejected_by_merge.
    let mut accounted: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut duplicate = false;
    for t in &targets.targets {
        for mf in &t.merged_from {
            if !accounted.insert((mf.family_id.clone(), mf.target_index)) {
                duplicate = true;
            }
        }
    }
    for r in &targets.rejected_by_merge {
        if !accounted.insert((r.family_id.clone(), r.target_index)) {
            duplicate = true;
        }
    }
    if duplicate || accounted != expected_pairs {
        bail!(
            "merge: target coverage invariant failed (every (family_id, target_index) from \
             accepted analyses must appear in EXACTLY ONE of merged_from / rejected_by_merge). \
             expected={:?} accounted={:?}",
            expected_pairs.len(),
            accounted.len()
        );
    }

    // Lens-disposition coverage: every accepted family_id appears once.
    let mut present_lens: BTreeSet<String> = BTreeSet::new();
    let mut lens_dup = false;
    for d in &targets.lens_dispositions {
        if !present_lens.insert(d.family_id.clone()) {
            lens_dup = true;
        }
    }
    if lens_dup || present_lens != accepted_family_ids {
        bail!(
            "merge: lens_dispositions coverage failed (every accepted family_id must appear \
             exactly once in lens_dispositions[])"
        );
    }

    // No cross-bucket merges.
    for t in &targets.targets {
        let buckets: BTreeSet<Bucket> = t
            .merged_from
            .iter()
            .filter_map(|mf| bucket_by_pair.get(&(mf.family_id.clone(), mf.target_index)))
            .copied()
            .collect();
        if buckets.len() > 1 || (buckets.len() == 1 && !buckets.contains(&t.bucket)) {
            bail!(
                "merge: cross-bucket merge detected on target `{}` (merged_from references {} \
                 distinct buckets; merged target's bucket={:?})",
                t.id,
                buckets.len(),
                t.bucket
            );
        }
    }

    // No intra-analysis merges.
    for t in &targets.targets {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for mf in &t.merged_from {
            if !seen.insert(mf.family_id.as_str()) {
                bail!(
                    "merge: intra-analysis merge detected on target `{}` (two merged_from entries \
                     reference family_id={:?}; the analyzer's distinct findings must remain \
                     separate)",
                    t.id,
                    mf.family_id
                );
            }
        }
    }

    // No cross-consensus merges.
    for t in &targets.targets {
        let signatures: BTreeSet<(bool, Option<BreakageClass>)> = t
            .merged_from
            .iter()
            .filter_map(|mf| consensus_by_pair.get(&(mf.family_id.clone(), mf.target_index)))
            .copied()
            .collect();
        let merged_sig = (t.consensus_breaking, t.breakage_class);
        if signatures.len() > 1 || (signatures.len() == 1 && !signatures.contains(&merged_sig)) {
            bail!(
                "merge: cross-consensus merge detected on target `{}` (merged_from references {} \
                 distinct (consensus_breaking, breakage_class) signatures; merged target's \
                 signature={:?})",
                t.id,
                signatures.len(),
                merged_sig
            );
        }
    }

    Ok(())
}

fn is_non_empty_file(path: &std::path::Path) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

fn mtime(path: &std::path::Path) -> std::io::Result<std::time::SystemTime> {
    fs::metadata(path)?.modified()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::analyze::AnalyzerTarget;

    /// Catch the case the hand-written merge-analyses.sh's
    /// `no_cross_bucket_merges` was meant to trip: a merged target
    /// claiming bucket X but referencing a contributor with bucket Y.
    #[test]
    fn detects_cross_bucket_merge() {
        // Build a minimal accepted analysis with two targets in different buckets.
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![
                make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None),
                make_target("x::z", Bucket::BlockCommit, "fix-2", false, None),
            ],
        );
        let accepted = vec![&analysis];

        let mut targets = make_targets_doc(vec![merged_target_for(
            "merged-1",
            Bucket::BlockProcessing,
            vec![("fam-a", 0), ("fam-a", 1)],
            false,
            None,
        )]);
        // Provide lens_dispositions so the lens coverage check passes.
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        let err =
            validate_merge_output(&targets, &accepted).expect_err("expected cross-bucket fail");
        assert!(
            err.to_string()
                .contains("cross-bucket"),
            "got: {err}"
        );
    }

    #[test]
    fn detects_intra_analysis_merge() {
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![
                make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None),
                make_target("x::z", Bucket::BlockProcessing, "fix-2", false, None),
            ],
        );
        let accepted = vec![&analysis];
        let mut targets = make_targets_doc(vec![merged_target_for(
            "merged-1",
            Bucket::BlockProcessing,
            vec![("fam-a", 0), ("fam-a", 1)],
            false,
            None,
        )]);
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        let err =
            validate_merge_output(&targets, &accepted).expect_err("expected intra-analysis fail");
        assert!(
            err.to_string()
                .contains("intra-analysis"),
            "got: {err}"
        );
    }

    #[test]
    fn coverage_invariant_fires_on_missing_target_reference() {
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None)],
        );
        let accepted = vec![&analysis];
        // Empty targets + no rejected_by_merge → coverage fails.
        let mut targets = make_targets_doc(vec![]);
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        let err = validate_merge_output(&targets, &accepted).expect_err("expected coverage fail");
        assert!(
            err.to_string()
                .contains("target coverage"),
            "got: {err}"
        );
    }

    fn make_accepted_with_targets(
        family_id: &str,
        targets: Vec<AnalyzerTarget>,
    ) -> AcceptedAnalysis {
        use crate::models::analyze::{AcceptedStatusTag, LensDisposition};
        use crate::models::common::{LensDispositionStatus, SchemaVersionV2, SelectionLens};
        AcceptedAnalysis {
            schema_version: SchemaVersionV2,
            family_id: family_id.to_owned(),
            status: AcceptedStatusTag::Accepted,
            selection_lens: SelectionLens::TxLatency,
            lens_disposition: LensDisposition {
                lens: SelectionLens::TxLatency,
                status: LensDispositionStatus::Addressed,
                reason: None,
            },
            targets,
            global_materiality_note: None,
        }
    }

    fn make_target(
        target_span: &str,
        bucket: Bucket,
        fix_signature: &str,
        consensus_breaking: bool,
        breakage_class: Option<BreakageClass>,
    ) -> AnalyzerTarget {
        use crate::models::common::{Hotspot, ImprovementVector, Risk};
        AnalyzerTarget {
            target_span: target_span.to_owned(),
            bucket,
            fix_signature: fix_signature.to_owned(),
            hotspot: Hotspot {
                span: target_span.to_owned(),
                self_wall_us: 1,
                total_wall_us: 1,
                calls: 1,
                location: "x.rs:1".to_owned(),
            },
            files: vec!["x.rs".to_owned()],
            evidence: "e".to_owned(),
            proposed_change: "p".to_owned(),
            expected_improvement: ImprovementVector {
                tx_latency: 1.0,
                tenure_throughput: 0.0,
                commit_time: 0.0,
            },
            risk: Risk::Low,
            verification_plan: "v".to_owned(),
            verification_replay: None,
            consensus_breaking,
            breakage_class,
            poc_implementable: None,
            poc_test_scope: None,
            consensus_writeup: None,
        }
    }

    fn merged_target_for(
        id: &str,
        bucket: Bucket,
        merged_from: Vec<(&str, usize)>,
        consensus_breaking: bool,
        breakage_class: Option<BreakageClass>,
    ) -> crate::models::targets::MergedTarget {
        use crate::models::common::{DeliveryMode, Hotspot, ImprovementVector, Risk};
        use crate::models::targets::{MergedFrom, MergedTarget};
        let merged_from: Vec<MergedFrom> = merged_from
            .into_iter()
            .map(|(fid, idx)| MergedFrom {
                family_id: fid.to_owned(),
                target_index: idx,
            })
            .collect();
        let convergence_count = merged_from.len();
        let delivery_mode = DeliveryMode::derive(consensus_breaking, None);
        MergedTarget {
            id: id.to_owned(),
            merged_from,
            convergence_count,
            rank: None,
            target_span: "x::y".to_owned(),
            bucket,
            hotspot: Hotspot {
                span: "x::y".to_owned(),
                self_wall_us: 1,
                total_wall_us: 1,
                calls: 1,
                location: "x.rs:1".to_owned(),
            },
            files: vec!["x.rs".to_owned()],
            evidence: "e".to_owned(),
            proposed_change: "p".to_owned(),
            expected_improvement: ImprovementVector {
                tx_latency: 1.0,
                tenure_throughput: 0.0,
                commit_time: 0.0,
            },
            risk: Risk::Low,
            verification_plan: "v".to_owned(),
            verification_replay: None,
            merge_notes: None,
            contributor_differences: None,
            consensus_breaking,
            breakage_class,
            poc_implementable: None,
            poc_test_scope: None,
            consensus_writeup: None,
            delivery_mode,
            bench_eligible: delivery_mode.bench_eligible(),
        }
    }

    fn make_targets_doc(targets: Vec<crate::models::targets::MergedTarget>) -> OptimizationTargets {
        use crate::models::common::SchemaVersionV2;
        use crate::models::targets::MergeMethod;
        OptimizationTargets {
            schema_version: SchemaVersionV2,
            session_id: "x".to_owned(),
            baseline_run_id: 100,
            baseline_rerun_id: 101,
            noise_floor_pct: 0.8,
            merge_method: MergeMethod::Llm,
            merge_model: "test".to_owned(),
            targets,
            rejected_by_merge: vec![],
            lens_dispositions: vec![],
        }
    }

    fn make_lens_disposition_entry(family_id: &str) -> crate::models::common::LensDispositionEntry {
        use crate::models::common::{LensDispositionEntry, LensDispositionStatus, SelectionLens};
        LensDispositionEntry {
            family_id: family_id.to_owned(),
            lens: SelectionLens::TxLatency,
            status: LensDispositionStatus::Addressed,
            reason: None,
        }
    }
}
