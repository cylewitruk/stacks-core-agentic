//! `analyze/<target-id>/results-analysis.json` — output of the
//! post-bench results-analyzer agent (Phase 3.5).
//!
//! The results-analyzer runs after the Phase 3 verification bench, before
//! Phase 4 finalize. Inputs: the target's
//! [`MergedTarget`](crate::models::targets::MergedTarget) (carries the
//! analyzer's `verification_replay` hypothesis), the optimizer agent's
//! [`OptimizerReport`](crate::models::optimizer_report::OptimizerReport)
//! (claims + diff), per-invocation target calibration baseline + verification
//! bench `bench-run.json` files, bench DB (read-only), and repo context.
//! Output: this typed verdict, which Phase 4 finalize sources as the canonical
//! headline / per-invocation breakdown / PR-body summary for the target.
//!
//! Schema authority lives in this typed model; the committed
//! `schemas/results-analysis.schema.json` is regenerated from these
//! types via `sbagent schema export`.

use anyhow::{Context as _, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;
use crate::models::common::{
    INVOCATION_ID_PATTERN, KEBAB_PATTERN, SchemaVersionV1, SelectionLens, is_valid_invocation_id,
};

/// Top-level shape of `results-analysis.json`. Single canonical record
/// per target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResultsAnalysis {
    /// Constant: 1. New artifact; starts at v1.
    pub schema_version: SchemaVersionV1,
    /// Session id this verdict belongs to. Matches the parent session;
    /// the Phase 4 loader cross-checks.
    pub session_id: String,
    /// Target id this verdict belongs to. Matches
    /// `MergedTarget.id`; the loader cross-checks.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub target_id: String,
    /// Selection lens the verdict is denominated against. Carried
    /// verbatim from the target's
    /// `verification_replay.invocations[].expected_signal.axis`
    /// (all invocations on one target share an axis in v1 — the
    /// analyzer commits to one lens per target).
    pub axis: SelectionLens,

    /// The agent's overall verdict on the measured results.
    pub verdict: Verdict,
    /// How sure the analyzer is about the verdict. Coordinator uses
    /// this against `results_analysis.confidence_floor` to gate
    /// publishing.
    pub confidence: Confidence,
    /// One-line agent-written narrative defending the verdict.
    /// Non-blank. Surfaced in `summary.md` next to the headline number.
    pub headline_rationale: String,
    /// The canonical per-target `improvement_pct` the analyzer commits
    /// to. Phase 4 finalize sources `Experiment.improvement_pct` from
    /// here verbatim. None when the analyzer declines to commit a
    /// single number (verdict = mixed or rejected).
    ///
    /// Sign convention: positive = candidate faster than baseline (same
    /// as today's `improvement_pct`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline_improvement_pct: Option<f64>,

    /// One entry per invocation in the target's
    /// `verification_replay.invocations[]`. Order matches the
    /// invocations array; each `invocation_id` maps to the same key
    /// on the source target.
    #[schemars(length(min = 1))]
    pub per_invocation: Vec<PerInvocationResult>,

    /// Structured caveats — observations the agent flagged as worth
    /// surfacing alongside the verdict but not severe enough to demote
    /// it. May be empty. Surface verbatim in the PR body and
    /// `summary.md`.
    #[serde(default)]
    pub caveats: Vec<String>,

    /// PR-body result-section prose written by the analyzer. Phase 5
    /// PR-writer reads this verbatim into the PR description's "Result"
    /// section — no templating around a number on the orchestrator side.
    /// Non-blank when present; `None` only when `verdict = rejected`
    /// (no PR opens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_body_summary: Option<String>,

    /// Read-only SQL queries the analyzer issued against the bench DB
    /// while forming its verdict, with their CSV outputs persisted next
    /// to this file. Audit trail; the verifier (Pass 2) carries the
    /// same field shape.
    #[serde(default)]
    pub db_queries: Vec<DbQueryRef>,
}

