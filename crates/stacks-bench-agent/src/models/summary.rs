//! `summary.json` — final per-session machine-readable summary.

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;
use crate::models::common::{
    BreakageClass, DeliveryMode, KEBAB_PATTERN, LensDispositionEntry, SchemaVersionV3,
};

/// Top-level shape of `summary.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    /// Constant: 3.
    pub schema_version: SchemaVersionV3,
    /// Session this artifact belongs to.
    pub session_id: String,
    /// Baseline run id.
    pub baseline_run_id: i64,
    /// Baseline rerun id.
    pub baseline_rerun_id: i64,
    /// Per-host noise floor as a percentage.
    pub noise_floor_pct: f64,
    /// One row per merged target.
    pub experiments: Vec<Experiment>,
    /// Demo-friendly aggregate counts by (delivery_mode, status).
    pub outcome_counts: OutcomeCounts,
    /// Carried verbatim from `optimization-targets.json`.
    pub lens_dispositions: Vec<LensDispositionEntry>,
    /// Operator-facing recommendation for the next session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_targets_hint: Option<String>,
}

/// One experiment row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    /// Canonical kebab-case target id.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub target_id: String,
    /// Carried from `optimization-targets.json`.
    pub delivery_mode: DeliveryMode,
    /// Outcome of this target. Valid values depend on `delivery_mode`; see
    /// [`Experiment::validate_model`] for the seven legal combinations.
    pub status: ExperimentStatus,
    /// Run ids associated with this experiment (only set for `NormalPr`
    /// targets that reached benchmarking).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_ids: Option<Vec<i64>>,
    /// Per-target baseline run ids — pooled (phase-aware breakdown
    /// lives on disk at `verify/<target>/baseline-run-ids.json`).
    /// Present when Phase 1.8 ran for this target (i.e. the target
    /// had a non-empty `verification_replay` and Pass 1a's
    /// targeted-baseline calibration produced results). Absent
    /// when finalize falls back to the session-level baseline
    /// (legacy path for targets without `verification_replay`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_run_ids: Option<Vec<i64>>,
    /// Improvement vs baseline as a signed percentage. Set only for
    /// `NormalPr` targets that reached benchmarking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub improvement_pct: Option<f64>,
    /// Surfaced for `consensus_poc_pr` / `consensus_issue` rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakage_class: Option<BreakageClass>,
    /// Parent of the coordinator's per-target commit. Populated from
    /// `optimize/<id>/coordinator-provenance.json.base_sha` when the
    /// sidecar exists. Used by Phase 5 PR-writer + audit to prove the
    /// optimization was applied on top of the same source SHA Phase 0a
    /// archived. Absent for aborted experiments (no commit) and for
    /// sessions that predate the provenance sidecar contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    /// Coordinator's per-target commit SHA. Populated from
    /// `coordinator-provenance.json.head_sha` (see [`base_sha`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// Free-form reason (e.g. abort message excerpt, "within noise floor").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ValidateModel for Experiment {
    /// Cross-field check: `status` must be valid for the given `delivery_mode`.
    fn validate_model(&self) -> Result<()> {
        let ok = matches!(
            (self.delivery_mode, self.status),
            (
                DeliveryMode::NormalPr,
                ExperimentStatus::Accepted | ExperimentStatus::Rejected | ExperimentStatus::Aborted,
            ) | (
                DeliveryMode::ConsensusPocPr,
                ExperimentStatus::PocLanded | ExperimentStatus::Aborted,
            ) | (
                DeliveryMode::ConsensusIssue,
                ExperimentStatus::RoutedToIssue | ExperimentStatus::Aborted,
            )
        );
        if !ok {
            bail!(
                "experiment `{}`: status={:?} invalid for delivery_mode={:?}",
                self.target_id,
                self.status,
                self.delivery_mode
            );
        }
        Ok(())
    }
}

/// Outcome of an experiment row.
///
/// Valid combinations with `delivery_mode`:
/// - `NormalPr` → `Accepted | Rejected | Aborted`
/// - `ConsensusPocPr` → `PocLanded | Aborted`
/// - `ConsensusIssue` → `RoutedToIssue | Aborted`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus {
    /// Bench measured a real improvement (NormalPr only).
    Accepted,
    /// Bench did not show an improvement (NormalPr only).
    Rejected,
    /// Pipeline did not produce a usable artifact (any mode).
    Aborted,
    /// Scoped tests passed; no benchmark by design (ConsensusPocPr only).
    PocLanded,
    /// Routing marker present; analyzer's writeup is the artifact
    /// (ConsensusIssue only).
    RoutedToIssue,
}

/// Aggregate counts by (delivery_mode, status). Mirrors the schema's nested
/// shape so summary.md can render headline tallies without re-grouping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeCounts {
    /// Counts for `delivery_mode == NormalPr`.
    pub normal_pr: NormalPrCounts,
    /// Counts for `delivery_mode == ConsensusPocPr`.
    pub consensus_poc_pr: ConsensusPocPrCounts,
    /// Counts for `delivery_mode == ConsensusIssue`.
    pub consensus_issue: ConsensusIssueCounts,
}

/// Counts for `NormalPr` rows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalPrCounts {
    /// `status == Accepted`.
    pub accepted: u32,
    /// `status == Rejected`.
    pub rejected: u32,
    /// `status == Aborted`.
    pub aborted: u32,
}

/// Counts for `ConsensusPocPr` rows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsensusPocPrCounts {
    /// `status == PocLanded`.
    pub poc_landed: u32,
    /// `status == Aborted`.
    pub aborted: u32,
}

/// Counts for `ConsensusIssue` rows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsensusIssueCounts {
    /// `status == RoutedToIssue`.
    pub routed_to_issue: u32,
    /// `status == Aborted`.
    pub aborted: u32,
}
