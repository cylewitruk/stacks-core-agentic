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

use anyhow::{Context as _, Result, bail};

use crate::harnesses::{AgentHarness, InvokeInputs};
use crate::layout::Layout;
use crate::models::analyze::{AcceptedAnalysis, Analysis};
use crate::models::common::{BreakageClass, Bucket, LensDispositionEntry, SchemaVersionV4};
use crate::models::targets::{OptimizationTargets, RejectedByMerge};
use crate::models::{ToJson, ValidateModel};
use crate::prompts;
use crate::session::dedup::{self, DedupDecision};
use crate::session::{SessionLayout, ledger_reader, loader, maintain_ledger};
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
    /// Number of analyzer-emitted targets deterministically rejected
    /// by cross-session dedup.
    pub deduped_target_count: usize,
    /// True iff the merge LLM was invoked. False for no accepted
    /// analyses and for all-targets-deduped shortcuts.
    pub llm_invoked: bool,
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
    let analyses_map = loader::read_all_analyses(layout)?;
    for (fid, a) in &analyses_map {
        a.validate_model()
            .with_context(|| format!("analysis/{fid}/analysis.json failed validation"))?;
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
            schema_version: crate::models::common::SchemaVersionV4,
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
        fs::write(layout.optimization_targets_json(), empty.to_json_pretty()? + "\n")?;
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
            deduped_target_count: 0,
            llm_invoked: false,
            conversation_id: None,
        });
    }

    let (filtered_accepted, dedup_decisions) = compute_dedup_filtered_inputs(inputs, &accepted)?;
    let deduped_target_count = dedup_decisions.len();

    if filtered_accepted
        .iter()
        .all(|a| a.targets.is_empty())
    {
        let empty = OptimizationTargets {
            schema_version: SchemaVersionV4,
            session_id: candidates.session_id.clone(),
            baseline_run_id: candidates.baseline_run_id,
            baseline_rerun_id: candidates.baseline_rerun_id,
            noise_floor_pct: candidates.noise_floor_pct,
            merge_method: crate::models::targets::MergeMethod::Llm,
            merge_model: String::new(),
            targets: vec![],
            rejected_by_merge: dedup_rejections(&dedup_decisions),
            lens_dispositions: lens_dispositions_from_accepted(&accepted),
        };
        fs::create_dir_all(layout.merge_dir())
            .with_context(|| format!("creating {}", layout.merge_dir().display()))?;
        fs::write(layout.optimization_targets_json(), empty.to_json_pretty()? + "\n")?;
        fs::write(
            layout.merge_final_message(),
            format!(
                "# Merge phase: dedup-only\n\nAll analyzer-emitted targets were skipped by \
                 cross-session dedup.\n\nDedup rejections: {}\n\nCoverage check satisfied: {} \
                 input target(s), 0 merged target(s), {} rejected_by_merge row(s).\n",
                dedup_summary_markdown(&dedup_decisions),
                deduped_target_count,
                deduped_target_count,
            ),
        )?;
        validate_merge_output_with_dedup(&empty, &accepted, &dedup_decisions)?;
        return Ok(Outputs {
            accepted_input_count: accepted_count,
            merged_target_count: 0,
            empty_input_shortcut: false,
            deduped_target_count,
            llm_invoked: false,
            conversation_id: None,
        });
    }

    // Render the merge prompt with the accepted-analyses array inlined.
    let accepted_json = filtered_accepted
        .to_json_pretty()
        .context("serializing accepted analyses for merge prompt")?;
    let dedup_json = dedup_decisions
        .to_json_pretty()
        .context("serializing dedup decisions for merge prompt")?;
    let merge_reasoning_effort = inputs
        .settings
        .codex
        .effective_merge_reasoning_effort()
        .unwrap_or("");
    let merge_model_id = inputs
        .settings
        .codex
        .effective_model();

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
            dedup_rejections_json: dedup_json,
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
        .codex
        .effective_exec_timeout();
    let dangerous = inputs
        .settings
        .codex
        .dangerously_bypass_sandbox
        .unwrap_or(false);
    // Agent reads the schema file + bucket-anchors.md (both rendered
    // into the prompt as absolute paths). The schema file is under
    // `<operator>/.sbagent/schemas/`; bucket-anchors.md is under
    // `<operator>/.sbagent/context/`.
    let mut add_dirs: Vec<PathBuf> = vec![
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
    add_dirs.extend(
        inputs
            .settings
            .codex
            .extra_writable_roots
            .iter()
            .cloned(),
    );

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
            reasoning_effort: if merge_reasoning_effort.is_empty() {
                None
            } else {
                Some(merge_reasoning_effort)
            },
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
    let mut targets = loader::read_optimization_targets(layout)?;
    targets
        .rejected_by_merge
        .extend(dedup_rejections(&dedup_decisions));
    fs::write(layout.optimization_targets_json(), targets.to_json_pretty()? + "\n")?;
    validate_merge_output_with_dedup(&targets, &accepted, &dedup_decisions)?;

    Ok(Outputs {
        accepted_input_count: accepted_count,
        merged_target_count: targets.targets.len(),
        empty_input_shortcut: false,
        deduped_target_count,
        llm_invoked: true,
        conversation_id: invoke_outputs.conversation_id,
    })
}