/// Overall verdict on the measured results vs the analyzer's
/// hypothesis.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Measured signal matches the hypothesis. Ship.
    Accepted,
    /// Measured signal partially matches the hypothesis — improvement
    /// exists but the per-invocation shape disagrees with the
    /// expected pattern (e.g. cold gained instead of warm). Coordinator
    /// escalates: draft PR with caveats, or operator review.
    Mixed,
    /// Measured signal contradicts the analyzer's mechanism claim.
    /// Experiment closes as `Rejected (mechanism mismatch)`. No PR.
    Rejected,
}

/// How sure the analyzer is about the verdict.
///
/// Variant order matches schema serialization order (`high`, `medium`,
/// `low`); for strength comparisons use [`Self::level`] rather than
/// the derived `Ord`, which sorts in declaration order (i.e.
/// `High < Medium < Low`) and would invert the intuitive direction.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Strong evidence: matches the expected signal direction across
    /// all invocations; magnitudes within (or close to) tolerance.
    /// PR-grade as-is.
    High,
    /// Mostly aligned but with notable caveats. Coordinator surfaces
    /// caveats prominently in the PR body.
    Medium,
    /// Weak evidence — possibly noise, possibly real but unclear.
    /// Demote to caveats-heavy or hold for operator review per
    /// `results_analysis.confidence_floor`.
    Low,
}

impl Confidence {
    /// Strength on a 0..=2 scale (`Low = 0`, `Medium = 1`, `High = 2`).
    /// Use this to compare a verdict's confidence against an operator
    /// floor: `verdict.level() >= floor.level()` reads as "at or
    /// above" without having to mentally invert the declaration-order
    /// `Ord`. Stable + monotonic — the publisher gate relies on the
    /// scale.
    pub fn level(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
        }
    }
}

/// One row of [`ResultsAnalysis::per_invocation`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PerInvocationResult {
    /// Matches the corresponding `BenchInvocation.id` on the target's
    /// `verification_replay`.
    pub invocation_id: String,
    /// Human label carried through from `BenchInvocation.label` for
    /// readability in summaries. Redundant with the source but the
    /// results-analyzer copies it so consumers don't have to cross-ref.
    pub label: String,
    /// Phase 1.8 baseline run id for this invocation.
    pub baseline_run_id: i64,
    /// Phase 3 candidate run id for this invocation.
    pub candidate_run_id: i64,
    /// Measured improvement percent. Positive = candidate faster than
    /// baseline.
    pub measured_pct: f64,
    /// True iff the measured direction + magnitude (when an
    /// `expected_signal.estimate_pct` was provided) fall within the
    /// analyzer's hypothesis. Direction mismatch always sets this to
    /// false. Magnitude-only mismatch (direction right, magnitude
    /// outside tolerance) is up to the analyzer — usually false.
    pub matches_expected_signal: bool,
    /// Structured observations the analyzer surfaced about this
    /// invocation — variance bands, span shifts, profiler-kv
    /// reflections. May be empty.
    #[serde(default)]
    pub observations: Vec<String>,
}

/// Reference to one read-only SQL query the analyzer issued. Raw SQL
/// lives alongside its CSV output (not in this JSON), keyed on
/// `query_digest`. Same shape as the Pass 2 verifier's `db_queries[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbQueryRef {
    /// One-line purpose: what the query was checking.
    pub purpose: String,
    /// sha256 hex of the raw SQL text; matches the on-disk file name.
    pub query_digest: String,
    /// Row count returned. Quick sanity field; full results on disk.
    pub rows_returned: u64,
    /// Relative path from the session results dir to the CSV output
    /// (e.g. `analyze/<target>/queries/<digest>.csv`).
    pub output_path: String,
}

