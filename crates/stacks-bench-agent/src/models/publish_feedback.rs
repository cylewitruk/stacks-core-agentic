//! `<session>/results/optimize/<target>/publish-feedback.json` —
//! per-target sidecar carrying the GitHub URL that Phase 5 publish
//! produced.
//!
//! Written by [`crate::session::publish`] immediately after the
//! `octocrab.create_pr` / `create_issue` call returns successfully,
//! before any subsequent target's publish runs. This way a Phase 5
//! crash mid-fanout doesn't lose URLs for targets that already
//! published — operators reviewing `sessions.jsonl` can still link
//! the target row to the PR / issue without grepping logs.
//!
//! Archive reads it during
//! [`crate::session::archive::build_target_records`] and lands the
//! URL on [`crate::models::session_record::TargetRecord::pr_url`] /
//! [`TargetRecord::issue_url`](crate::models::session_record::TargetRecord::issue_url).
//! Absent file leaves both fields `None` (legacy sessions + sessions
//! where Phase 5 was skipped).
//!
//! A given file carries either `pr_url` or `issue_url` set, never
//! both: PR-mode targets (`NormalPr` / `ConsensusPocPr`) populate
//! `pr_url`; issue-mode targets (`ConsensusIssue`) populate
//! `issue_url`. The `opened_at` timestamp anchors the moment the
//! GitHub API returned success.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;
use crate::models::common::SchemaVersionV1;

/// v1 of the publish-feedback sidecar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishFeedback {
    /// Constant: 1.
    pub schema_version: SchemaVersionV1,
    /// PR HTML URL, set for `NormalPr` / `ConsensusPocPr` delivery
    /// modes. `None` when the target opened an issue instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// Issue HTML URL, set for `ConsensusIssue` delivery mode.
    /// `None` when the target opened a PR instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
    /// ISO 8601 UTC timestamp at which the GitHub API call returned
    /// success.
    pub opened_at: String,
}

impl ValidateModel for PublishFeedback {
    /// Cross-field invariants:
    ///
    /// 1. Exactly one of `pr_url` / `issue_url` is set — PR-mode and issue-mode
    ///    targets are disjoint (`NormalPr` / `ConsensusPocPr` produce PRs;
    ///    `ConsensusIssue` produces issues). A record with both or neither
    ///    indicates a bug in the writer (or a tampered file on disk) and would
    ///    mint a confusing `TargetRecord` if archive accepted it silently.
    /// 2. `opened_at` is non-empty — an empty timestamp would let archive land
    ///    a row with no provenance anchor.
    fn validate_model(&self) -> Result<()> {
        match (self.pr_url.as_deref(), self.issue_url.as_deref()) {
            (Some(_), Some(_)) => bail!(
                "publish-feedback.json carries both `pr_url` and `issue_url`; exactly one is \
                 required (PR-mode and issue-mode targets are disjoint)",
            ),
            (None, None) => bail!(
                "publish-feedback.json carries neither `pr_url` nor `issue_url`; exactly one is \
                 required",
            ),
            _ => {}
        }
        if self
            .opened_at
            .trim()
            .is_empty()
        {
            bail!(
                "publish-feedback.json has empty `opened_at`; the timestamp is the provenance \
                 anchor"
            );
        }
        Ok(())
    }
}

