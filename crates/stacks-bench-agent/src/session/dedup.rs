//! Cross-session exact-signature dedup projection.
//!
//! v12 consumes the durable ledgers that already exist:
//! `sessions.jsonl` tells us which fix signatures were archived, while
//! `maintain.jsonl` tells us what happened to their PRs/issues after
//! publish. The projection is deterministic and read-only; merge uses it
//! to precompute `rejected_by_merge` rows before optimizer fan-out.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Serialize;

use crate::models::analyze::AnalyzerTarget;
use crate::models::maintain_event::{MaintEvent, MaintEventKind};
use crate::models::session_record::{SessionRecord, TargetStatus};
use crate::models::targets::RejectedByMerge;
use crate::session::history_projection::{HistoryProjectionV1, ProjectedArtifactStateV1};

/// Stable prefix used for deterministic dedup rejections.
pub const DEDUP_REASON_OPEN_PR: &str = "dedup:open-pr";
/// Stable prefix used for deterministic dedup rejections.
pub const DEDUP_REASON_OPEN_ISSUE: &str = "dedup:open-issue";
/// Stable prefix used for deterministic dedup rejections.
pub const DEDUP_REASON_MERGED: &str = "dedup:merged";
/// Stable prefix used for deterministic dedup rejections.
pub const DEDUP_REASON_REPEATED_FAILURE: &str = "dedup:repeated-failure";

/// True iff `reason` is one of v12's closed dedup categories.
pub fn is_dedup_reason(reason: &str) -> bool {
    matches!(
        reason,
        DEDUP_REASON_OPEN_PR
            | DEDUP_REASON_OPEN_ISSUE
            | DEDUP_REASON_MERGED
            | DEDUP_REASON_REPEATED_FAILURE
    )
}

/// One deterministic dedup decision for an analyzer-emitted target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DedupDecision {
    /// Family id of the source analysis.
    pub family_id: String,
    /// Zero-based index into the source analysis's `targets[]`.
    pub target_index: usize,
    /// Exact structural-fix signature that matched history.
    pub fix_signature: String,
    /// Stable closed reason (`dedup:*`).
    pub reason: String,
    /// Short human-facing context for `final-message.md`.
    pub detail: String,
}

impl DedupDecision {
    /// Convert into the existing merge rejection shape.
    pub fn to_rejected_by_merge(&self) -> RejectedByMerge {
        RejectedByMerge {
            family_id: self.family_id.clone(),
            target_index: self.target_index,
            reason: self.reason.clone(),
        }
    }
}

/// Read-only projection keyed by exact fix signature.
#[derive(Debug, Default)]
pub struct DedupProjection {
    by_signature: BTreeMap<String, SignatureState>,
    threshold: usize,
}

#[derive(Debug, Default)]
struct SignatureState {
    open_pr: Option<String>,
    open_issue: Option<String>,
    merged_pr: Option<String>,
    unsuccessful_attempts: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactState {
    Open,
    Merged,
    ClosedUnmerged,
    Stale,
    BranchDeleted,
    IssueOpen,
    IssueClosed,
}

impl DedupProjection {
    /// Build a projection from already-read ledgers. `threshold == 0`
    /// disables repeated-failure blocking while preserving open/merged
    /// blocking.
    pub fn from_ledgers(
        sessions: &[SessionRecord],
        maintain_events: &[MaintEvent],
        threshold: usize,
    ) -> Self {
        let history = HistoryProjectionV1::from_ledgers_v1(sessions, maintain_events);
        Self::from_history_projection(&history, threshold)
    }

