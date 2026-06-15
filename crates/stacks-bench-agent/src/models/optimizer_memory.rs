//! `<session>/results/optimizer-memory.json` — compact cross-session memory
//! for the current session's agent prompts.
//!
//! Built from the operator ledgers (`sessions.jsonl` + `maintain.jsonl`) after
//! triage identifies the current candidate families. The artifact is advisory:
//! it helps analyzer / merge / optimizer agents cite prior outcomes, but it
//! never removes targets. Deterministic hard blocking stays in v12 dedup.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;
use crate::models::common::{DeliveryMode, KEBAB_PATTERN, SchemaVersionV1};
use crate::models::maintain_event::MaintEventKind;
use crate::models::session_record::TargetStatus;

/// v1 of the optimizer-memory artifact.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptimizerMemoryJson {
    /// Constant: 1.
    pub schema_version: SchemaVersionV1,
    /// ISO 8601 UTC timestamp when this memory snapshot was generated.
    pub generated_at: String,
    /// Current session source SHA, when source materialization recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_source_sha: Option<String>,
    /// One compact row per current-session family.
    pub families: Vec<OptimizerMemoryFamily>,
}

impl OptimizerMemoryJson {
    /// Read + parse + validate. Returns `None` when the file is missing so
    /// standalone/resume flows can fall back to "no relevant memory" without
    /// special casing.
    pub fn read_optional(path: &Path) -> Result<Option<Self>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(anyhow::Error::new(e)).with_context(|| {
                    format!("reading optimizer-memory.json at {}", path.display())
                });
            }
        };
        let parsed: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parsing optimizer-memory.json at {}", path.display()))?;
        parsed
            .validate_model()
            .with_context(|| format!("validating optimizer-memory.json at {}", path.display()))?;
        Ok(Some(parsed))
    }

    /// Atomic write via temp-file-then-rename. This artifact is derived from
    /// append-only ledgers and may be regenerated on resume; it is not
    /// write-once.
    pub fn write_atomic(&self, path: &Path) -> Result<()> {
        self.validate_model()?;
        let parent = path
            .parent()
            .with_context(|| {
                format!("optimizer-memory.json path has no parent: {}", path.display())
            })?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating optimizer-memory parent {}", parent.display()))?;
        let pretty = serde_json::to_string_pretty(self)
            .context("serializing OptimizerMemoryJson to JSON")?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".optimizer-memory.json.")
            .suffix(".tmp")
            .tempfile_in(parent)
            .with_context(|| format!("opening temp for {}", path.display()))?;
        {
            use std::io::Write as _;
            tmp.write_all(pretty.as_bytes())
                .with_context(|| format!("writing temp for {}", path.display()))?;
            tmp.as_file_mut()
                .sync_all()
                .with_context(|| format!("fsync temp for {}", path.display()))?;
        }
        tmp.persist(path)
            .map_err(|e| {
                anyhow::anyhow!("persisting optimizer-memory.json {}: {}", path.display(), e.error)
            })?;
        Ok(())
    }
}

impl ValidateModel for OptimizerMemoryJson {
    fn validate_model(&self) -> Result<()> {
        if self
            .generated_at
            .trim()
            .is_empty()
        {
            bail!("optimizer-memory.json has empty generated_at");
        }
        for family in &self.families {
            family.validate_model()?;
        }
        Ok(())
    }
}

/// Memory relevant to one current-session family.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptimizerMemoryFamily {
    /// Current-session family id.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: String,
    /// Prior signatures in this family, bounded by relevance/recency.
    pub signatures: Vec<OptimizerMemorySignature>,
    /// Number of same-family historical signatures omitted for compactness.
    pub omitted_sibling_signatures: usize,
}

impl ValidateModel for OptimizerMemoryFamily {
    fn validate_model(&self) -> Result<()> {
        if self
            .family_id
            .trim()
            .is_empty()
        {
            bail!("optimizer-memory family has empty family_id");
        }
        for sig in &self.signatures {
            sig.validate_model()?;
        }
        Ok(())
    }
}

/// Memory for one historical fix signature.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptimizerMemorySignature {
    /// Historical fix signature / archived target id.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub fix_signature: String,
    /// Recent attempts for this exact signature, newest first.
    pub attempts: Vec<OptimizerMemoryAttempt>,
    /// Number of older attempts omitted for compactness.
    pub omitted_attempts: usize,
}

