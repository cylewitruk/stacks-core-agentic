//! `analysis/<family-id>/analysis.json` — output of one analyzer agent.
//!
//! Two top-level shapes: `accepted` (with lens disposition + zero or more
//! concrete targets) and `rejected` (the triage signal was a false positive
//! at code level). The distinction matters: a `not_actionable` lens
//! disposition on an accepted analysis means the signal was real but
//! unfixable; `rejected` means the signal itself was wrong.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::{
    BreakageClass, Bucket, Hotspot, ImprovementVector, KEBAB_PATTERN, LensDispositionStatus, Risk,
    SchemaVersionV2, SelectionLens, VerificationReplay,
};

/// Top-level shape of `analysis.json`. Untagged: serde tries
/// [`AcceptedAnalysis`] first (matches when `status: "accepted"`), then
/// [`RejectedAnalysis`] (matches when `status: "rejected"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Analysis {
    /// Analyzer drilled in; either committed targets or explicitly recorded
    /// the lens as not actionable.
    Accepted(AcceptedAnalysis),
    /// Analyzer determined the triage signal was a false positive at code
    /// level; no targets, just a reason.
    Rejected(RejectedAnalysis),
}

impl Analysis {
    /// Family id this analysis belongs to (carried on both variants).
    pub fn family_id(&self) -> &str {
        match self {
            Self::Accepted(a) => &a.family_id,
            Self::Rejected(r) => &r.family_id,
        }
    }

    /// True iff this analysis was accepted (i.e. signal was real). Used by
    /// callers that need to filter accepted families.
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted(_))
    }

    /// Cross-field validation. Run by callers that need stronger checks than
    /// serde's structural validation. In particular:
    /// - `lens_disposition.lens` must equal `selection_lens`;
    /// - `lens_disposition.status == NotActionable` requires `reason`;
    /// - `lens_disposition.status == Addressed` requires non-empty targets;
    /// - per-target consensus-field consistency (see
    ///   [`AnalyzerTarget::validate`]).
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Accepted(a) => a.validate(),
            Self::Rejected(_) => Ok(()),
        }
    }
}

/// Body of an accepted analysis. `status` is a literal `"accepted"` tag that
/// discriminates this variant from [`RejectedAnalysis`] under the untagged
/// [`Analysis`] enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptedAnalysis {
    /// Constant: 2.
    pub schema_version: SchemaVersionV2,
    /// Family id (matches the candidate's `id`).
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: String,
    /// Status discriminator; literal `"accepted"`.
    pub status: AcceptedStatusTag,
    /// Carried verbatim from the candidate's `selection_lens`.
    pub selection_lens: SelectionLens,
    /// Required disposition of the triage signal.
    pub lens_disposition: LensDisposition,
    /// Concrete optimization targets the analyzer commits to. May be empty
    /// when `lens_disposition.status == NotActionable`.
    pub targets: Vec<AnalyzerTarget>,
    /// Optional analyst note about cross-family relevance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_materiality_note: Option<String>,
}

impl AcceptedAnalysis {
    /// Cross-field validation per the schema's allOf chain.
    pub fn validate(&self) -> Result<(), String> {
        if self.lens_disposition.lens != self.selection_lens {
            return Err(format!(
                "analysis `{}`: lens_disposition.lens ({:?}) must equal selection_lens ({:?})",
                self.family_id, self.lens_disposition.lens, self.selection_lens
            ));
        }
        if self.lens_disposition.status == LensDispositionStatus::NotActionable
            && self
                .lens_disposition
                .reason
                .is_none()
        {
            return Err(format!(
                "analysis `{}`: lens_disposition.status=not_actionable requires `reason`",
                self.family_id
            ));
        }
        if self.lens_disposition.status == LensDispositionStatus::Addressed
            && self.targets.is_empty()
        {
            return Err(format!(
                "analysis `{}`: lens_disposition.status=addressed requires at least one target",
                self.family_id
            ));
        }
        for (i, t) in self
            .targets
            .iter()
            .enumerate()
        {
            t.validate()
                .map_err(|e| format!("analysis `{}` target[{}]: {}", self.family_id, i, e))?;
        }
        Ok(())
    }
}

/// Status discriminator carried on accepted analyses. Serializes as the
/// literal string `"accepted"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedStatusTag {
    /// Literal `"accepted"`.
    Accepted,
}

/// Body of a rejected analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectedAnalysis {
    /// Constant: 2.
    pub schema_version: SchemaVersionV2,
    /// Family id (matches the candidate's `id`).
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: String,
    /// Status discriminator; literal `"rejected"`.
    pub status: RejectedStatusTag,
    /// Code-level reason the triage signal was wrong.
    pub reason: String,
}