    /// Build a dedup policy projection from the shared read-side history
    /// projection.
    pub fn from_history_projection(history: &HistoryProjectionV1, threshold: usize) -> Self {
        let mut out = Self {
            by_signature: BTreeMap::new(),
            threshold,
        };

        for signature in history.signatures() {
            for attempt in history.attempts_for_signature(&signature.fix_signature) {
                let state = out
                    .by_signature
                    .entry(attempt.fix_signature.clone())
                    .or_default();
                if unsuccessful_from_archived_status(attempt.status) {
                    state.unsuccessful_attempts += 1;
                }

                if let Some(pr_url) = &attempt.pr_url {
                    match history
                        .latest_artifact_state(pr_url)
                        .map(artifact_state_from_projection)
                        .unwrap_or(ArtifactState::Open)
                    {
                        ArtifactState::Open => {
                            state
                                .open_pr
                                .get_or_insert_with(|| pr_url.clone());
                        }
                        ArtifactState::Merged => {
                            state.merged_pr = Some(pr_url.clone());
                        }
                        ArtifactState::ClosedUnmerged => {
                            state.unsuccessful_attempts += 1;
                        }
                        ArtifactState::Stale | ArtifactState::BranchDeleted => {}
                        ArtifactState::IssueOpen | ArtifactState::IssueClosed => {}
                    }
                }

                if let Some(issue_url) = &attempt.issue_url {
                    match history
                        .latest_artifact_state(issue_url)
                        .map(artifact_state_from_projection)
                        .unwrap_or(ArtifactState::IssueOpen)
                    {
                        ArtifactState::IssueOpen => {
                            state
                                .open_issue
                                .get_or_insert_with(|| issue_url.clone());
                        }
                        ArtifactState::IssueClosed => {
                            state.unsuccessful_attempts += 1;
                        }
                        _ => {}
                    }
                }
            }
        }

        out
    }

