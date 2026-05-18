//! `optimize/<target-id>/optimizer-report.json` — typed output of one
//! optimizer agent. Replaces the previous marker-file contract
//! (`implementation.md` / `abort.md`).
//!
//! Two top-level shapes in Phase 1: `implemented` (gates passed, code in
//! worktree, coordinator may commit) and `aborted` (no usable
//! implementation; no commit). Phase 2 will add `consensus_review_needed`
//! for the "built something, parity unproven" case.
//!
//! Architecture note: this is the agent's authoritative output. The
//! `implementation.md` / `abort.md` files become coordinator-rendered
//! markdown views derived from this JSON post-hoc (same pattern as
//! `candidates.md` from `candidates.json`).
//!
//! Naming: deliberately distinct from finalize-phase
//! [`crate::models::summary::ExperimentStatus`]. The optimizer report
//! captures what the agent *did* in one attempt; finalize's
//! `ExperimentStatus` captures whether the experiment *won* after
//! bench/evaluation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::{DeliveryMode, SchemaVersionV2};

/// True iff `s` is non-empty after trimming. Centralized so every
/// string-non-empty invariant in this module agrees on the rule
/// (`""`, `"   "`, `"\t\n"` all fail).
fn is_non_blank(s: &str) -> bool {
    !s.trim().is_empty()
}

/// True iff `xs` contains at least one element whose trimmed form is
/// non-empty. Used for arrays where the *existence* of a contentful
/// entry is what carries the invariant, not raw `len() > 0`.
fn has_non_blank(xs: &[String]) -> bool {
    xs.iter()
        .any(|s| is_non_blank(s))
}

/// Top-level shape of `optimizer-report.json`. Untagged: serde tries
/// each variant in order, matching by the `outcome` discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OptimizerReport {
    /// All gates passed; code is in the worktree, coordinator may commit
    /// and (for `normal_pr`) route to Phase 3 bench.
    Implemented(ImplementedReport),
    /// No usable implementation. Coordinator cleans up; no commit, no bench.
    Aborted(AbortedReport),
    // Phase 2: ConsensusReviewNeeded(ConsensusReviewReport),
}

impl OptimizerReport {
    /// Target id this report belongs to (carried on every variant).
    pub fn target_id(&self) -> &str {
        match self {
            Self::Implemented(r) => &r.target_id,
            Self::Aborted(r) => &r.target_id,
        }
    }

    /// Delivery mode carried verbatim from the merged target.
    pub fn delivery_mode(&self) -> DeliveryMode {
        match self {
            Self::Implemented(r) => r.delivery_mode,
            Self::Aborted(r) => r.delivery_mode,
        }
    }

    /// True iff this report indicates the agent produced a usable
    /// implementation (i.e. coordinator should commit, not clean up).
    pub fn is_implemented(&self) -> bool {
        matches!(self, Self::Implemented(_))
    }

    /// Cross-field validation. Mirrors the schema invariants emitted by
    /// [`crate::schema_export::transform`] so Rust-side callers can fail
    /// fast without round-tripping through a schema validator.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Implemented(r) => r.validate(),
            Self::Aborted(r) => r.validate(),
        }
    }
}

