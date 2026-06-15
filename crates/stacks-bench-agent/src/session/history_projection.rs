//! Shared read-side projection over the operator ledgers.
//!
//! `sessions.jsonl` and `maintain.jsonl` remain the durable source ledgers.
//! This module builds a rebuildable, read-only view for cross-session
//! consumers. It deliberately differs from
//! [`crate::session::maintain::ArtifactProjection`]: maintain's projection is
//! write-side and invocation-scoped, deciding which new lifecycle events to
//! emit. `HistoryProjectionV1` is read-side and shared by consumers:
//! v11 autonomy gates (`session/autonomy.rs`), v12 dedup
//! (`session/dedup.rs`), v13 optimizer memory
//! (`session/optimizer_memory.rs`), v6 history show (`cli/history.rs`), and
//! the chained-session orchestrator.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};

use crate::models::common::DeliveryMode;
use crate::models::maintain_event::{MaintEvent, MaintEventKind};
use crate::models::session_record::{SessionRecord, TargetStatus};
use crate::session::{ledger_reader, maintain_ledger};

/// Result of reading both operator ledgers for projection construction.
#[derive(Debug, Clone, Default)]
pub struct HistoryProjectionReadReport {
    /// Rebuildable read-side projection.
    pub projection: HistoryProjectionV1,
    /// Malformed `sessions.jsonl` lines skipped by the typed reader.
    pub skipped_sessions: Vec<ledger_reader::SkippedLine>,
    /// Malformed `maintain.jsonl` lines skipped by the typed reader.
    pub skipped_maintain: Vec<maintain_ledger::SkippedMaintLine>,
}

/// v1 read-side projection built from parsed ledger records.
#[derive(Debug, Clone, Default)]
pub struct HistoryProjectionV1 {
    sessions: Vec<SessionRecord>,
    attempts_by_signature: BTreeMap<String, Vec<ProjectedAttemptV1>>,
    signatures_by_family: BTreeMap<String, Vec<ProjectedSignatureV1>>,
    artifacts_by_url: BTreeMap<String, ProjectedArtifactStateV1>,
    maintenance_events_by_session: BTreeMap<String, Vec<ProjectedMaintenanceEventV1>>,
}

/// One historical attempt copied from an archived target row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedAttemptV1 {
    /// Archived session id.
    pub session_id: String,
    /// Archived target id. This is the canonical exact fix signature for
    /// archived rows.
    pub target_id: String,
    /// Exact fix signature. Equal to `target_id` for archived rows.
    pub fix_signature: String,
    /// Archived family id.
    pub family_id: String,
    /// Session finished timestamp.
    pub finished_at: String,
    /// Archived target status.
    pub status: TargetStatus,
    /// Archived delivery mode.
    pub delivery_mode: DeliveryMode,
    /// Optional reason code from the archived target row.
    pub reason_code: Option<String>,
    /// Source SHA recorded by the archived session. Missing means source drift
    /// is unknown.
    pub source_sha: Option<String>,
    /// PR URL, when the target published one.
    pub pr_url: Option<String>,
    /// Issue URL, when the target published one.
    pub issue_url: Option<String>,
    /// Target head SHA recorded by the archive, when present.
    pub target_head_sha: Option<String>,
}

/// Attempts grouped by exact fix signature within one family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedSignatureV1 {
    /// Exact fix signature.
    pub fix_signature: String,
    /// Attempts for this signature, newest first.
    pub attempts: Vec<ProjectedAttemptV1>,
}

/// Latest lifecycle state for a PR/issue URL, derived from `maintain.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedArtifactStateV1 {
    /// PR or issue URL.
    pub url: String,
    /// Latest lifecycle event kind.
    pub kind: MaintEventKind,
    /// Observation timestamp for the latest event.
    pub observed_at: String,
    /// Normalized state recorded by maintain.
    pub new_state: String,
    /// PR head SHA when maintain reported one.
    pub head_sha: Option<String>,
}

/// One lifecycle event projected for history/report rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedMaintenanceEventV1 {
    /// Archived session id this lifecycle event belongs to.
    pub session_id: String,
    /// Event kind.
    pub kind: MaintEventKind,
    /// Observation timestamp.
    pub observed_at: String,
    /// Target id, when maintain recorded one.
    pub target_id: Option<String>,
    /// Normalized state recorded by maintain.
    pub new_state: String,
    /// PR URL, when this is a PR lifecycle event.
    pub pr_url: Option<String>,
    /// Issue URL, when this is an issue lifecycle event.
    pub issue_url: Option<String>,
}