fn compute_dedup_filtered_inputs(
    inputs: &Inputs<'_, impl AgentHarness>,
    accepted: &[&AcceptedAnalysis],
) -> Result<(Vec<AcceptedAnalysis>, Vec<DedupDecision>)> {
    let operator = inputs
        .framework
        .require_operator_repo_root()?;
    let sessions = ledger_reader::read_all(&operator.join("sessions.jsonl"))
        .context("reading sessions.jsonl for merge dedup projection")?;
    for skipped in &sessions.skipped {
        eprintln!(
            "merge dedup: skipping malformed sessions.jsonl line {}: {}",
            skipped.line_number, skipped.error
        );
    }
    let maintain = maintain_ledger::read_all(&operator.join("maintain.jsonl"))
        .context("reading maintain.jsonl for merge dedup projection")?;
    for skipped in &maintain.skipped {
        eprintln!(
            "merge dedup: skipping malformed maintain.jsonl line {}: {}",
            skipped.line_number, skipped.error
        );
    }
    let projection = dedup::DedupProjection::from_ledgers(
        &sessions.records,
        &maintain.events,
        inputs
            .settings
            .autonomy
            .dedup_failure_threshold,
    );

    Ok(filter_accepted_for_dedup(accepted, &projection))
}

fn filter_accepted_for_dedup(
    accepted: &[&AcceptedAnalysis],
    projection: &dedup::DedupProjection,
) -> (Vec<AcceptedAnalysis>, Vec<DedupDecision>) {
    let mut decisions = Vec::new();
    let mut filtered = Vec::new();
    for analysis in accepted {
        let mut next = (*analysis).clone();
        next.targets.clear();
        for (idx, target) in analysis
            .targets
            .iter()
            .enumerate()
        {
            if let Some(decision) = projection.decision_for(&analysis.family_id, idx, target) {
                decisions.push(decision);
            } else {
                next.targets
                    .push(target.clone());
            }
        }
        filtered.push(next);
    }
    (filtered, decisions)
}

fn dedup_rejections(decisions: &[DedupDecision]) -> Vec<RejectedByMerge> {
    decisions
        .iter()
        .map(DedupDecision::to_rejected_by_merge)
        .collect()
}

fn lens_dispositions_from_accepted(accepted: &[&AcceptedAnalysis]) -> Vec<LensDispositionEntry> {
    accepted
        .iter()
        .map(|a| LensDispositionEntry {
            family_id: a.family_id.clone(),
            lens: a.lens_disposition.lens,
            status: a.lens_disposition.status,
            reason: a
                .lens_disposition
                .reason
                .clone(),
        })
        .collect()
}