/// Body of an implemented report — all keep/abort gates passed for the
/// target's delivery mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImplementedReport {
    /// Constant: 2.
    pub schema_version: SchemaVersionV2,
    /// Session id (matches the merge phase's session_id).
    pub session_id: String,
    /// Target id (matches the merged target's `id`).
    pub target_id: String,
    /// Status discriminator; literal `"implemented"`.
    pub outcome: ImplementedOutcomeTag,
    /// Carried verbatim from the merged target's `delivery_mode`.
    pub delivery_mode: DeliveryMode,

    /// One-line summary of what was changed and why. Coordinator uses
    /// this as the commit-message body and the PR-description lead.
    pub implementation_summary: String,

    /// Optional: how the implementation diverged from the analyzer's
    /// `proposed_change`. `None` when the agent followed the proposal
    /// without material deviation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deviation_from_proposed_change: Option<String>,

    /// Optional: dependency version bumps applied as part of the change.
    /// `None` when no Cargo.toml versions changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_changes: Option<String>,

    /// Test-suite outcome from the inner-loop gate.
    pub test_summary: TestSummary,

    /// Clippy outcome.
    ///
    /// For `normal_pr`: REQUIRED `Some(true)` — clippy is the gate, and
    /// `implemented` is only allowed when clippy was clean. A `Some(false)`
    /// or `None` here on `normal_pr` is a validation error (use `aborted`
    /// with `failed_gate=Clippy` if clippy actually failed).
    ///
    /// For `consensus_poc_pr`: UNCONSTRAINED. Clippy isn't the gate
    /// (PoCs may intentionally introduce code clippy doesn't love yet),
    /// but if the agent chose to run it the result is informative:
    /// - `None` — agent skipped clippy
    /// - `Some(true)` — agent ran clippy, was clean (bonus signal)
    /// - `Some(false)` — agent ran clippy, found issues; agent still chose to
    ///   ship the PoC because the scoped tests pass
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clippy_clean: Option<bool>,

    /// One-line PR title proposal (e.g. `"perf: bypass generic Write in
    /// MARF seal hashing"`). Coordinator uses this as the PR title;
    /// coordinator may sanitize/truncate.
    pub pr_title: String,

    /// Parity report. ALWAYS present on `implemented` — forces the agent
    /// to consciously evaluate "did this land in consensus-sensitive
    /// code?" rather than communicating that via field presence/absence.
    pub parity: ParityReport,

    /// Optional: deferred-to-hard-fork follow-on. Populated when the
    /// **full** throughput win requires a future cost recalibration,
    /// semantics change, or wire-format tweak. Describes the opportunity
    /// for reviewers and future analyzers; NOT implemented in this PR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hard_fork_followup: Option<String>,
}

impl ImplementedReport {
    /// Cross-field validation per the schema's allOf chain.
    pub fn validate(&self) -> Result<(), String> {
        // Invariant 1: consensus_sensitive=true requires
        // non-blank (trim-aware) evidence + tests — an array of `[""]`
        // shouldn't satisfy parity. Centralized via [`has_non_blank`].
        if self
            .parity
            .consensus_sensitive
        {
            if !has_non_blank(&self.parity.evidence) {
                return Err(format!(
                    "implemented report `{}`: parity.consensus_sensitive=true requires at least \
                     one non-blank parity.evidence entry",
                    self.target_id
                ));
            }
            if !has_non_blank(&self.parity.tests) {
                return Err(format!(
                    "implemented report `{}`: parity.consensus_sensitive=true requires at least \
                     one non-blank parity.tests entry",
                    self.target_id
                ));
            }
        }
        // Invariant 2: implemented must not carry unproven_risk — that
        // signals "I built something but parity is unproven," which is
        // Phase 2's consensus_review_needed outcome, not implemented.
        if self
            .parity
            .unproven_risk
            .is_some()
        {
            return Err(format!(
                "implemented report `{}`: parity.unproven_risk must be null on `implemented` (use \
                 consensus_review_needed outcome instead — Phase 2)",
                self.target_id
            ));
        }
        // Invariant 3: test_summary.failed must be 0 on implemented
        // (otherwise the agent would have emitted aborted with
        // failed_gate=Nextest + a failing_tests list).
        if self.test_summary.failed != 0 {
            return Err(format!(
                "implemented report `{}`: test_summary.failed must be 0 on `implemented` (got {}; \
                 emit `aborted` with failed_gate=nextest + failing_tests list if tests actually \
                 failed)",
                self.target_id, self.test_summary.failed
            ));
        }
        // Invariant 4: free-text fields must be non-blank. Mirrors the
        // AbortedReport.reason check and protects against empty-string
        // / whitespace-only agent output that would otherwise serialize
        // valid but render useless.
        if !is_non_blank(&self.implementation_summary) {
            return Err(format!(
                "implemented report `{}`: implementation_summary must be non-blank",
                self.target_id
            ));
        }
        if !is_non_blank(&self.pr_title) {
            return Err(format!(
                "implemented report `{}`: pr_title must be non-blank",
                self.target_id
            ));
        }
        if !is_non_blank(&self.test_summary.log_path) {
            return Err(format!(
                "implemented report `{}`: test_summary.log_path must be non-blank",
                self.target_id
            ));
        }
        // Invariant 5: test_summary.duration_secs must be finite and
        // non-negative. NaN/Inf can't come from JSON but Rust-side
        // construction (and tests that mock it) can; negative is wrong
        // by definition.
        if !self
            .test_summary
            .duration_secs
            .is_finite()
            || self
                .test_summary
                .duration_secs
                < 0.0
        {
            return Err(format!(
                "implemented report `{}`: test_summary.duration_secs must be finite and >= 0 (got \
                 {})",
                self.target_id,
                self.test_summary
                    .duration_secs
            ));
        }
        // Invariant 6: clippy_clean must be Some(true) for normal_pr.
        // consensus_poc_pr is unconstrained (clippy isn't the gate).
        // consensus_issue never produces an implemented report.
        match (self.delivery_mode, self.clippy_clean) {
            (DeliveryMode::NormalPr, Some(true)) => {}
            (DeliveryMode::NormalPr, Some(false)) => {
                return Err(format!(
                    "implemented report `{}`: normal_pr requires clippy_clean=true (if clippy \
                     failed, emit `aborted` with failed_gate=clippy)",
                    self.target_id
                ));
            }
            (DeliveryMode::NormalPr, None) => {
                return Err(format!(
                    "implemented report `{}`: normal_pr requires clippy_clean to be set",
                    self.target_id
                ));
            }
            // consensus_poc_pr: any value (or omission) is acceptable.
            (DeliveryMode::ConsensusPocPr, _) => {}
            (DeliveryMode::ConsensusIssue, _) => {
                return Err(format!(
                    "implemented report `{}`: consensus_issue mode never produces an implemented \
                     report (coordinator skips the optimizer for those targets)",
                    self.target_id
                ));
            }
        }
        Ok(())
    }
}