impl HistoryProjectionV1 {
    /// Build the v1 projection from already-parsed ledgers.
    ///
    /// This function does not read files, emit stderr, or report skipped
    /// ledger lines. Callers keep that responsibility at the CLI/orchestration
    /// boundary.
    pub fn from_ledgers_v1(sessions: &[SessionRecord], maintain_events: &[MaintEvent]) -> Self {
        let artifacts_by_url = project_artifacts_by_observed_at(maintain_events);
        let mut attempts_by_signature: BTreeMap<String, Vec<ProjectedAttemptV1>> = BTreeMap::new();
        let mut by_family_signature: BTreeMap<String, BTreeMap<String, Vec<ProjectedAttemptV1>>> =
            BTreeMap::new();

        for session in sessions {
            for target in &session.targets {
                let attempt = ProjectedAttemptV1 {
                    session_id: session.id.clone(),
                    target_id: target.id.clone(),
                    fix_signature: target.id.clone(),
                    family_id: target.family_id.clone(),
                    finished_at: session.finished_at.clone(),
                    status: target.status,
                    delivery_mode: target.delivery_mode,
                    reason_code: target.reason_code.clone(),
                    source_sha: session.source_sha.clone(),
                    pr_url: target.pr_url.clone(),
                    issue_url: target.issue_url.clone(),
                    target_head_sha: target.head_sha.clone(),
                };
                attempts_by_signature
                    .entry(attempt.fix_signature.clone())
                    .or_default()
                    .push(attempt.clone());
                by_family_signature
                    .entry(attempt.family_id.clone())
                    .or_default()
                    .entry(attempt.fix_signature.clone())
                    .or_default()
                    .push(attempt);
            }
        }

        for attempts in attempts_by_signature.values_mut() {
            sort_attempts_newest_first(attempts);
        }

        let signatures_by_family = by_family_signature
            .into_iter()
            .map(|(family_id, by_signature)| {
                let mut signatures: Vec<ProjectedSignatureV1> = by_signature
                    .into_iter()
                    .map(|(fix_signature, mut attempts)| {
                        sort_attempts_newest_first(&mut attempts);
                        ProjectedSignatureV1 { fix_signature, attempts }
                    })
                    .collect();
                signatures.sort_by(|a, b| {
                    latest_finished_at(b)
                        .cmp(latest_finished_at(a))
                        .then_with(|| {
                            a.fix_signature
                                .cmp(&b.fix_signature)
                        })
                });
                (family_id, signatures)
            })
            .collect();

        Self {
            sessions: sessions.to_vec(),
            attempts_by_signature,
            signatures_by_family,
            artifacts_by_url,
            maintenance_events_by_session: project_maintenance_events_by_session(maintain_events),
        }
    }

    /// Archived session records in ledger order.
    pub fn sessions(&self) -> &[SessionRecord] {
        &self.sessions
    }

    /// Archived session record by id.
    pub fn session(&self, id: &str) -> Option<&SessionRecord> {
        self.sessions
            .iter()
            .find(|session| session.id == id)
    }