/// Status discriminator carried on rejected analyses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RejectedStatusTag {
    /// Literal `"rejected"`.
    Rejected,
}

/// Required lens disposition on accepted analyses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LensDisposition {
    /// Lens being disposed of. Must equal the analysis's `selection_lens`.
    pub lens: SelectionLens,
    /// Disposition status.
    pub status: LensDispositionStatus,
    /// Required when `status == NotActionable`; optional but allowed when
    /// `status == Addressed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One concrete optimization target emitted by an analyzer.
///
/// Consensus-routing fields (`breakage_class`, `poc_implementable`,
/// `poc_test_scope`, `consensus_writeup`) are conditionally required based
/// on `consensus_breaking` + `breakage_class`; see [`AnalyzerTarget::validate`]
/// for the cross-field rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalyzerTarget {
    /// Span the optimizer should target. Must NOT be a bucket anchor or a
    /// non-target entry.
    pub target_span: String,
    /// Authoritative bucket classification.
    pub bucket: Bucket,
    /// Stable kebab-case structural-fix identifier.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub fix_signature: String,
    /// Hotspot record (self/total wall, calls, location).
    pub hotspot: Hotspot,
    /// Repo-relative paths, ordered most-likely first.
    pub files: Vec<String>,
    /// Trace + code evidence.
    pub evidence: String,
    /// Concrete proposed change.
    pub proposed_change: String,
    /// Three-axis honest improvement estimate.
    pub expected_improvement: ImprovementVector,
    /// Subjective risk level.
    pub risk: Risk,
    /// Verification plan (which tests must pass / what to spot-check).
    pub verification_plan: String,
    /// Optional targeted-replay recipe. When present, Phase 3 bench runs
    /// will replay these txids / blocks with `--repetitions` instead of
    /// the session's full-range bench, dramatically shrinking per-target
    /// wall time. Absent → full-range fallback. See
    /// [`VerificationReplay`] for the hash-only emission contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_replay: Option<VerificationReplay>,
    /// True iff implementing this fix would change consensus rules.
    pub consensus_breaking: bool,
    /// Required when `consensus_breaking == true`. Forbidden otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakage_class: Option<BreakageClass>,
    /// Required when `consensus_breaking == true`. Forbidden otherwise.
    /// Forced to `false` when `breakage_class == BlockValidation`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poc_implementable: Option<bool>,
    /// Required when `poc_implementable == true`. Forbidden otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poc_test_scope: Option<Vec<String>>,
    /// Required when `consensus_breaking == true`. Forbidden otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consensus_writeup: Option<String>,
}

impl AnalyzerTarget {
    /// Cross-field validation per the schema's consensus-routing allOf chain,
    /// plus a sanity check on `verification_replay` (must have at least one
    /// non-empty section when present — otherwise it's a misleading recipe
    /// that would silently fall back to full-range bench).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(vr) = &self.verification_replay {
            vr.validate()
                .map_err(|e| format!("verification_replay: {e}"))?;
        }
        if self.consensus_breaking {
            if self.breakage_class.is_none() {
                return Err("consensus_breaking=true requires breakage_class".to_owned());
            }
            if self
                .poc_implementable
                .is_none()
            {
                return Err("consensus_breaking=true requires poc_implementable".to_owned());
            }
            if self
                .consensus_writeup
                .is_none()
            {
                return Err("consensus_breaking=true requires consensus_writeup".to_owned());
            }
            if self.breakage_class == Some(BreakageClass::BlockValidation)
                && self.poc_implementable != Some(false)
            {
                return Err("breakage_class=block_validation forces poc_implementable=false \
                            (bench harness does not exercise validation path)"
                    .to_owned());
            }
            if self.poc_implementable == Some(true) {
                match &self.poc_test_scope {
                    Some(s) if !s.is_empty() => {}
                    _ => {
                        return Err(
                            "poc_implementable=true requires non-empty poc_test_scope".to_owned()
                        );
                    }
                }
            } else if self.poc_test_scope.is_some() {
                return Err("poc_test_scope is allowed only when poc_implementable=true".to_owned());
            }
        } else {
            if self.breakage_class.is_some() {
                return Err("non-consensus target must not carry breakage_class".to_owned());
            }
            if self
                .poc_implementable
                .is_some()
            {
                return Err("non-consensus target must not carry poc_implementable".to_owned());
            }
            if self.poc_test_scope.is_some() {
                return Err("non-consensus target must not carry poc_test_scope".to_owned());
            }
            if self
                .consensus_writeup
                .is_some()
            {
                return Err("non-consensus target must not carry consensus_writeup".to_owned());
            }
        }
        Ok(())
    }
}
