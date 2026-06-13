//! Typed shape of one line in `maintain.jsonl` — the operator repo's
//! append-only ledger of post-publish GitHub lifecycle observations.
//!
//! `maintain.jsonl` is a sibling to `sessions.jsonl` on operator `main`.
//! It is written by `sbagent maintain`, never by `publish`, and records
//! observations such as "PR first seen open", "PR merged", or "branch
//! deleted while PR is still open".

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;
use crate::models::common::{KEBAB_PATTERN, SchemaVersionV1};

/// Discriminator for one maintenance observation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MaintEventKind {
    /// First time `sbagent maintain` observes a PR as open. Publish
    /// creates the PR; maintain records the first observation.
    PrOpen,
    /// PR merged into its base branch.
    PrMerged,
    /// PR closed without merging.
    PrClosedUnmerged,
    /// PR stayed open past the configured stale threshold.
    PrStale,
    /// PR head sha changed after a prior observation.
    PrForcePushed,
    /// PR is open but its head ref no longer exists.
    PrBranchDeleted,
    /// First time `sbagent maintain` observes an issue as open.
    /// Publish creates the issue; maintain records the first
    /// observation.
    IssueOpen,
    /// Issue closed.
    IssueClosed,
}

impl MaintEventKind {
    /// True iff this kind represents a terminal artifact state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::PrMerged | Self::PrClosedUnmerged | Self::IssueClosed)
    }

    /// True iff this kind belongs to a PR artifact.
    pub fn is_pr(self) -> bool {
        matches!(
            self,
            Self::PrOpen
                | Self::PrMerged
                | Self::PrClosedUnmerged
                | Self::PrStale
                | Self::PrForcePushed
                | Self::PrBranchDeleted
        )
    }

    /// True iff this kind belongs to an issue artifact.
    pub fn is_issue(self) -> bool {
        matches!(self, Self::IssueOpen | Self::IssueClosed)
    }
}

/// One maintenance ledger line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaintEvent {
    /// Constant: 1.
    pub schema_version: SchemaVersionV1,
    /// Observation kind.
    pub kind: MaintEventKind,
    /// ISO 8601 UTC timestamp when maintain observed the state.
    pub observed_at: String,
    /// Session id whose archived target produced the PR/issue.
    pub session_id: String,
    /// Target id. `None` only for future session-level events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub target_id: Option<String>,
    /// Family id copied from the archived target row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: Option<String>,
    /// Fix signature copied from the archived target row when
    /// available. v10 uses the target id as the best available
    /// signature; later dedup work may tighten this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub fix_signature: Option<String>,
    /// PR URL for PR lifecycle events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// Issue URL for issue lifecycle events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
    /// Previous normalized state, or `None` for initial `PrOpen` /
    /// `IssueOpen` observations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_state: Option<String>,
    /// New normalized state represented by this event.
    pub new_state: String,
    /// PR head sha for PR events when GitHub reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
}

impl MaintEvent {
    /// Read one `maintain.jsonl` line. v1 is the only supported
    /// schema today; the method mirrors `SessionRecord::from_ledger_line`
    /// so future promotions have one obvious place to live.
    pub fn from_ledger_line(line: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("maintain ledger line is not valid JSON: {e}"))?;
        let version = value
            .get("schema_version")
            .and_then(|s| s.as_u64())
            .unwrap_or(1);
        match version {
            1 => {
                let event: Self = serde_json::from_value(value)
                    .map_err(|e| anyhow::anyhow!("v1 maintain ledger line: {e}"))?;
                event.validate_model()?;
                Ok(event)
            }
            other => {
                bail!("unsupported maintain.jsonl schema_version {other}; expected 1");
            }
        }
    }
}

impl ValidateModel for MaintEvent {
    fn validate_model(&self) -> Result<()> {
        if self
            .observed_at
            .trim()
            .is_empty()
        {
            bail!("maintain event has empty observed_at");
        }
        if self
            .session_id
            .trim()
            .is_empty()
        {
            bail!("maintain event has empty session_id");
        }
        if self
            .new_state
            .trim()
            .is_empty()
        {
            bail!("maintain event has empty new_state");
        }
        match (self.kind.is_pr(), self.pr_url.as_deref(), self.issue_url.as_deref()) {
            (true, Some(_), None) => {}
            (true, ..) => bail!("PR maintenance events require pr_url and no issue_url"),
            (false, None, Some(_)) if self.kind.is_issue() => {}
            (false, ..) if self.kind.is_issue() => {
                bail!("issue maintenance events require issue_url and no pr_url")
            }
            _ => {}
        }
        if matches!(self.kind, MaintEventKind::PrForcePushed)
            && self
                .head_sha
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            bail!("PrForcePushed events require head_sha");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_event() -> MaintEvent {
        MaintEvent {
            schema_version: SchemaVersionV1,
            kind: MaintEventKind::PrOpen,
            observed_at: "2026-06-13T12:00:00Z".to_owned(),
            session_id: "20260611-172955".to_owned(),
            target_id: Some("marf-cache".to_owned()),
            family_id: Some("marf".to_owned()),
            fix_signature: Some("marf-cache".to_owned()),
            pr_url: Some("https://github.com/owner/repo/pull/1".to_owned()),
            issue_url: None,
            prior_state: None,
            new_state: "open".to_owned(),
            head_sha: Some("abc123".to_owned()),
        }
    }

    #[test]
    fn round_trips_pr_event() {
        let event = pr_event();
        let json = serde_json::to_string(&event).unwrap();
        let back = MaintEvent::from_ledger_line(&json).unwrap();
        assert_eq!(back, event);
        assert!(!json.contains("\"issue_url\""));
    }

    #[test]
    fn rejects_pr_without_pr_url() {
        let mut event = pr_event();
        event.pr_url = None;
        let err = event
            .validate_model()
            .unwrap_err();
        assert!(format!("{err:#}").contains("require pr_url"));
    }

    #[test]
    fn rejects_issue_with_pr_url() {
        let mut event = pr_event();
        event.kind = MaintEventKind::IssueOpen;
        event.issue_url = Some("https://github.com/owner/repo/issues/1".to_owned());
        let err = event
            .validate_model()
            .unwrap_err();
        assert!(format!("{err:#}").contains("require issue_url"));
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let bad = r#"{"schema_version":2,"kind":"pr_open","observed_at":"x","session_id":"s","pr_url":"https://x","new_state":"open"}"#;
        let err = MaintEvent::from_ledger_line(bad).unwrap_err();
        assert!(format!("{err:#}").contains("unsupported maintain.jsonl schema_version 2"));
    }
}
