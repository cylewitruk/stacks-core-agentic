//! `optimization-targets.json` — output of the merge phase.
//!
//! Carries the deduplicated optimization targets ready for optimizer
//! fan-out, plus per-family lens dispositions for the summary phase.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::{
    BreakageClass, Bucket, DeliveryMode, Hotspot, ImprovementVector, KEBAB_PATTERN,
    LensDispositionEntry, LensDispositionStatus, Risk, SchemaVersionV2, VerificationReplay,
};

/// Top-level shape of `optimization-targets.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptimizationTargets {
    /// Constant: 2.
    pub schema_version: SchemaVersionV2,
    /// Session this artifact belongs to.
    pub session_id: String,
    /// Baseline run id (matches `<results>/baseline-run-id`).
    pub baseline_run_id: i64,
    /// Baseline rerun id (matches `<results>/baseline-rerun-id`).
    pub baseline_rerun_id: i64,
    /// Per-host noise floor as a percentage.
    pub noise_floor_pct: f64,
    /// Identifier of the merge implementation. Currently only `Llm`.
    pub merge_method: MergeMethod,
    /// Exact model identifier used for the merge call.
    pub merge_model: String,
    /// Merged optimization targets.
    pub targets: Vec<MergedTarget>,
    /// Analyzer-emitted targets the merger affirmatively dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_by_merge: Vec<RejectedByMerge>,
    /// Per-family lens dispositions, one per accepted analysis.
    pub lens_dispositions: Vec<LensDispositionEntry>,
}

impl OptimizationTargets {
    /// Cross-field validation: every entry's `not_actionable` lens
    /// disposition has a reason; every target's consensus-routing fields
    /// are consistent; coverage invariants are checked separately by the
    /// merge-phase validator (against the source analyses).
    pub fn validate(&self) -> Result<(), String> {
        for (i, t) in self
            .targets
            .iter()
            .enumerate()
        {
            t.validate()
                .map_err(|e| format!("targets[{i}]: {e}"))?;
        }
        for (i, d) in self
            .lens_dispositions
            .iter()
            .enumerate()
        {
            if d.status == LensDispositionStatus::NotActionable && d.reason.is_none() {
                return Err(format!(
                    "lens_dispositions[{i}] (family `{}`): not_actionable requires reason",
                    d.family_id
                ));
            }
        }
        Ok(())
    }
}

/// Identifier of the merge implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    /// LLM consolidation pass (the only implementation today).
    Llm,
}

/// Reference to one analyzer-emitted target, by family id and zero-based
/// index into that analysis's `targets[]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergedFrom {
    /// Family id (kebab-case).
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: String,
    /// Zero-based index into the source analysis's `targets[]`.
    pub target_index: usize,
}

/// Analyzer-emitted target the merger affirmatively dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectedByMerge {
    /// Family id of the source analysis.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: String,
    /// Zero-based index into the source analysis's `targets[]`.
    pub target_index: usize,
    /// Why the target was dropped (e.g. duplicate of an already-shipped fix).
    pub reason: String,
}

/// One merged optimization target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergedTarget {
    /// Canonical kebab-case identifier (= canonical fix_signature).
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub id: String,
    /// References to the analyzer-emitted targets that contributed.
    #[schemars(length(min = 1))]
    pub merged_from: Vec<MergedFrom>,
    /// `merged_from.len()` — confidence signal.
    #[schemars(range(min = 1))]
    pub convergence_count: usize,
    /// Optional priority ordering hint for the optimizer fan-out.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub rank: Option<usize>,
    /// Span the optimizer should target.
    pub target_span: String,
    /// Authoritative bucket. Must match across all contributors.
    pub bucket: Bucket,
    /// Hotspot record carried from the contributor with the largest total
    /// wall time.
    pub hotspot: Hotspot,
    /// Repo-relative paths.
    pub files: Vec<String>,
    /// Synthesized evidence.
    pub evidence: String,
    /// Canonical proposed change.
    pub proposed_change: String,
    /// Per-axis median improvement across contributors.
    pub expected_improvement: ImprovementVector,
    /// Maximum (most cautious) risk among contributors.
    pub risk: Risk,
    /// Union of contributors' verification requirements.
    pub verification_plan: String,
    /// Optional targeted-replay recipe carried through from analyzer
    /// contributors. When multiple contributors disagree, the merger
    /// picks the most specific recipe (broadest txid/block set) or
    /// drops to `None` (full-range fallback) — recorded in
    /// `merge_notes`. Absent → Phase 3 falls back to the session's
    /// full-range bench.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_replay: Option<VerificationReplay>,
    /// Optional reviewer-facing note from the merger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_notes: Option<String>,
    /// Optional: short bullets describing where contributors disagreed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contributor_differences: Option<Vec<String>>,
    /// Carried through from contributors. Must match across all contributors.
    pub consensus_breaking: bool,
    /// Required when `consensus_breaking == true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakage_class: Option<BreakageClass>,
    /// Required when `consensus_breaking == true`. Conservative (false if
    /// any contributor said false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poc_implementable: Option<bool>,
    /// Required when `poc_implementable == true`. Union of contributors'
    /// scopes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poc_test_scope: Option<Vec<String>>,
    /// Required when `consensus_breaking == true`. Most-detailed contributor
    /// writeup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consensus_writeup: Option<String>,
    /// Derived: routing decision for Phase 5.
    pub delivery_mode: DeliveryMode,
    /// Derived: true iff `delivery_mode == NormalPr`.
    pub bench_eligible: bool,
}