impl ValidateModel for ResultsAnalysis {
    fn validate_model(&self) -> Result<()> {
        if self
            .session_id
            .trim()
            .is_empty()
        {
            bail!("results_analysis.session_id must be non-empty");
        }
        if self
            .target_id
            .trim()
            .is_empty()
        {
            bail!("results_analysis.target_id must be non-empty");
        }
        if self
            .headline_rationale
            .trim()
            .is_empty()
        {
            bail!("results_analysis.headline_rationale must be non-blank");
        }
        if let Some(hp) = self.headline_improvement_pct
            && !hp.is_finite()
        {
            bail!("results_analysis.headline_improvement_pct = {hp} must be finite");
        }
        if self.per_invocation.is_empty() {
            bail!("results_analysis.per_invocation must contain at least one entry");
        }
        let mut seen = std::collections::HashSet::with_capacity(self.per_invocation.len());
        for (i, p) in self
            .per_invocation
            .iter()
            .enumerate()
        {
            if !seen.insert(p.invocation_id.as_str()) {
                bail!(
                    "results_analysis.per_invocation[{i}]: duplicate invocation_id {:?}",
                    p.invocation_id
                );
            }
            p.validate_model()
                .with_context(|| format!("results_analysis.per_invocation[{i}]"))?;
        }
        // Verdict/confidence/headline cross-checks.
        match (
            self.verdict,
            self.headline_improvement_pct
                .is_some(),
        ) {
            // Accepted MUST commit a headline number.
            (Verdict::Accepted, false) => {
                bail!(
                    "results_analysis.verdict=accepted requires headline_improvement_pct (the \
                     analyzer must commit a single number when accepting)"
                );
            }
            // Rejected MUST NOT carry a headline number — no PR ships.
            (Verdict::Rejected, true) => {
                bail!(
                    "results_analysis.verdict=rejected forbids headline_improvement_pct (no PR \
                     ships; the field would be misleading)"
                );
            }
            _ => {}
        }
        match (self.verdict, self.pr_body_summary.is_some()) {
            (Verdict::Accepted | Verdict::Mixed, false) => {
                bail!(
                    "results_analysis.verdict={:?} requires pr_body_summary (Phase 5 PR-writer \
                     sources the PR body from this field)",
                    self.verdict
                );
            }
            (Verdict::Rejected, true) => {
                bail!("results_analysis.verdict=rejected forbids pr_body_summary (no PR ships)");
            }
            _ => {}
        }
        if let Some(s) = &self.pr_body_summary
            && s.trim().is_empty()
        {
            bail!("results_analysis.pr_body_summary must be non-blank when present");
        }
        for (i, c) in self
            .caveats
            .iter()
            .enumerate()
        {
            if c.trim().is_empty() {
                bail!("results_analysis.caveats[{i}] must be non-blank");
            }
        }
        Ok(())
    }
}

