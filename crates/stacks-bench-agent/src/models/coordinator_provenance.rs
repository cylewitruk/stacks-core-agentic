//! `optimize/<target-id>/coordinator-provenance.json` — typed sidecar
//! recording the git state the coordinator observed when committing
//! the optimizer agent's changes. Separate from `OptimizerReport`
//! because ownership differs: agent owns its report, coordinator
//! owns provenance.
//!
//! Written post-commit by `coordinator_commit_if_kept`. Read by the
//! `--resume` gate (verifies `base_sha` against the session's
//! archived baseline) and finalize (propagates SHAs into
//! `Experiment` for the audit trail). Cleared by
//! `clear_optimizer_artifacts` on re-run.

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;
use crate::models::common::{DeliveryMode, SchemaVersionV1};

/// Coordinator-observed git facts captured at the moment of the
/// per-target commit. All four scalar fields are required; the
/// coordinator can always compute them, and absence breaks the
/// audit-trail invariant the file exists to provide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorProvenance {
    pub schema_version: SchemaVersionV1,
    /// Session id this provenance belongs to. Must match the parent
    /// session's id; resume gate cross-checks.
    pub session_id: String,
    /// Target id this provenance belongs to. Must match the parent
    /// `optimize/<target_id>/` directory; resume gate cross-checks.
    pub target_id: String,
    /// Delivery mode the coordinator committed under. Carried for the
    /// same defense-in-depth reason `optimizer-report.json` carries it:
    /// catches mode-drift between agent emission and coordinator
    /// observation. Always `normal_pr` or `consensus_poc_pr` because
    /// `consensus_issue` skips the coordinator commit entirely.
    pub delivery_mode: DeliveryMode,
    /// Parent of `head_sha`. The base the agent's commit was applied
    /// on top of in the per-target clone. Resume gate compares this
    /// to `<session>/baseline/bin/manifest.json.source_sha`; mismatch
    /// indicates the per-target branch was built against a different
    /// source than Phase 0a archived, and the apples-to-apples
    /// invariant doesn't hold.
    pub base_sha: String,
    /// The commit the coordinator produced — agent's changes applied
    /// on top of `base_sha`. Quoted by finalize / Phase 5 PR-writer so
    /// reviewers can resolve "what code was actually benchmarked."
    pub head_sha: String,
    /// Coordinator-supplied commit message. Today always
    /// `perf: optimize <target_id>`. Recorded for audit completeness.
    pub commit_message: String,
}

impl CoordinatorProvenance {
    fn validate_sha(field: &str, sha: &str) -> Result<()> {
        if sha.len() != 40 {
            bail!("{field}: expected 40-char hex SHA, got {} chars", sha.len());
        }
        if !sha
            .chars()
            .all(|c| c.is_ascii_hexdigit())
        {
            bail!("{field}: contains non-hex characters");
        }
        Ok(())
    }

    /// Cross-check this provenance's `session_id`, `target_id`, and
    /// `delivery_mode` against an expected context. Mirrors
    /// [`crate::session::optimizers::validate_report_context`] for the
    /// agent report — without this, a stale sidecar from another
    /// target or another session that happens to share a `base_sha`
    /// could trick `--resume` into skipping a target whose actual
    /// commit was made against a different parent, or feed the wrong
    /// `head_sha` into finalize's `summary.json`.
    pub fn validate_context(
        &self,
        expected_session_id: &str,
        expected_target_id: &str,
        expected_delivery_mode: DeliveryMode,
    ) -> Result<()> {
        if self.session_id != expected_session_id {
            bail!(
                "coordinator-provenance.json for {expected_target_id}: session_id={:?} does not \
                 match expected session_id={expected_session_id:?}",
                self.session_id,
            );
        }
        if self.target_id != expected_target_id {
            bail!(
                "coordinator-provenance.json: target_id={:?} does not match expected \
                 target_id={expected_target_id:?} (sidecar from the wrong target dir?)",
                self.target_id,
            );
        }
        if self.delivery_mode != expected_delivery_mode {
            bail!(
                "coordinator-provenance.json for {expected_target_id}: delivery_mode={:?} does \
                 not match expected delivery_mode={:?}",
                self.delivery_mode,
                expected_delivery_mode,
            );
        }
        Ok(())
    }
}