/// Status discriminator carried on implemented reports. Serializes as
/// the literal string `"implemented"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImplementedOutcomeTag {
    Implemented,
}

/// Body of an aborted report — no usable implementation produced. The
/// coordinator does not commit anything and cleans up the worktree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AbortedReport {
    /// Constant: 2.
    pub schema_version: SchemaVersionV2,
    /// Session id (matches the merge phase's session_id).
    pub session_id: String,
    /// Target id (matches the merged target's `id`).
    pub target_id: String,
    /// Status discriminator; literal `"aborted"`.
    pub outcome: AbortedOutcomeTag,
    /// Carried verbatim from the merged target's `delivery_mode`.
    pub delivery_mode: DeliveryMode,

    /// Operator-readable reason for the abort. Free text.
    pub reason: String,

    /// Optional: structural identifier for the gate that failed. Lets
    /// downstream tooling bucket aborts by cause without parsing the
    /// free-text `reason`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_gate: Option<FailedGate>,

    /// Optional: names of the specific tests that failed, when the
    /// abort cause was a test-suite failure. REQUIRED non-empty when
    /// `failed_gate == Some(FailedGate::Nextest)` — the whole point of
    /// surfacing nextest aborts structurally is so next-session triage
    /// can see which tests blocked this attempt.
    ///
    /// Format: fully-qualified nextest test ids (e.g.
    /// `stackslib::chainstate::stacks::tests::test_block_validation`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failing_tests: Option<Vec<String>>,
}

impl AbortedReport {
    /// Cross-field validation.
    pub fn validate(&self) -> Result<(), String> {
        if !is_non_blank(&self.reason) {
            return Err(format!("aborted report `{}`: reason must be non-blank", self.target_id));
        }
        // Invariant: failed_gate=Nextest requires at least one non-blank
        // failing_tests entry. An array of `[""]` or omission both fail
        // (so next-session triage always sees real test ids).
        if matches!(self.failed_gate, Some(FailedGate::Nextest)) {
            let has_real = self
                .failing_tests
                .as_deref()
                .map(has_non_blank)
                .unwrap_or(false);
            if !has_real {
                return Err(format!(
                    "aborted report `{}`: failed_gate=nextest requires at least one non-blank \
                     failing_tests entry (so next-session triage knows which tests blocked this \
                     attempt)",
                    self.target_id
                ));
            }
        }
        Ok(())
    }
}

/// Status discriminator carried on aborted reports. Serializes as the
/// literal string `"aborted"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AbortedOutcomeTag {
    Aborted,
}