    /// Attempts for an exact fix signature, newest first.
    pub fn attempts_for_signature(&self, fix_signature: &str) -> &[ProjectedAttemptV1] {
        self.attempts_by_signature
            .get(fix_signature)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Historical signatures for a family, newest signature first.
    pub fn signatures_for_family(&self, family_id: &str) -> &[ProjectedSignatureV1] {
        self.signatures_by_family
            .get(family_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// All projected signatures, grouped internally by family but exposed as a
    /// flat read-only iterator for policy views.
    pub fn signatures(&self) -> impl Iterator<Item = &ProjectedSignatureV1> {
        self.signatures_by_family
            .values()
            .flat_map(|signatures| signatures.iter())
    }

    /// Latest lifecycle state for a PR/issue URL.
    pub fn latest_artifact_state(&self, url: &str) -> Option<&ProjectedArtifactStateV1> {
        self.artifacts_by_url.get(url)
    }

    /// Maintenance events for an archived session, sorted by `observed_at`.
    pub fn maintenance_events_for_session(
        &self,
        session_id: &str,
    ) -> &[ProjectedMaintenanceEventV1] {
        self.maintenance_events_by_session
            .get(session_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// Read the current operator ledgers and build a v1 projection.
///
/// This helper returns skipped lines in-band and never writes to stderr.
/// Callers decide whether those skipped lines should become warnings.
pub fn read_operator_projection_v1(
    operator_repo_root: &Path,
) -> Result<HistoryProjectionReadReport> {
    let sessions = ledger_reader::read_all(&operator_repo_root.join("sessions.jsonl"))
        .context("reading sessions.jsonl for history projection")?;
    let maintain = maintain_ledger::read_all(&operator_repo_root.join("maintain.jsonl"))
        .context("reading maintain.jsonl for history projection")?;
    let projection = HistoryProjectionV1::from_ledgers_v1(&sessions.records, &maintain.events);
    Ok(HistoryProjectionReadReport {
        projection,
        skipped_sessions: sessions.skipped,
        skipped_maintain: maintain.skipped,
    })
}

fn project_artifacts_by_observed_at(
    events: &[MaintEvent],
) -> BTreeMap<String, ProjectedArtifactStateV1> {
    let mut sorted: Vec<&MaintEvent> = events.iter().collect();
    sorted.sort_by(|a, b| {
        a.observed_at
            .cmp(&b.observed_at)
            .then_with(|| a.pr_url.cmp(&b.pr_url))
            .then_with(|| a.issue_url.cmp(&b.issue_url))
    });

    let mut out = BTreeMap::new();
    for event in sorted {
        let Some(url) = event
            .pr_url
            .as_ref()
            .or(event.issue_url.as_ref())
        else {
            continue;
        };
        out.insert(
            url.clone(),
            ProjectedArtifactStateV1 {
                url: url.clone(),
                kind: event.kind,
                observed_at: event.observed_at.clone(),
                new_state: event.new_state.clone(),
                head_sha: event.head_sha.clone(),
            },
        );
    }
    out
}

fn project_maintenance_events_by_session(
    events: &[MaintEvent],
) -> BTreeMap<String, Vec<ProjectedMaintenanceEventV1>> {
    let mut by_session: BTreeMap<String, Vec<ProjectedMaintenanceEventV1>> = BTreeMap::new();
    for event in events {
        by_session
            .entry(event.session_id.clone())
            .or_default()
            .push(ProjectedMaintenanceEventV1 {
                session_id: event.session_id.clone(),
                kind: event.kind,
                observed_at: event.observed_at.clone(),
                target_id: event.target_id.clone(),
                new_state: event.new_state.clone(),
                pr_url: event.pr_url.clone(),
                issue_url: event.issue_url.clone(),
            });
    }
    for events in by_session.values_mut() {
        events.sort_by(|a, b| {
            a.observed_at
                .cmp(&b.observed_at)
                .then_with(|| a.target_id.cmp(&b.target_id))
                .then_with(|| a.pr_url.cmp(&b.pr_url))
                .then_with(|| a.issue_url.cmp(&b.issue_url))
        });
    }
    by_session
}

fn sort_attempts_newest_first(attempts: &mut [ProjectedAttemptV1]) {
    attempts.sort_by(|a, b| {
        b.finished_at
            .cmp(&a.finished_at)
            .then_with(|| {
                b.session_id
                    .cmp(&a.session_id)
            })
            .then_with(|| b.target_id.cmp(&a.target_id))
    });
}

fn latest_finished_at(sig: &ProjectedSignatureV1) -> &str {
    sig.attempts
        .first()
        .map(|a| a.finished_at.as_str())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::common::{DeliveryMode, SchemaVersionV1, SchemaVersionV3};
    use crate::models::session_record::{
        SessionRange, SessionRecordKind, SessionStatus, TargetRecord, TargetStatus,
        TargetStatusStage,
    };

    #[test]
    fn empty_inputs_project_empty_views() {
        let projection = HistoryProjectionV1::from_ledgers_v1(&[], &[]);

        assert!(
            projection
                .attempts_for_signature("missing")
                .is_empty()
        );
        assert!(
            projection
                .signatures_for_family("missing")
                .is_empty()
        );
        assert!(
            projection
                .latest_artifact_state("https://example.com")
                .is_none()
        );
    }

    #[test]
    fn observed_at_order_wins_over_file_order() {
        let pr = "https://github.com/owner/repo/pull/1";
        let projection = HistoryProjectionV1::from_ledgers_v1(
            &[],
            &[
                event(MaintEventKind::PrOpen, pr, "2026-06-13T02:00:00Z"),
                event(MaintEventKind::PrMerged, pr, "2026-06-13T01:00:00Z"),
            ],
        );

        let state = projection
            .latest_artifact_state(pr)
            .expect("artifact state");
        assert_eq!(state.kind, MaintEventKind::PrOpen);
        assert_eq!(state.observed_at, "2026-06-13T02:00:00Z");
    }

    #[test]
    fn groups_attempts_by_signature_and_family_newest_first() {
        let projection = HistoryProjectionV1::from_ledgers_v1(
            &[
                session(
                    "s1",
                    "2026-06-11T00:00:00Z",
                    Some("source-1"),
                    vec![target("fix-a", "family-a", TargetStatus::Failed)],
                ),
                session(
                    "s2",
                    "2026-06-12T00:00:00Z",
                    Some("source-2"),
                    vec![
                        target("fix-a", "family-a", TargetStatus::Accepted),
                        target("fix-b", "family-a", TargetStatus::Accepted),
                    ],
                ),
            ],
            &[],
        );

        let attempts = projection.attempts_for_signature("fix-a");
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].session_id, "s2");
        assert_eq!(
            attempts[0]
                .source_sha
                .as_deref(),
            Some("source-2")
        );
        assert_eq!(attempts[1].session_id, "s1");

        let signatures = projection.signatures_for_family("family-a");
        assert_eq!(signatures.len(), 2);
        assert_eq!(signatures[0].fix_signature, "fix-a");
        assert_eq!(signatures[1].fix_signature, "fix-b");
    }

    #[test]
    fn missing_source_sha_is_preserved_as_none() {
        let projection = HistoryProjectionV1::from_ledgers_v1(
            &[session(
                "s1",
                "2026-06-11T00:00:00Z",
                None,
                vec![target("fix-a", "family-a", TargetStatus::Accepted)],
            )],
            &[],
        );

        let attempt = &projection.attempts_for_signature("fix-a")[0];
        assert_eq!(attempt.source_sha, None);
    }

    #[test]
    fn exposes_session_records_by_id() {
        let projection = HistoryProjectionV1::from_ledgers_v1(
            &[session("s1", "2026-06-11T00:00:00Z", None, vec![])],
            &[],
        );

        assert_eq!(projection.sessions().len(), 1);
        assert_eq!(
            projection
                .session("s1")
                .map(|session| session.id.as_str()),
            Some("s1"),
        );
        assert!(
            projection
                .session("missing")
                .is_none()
        );
    }

    #[test]
    fn exposes_session_maintenance_events_sorted_by_observed_at() {
        let pr = "https://github.com/owner/repo/pull/1";
        let mut later = event(MaintEventKind::PrMerged, pr, "2026-06-13T02:00:00Z");
        later.session_id = "s1".to_owned();
        let mut earlier = event(MaintEventKind::PrOpen, pr, "2026-06-13T01:00:00Z");
        earlier.session_id = "s1".to_owned();
        let mut other = event(MaintEventKind::PrOpen, pr, "2026-06-13T00:00:00Z");
        other.session_id = "s2".to_owned();

        let projection = HistoryProjectionV1::from_ledgers_v1(&[], &[later, other, earlier]);

        let events = projection.maintenance_events_for_session("s1");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, MaintEventKind::PrOpen);
        assert_eq!(events[1].kind, MaintEventKind::PrMerged);
        assert!(
            projection
                .maintenance_events_for_session("missing")
                .is_empty()
        );
    }

    fn session(
        id: &str,
        finished_at: &str,
        source_sha: Option<&str>,
        targets: Vec<TargetRecord>,
    ) -> SessionRecord {
        SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: SchemaVersionV3,
            id: id.to_owned(),
            artifact_branch: format!("session/{id}"),
            artifact_sha: "a".repeat(40),
            artifact_url: None,
            started_at: "2026-06-10T00:00:00Z".to_owned(),
            finished_at: finished_at.to_owned(),
            status: SessionStatus::Succeeded,
            failure_phase: None,
            failure_reason: None,
            sbagent_version: "0.1.0".to_owned(),
            sbagent_git_sha: None,
            range: SessionRange {
                start_at: None,
                count: None,
                warmup: None,
                filter: None,
                network: "mainnet".to_owned(),
            },
            baseline_run_ids: vec![],
            phase_durations_secs: Default::default(),
            targets,
            source_url: None,
            source_branch: None,
            source_sha: source_sha.map(str::to_owned),
            source_fetched_at: None,
        }
    }

    fn target(id: &str, family_id: &str, status: TargetStatus) -> TargetRecord {
        TargetRecord {
            id: id.to_owned(),
            family_id: family_id.to_owned(),
            bucket: "block_processing".to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            status,
            status_stage: (status != TargetStatus::Accepted).then_some(TargetStatusStage::Bench),
            reason_code: None,
            head_sha: None,
            pr_url: None,
            issue_url: None,
            bench: None,
        }
    }

    fn event(kind: MaintEventKind, url: &str, observed_at: &str) -> MaintEvent {
        MaintEvent {
            schema_version: SchemaVersionV1,
            kind,
            observed_at: observed_at.to_owned(),
            session_id: "s1".to_owned(),
            target_id: Some("fix-a".to_owned()),
            family_id: Some("family-a".to_owned()),
            fix_signature: Some("fix-a".to_owned()),
            pr_url: Some(url.to_owned()),
            issue_url: None,
            prior_state: None,
            new_state: format!("{kind:?}"),
            head_sha: Some("abc123".to_owned()),
        }
    }
}