    /// Return a deterministic decision for this analyzer target, if
    /// history blocks it.
    pub fn decision_for(
        &self,
        family_id: &str,
        target_index: usize,
        target: &AnalyzerTarget,
    ) -> Option<DedupDecision> {
        let state = self
            .by_signature
            .get(&target.fix_signature)?;
        if let Some(url) = &state.open_pr {
            return Some(DedupDecision {
                family_id: family_id.to_owned(),
                target_index,
                fix_signature: target.fix_signature.clone(),
                reason: DEDUP_REASON_OPEN_PR.to_owned(),
                detail: format!("matching open PR: {url}"),
            });
        }
        if let Some(url) = &state.open_issue {
            return Some(DedupDecision {
                family_id: family_id.to_owned(),
                target_index,
                fix_signature: target.fix_signature.clone(),
                reason: DEDUP_REASON_OPEN_ISSUE.to_owned(),
                detail: format!("matching open issue: {url}"),
            });
        }
        if let Some(url) = &state.merged_pr {
            return Some(DedupDecision {
                family_id: family_id.to_owned(),
                target_index,
                fix_signature: target.fix_signature.clone(),
                reason: DEDUP_REASON_MERGED.to_owned(),
                detail: format!("matching merged PR: {url}"),
            });
        }
        if self.threshold > 0 && state.unsuccessful_attempts >= self.threshold {
            return Some(DedupDecision {
                family_id: family_id.to_owned(),
                target_index,
                fix_signature: target.fix_signature.clone(),
                reason: DEDUP_REASON_REPEATED_FAILURE.to_owned(),
                detail: format!("{} unsuccessful archived attempt(s)", state.unsuccessful_attempts),
            });
        }
        None
    }
}

fn unsuccessful_from_archived_status(status: TargetStatus) -> bool {
    matches!(status, TargetStatus::Rejected | TargetStatus::Failed | TargetStatus::Aborted)
}

fn artifact_state_from_projection(state: &ProjectedArtifactStateV1) -> ArtifactState {
    match state.kind {
        MaintEventKind::PrOpen | MaintEventKind::PrForcePushed => ArtifactState::Open,
        MaintEventKind::PrMerged => ArtifactState::Merged,
        MaintEventKind::PrClosedUnmerged => ArtifactState::ClosedUnmerged,
        MaintEventKind::PrStale => ArtifactState::Stale,
        MaintEventKind::PrBranchDeleted => ArtifactState::BranchDeleted,
        MaintEventKind::IssueOpen => ArtifactState::IssueOpen,
        MaintEventKind::IssueClosed => ArtifactState::IssueClosed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::common::{
        Bucket, DeliveryMode, Hotspot, ImprovementVector, Risk, SchemaVersionV1, SchemaVersionV3,
    };
    use crate::models::session_record::{
        SessionRange, SessionRecordKind, SessionStatus, TargetRecord, TargetStatusStage,
    };

    fn projection_with(
        target: TargetRecord,
        events: Vec<MaintEvent>,
        threshold: usize,
    ) -> DedupProjection {
        DedupProjection::from_ledgers(&[session(vec![target])], &events, threshold)
    }

    fn decision(projection: &DedupProjection, fix_signature: &str) -> Option<DedupDecision> {
        projection.decision_for("family-a", 0, &analyzer_target(fix_signature))
    }

    #[test]
    fn open_pr_blocks() {
        let pr = "https://github.com/owner/repo/pull/1";
        let p = projection_with(target_with_pr("fix-a", TargetStatus::Accepted, pr), vec![], 3);
        let d = decision(&p, "fix-a").expect("open PR should block");
        assert_eq!(d.reason, DEDUP_REASON_OPEN_PR);
        assert!(d.detail.contains(pr));
    }

    #[test]
    fn merged_pr_blocks() {
        let pr = "https://github.com/owner/repo/pull/1";
        let p = projection_with(
            target_with_pr("fix-a", TargetStatus::Accepted, pr),
            vec![event(MaintEventKind::PrMerged, pr, "2026-06-14T01:00:00Z")],
            3,
        );
        assert_eq!(
            decision(&p, "fix-a")
                .unwrap()
                .reason,
            DEDUP_REASON_MERGED
        );
    }

    #[test]
    fn stale_open_pr_does_not_block() {
        let pr = "https://github.com/owner/repo/pull/1";
        let p = projection_with(
            target_with_pr("fix-a", TargetStatus::Accepted, pr),
            vec![event(MaintEventKind::PrStale, pr, "2026-06-14T01:00:00Z")],
            3,
        );
        assert!(decision(&p, "fix-a").is_none());
    }

    #[test]
    fn closed_unmerged_counts_toward_threshold() {
        let pr = "https://github.com/owner/repo/pull/1";
        let p = projection_with(
            target_with_pr("fix-a", TargetStatus::Accepted, pr),
            vec![event(MaintEventKind::PrClosedUnmerged, pr, "2026-06-14T01:00:00Z")],
            1,
        );
        assert_eq!(
            decision(&p, "fix-a")
                .unwrap()
                .reason,
            DEDUP_REASON_REPEATED_FAILURE
        );
    }

    #[test]
    fn archived_failed_status_counts_toward_threshold() {
        let p = projection_with(target_without_artifact("fix-a", TargetStatus::Failed), vec![], 1);
        assert_eq!(
            decision(&p, "fix-a")
                .unwrap()
                .reason,
            DEDUP_REASON_REPEATED_FAILURE
        );
    }

    #[test]
    fn below_threshold_does_not_block() {
        let p =
            projection_with(target_without_artifact("fix-a", TargetStatus::Rejected), vec![], 2);
        assert!(decision(&p, "fix-a").is_none());
    }

    #[test]
    fn exact_signature_only() {
        let p = projection_with(target_without_artifact("fix-a", TargetStatus::Failed), vec![], 1);
        assert!(decision(&p, "fix-b").is_none());
    }

    #[test]
    fn observed_at_order_wins_over_file_order() {
        let pr = "https://github.com/owner/repo/pull/1";
        let p = projection_with(
            target_with_pr("fix-a", TargetStatus::Accepted, pr),
            vec![
                event(MaintEventKind::PrStale, pr, "2026-06-14T02:00:00Z"),
                event(MaintEventKind::PrOpen, pr, "2026-06-14T01:00:00Z"),
            ],
            3,
        );
        assert!(
            decision(&p, "fix-a").is_none(),
            "latest by observed_at is stale, not file-order PrOpen"
        );
    }

    #[test]
    fn force_push_after_stale_reblocks_as_open() {
        let pr = "https://github.com/owner/repo/pull/1";
        let p = projection_with(
            target_with_pr("fix-a", TargetStatus::Accepted, pr),
            vec![
                event(MaintEventKind::PrStale, pr, "2026-06-14T01:00:00Z"),
                event(MaintEventKind::PrForcePushed, pr, "2026-06-14T02:00:00Z"),
            ],
            3,
        );
        assert_eq!(
            decision(&p, "fix-a")
                .unwrap()
                .reason,
            DEDUP_REASON_OPEN_PR
        );
    }

    #[test]
    fn consensus_issue_open_blocks() {
        let issue = "https://github.com/owner/repo/issues/1";
        let p =
            projection_with(target_with_issue("fix-a", TargetStatus::Accepted, issue), vec![], 3);
        assert_eq!(
            decision(&p, "fix-a")
                .unwrap()
                .reason,
            DEDUP_REASON_OPEN_ISSUE
        );
    }

    fn session(targets: Vec<TargetRecord>) -> SessionRecord {
        SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: SchemaVersionV3,
            id: "20260611-172955".to_owned(),
            artifact_branch: "session/20260611-172955".to_owned(),
            artifact_sha: "abc123".to_owned(),
            artifact_url: None,
            started_at: "2026-06-11T17:29:55Z".to_owned(),
            finished_at: "2026-06-12T07:57:57Z".to_owned(),
            status: SessionStatus::Succeeded,
            failure_phase: None,
            failure_reason: None,
            sbagent_version: "0.1.0".to_owned(),
            sbagent_git_sha: None,
            range: SessionRange {
                start_at: Some(1),
                count: Some(1),
                warmup: Some(0),
                filter: None,
                network: "mainnet".to_owned(),
            },
            baseline_run_ids: vec![100, 101],
            phase_durations_secs: Default::default(),
            targets,
            source_url: None,
            source_branch: None,
            source_sha: None,
            source_fetched_at: None,
        }
    }

    fn target_without_artifact(id: &str, status: TargetStatus) -> TargetRecord {
        let status_stage = (status != TargetStatus::Accepted).then_some(TargetStatusStage::Bench);
        TargetRecord {
            id: id.to_owned(),
            family_id: "family-a".to_owned(),
            bucket: "block_processing".to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            status,
            status_stage,
            reason_code: None,
            head_sha: None,
            pr_url: None,
            issue_url: None,
            bench: None,
        }
    }

    fn target_with_pr(id: &str, status: TargetStatus, pr: &str) -> TargetRecord {
        let mut t = target_without_artifact(id, status);
        t.pr_url = Some(pr.to_owned());
        t
    }

    fn target_with_issue(id: &str, status: TargetStatus, issue: &str) -> TargetRecord {
        let mut t = target_without_artifact(id, status);
        t.delivery_mode = DeliveryMode::ConsensusIssue;
        t.issue_url = Some(issue.to_owned());
        t
    }

    fn event(kind: MaintEventKind, url: &str, observed_at: &str) -> MaintEvent {
        let is_issue = kind.is_issue();
        MaintEvent {
            schema_version: SchemaVersionV1,
            kind,
            observed_at: observed_at.to_owned(),
            session_id: "20260611-172955".to_owned(),
            target_id: Some("fix-a".to_owned()),
            family_id: Some("family-a".to_owned()),
            fix_signature: Some("fix-a".to_owned()),
            pr_url: (!is_issue).then(|| url.to_owned()),
            issue_url: is_issue.then(|| url.to_owned()),
            prior_state: None,
            new_state: "state".to_owned(),
            head_sha: Some("abc123".to_owned()),
        }
    }

    fn analyzer_target(fix_signature: &str) -> AnalyzerTarget {
        AnalyzerTarget {
            target_span: "span".to_owned(),
            bucket: Bucket::BlockProcessing,
            fix_signature: fix_signature.to_owned(),
            hotspot: Hotspot {
                span: "span".to_owned(),
                self_wall_us: 1,
                total_wall_us: 2,
                calls: 1,
                location: "src/lib.rs:1".to_owned(),
            },
            files: vec!["src/lib.rs".to_owned()],
            evidence: "evidence".to_owned(),
            evidence_queries: vec![],
            proposed_change: "change".to_owned(),
            expected_improvement: ImprovementVector {
                tx_latency: 1.0,
                tenure_throughput: 0.0,
                commit_time: 0.0,
            },
            risk: Risk::Low,
            verification_plan: "verify".to_owned(),
            verification_replay: None,
            consensus_breaking: true,
            breakage_class: Some(crate::models::common::BreakageClass::ClarityVmBehavior),
            poc_implementable: Some(false),
            poc_test_scope: None,
            consensus_writeup: Some("writeup".to_owned()),
        }
    }
}