impl MergedTarget {
    /// Cross-field validation per the merge schema's allOf chain, plus a
    /// sanity check on `verification_replay` (must have at least one
    /// non-empty section when present).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(vr) = &self.verification_replay {
            vr.validate()
                .map_err(|e| format!("target `{}`: verification_replay: {e}", self.id))?;
        }
        if self.merged_from.is_empty() {
            return Err(format!("target `{}`: merged_from is empty", self.id));
        }
        if self.merged_from.len() != self.convergence_count {
            return Err(format!(
                "target `{}`: convergence_count={} but merged_from has {} entries",
                self.id,
                self.convergence_count,
                self.merged_from.len()
            ));
        }
        // Consensus-routing field consistency mirrors the analyzer-target rules.
        if self.consensus_breaking {
            if self.breakage_class.is_none() {
                return Err(format!(
                    "target `{}`: consensus_breaking=true requires breakage_class",
                    self.id
                ));
            }
            if self
                .poc_implementable
                .is_none()
            {
                return Err(format!(
                    "target `{}`: consensus_breaking=true requires poc_implementable",
                    self.id
                ));
            }
            if self
                .consensus_writeup
                .is_none()
            {
                return Err(format!(
                    "target `{}`: consensus_breaking=true requires consensus_writeup",
                    self.id
                ));
            }
            if self.breakage_class == Some(BreakageClass::BlockValidation)
                && self.poc_implementable != Some(false)
            {
                return Err(format!(
                    "target `{}`: breakage_class=block_validation forces poc_implementable=false",
                    self.id
                ));
            }
            if self.poc_implementable == Some(true) {
                match &self.poc_test_scope {
                    Some(s) if !s.is_empty() => {}
                    _ => {
                        return Err(format!(
                            "target `{}`: poc_implementable=true requires non-empty poc_test_scope",
                            self.id
                        ));
                    }
                }
            } else if self.poc_test_scope.is_some() {
                return Err(format!(
                    "target `{}`: poc_test_scope allowed only when poc_implementable=true",
                    self.id
                ));
            }
        } else if self.breakage_class.is_some()
            || self
                .poc_implementable
                .is_some()
            || self.poc_test_scope.is_some()
            || self
                .consensus_writeup
                .is_some()
        {
            return Err(format!(
                "target `{}`: non-consensus target must not carry any consensus-only field",
                self.id
            ));
        }
        // Derived-field consistency.
        let expected_dm = DeliveryMode::derive(self.consensus_breaking, self.poc_implementable);
        if self.delivery_mode != expected_dm {
            return Err(format!(
                "target `{}`: delivery_mode={:?} inconsistent with consensus_breaking={} + \
                 poc_implementable={:?} (expected {:?})",
                self.id,
                self.delivery_mode,
                self.consensus_breaking,
                self.poc_implementable,
                expected_dm
            ));
        }
        if self.bench_eligible
            != self
                .delivery_mode
                .bench_eligible()
        {
            return Err(format!(
                "target `{}`: bench_eligible={} inconsistent with delivery_mode={:?}",
                self.id, self.bench_eligible, self.delivery_mode
            ));
        }
        Ok(())
    }
}