/// Structural identifier for which gate failed in an aborted run.
/// Buckets aborts by cause for cross-session analytics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailedGate {
    /// `cargo fmt-stacks` reported drift.
    Fmt,
    /// `cargo clippy-stacks` or `cargo clippy-stackslib` failed.
    Clippy,
    /// `cargo nextest run` reported failures after retries. Requires
    /// non-empty [`AbortedReport::failing_tests`].
    Nextest,
    /// `cargo build --release -p stacks-bench` failed.
    ReleaseBuild,
    /// Target's `target_span` matched an entry in `non-targets.md`.
    NonTargetsMatch,
    /// Agent investigated but found no implementation worth pursuing
    /// (e.g. the hotspot was inherent CPU work with no structural handle).
    NoImplementationFound,
    /// `normal_pr` agent determined parity in consensus-sensitive code
    /// was unprovable and chose not to ship. Phase 2 will redirect this
    /// to the `consensus_review_needed` outcome.
    ParityUnprovable,
    /// One of the inner-loop steps exceeded its time budget (codex
    /// invocation, nextest run, release build, etc.).
    TimeoutHit,
    /// Agent encountered an environmental error (sandbox denial, missing
    /// tool, etc.) it could not work around.
    EnvironmentalError,
}

/// Parity report. Always present on `implemented`; forces the agent to
/// consciously evaluate consensus-sensitivity rather than communicating
/// it implicitly via field presence/absence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParityReport {
    /// True iff the change touches consensus-sensitive code (Clarity VM,
    /// MARF, block validation, cost accounting). Forces explicit
    /// classification — the agent says yes or no, never implicit.
    pub consensus_sensitive: bool,

    /// Concrete parity proofs: what stayed identical and how. Required
    /// non-empty when `consensus_sensitive == true`. Example:
    /// `["MARF root hashes match across Immediate/Deferred/All on
    /// Node4/16/48/256"]`. May be empty when `consensus_sensitive ==
    /// false`.
    pub evidence: Vec<String>,

    /// Test paths that demonstrate parity. Required non-empty when
    /// `consensus_sensitive == true`. Example:
    /// `["chainstate::index::tests::trie_hash_equivalence_node4_node16"]`.
    pub tests: Vec<String>,

    /// Phase 2 readiness slot: when populated, parity could not be fully
    /// proven and the outcome should have been `consensus_review_needed`,
    /// not `implemented`. ALWAYS `null` on `implemented` outcomes; the
    /// validator rejects non-null here on implemented reports.
    pub unproven_risk: Option<String>,
}

/// Test-suite outcome captured during the inner-loop gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestSummary {
    /// Test framework. Always `nextest` in the current pipeline.
    pub framework: TestFramework,
    /// Number of test cases that passed.
    pub passed: u32,
    /// Number of test cases that failed. Must be `0` on `implemented`
    /// (otherwise the agent would have emitted `aborted` with
    /// `failed_gate=nextest`). Kept for symmetry and for future
    /// flake-tolerance reporting.
    pub failed: u32,
    /// Wall-clock duration of the test run in seconds.
    pub duration_secs: f64,
    /// Path to the nextest log (relative to the per-target dir, or
    /// absolute). Lets reviewers verify the test summary.
    pub log_path: String,
}