impl ValidateModel for PerInvocationResult {
    fn validate_model(&self) -> Result<()> {
        if !is_valid_invocation_id(&self.invocation_id) {
            bail!(
                "per_invocation_result.invocation_id = {:?} does not match {} (analyzers must \
                 echo the invocation_id from the target's verification_replay.invocations[].id)",
                self.invocation_id,
                INVOCATION_ID_PATTERN
            );
        }
        if self.label.trim().is_empty() {
            bail!("per_invocation_result `{}`: label must be non-blank", self.invocation_id);
        }
        if self.baseline_run_id <= 0 {
            bail!(
                "per_invocation_result `{}`: baseline_run_id = {} must be > 0 (stacks-bench DB \
                 primary keys are 1-indexed)",
                self.invocation_id,
                self.baseline_run_id
            );
        }
        if self.candidate_run_id <= 0 {
            bail!(
                "per_invocation_result `{}`: candidate_run_id = {} must be > 0 (stacks-bench DB \
                 primary keys are 1-indexed)",
                self.invocation_id,
                self.candidate_run_id
            );
        }
        if !self.measured_pct.is_finite() {
            bail!(
                "per_invocation_result `{}`: measured_pct = {} must be finite",
                self.invocation_id,
                self.measured_pct
            );
        }
        for (i, o) in self
            .observations
            .iter()
            .enumerate()
        {
            if o.trim().is_empty() {
                bail!(
                    "per_invocation_result `{}`: observations[{i}] must be non-blank",
                    self.invocation_id
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::common::SelectionLens;

    fn pir(id: &str, measured: f64, matches: bool) -> PerInvocationResult {
        PerInvocationResult {
            invocation_id: id.to_owned(),
            label: format!("label for {id}"),
            baseline_run_id: 100,
            candidate_run_id: 101,
            measured_pct: measured,
            matches_expected_signal: matches,
            observations: vec![],
        }
    }

    fn accepted() -> ResultsAnalysis {
        ResultsAnalysis {
            schema_version: SchemaVersionV1,
            session_id: "20260524-100000".to_owned(),
            target_id: "marf-historical-read-node-cache".to_owned(),
            axis: SelectionLens::TxLatency,
            verdict: Verdict::Accepted,
            confidence: Confidence::High,
            headline_rationale: "warm steady-state matched the hypothesis; cold neutral confirms \
                                 mechanism"
                .to_owned(),
            headline_improvement_pct: Some(4.7),
            per_invocation: vec![pir("cold-first-touch", 0.8, true), pir("warm-steady", 4.7, true)],
            caveats: vec![],
            pr_body_summary: Some(
                "MARF historical read benefits from a per-block node cache...".to_owned(),
            ),
            db_queries: vec![],
        }
    }

    #[test]
    fn accepted_baseline_validates() {
        accepted()
            .validate_model()
            .expect("baseline accepted should validate");
    }

    #[test]
    fn accepted_requires_headline_improvement_pct() {
        let mut r = accepted();
        r.headline_improvement_pct = None;
        let err = r
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(err.contains("headline_improvement_pct"), "{err}");
    }

    #[test]
    fn rejected_forbids_headline_improvement_pct() {
        let mut r = accepted();
        r.verdict = Verdict::Rejected;
        r.pr_body_summary = None;
        let err = r
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(err.contains("headline_improvement_pct"), "{err}");
    }

    #[test]
    fn rejected_forbids_pr_body_summary() {
        let mut r = accepted();
        r.verdict = Verdict::Rejected;
        r.headline_improvement_pct = None;
        let err = r
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(err.contains("pr_body_summary"), "{err}");
    }

    #[test]
    fn accepted_requires_pr_body_summary() {
        let mut r = accepted();
        r.pr_body_summary = None;
        let err = r
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(err.contains("pr_body_summary"), "{err}");
    }

    #[test]
    fn rejects_duplicate_invocation_id() {
        let mut r = accepted();
        r.per_invocation[1].invocation_id = "cold-first-touch".to_owned();
        let err = r
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn rejects_non_finite_measured_pct() {
        let mut r = accepted();
        r.per_invocation[0].measured_pct = f64::NAN;
        let err = format!(
            "{:#}",
            r.validate_model()
                .unwrap_err()
        );
        assert!(err.contains("measured_pct"), "{err}");
    }

    #[test]
    fn rejects_invalid_per_invocation_id_format() {
        let mut r = accepted();
        r.per_invocation[0].invocation_id = "Cold-First-Touch".into();
        let err = format!(
            "{:#}",
            r.validate_model()
                .unwrap_err()
        );
        assert!(err.contains(INVOCATION_ID_PATTERN), "{err}");
    }

    #[test]
    fn rejects_nonpositive_baseline_run_id() {
        for bad in [0, -1, i64::MIN] {
            let mut r = accepted();
            r.per_invocation[0].baseline_run_id = bad;
            let err = format!(
                "{:#}",
                r.validate_model()
                    .unwrap_err()
            );
            assert!(err.contains("baseline_run_id"), "run_id={bad} got: {err}");
            assert!(err.contains("> 0"), "{err}");
        }
    }

    #[test]
    fn rejects_nonpositive_candidate_run_id() {
        let mut r = accepted();
        r.per_invocation[0].candidate_run_id = -5;
        let err = format!(
            "{:#}",
            r.validate_model()
                .unwrap_err()
        );
        assert!(err.contains("candidate_run_id"), "{err}");
    }
}