impl ValidateModel for OptimizerMemorySignature {
    fn validate_model(&self) -> Result<()> {
        if self
            .fix_signature
            .trim()
            .is_empty()
        {
            bail!("optimizer-memory signature has empty fix_signature");
        }
        for attempt in &self.attempts {
            attempt.validate_model()?;
        }
        Ok(())
    }
}

/// One archived attempt enriched with the latest known lifecycle state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptimizerMemoryAttempt {
    /// Archived session id.
    pub session_id: String,
    /// Archived target id.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub target_id: String,
    /// Session finished timestamp. Used for deterministic newest-first order.
    pub finished_at: String,
    /// Archived target status.
    pub status: TargetStatus,
    /// Delivery mode used by the archived target.
    pub delivery_mode: DeliveryMode,
    /// Optional reason code from the archived target row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    /// Source SHA recorded by the archived session. Missing means drift
    /// unknown, not "same source."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha: Option<String>,
    /// PR URL, when the archived target opened a PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// Issue URL, when the archived target opened an issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
    /// Latest maintain event kind for the PR/issue URL, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_kind: Option<MaintEventKind>,
    /// Latest normalized maintain state, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_state: Option<String>,
    /// Timestamp of the latest maintain observation, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_observed_at: Option<String>,
    /// Latest PR head SHA, when GitHub reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
}

impl ValidateModel for OptimizerMemoryAttempt {
    fn validate_model(&self) -> Result<()> {
        if self
            .session_id
            .trim()
            .is_empty()
        {
            bail!("optimizer-memory attempt has empty session_id");
        }
        if self
            .target_id
            .trim()
            .is_empty()
        {
            bail!("optimizer-memory attempt has empty target_id");
        }
        if self
            .finished_at
            .trim()
            .is_empty()
        {
            bail!("optimizer-memory attempt has empty finished_at");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::common::SchemaVersionV1;

    #[test]
    fn round_trips_memory_json() {
        let model = sample();
        let json = serde_json::to_string(&model).unwrap();
        let back: OptimizerMemoryJson = serde_json::from_str(&json).unwrap();
        back.validate_model().unwrap();
        assert_eq!(back, model);
    }

    #[test]
    fn rejects_empty_generated_at() {
        let mut model = sample();
        model.generated_at.clear();
        let err = model
            .validate_model()
            .unwrap_err();
        assert!(format!("{err:#}").contains("generated_at"));
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = r#"{"schema_version":1,"generated_at":"2026-06-14T00:00:00Z","families":[],"bogus":true}"#;
        assert!(serde_json::from_str::<OptimizerMemoryJson>(bad).is_err());
    }

    fn sample() -> OptimizerMemoryJson {
        OptimizerMemoryJson {
            schema_version: SchemaVersionV1,
            generated_at: "2026-06-14T00:00:00Z".to_owned(),
            current_source_sha: Some("0ad33704c259da4102b5f195617760003ac89c18".to_owned()),
            families: vec![OptimizerMemoryFamily {
                family_id: "family-a".to_owned(),
                signatures: vec![OptimizerMemorySignature {
                    fix_signature: "fix-a".to_owned(),
                    attempts: vec![OptimizerMemoryAttempt {
                        session_id: "20260611-172955".to_owned(),
                        target_id: "fix-a".to_owned(),
                        finished_at: "2026-06-12T07:57:57Z".to_owned(),
                        status: TargetStatus::Accepted,
                        delivery_mode: DeliveryMode::NormalPr,
                        reason_code: None,
                        source_sha: Some("1234567890abcdef1234567890abcdef12345678".to_owned()),
                        pr_url: Some("https://github.com/owner/repo/pull/1".to_owned()),
                        issue_url: None,
                        lifecycle_kind: Some(MaintEventKind::PrMerged),
                        lifecycle_state: Some("merged".to_owned()),
                        lifecycle_observed_at: Some("2026-06-13T00:00:00Z".to_owned()),
                        head_sha: Some("abc123".to_owned()),
                    }],
                    omitted_attempts: 0,
                }],
                omitted_sibling_signatures: 0,
            }],
        }
    }
}