impl ValidateModel for CoordinatorProvenance {
    /// Validate cross-field invariants beyond what serde + schemars
    /// already enforce. Called eagerly on read so dangling/garbled
    /// sidecars don't reach the resume gate.
    fn validate_model(&self) -> Result<()> {
        if self
            .session_id
            .trim()
            .is_empty()
        {
            bail!("session_id must be non-empty");
        }
        if self
            .target_id
            .trim()
            .is_empty()
        {
            bail!("target_id must be non-empty");
        }
        Self::validate_sha("base_sha", &self.base_sha)?;
        Self::validate_sha("head_sha", &self.head_sha)?;
        if self.base_sha == self.head_sha {
            bail!(
                "base_sha == head_sha ({}); coordinator must observe a HEAD that advanced past \
                 the base after the agent's commit",
                self.base_sha
            );
        }
        if self
            .commit_message
            .trim()
            .is_empty()
        {
            bail!("commit_message must be non-empty");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CoordinatorProvenance {
        CoordinatorProvenance {
            schema_version: SchemaVersionV1,
            session_id: "20260521-051649".to_owned(),
            target_id: "marf-historical-read-node-cache".to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            base_sha: "0ad33704c259da4102b5f195617760003ac89c18".to_owned(),
            head_sha: "f994e6ef03002fb7b1acdc1b5018da40e73b105b".to_owned(),
            commit_message: "perf: optimize marf-historical-read-node-cache".to_owned(),
        }
    }

    #[test]
    fn round_trips_through_serde() {
        let p = sample();
        let s = serde_json::to_string_pretty(&p).unwrap();
        let back: CoordinatorProvenance = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn schema_version_must_be_one() {
        let bad = r#"{"schema_version":2,"session_id":"x","target_id":"y",
            "delivery_mode":"normal_pr","base_sha":"00","head_sha":"01",
            "commit_message":"m"}"#;
        let err = serde_json::from_str::<CoordinatorProvenance>(bad).unwrap_err();
        assert!(
            err.to_string()
                .contains("expected schema_version=1")
        );
    }

    #[test]
    fn validate_rejects_short_sha() {
        let mut p = sample();
        p.base_sha = "abc".to_owned();
        let err = p
            .validate_model()
            .expect_err("short sha");
        assert!(
            err.to_string()
                .contains("40-char hex"),
            "{err}"
        );
    }

    #[test]
    fn validate_rejects_non_hex_sha() {
        let mut p = sample();
        p.head_sha = "zzzz3704c259da4102b5f195617760003ac89c18".to_owned();
        let err = p
            .validate_model()
            .expect_err("non-hex sha");
        assert!(
            err.to_string()
                .contains("non-hex"),
            "{err}"
        );
    }

    #[test]
    fn validate_rejects_base_equals_head() {
        let mut p = sample();
        p.head_sha = p.base_sha.clone();
        let err = p
            .validate_model()
            .expect_err("base==head");
        assert!(
            err.to_string()
                .contains("base_sha == head_sha"),
            "{err}"
        );
    }

    #[test]
    fn validate_rejects_blank_session_id() {
        let mut p = sample();
        p.session_id = "   ".to_owned();
        let err = p
            .validate_model()
            .expect_err("blank session");
        assert!(
            err.to_string()
                .contains("session_id"),
            "{err}"
        );
    }

    #[test]
    fn validate_accepts_correct_shape() {
        sample()
            .validate_model()
            .expect("sample should validate clean");
    }

    #[test]
    fn validate_context_accepts_matching_triple() {
        sample()
            .validate_context(
                "20260521-051649",
                "marf-historical-read-node-cache",
                DeliveryMode::NormalPr,
            )
            .expect("matching context");
    }

    #[test]
    fn validate_context_rejects_session_id_mismatch() {
        let err = sample()
            .validate_context(
                "20991231-235959",
                "marf-historical-read-node-cache",
                DeliveryMode::NormalPr,
            )
            .expect_err("session mismatch");
        assert!(
            err.to_string()
                .contains("session_id"),
            "{err}"
        );
    }

    #[test]
    fn validate_context_rejects_target_id_mismatch() {
        let err = sample()
            .validate_context("20260521-051649", "wrong-target", DeliveryMode::NormalPr)
            .expect_err("target mismatch");
        assert!(
            err.to_string()
                .contains("target_id"),
            "{err}"
        );
    }

    #[test]
    fn validate_context_rejects_delivery_mode_mismatch() {
        let err = sample()
            .validate_context(
                "20260521-051649",
                "marf-historical-read-node-cache",
                DeliveryMode::ConsensusPocPr,
            )
            .expect_err("delivery mode mismatch");
        assert!(
            err.to_string()
                .contains("delivery_mode"),
            "{err}"
        );
    }
}