impl PublishFeedback {
    /// Atomic write via temp-file-then-rename. Write-once: refuses
    /// to overwrite an existing file (re-publishing the same target
    /// should be an explicit re-run, not a silent overwrite).
    /// Validates internal invariants ([`Self::validate_model`])
    /// before writing — refuses to persist a malformed record.
    pub fn write(&self, path: &Path) -> Result<()> {
        self.validate_model()?;
        let parent = path
            .parent()
            .with_context(|| {
                format!("publish-feedback.json path has no parent: {}", path.display())
            })?;
        std::fs::create_dir_all(parent).with_context(|| {
            format!("creating publish-feedback.json parent {}", parent.display())
        })?;
        let pretty =
            serde_json::to_string_pretty(self).context("serializing PublishFeedback to JSON")?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".publish-feedback.json.")
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
        tmp.persist_noclobber(path)
            .map_err(|e| {
                anyhow::anyhow!(
                    "publish-feedback.json at {} already exists; refusing to overwrite: {}",
                    path.display(),
                    e.error,
                )
            })?;
        Ok(())
    }

    /// Read + parse + validate. Returns `None` (not an error) when
    /// the file is missing, so archive can fall back to `None`-valued
    /// `pr_url`/`issue_url` for targets that didn't publish. A
    /// present-but-malformed file (parse failure OR invariant
    /// violation per [`Self::validate_model`]) is a hard error —
    /// silently ignoring a tampered sidecar would let archive
    /// produce a misleading ledger row.
    pub fn read_optional(path: &Path) -> Result<Option<Self>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(anyhow::Error::new(e)).with_context(|| {
                    format!("reading publish-feedback.json at {}", path.display())
                });
            }
        };
        let parsed: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parsing publish-feedback.json at {}", path.display()))?;
        parsed
            .validate_model()
            .with_context(|| format!("validating publish-feedback.json at {}", path.display()))?;
        Ok(Some(parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_record() -> PublishFeedback {
        PublishFeedback {
            schema_version: SchemaVersionV1,
            pr_url: Some("https://github.com/owner/repo/pull/42".to_owned()),
            issue_url: None,
            opened_at: "2026-06-11T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn pr_record_round_trips() {
        let r = pr_record();
        let json = serde_json::to_string(&r).unwrap();
        let back: PublishFeedback = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
        // issue_url absent (skip_serializing_if), not null.
        assert!(!json.contains("\"issue_url\""));
    }

    #[test]
    fn issue_record_round_trips() {
        let r = PublishFeedback {
            schema_version: SchemaVersionV1,
            pr_url: None,
            issue_url: Some("https://github.com/owner/repo/issues/7".to_owned()),
            opened_at: "2026-06-11T12:01:00Z".to_owned(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: PublishFeedback = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
        assert!(!json.contains("\"pr_url\""));
    }

    #[test]
    fn write_then_read_round_trip_via_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("publish-feedback.json");
        let r = pr_record();
        r.write(&path).unwrap();
        let read_back = PublishFeedback::read_optional(&path)
            .unwrap()
            .unwrap();
        assert_eq!(read_back, r);
    }

    #[test]
    fn write_refuses_to_overwrite_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("publish-feedback.json");
        pr_record()
            .write(&path)
            .unwrap();
        let err = pr_record()
            .write(&path)
            .unwrap_err();
        assert!(format!("{err:#}").contains("already exists"));
    }

    #[test]
    fn read_optional_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp
            .path()
            .join("does-not-exist.json");
        assert!(
            PublishFeedback::read_optional(&missing)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = r#"{"schema_version":1,"opened_at":"2026-06-11T12:00:00Z","bogus":42}"#;
        assert!(serde_json::from_str::<PublishFeedback>(bad).is_err());
    }

    #[test]
    fn validate_rejects_both_urls_set() {
        let r = PublishFeedback {
            schema_version: SchemaVersionV1,
            pr_url: Some("https://github.com/o/r/pull/1".to_owned()),
            issue_url: Some("https://github.com/o/r/issues/1".to_owned()),
            opened_at: "2026-06-11T12:00:00Z".to_owned(),
        };
        let err = r
            .validate_model()
            .unwrap_err();
        assert!(format!("{err:#}").contains("both"));
    }

    #[test]
    fn validate_rejects_neither_url_set() {
        let r = PublishFeedback {
            schema_version: SchemaVersionV1,
            pr_url: None,
            issue_url: None,
            opened_at: "2026-06-11T12:00:00Z".to_owned(),
        };
        let err = r
            .validate_model()
            .unwrap_err();
        assert!(format!("{err:#}").contains("neither"));
    }

    #[test]
    fn validate_rejects_empty_opened_at() {
        let r = PublishFeedback {
            schema_version: SchemaVersionV1,
            pr_url: Some("https://github.com/o/r/pull/1".to_owned()),
            issue_url: None,
            opened_at: "   ".to_owned(),
        };
        let err = r
            .validate_model()
            .unwrap_err();
        assert!(format!("{err:#}").contains("opened_at"));
    }

    /// `write` refuses to persist an invalid record so a buggy
    /// caller can't poison the sidecar on disk.
    #[test]
    fn write_validates_before_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("publish-feedback.json");
        let bad = PublishFeedback {
            schema_version: SchemaVersionV1,
            pr_url: None,
            issue_url: None,
            opened_at: "2026-06-11T12:00:00Z".to_owned(),
        };
        let err = bad.write(&path).unwrap_err();
        assert!(format!("{err:#}").contains("neither"));
        assert!(!path.exists(), "no file should be written on validation failure");
    }

    /// A tampered sidecar (both URLs set) on disk fails
    /// `read_optional` rather than silently propagating into
    /// `TargetRecord`.
    #[test]
    fn read_optional_rejects_tampered_file_with_both_urls() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("publish-feedback.json");
        // Hand-write a doubly-set file (bypassing the typed writer).
        std::fs::write(
            &path,
            r#"{"schema_version":1,"pr_url":"https://github.com/o/r/pull/1","issue_url":"https://github.com/o/r/issues/1","opened_at":"2026-06-11T12:00:00Z"}"#,
        )
        .unwrap();
        let err = PublishFeedback::read_optional(&path).unwrap_err();
        assert!(format!("{err:#}").contains("validating"));
    }
}