fn dedup_summary_markdown(decisions: &[DedupDecision]) -> String {
    if decisions.is_empty() {
        return "none".to_owned();
    }
    decisions
        .iter()
        .map(|d| format!("{} target[{}] {} ({})", d.family_id, d.target_index, d.reason, d.detail))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Validate the merged output against (1) the typed model's per-record
/// invariants and (2) the merge-phase-specific cross-record invariants
/// (coverage, no cross-bucket / intra-analysis / cross-consensus merges).
pub fn validate_merge_output(
    targets: &OptimizationTargets,
    accepted: &[&AcceptedAnalysis],
) -> Result<()> {
    validate_merge_output_with_dedup(targets, accepted, &[])
}

/// Validate merge output plus coordinator-computed dedup decisions.
pub fn validate_merge_output_with_dedup(
    targets: &OptimizationTargets,
    accepted: &[&AcceptedAnalysis],
    expected_dedup: &[DedupDecision],
) -> Result<()> {
    targets
        .validate_model()
        .context("merge output validation")?;

    // Reference set of every analyzer-emitted target by (family_id, target_index).
    let mut expected_pairs: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut accepted_family_ids: BTreeSet<String> = BTreeSet::new();
    let mut bucket_by_pair: BTreeMap<(String, usize), Bucket> = BTreeMap::new();
    let mut consensus_by_pair: BTreeMap<(String, usize), (bool, Option<BreakageClass>)> =
        BTreeMap::new();
    let mut evidence_by_pair: BTreeMap<(String, usize), Vec<crate::models::common::EvidenceQuery>> =
        BTreeMap::new();
    for a in accepted {
        accepted_family_ids.insert(a.family_id.clone());
        for (i, t) in a.targets.iter().enumerate() {
            let key = (a.family_id.clone(), i);
            expected_pairs.insert(key.clone());
            bucket_by_pair.insert(key.clone(), t.bucket);
            consensus_by_pair.insert(key.clone(), (t.consensus_breaking, t.breakage_class));
            evidence_by_pair.insert(key.clone(), t.evidence_queries.clone());
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

    let expected_dedup_pairs: BTreeSet<(String, usize, String)> = expected_dedup
        .iter()
        .map(|d| (d.family_id.clone(), d.target_index, d.reason.clone()))
        .collect();
    let actual_dedup_pairs: BTreeSet<(String, usize, String)> = targets
        .rejected_by_merge
        .iter()
        .filter(|r| r.reason.starts_with("dedup:"))
        .map(|r| (r.family_id.clone(), r.target_index, r.reason.clone()))
        .collect();
    if actual_dedup_pairs != expected_dedup_pairs {
        bail!(
            "merge: dedup rejection invariant failed (expected coordinator-computed dedup \
             decisions exactly once). expected={:?} actual={:?}",
            expected_dedup_pairs,
            actual_dedup_pairs
        );
    }
    for r in &targets.rejected_by_merge {
        if r.reason.starts_with("dedup:") && !dedup::is_dedup_reason(&r.reason) {
            bail!(
                "merge: unknown dedup rejection reason `{}` for family_id={} target_index={}",
                r.reason,
                r.family_id,
                r.target_index
            );
        }
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

    // Evidence provenance passthrough: the merge phase may group targets, but
    // it must not invent or drop analyzer query trails. Results analysis
    // relies on this being the exact union of contributing targets' evidence.
    for t in &targets.targets {
        // Duplicate evidence from multiple contributors is still one evidence
        // item; set equality intentionally collapses repeated rows.
        let expected: BTreeSet<_> = t
            .merged_from
            .iter()
            .flat_map(|mf| {
                evidence_by_pair
                    .get(&(mf.family_id.clone(), mf.target_index))
                    .into_iter()
                    .flatten()
                    .cloned()
            })
            .collect();
        let actual: BTreeSet<_> = t
            .evidence_queries
            .iter()
            .cloned()
            .collect();
        if actual != expected {
            bail!(
                "merge: evidence provenance mismatch on target `{}` (expected exact union of \
                 contributor evidence_queries: {}, got {})",
                t.id,
                expected.len(),
                actual.len()
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

    #[test]
    fn detects_evidence_provenance_mismatch() {
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None)],
        );
        let accepted = vec![&analysis];
        let mut targets = make_targets_doc(vec![merged_target_for(
            "merged-1",
            Bucket::BlockProcessing,
            vec![("fam-a", 0)],
            false,
            None,
        )]);
        targets.targets[0].evidence_queries[0].key_observation =
            "invented observation not present in contributor".to_owned();
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        let err =
            validate_merge_output(&targets, &accepted).expect_err("expected evidence mismatch");
        assert!(
            err.to_string()
                .contains("evidence provenance mismatch"),
            "got: {err}"
        );
    }

    #[test]
    fn accepts_expected_dedup_rejection() {
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None)],
        );
        let accepted = vec![&analysis];
        let decision = dedup_decision("fam-a", 0, "fix-1", dedup::DEDUP_REASON_OPEN_PR);
        let mut targets = make_targets_doc(vec![]);
        targets
            .rejected_by_merge
            .push(decision.to_rejected_by_merge());
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        validate_merge_output_with_dedup(&targets, &accepted, &[decision]).unwrap();
    }

    #[test]
    fn filter_removes_deduped_targets_and_keeps_unrelated_targets() {
        let prior = crate::models::session_record::SessionRecord {
            kind: crate::models::session_record::SessionRecordKind::SessionCompleted,
            schema_version: crate::models::common::SchemaVersionV3,
            id: "prior".to_owned(),
            artifact_branch: "session/prior".to_owned(),
            artifact_sha: "abc123".to_owned(),
            artifact_url: None,
            started_at: "2026-06-11T00:00:00Z".to_owned(),
            finished_at: "2026-06-11T01:00:00Z".to_owned(),
            status: crate::models::session_record::SessionStatus::Succeeded,
            failure_phase: None,
            failure_reason: None,
            sbagent_version: "0.1.0".to_owned(),
            sbagent_git_sha: None,
            range: crate::models::session_record::SessionRange {
                start_at: Some(1),
                count: Some(1),
                warmup: Some(0),
                filter: None,
                network: "mainnet".to_owned(),
            },
            baseline_run_ids: vec![100],
            phase_durations_secs: Default::default(),
            targets: vec![crate::models::session_record::TargetRecord {
                id: "fix-1".to_owned(),
                family_id: "prior-family".to_owned(),
                bucket: "block_processing".to_owned(),
                delivery_mode: crate::models::common::DeliveryMode::NormalPr,
                status: crate::models::session_record::TargetStatus::Accepted,
                status_stage: None,
                reason_code: None,
                head_sha: None,
                pr_url: Some("https://github.com/owner/repo/pull/1".to_owned()),
                issue_url: None,
                bench: None,
            }],
            source_url: None,
            source_branch: None,
            source_sha: None,
            source_fetched_at: None,
        };
        let projection = dedup::DedupProjection::from_ledgers(&[prior], &[], 3);
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![
                make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None),
                make_target("x::z", Bucket::BlockProcessing, "fix-2", false, None),
            ],
        );
        let accepted = vec![&analysis];

        let (filtered, decisions) = filter_accepted_for_dedup(&accepted, &projection);

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].fix_signature, "fix-1");
        assert_eq!(decisions[0].reason, dedup::DEDUP_REASON_OPEN_PR);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].targets.len(), 1);
        assert_eq!(filtered[0].targets[0].fix_signature, "fix-2");
    }

    #[test]
    fn rejects_missing_expected_dedup_rejection() {
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None)],
        );
        let accepted = vec![&analysis];
        let decision = dedup_decision("fam-a", 0, "fix-1", dedup::DEDUP_REASON_OPEN_PR);
        let mut targets = make_targets_doc(vec![]);
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        let err = validate_merge_output_with_dedup(&targets, &accepted, &[decision])
            .expect_err("expected missing dedup row to fail");
        assert!(
            err.to_string()
                .contains("target coverage"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_invented_dedup_rejection() {
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None)],
        );
        let accepted = vec![&analysis];
        let mut targets = make_targets_doc(vec![]);
        targets
            .rejected_by_merge
            .push(crate::models::targets::RejectedByMerge {
                family_id: "fam-a".to_owned(),
                target_index: 0,
                reason: dedup::DEDUP_REASON_OPEN_PR.to_owned(),
            });
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        let err = validate_merge_output_with_dedup(&targets, &accepted, &[])
            .expect_err("expected invented dedup row to fail");
        assert!(
            err.to_string()
                .contains("dedup rejection invariant"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_unknown_dedup_reason() {
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None)],
        );
        let accepted = vec![&analysis];
        let decision = dedup_decision("fam-a", 0, "fix-1", "dedup:unknown");
        let mut targets = make_targets_doc(vec![]);
        targets
            .rejected_by_merge
            .push(decision.to_rejected_by_merge());
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        let err = validate_merge_output_with_dedup(&targets, &accepted, &[decision])
            .expect_err("expected unknown dedup reason to fail");
        assert!(
            err.to_string()
                .contains("unknown dedup rejection reason"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_deduped_target_also_in_targets() {
        let analysis = make_accepted_with_targets(
            "fam-a",
            vec![make_target("x::y", Bucket::BlockProcessing, "fix-1", false, None)],
        );
        let accepted = vec![&analysis];
        let decision = dedup_decision("fam-a", 0, "fix-1", dedup::DEDUP_REASON_OPEN_PR);
        let mut targets = make_targets_doc(vec![merged_target_for(
            "merged-1",
            Bucket::BlockProcessing,
            vec![("fam-a", 0)],
            false,
            None,
        )]);
        targets
            .rejected_by_merge
            .push(decision.to_rejected_by_merge());
        targets
            .lens_dispositions
            .push(make_lens_disposition_entry("fam-a"));

        let err = validate_merge_output_with_dedup(&targets, &accepted, &[decision])
            .expect_err("expected double-accounted dedup target to fail");
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
        use crate::models::common::{LensDispositionStatus, SchemaVersionV4, SelectionLens};
        AcceptedAnalysis {
            schema_version: SchemaVersionV4,
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

    fn dedup_decision(
        family_id: &str,
        target_index: usize,
        fix_signature: &str,
        reason: &str,
    ) -> DedupDecision {
        DedupDecision {
            family_id: family_id.to_owned(),
            target_index,
            fix_signature: fix_signature.to_owned(),
            reason: reason.to_owned(),
            detail: "test detail".to_owned(),
        }
    }

    fn default_vr() -> crate::models::common::VerificationReplay {
        use crate::models::common::{
            BenchInvocation, BenchSamples, ExpectedSignal, ProfilerMode, SelectionLens,
            SignalDirection, VerificationReplay,
        };
        VerificationReplay {
            rationale: "test".into(),
            invocations: vec![BenchInvocation {
                id: "warm-steady".into(),
                label: "warm".into(),
                purpose: "smoke".into(),
                samples: BenchSamples::Blocks {
                    blocks: vec![format!("0x{}", "a".repeat(64))],
                },
                warmup: 10,
                repetitions: 20,
                profiler: ProfilerMode::Rich,
                expected_signal: ExpectedSignal {
                    axis: SelectionLens::TxLatency,
                    direction: SignalDirection::Improves,
                    estimate_pct: Some(4.0),
                    tolerance_pct: Some(2.0),
                },
            }],
            suspected_spans: None,
        }
    }

    fn make_target(
        target_span: &str,
        bucket: Bucket,
        fix_signature: &str,
        consensus_breaking: bool,
        breakage_class: Option<BreakageClass>,
    ) -> AnalyzerTarget {
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        use crate::models::common::{EvidenceQuery, Hotspot, ImprovementVector, Risk};
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
            evidence_queries: if consensus_breaking {
                vec![]
            } else {
                vec![EvidenceQuery {
                    purpose: "prove span movement".to_owned(),
                    sql_path: PathBuf::from("queries/span_run_drift.sql"),
                    params: BTreeMap::from([("span".to_owned(), target_span.to_owned())]),
                    output_path: "queries/span-run-drift.csv".to_owned(),
                    key_observation: "baseline p95 self_wall_us = 1000".to_owned(),
                    supports_invocations: vec!["warm-steady".to_owned()],
                }]
            },
            proposed_change: "p".to_owned(),
            expected_improvement: ImprovementVector {
                tx_latency: 1.0,
                tenure_throughput: 0.0,
                commit_time: 0.0,
            },
            risk: Risk::Low,
            verification_plan: "v".to_owned(),
            verification_replay: if consensus_breaking { None } else { Some(default_vr()) },
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
        let evidence_queries = if consensus_breaking {
            vec![]
        } else {
            vec![crate::models::common::EvidenceQuery {
                purpose: "prove span movement".to_owned(),
                sql_path: std::path::PathBuf::from("queries/span_run_drift.sql"),
                params: BTreeMap::from([("span".to_owned(), "x::y".to_owned())]),
                output_path: "queries/span-run-drift.csv".to_owned(),
                key_observation: "baseline p95 self_wall_us = 1000".to_owned(),
                supports_invocations: vec!["warm-steady".to_owned()],
            }]
        };
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
            evidence_queries,
            proposed_change: "p".to_owned(),
            expected_improvement: ImprovementVector {
                tx_latency: 1.0,
                tenure_throughput: 0.0,
                commit_time: 0.0,
            },
            risk: Risk::Low,
            verification_plan: "v".to_owned(),
            verification_replay: if consensus_breaking { None } else { Some(default_vr()) },
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
        use crate::models::common::SchemaVersionV4;
        use crate::models::targets::MergeMethod;
        OptimizationTargets {
            schema_version: SchemaVersionV4,
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