/// Test framework discriminator. Single-variant for now; reserved for
/// future expansion (e.g. if PoC-mode adds a custom harness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestFramework {
    /// `cargo nextest run`.
    Nextest,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A baseline `implemented` report on `normal_pr` that satisfies
    /// every invariant. Tests mutate clones of this and assert
    /// `validate()` rejects the mutation.
    fn baseline_implemented_normal() -> ImplementedReport {
        ImplementedReport {
            schema_version: SchemaVersionV2,
            session_id: "20260517-200000".to_owned(),
            target_id: "marf-seal-direct-digest".to_owned(),
            outcome: ImplementedOutcomeTag::Implemented,
            delivery_mode: DeliveryMode::NormalPr,
            implementation_summary: "Bypass generic Write in seal hashing".to_owned(),
            deviation_from_proposed_change: None,
            dependency_changes: None,
            test_summary: TestSummary {
                framework: TestFramework::Nextest,
                passed: 1234,
                failed: 0,
                duration_secs: 421.3,
                log_path: "nextest.log".to_owned(),
            },
            clippy_clean: Some(true),
            pr_title: "perf: bypass generic Write in MARF seal hashing".to_owned(),
            parity: ParityReport {
                consensus_sensitive: true,
                evidence: vec!["root hashes match across Immediate/Deferred/All".to_owned()],
                tests: vec!["chainstate::index::tests::trie_hash_equivalence".to_owned()],
                unproven_risk: None,
            },
            hard_fork_followup: None,
        }
    }

    /// Baseline `aborted` report; tests mutate clones to verify rejection.
    fn baseline_aborted() -> AbortedReport {
        AbortedReport {
            schema_version: SchemaVersionV2,
            session_id: "20260517-200000".to_owned(),
            target_id: "marf-seal-direct-digest".to_owned(),
            outcome: AbortedOutcomeTag::Aborted,
            delivery_mode: DeliveryMode::NormalPr,
            reason: "clippy failed; see nextest.log".to_owned(),
            failed_gate: Some(FailedGate::Clippy),
            failing_tests: None,
        }
    }

    #[test]
    fn implemented_baseline_validates() {
        baseline_implemented_normal()
            .validate()
            .expect("baseline implemented should validate");
    }

    #[test]
    fn aborted_baseline_validates() {
        baseline_aborted()
            .validate()
            .expect("baseline aborted should validate");
    }

    // ----- ImplementedReport invariants -----

    #[test]
    fn implemented_rejects_consensus_sensitive_without_evidence() {
        let mut r = baseline_implemented_normal();
        r.parity.evidence.clear();
        let err = r.validate().unwrap_err();
        assert!(err.contains("parity.evidence"), "{err}");
    }

    #[test]
    fn implemented_rejects_consensus_sensitive_with_only_blank_evidence() {
        let mut r = baseline_implemented_normal();
        r.parity.evidence = vec!["".to_owned(), "   ".to_owned(), "\t\n".to_owned()];
        let err = r.validate().unwrap_err();
        assert!(err.contains("parity.evidence"), "{err}");
    }

    #[test]
    fn implemented_rejects_consensus_sensitive_without_tests() {
        let mut r = baseline_implemented_normal();
        r.parity.tests.clear();
        let err = r.validate().unwrap_err();
        assert!(err.contains("parity.tests"), "{err}");
    }

    #[test]
    fn implemented_allows_consensus_sensitive_false_with_empty_arrays() {
        let mut r = baseline_implemented_normal();
        r.parity.consensus_sensitive = false;
        r.parity.evidence.clear();
        r.parity.tests.clear();
        r.validate()
            .expect("consensus_sensitive=false permits empty evidence/tests");
    }

    #[test]
    fn implemented_rejects_unproven_risk_set() {
        let mut r = baseline_implemented_normal();
        r.parity.unproven_risk = Some("ordering may drift on N > 8".to_owned());
        let err = r.validate().unwrap_err();
        assert!(err.contains("unproven_risk"), "{err}");
    }

    #[test]
    fn implemented_rejects_nonzero_failed_tests() {
        let mut r = baseline_implemented_normal();
        r.test_summary.failed = 3;
        let err = r.validate().unwrap_err();
        assert!(err.contains("test_summary.failed"), "{err}");
    }

    #[test]
    fn implemented_rejects_blank_implementation_summary() {
        let mut r = baseline_implemented_normal();
        r.implementation_summary = "   ".to_owned();
        let err = r.validate().unwrap_err();
        assert!(err.contains("implementation_summary"), "{err}");
    }

    #[test]
    fn implemented_rejects_blank_pr_title() {
        let mut r = baseline_implemented_normal();
        r.pr_title = "".to_owned();
        let err = r.validate().unwrap_err();
        assert!(err.contains("pr_title"), "{err}");
    }

    #[test]
    fn implemented_rejects_blank_log_path() {
        let mut r = baseline_implemented_normal();
        r.test_summary.log_path = "\t".to_owned();
        let err = r.validate().unwrap_err();
        assert!(err.contains("log_path"), "{err}");
    }

    #[test]
    fn implemented_rejects_negative_duration() {
        let mut r = baseline_implemented_normal();
        r.test_summary.duration_secs = -1.0;
        let err = r.validate().unwrap_err();
        assert!(err.contains("duration_secs"), "{err}");
    }

    #[test]
    fn implemented_rejects_non_finite_duration() {
        let mut r = baseline_implemented_normal();
        r.test_summary.duration_secs = f64::NAN;
        let err = r.validate().unwrap_err();
        assert!(err.contains("duration_secs"), "{err}");
    }

    #[test]
    fn implemented_normal_pr_requires_clippy_clean_true() {
        let mut r = baseline_implemented_normal();
        r.clippy_clean = Some(false);
        let err = r.validate().unwrap_err();
        assert!(err.contains("clippy_clean=true"), "{err}");
        r.clippy_clean = None;
        let err = r.validate().unwrap_err();
        assert!(err.contains("clippy_clean to be set"), "{err}");
    }

    #[test]
    fn implemented_poc_pr_accepts_any_clippy_value() {
        let mut r = baseline_implemented_normal();
        r.delivery_mode = DeliveryMode::ConsensusPocPr;
        // PoC doesn't require parity proofs by default; relax for this case.
        r.parity.consensus_sensitive = false;
        r.parity.evidence.clear();
        r.parity.tests.clear();
        for value in [None, Some(true), Some(false)] {
            r.clippy_clean = value;
            r.validate()
                .unwrap_or_else(|e| {
                    panic!("PoC + clippy_clean={value:?} should validate; got {e}")
                });
        }
    }

    #[test]
    fn implemented_consensus_issue_is_rejected_unconditionally() {
        let mut r = baseline_implemented_normal();
        r.delivery_mode = DeliveryMode::ConsensusIssue;
        let err = r.validate().unwrap_err();
        assert!(err.contains("consensus_issue"), "{err}");
    }

    // ----- AbortedReport invariants -----

    #[test]
    fn aborted_rejects_blank_reason() {
        let mut r = baseline_aborted();
        r.reason = "   ".to_owned();
        let err = r.validate().unwrap_err();
        assert!(err.contains("reason"), "{err}");
    }

    #[test]
    fn aborted_nextest_requires_failing_tests() {
        let mut r = baseline_aborted();
        r.failed_gate = Some(FailedGate::Nextest);
        // None → reject
        r.failing_tests = None;
        let err = r.validate().unwrap_err();
        assert!(err.contains("failing_tests"), "{err}");
        // Empty vec → reject
        r.failing_tests = Some(vec![]);
        let err = r.validate().unwrap_err();
        assert!(err.contains("failing_tests"), "{err}");
        // Only-blank entries → reject
        r.failing_tests = Some(vec!["".to_owned(), "   ".to_owned()]);
        let err = r.validate().unwrap_err();
        assert!(err.contains("failing_tests"), "{err}");
        // At least one real entry → accept
        r.failing_tests = Some(vec!["".to_owned(), "stackslib::tests::a".to_owned()]);
        r.validate()
            .expect("nextest + non-blank entry should validate");
    }

    #[test]
    fn aborted_non_nextest_gates_dont_require_failing_tests() {
        for gate in [
            FailedGate::Fmt,
            FailedGate::Clippy,
            FailedGate::ReleaseBuild,
            FailedGate::NonTargetsMatch,
            FailedGate::NoImplementationFound,
            FailedGate::ParityUnprovable,
            FailedGate::TimeoutHit,
            FailedGate::EnvironmentalError,
        ] {
            let mut r = baseline_aborted();
            r.failed_gate = Some(gate);
            r.failing_tests = None;
            r.validate()
                .unwrap_or_else(|e| panic!("{gate:?} should not require failing_tests; got {e}"));
        }
    }

    // ----- Untagged enum dispatch + roundtrip -----

    #[test]
    fn roundtrip_implemented_preserves_outcome_tag() {
        let r = OptimizerReport::Implemented(baseline_implemented_normal());
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains(r#""outcome":"implemented""#), "{json}");
        let parsed: OptimizerReport = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(parsed, OptimizerReport::Implemented(_)));
        assert_eq!(parsed.target_id(), "marf-seal-direct-digest");
        assert_eq!(parsed.delivery_mode(), DeliveryMode::NormalPr);
        assert!(parsed.is_implemented());
    }

    #[test]
    fn roundtrip_aborted_preserves_outcome_tag() {
        let r = OptimizerReport::Aborted(baseline_aborted());
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains(r#""outcome":"aborted""#), "{json}");
        let parsed: OptimizerReport = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(parsed, OptimizerReport::Aborted(_)));
        assert!(!parsed.is_implemented());
    }

    #[test]
    fn validate_delegates_to_variant() {
        let mut r = baseline_implemented_normal();
        r.test_summary.failed = 1;
        let wrapped = OptimizerReport::Implemented(r);
        wrapped
            .validate()
            .expect_err("top-level validate should fail on bad implemented");
    }
}
