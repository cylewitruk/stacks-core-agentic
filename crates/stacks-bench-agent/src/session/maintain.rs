//! Maintenance reconciliation: project `maintain.jsonl`, query GitHub
//! for current PR/issue state, and derive append-only lifecycle events.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};

use crate::models::common::SchemaVersionV1;
use crate::models::maintain_event::{MaintEvent, MaintEventKind};
use crate::models::session_record::{SessionRecord, TargetRecord};
use crate::session::maintain_ledger::MaintLedgerReport;
use crate::session::publish::{GhClient, IssueLifecycleState, PrState};
use crate::settings::MaintainSettings;

/// Reconcile sessions + existing maintenance events against GitHub.
pub struct MaintainReconciler<'a, G: GhClient> {
    /// GitHub client.
    pub gh: &'a G,
    /// Maintain settings.
    pub settings: &'a MaintainSettings,
    /// Injected clock for deterministic stale tests.
    pub now: SystemTime,
}

/// Reconciler output.
#[derive(Debug, Default)]
pub struct ReconcileOutcome {
    /// Newly-derived events, in scan order.
    pub new_events: Vec<MaintEvent>,
    /// Artifacts skipped because of `--limit` or rate-limit floor.
    pub deferred: Vec<DeferredArtifact>,
    /// Number of GitHub state reads attempted.
    pub queried: usize,
}

/// Artifact that should be retried by a later maintain invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredArtifact {
    /// PR/issue URL.
    pub url: String,
    /// Reason for deferral.
    pub reason: String,
}

/// True iff at least one archived PR/issue artifact is non-terminal
/// according to `maintain.jsonl` and therefore needs a GitHub query.
pub fn needs_github_queries(
    sessions: &[SessionRecord],
    maintain: &MaintLedgerReport,
) -> Result<bool> {
    let projections = project_events(&maintain.events);
    Ok(candidates_from_sessions(sessions)?
        .iter()
        .any(|candidate| {
            !projections
                .get(&candidate.url)
                .map(|p| p.terminal)
                .unwrap_or(false)
        }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactKind {
    Pr,
    Issue,
}

#[derive(Debug, Clone)]
struct ArtifactCandidate<'a> {
    kind: ArtifactKind,
    session: &'a SessionRecord,
    target: &'a TargetRecord,
    url: String,
    owner: String,
    repo: String,
    number: u64,
}

#[derive(Debug, Clone, Default)]
struct ArtifactProjection {
    terminal: bool,
    head_sha: Option<String>,
    head_ref_deleted_emitted: bool,
    stale_emitted: bool,
    last_updated_at: Option<String>,
}

impl<G: GhClient> MaintainReconciler<'_, G> {
    /// Reconcile archived sessions against GitHub.
    pub async fn reconcile(
        &self,
        sessions: &[SessionRecord],
        maintain: &MaintLedgerReport,
        limit: usize,
    ) -> Result<ReconcileOutcome> {
        let projections = project_events(&maintain.events);
        let candidates = candidates_from_sessions(sessions)?;
        let mut outcome = ReconcileOutcome::default();

        for (i, candidate) in candidates.iter().enumerate() {
            if projections
                .get(&candidate.url)
                .map(|p| p.terminal)
                .unwrap_or(false)
            {
                continue;
            }
            if outcome.queried >= limit {
                outcome
                    .deferred
                    .push(DeferredArtifact {
                        url: candidate.url.clone(),
                        reason: "limit".to_owned(),
                    });
                continue;
            }
            outcome.queried += 1;
            let projection = projections.get(&candidate.url);
            let mut event = match candidate.kind {
                ArtifactKind::Pr => {
                    let read = self
                        .gh
                        .query_pr_state(&candidate.owner, &candidate.repo, candidate.number)
                        .await
                        .with_context(|| format!("querying PR {}", candidate.url))?;
                    if read
                        .rate_limit
                        .below_floor_pct(
                            self.settings
                                .secondary_rate_limit_floor_pct,
                        )
                    {
                        defer_tail(&mut outcome, &candidates, i, "rate-limit-floor");
                        break;
                    }
                    derive_pr_event(candidate, projection, &read.state, self.now, self.settings)?
                }
                ArtifactKind::Issue => {
                    let read = self
                        .gh
                        .query_issue_state(&candidate.owner, &candidate.repo, candidate.number)
                        .await
                        .with_context(|| format!("querying issue {}", candidate.url))?;
                    if read
                        .rate_limit
                        .below_floor_pct(
                            self.settings
                                .secondary_rate_limit_floor_pct,
                        )
                    {
                        defer_tail(&mut outcome, &candidates, i, "rate-limit-floor");
                        break;
                    }
                    derive_issue_event(candidate, projection, &read.state)?
                }
            };
            if let Some(ref mut event) = event {
                event.observed_at = format_system_time(self.now);
                outcome
                    .new_events
                    .push(event.clone());
            }
        }
        Ok(outcome)
    }
}

fn defer_tail(
    outcome: &mut ReconcileOutcome,
    candidates: &[ArtifactCandidate<'_>],
    start: usize,
    reason: &str,
) {
    for candidate in &candidates[start..] {
        outcome
            .deferred
            .push(DeferredArtifact {
                url: candidate.url.clone(),
                reason: reason.to_owned(),
            });
    }
}

fn project_events(events: &[MaintEvent]) -> BTreeMap<String, ArtifactProjection> {
    let mut out = BTreeMap::<String, ArtifactProjection>::new();
    for event in events {
        let Some(url) = event
            .pr_url
            .as_ref()
            .or(event.issue_url.as_ref())
        else {
            continue;
        };
        let p = out
            .entry(url.clone())
            .or_default();
        p.terminal = event.kind.is_terminal();
        p.last_updated_at = Some(event.new_state.clone());
        match event.kind {
            MaintEventKind::PrOpen | MaintEventKind::IssueOpen => {}
            MaintEventKind::PrMerged
            | MaintEventKind::PrClosedUnmerged
            | MaintEventKind::IssueClosed => {
                p.terminal = true;
            }
            MaintEventKind::PrForcePushed => {
                p.head_sha = event.head_sha.clone();
                p.head_ref_deleted_emitted = false;
                p.stale_emitted = false;
                p.terminal = false;
            }
            MaintEventKind::PrBranchDeleted => {
                p.head_ref_deleted_emitted = true;
                p.terminal = false;
            }
            MaintEventKind::PrStale => {
                p.stale_emitted = true;
                p.terminal = false;
            }
        }
        if event.head_sha.is_some() {
            p.head_sha = event.head_sha.clone();
        }
    }
    out
}

fn candidates_from_sessions(sessions: &[SessionRecord]) -> Result<Vec<ArtifactCandidate<'_>>> {
    let mut out = Vec::new();
    for session in sessions {
        for target in &session.targets {
            if let Some(url) = &target.pr_url {
                let (owner, repo, number) = parse_github_artifact_url(url, "pull")?;
                out.push(ArtifactCandidate {
                    kind: ArtifactKind::Pr,
                    session,
                    target,
                    url: url.clone(),
                    owner,
                    repo,
                    number,
                });
            }
            if let Some(url) = &target.issue_url {
                let (owner, repo, number) = parse_github_artifact_url(url, "issues")?;
                out.push(ArtifactCandidate {
                    kind: ArtifactKind::Issue,
                    session,
                    target,
                    url: url.clone(),
                    owner,
                    repo,
                    number,
                });
            }
        }
    }
    Ok(out)
}

fn parse_github_artifact_url(url: &str, segment: &str) -> Result<(String, String, u64)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| anyhow::anyhow!("expected GitHub URL, got `{url}`"))?;
    let parts: Vec<&str> = rest
        .trim_end_matches('/')
        .split('/')
        .collect();
    if parts.len() != 4 || parts[2] != segment {
        bail!("expected GitHub {segment} URL, got `{url}`");
    }
    let number = parts[3]
        .parse::<u64>()
        .with_context(|| format!("parsing GitHub artifact number from `{url}`"))?;
    Ok((parts[0].to_owned(), parts[1].to_owned(), number))
}

fn derive_pr_event(
    c: &ArtifactCandidate<'_>,
    projection: Option<&ArtifactProjection>,
    state: &PrState,
    now: SystemTime,
    settings: &MaintainSettings,
) -> Result<Option<MaintEvent>> {
    let prior = projection.and_then(prior_state_string);
    if state.is_merged {
        return Ok(Some(event_for_candidate(
            c,
            MaintEventKind::PrMerged,
            prior,
            "merged",
            state.head_sha.clone(),
        )));
    }
    if state.is_closed_unmerged {
        return Ok(Some(event_for_candidate(
            c,
            MaintEventKind::PrClosedUnmerged,
            prior,
            "closed_unmerged",
            state.head_sha.clone(),
        )));
    }
    if projection.is_none() && state.is_open {
        return Ok(Some(event_for_candidate(
            c,
            MaintEventKind::PrOpen,
            None,
            "open",
            state.head_sha.clone(),
        )));
    }
    let projection = match projection {
        Some(p) => p,
        None => return Ok(None),
    };
    if let Some(head_sha) = &state.head_sha
        && projection
            .head_sha
            .as_ref()
            .is_some_and(|prior| prior != head_sha)
    {
        return Ok(Some(event_for_candidate(
            c,
            MaintEventKind::PrForcePushed,
            prior,
            "open",
            Some(head_sha.clone()),
        )));
    }
    if state.head_ref_deleted && !projection.head_ref_deleted_emitted {
        return Ok(Some(event_for_candidate(
            c,
            MaintEventKind::PrBranchDeleted,
            prior,
            "open_branch_deleted",
            state.head_sha.clone(),
        )));
    }
    if !projection.stale_emitted && is_stale(&state.updated_at, now, settings.stale_after_days)? {
        return Ok(Some(event_for_candidate(
            c,
            MaintEventKind::PrStale,
            prior,
            "open_stale",
            state.head_sha.clone(),
        )));
    }
    Ok(None)
}

fn derive_issue_event(
    c: &ArtifactCandidate<'_>,
    projection: Option<&ArtifactProjection>,
    state: &IssueLifecycleState,
) -> Result<Option<MaintEvent>> {
    let prior = projection.and_then(prior_state_string);
    if state.is_closed {
        return Ok(Some(event_for_candidate(
            c,
            MaintEventKind::IssueClosed,
            prior,
            "closed",
            None,
        )));
    }
    if projection.is_none() && state.is_open {
        return Ok(Some(event_for_candidate(c, MaintEventKind::IssueOpen, None, "open", None)));
    }
    Ok(None)
}

fn event_for_candidate(
    c: &ArtifactCandidate<'_>,
    kind: MaintEventKind,
    prior_state: Option<String>,
    new_state: &str,
    head_sha: Option<String>,
) -> MaintEvent {
    MaintEvent {
        schema_version: SchemaVersionV1,
        kind,
        observed_at: String::new(),
        session_id: c.session.id.clone(),
        target_id: Some(c.target.id.clone()),
        family_id: Some(c.target.family_id.clone()),
        fix_signature: Some(c.target.id.clone()),
        pr_url: (c.kind == ArtifactKind::Pr).then(|| c.url.clone()),
        issue_url: (c.kind == ArtifactKind::Issue).then(|| c.url.clone()),
        prior_state,
        new_state: new_state.to_owned(),
        head_sha,
    }
}

fn prior_state_string(p: &ArtifactProjection) -> Option<String> {
    if p.terminal {
        Some("terminal".to_owned())
    } else if p.head_ref_deleted_emitted {
        Some("open_branch_deleted".to_owned())
    } else if p.stale_emitted {
        Some("open_stale".to_owned())
    } else {
        Some("open".to_owned())
    }
}

fn is_stale(updated_at: &str, now: SystemTime, stale_after_days: u64) -> Result<bool> {
    let updated = parse_isoish_utc(updated_at)?;
    let threshold = Duration::from_secs(stale_after_days.saturating_mul(24 * 60 * 60));
    Ok(now
        .duration_since(updated)
        .unwrap_or_default()
        > threshold)
}

fn parse_isoish_utc(s: &str) -> Result<SystemTime> {
    if s.len() < 19 {
        bail!("timestamp `{s}` too short; expected ISO 8601");
    }
    let year: i32 = s[0..4].parse()?;
    let month: u32 = s[5..7].parse()?;
    let day: u32 = s[8..10].parse()?;
    let hour: u64 = s[11..13].parse()?;
    let min: u64 = s[14..16].parse()?;
    let sec: u64 = s[17..19].parse()?;
    let days = days_from_civil(year, month, day);
    let total = days
        .saturating_mul(86_400)
        .saturating_add((hour * 3600 + min * 60 + sec) as i64);
    if total < 0 {
        bail!("timestamp `{s}` predates Unix epoch");
    }
    Ok(UNIX_EPOCH + Duration::from_secs(total as u64))
}

fn format_system_time(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

// Howard Hinnant civil calendar conversions. Duplicated locally to
// avoid pulling a date-time crate for maintain's tiny timestamp needs.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i32;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    ((y + i64::from(m <= 2)) as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use parking_lot::Mutex;

    use super::*;
    use crate::models::common::{DeliveryMode, SchemaVersionV3};
    use crate::models::session_record::{
        SessionRange, SessionRecordKind, SessionStatus, TargetStatus,
    };
    use crate::session::publish::{GhStateRead, RateLimitSnapshot};

    #[derive(Default)]
    struct FakeGh {
        pr_states: Mutex<VecDeque<GhStateRead<PrState>>>,
        issue_states: Mutex<VecDeque<GhStateRead<IssueLifecycleState>>>,
        pr_queries: Mutex<usize>,
        issue_queries: Mutex<usize>,
    }

    impl FakeGh {
        fn push_pr(&self, state: PrState) {
            self.pr_states
                .lock()
                .push_back(GhStateRead {
                    state,
                    rate_limit: RateLimitSnapshot {
                        remaining: 5_000,
                        limit: 5_000,
                        resets_at: UNIX_EPOCH,
                    },
                });
        }

        fn push_pr_low_rate(&self, state: PrState) {
            self.pr_states
                .lock()
                .push_back(GhStateRead {
                    state,
                    rate_limit: RateLimitSnapshot {
                        remaining: 1,
                        limit: 100,
                        resets_at: UNIX_EPOCH,
                    },
                });
        }
    }

    impl GhClient for FakeGh {
        fn worktree_remote_url(&self, _: &std::path::Path, _: &str) -> Result<String> {
            unreachable!()
        }
        fn switch_branch(&self, _: &std::path::Path, _: &str) -> Result<()> {
            unreachable!()
        }
        fn add_modified(&self, _: &std::path::Path) -> Result<()> {
            unreachable!()
        }
        fn commit_if_staged(&self, _: &std::path::Path, _: &str) -> Result<()> {
            unreachable!()
        }
        fn push_branch(
            &self,
            _: &std::path::Path,
            _: &str,
            _: &str,
            _: crate::session::publish::GitPushAuth<'_>,
        ) -> Result<()> {
            unreachable!()
        }
        async fn pr_exists(&self, _: &str, _: &str, _: &str, _: &str) -> Result<bool> {
            unreachable!()
        }
        async fn issue_exists(&self, _: &str, _: &str) -> Result<bool> {
            unreachable!()
        }
        async fn create_pr<'a>(
            &'a self,
            _: crate::session::publish::CreatePrArgs<'a>,
        ) -> Result<String> {
            unreachable!()
        }
        async fn create_issue<'a>(
            &'a self,
            _: &'a str,
            _: &'a [String],
            _: &'a str,
            _: &'a str,
        ) -> Result<String> {
            unreachable!()
        }
        async fn query_pr_state(&self, _: &str, _: &str, _: u64) -> Result<GhStateRead<PrState>> {
            *self.pr_queries.lock() += 1;
            Ok(self
                .pr_states
                .lock()
                .pop_front()
                .expect("seeded PR state"))
        }
        async fn query_issue_state(
            &self,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<GhStateRead<IssueLifecycleState>> {
            *self.issue_queries.lock() += 1;
            Ok(self
                .issue_states
                .lock()
                .pop_front()
                .expect("seeded issue state"))
        }
    }

    fn pr_state(head: &str, updated_at: &str) -> PrState {
        PrState {
            is_open: true,
            is_merged: false,
            is_closed_unmerged: false,
            is_draft: true,
            head_sha: Some(head.to_owned()),
            head_ref_deleted: false,
            base_ref: "feat/stacks-bench".to_owned(),
            updated_at: updated_at.to_owned(),
        }
    }

    fn session() -> SessionRecord {
        session_with_targets(vec![TargetRecord {
            id: "target-a".to_owned(),
            family_id: "family-a".to_owned(),
            bucket: "block_processing".to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            status: TargetStatus::Accepted,
            status_stage: None,
            reason_code: None,
            head_sha: Some("aaa".to_owned()),
            pr_url: Some("https://github.com/owner/repo/pull/1".to_owned()),
            issue_url: None,
            bench: None,
        }])
    }

    fn session_with_targets(targets: Vec<TargetRecord>) -> SessionRecord {
        SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: SchemaVersionV3,
            id: "20260611-172955".to_owned(),
            artifact_branch: "session/20260611-172955".to_owned(),
            artifact_sha: "a".repeat(40),
            artifact_url: None,
            started_at: "2026-06-11T17:29:55Z".to_owned(),
            finished_at: "2026-06-12T07:57:57Z".to_owned(),
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
            source_sha: None,
            source_fetched_at: None,
        }
    }

    fn event(kind: MaintEventKind, head: Option<&str>, new_state: &str) -> MaintEvent {
        MaintEvent {
            schema_version: SchemaVersionV1,
            kind,
            observed_at: "2026-06-12T00:00:00Z".to_owned(),
            session_id: "20260611-172955".to_owned(),
            target_id: Some("target-a".to_owned()),
            family_id: Some("family-a".to_owned()),
            fix_signature: Some("target-a".to_owned()),
            pr_url: Some("https://github.com/owner/repo/pull/1".to_owned()),
            issue_url: None,
            prior_state: None,
            new_state: new_state.to_owned(),
            head_sha: head.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn first_open_observation_emits_pr_open() {
        let gh = FakeGh::default();
        gh.push_pr(pr_state("aaa", "2026-06-12T00:00:00Z"));
        let rec = MaintainReconciler {
            gh: &gh,
            settings: &MaintainSettings::default(),
            now: parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
        };
        let outcome = rec
            .reconcile(&[session()], &MaintLedgerReport::default(), 50)
            .await
            .unwrap();
        assert_eq!(outcome.new_events.len(), 1);
        assert_eq!(outcome.new_events[0].kind, MaintEventKind::PrOpen);
        assert_eq!(outcome.queried, 1);
    }

    #[tokio::test]
    async fn open_to_merged_emits_pr_merged() {
        let gh = FakeGh::default();
        gh.push_pr(PrState {
            is_open: false,
            is_merged: true,
            is_closed_unmerged: false,
            is_draft: false,
            head_sha: Some("aaa".to_owned()),
            head_ref_deleted: false,
            base_ref: "feat/stacks-bench".to_owned(),
            updated_at: "2026-06-12T00:00:00Z".to_owned(),
        });
        let rec = MaintainReconciler {
            gh: &gh,
            settings: &MaintainSettings::default(),
            now: parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
        };
        let maintain = MaintLedgerReport {
            events: vec![event(MaintEventKind::PrOpen, Some("aaa"), "open")],
            skipped: vec![],
        };
        let outcome = rec
            .reconcile(&[session()], &maintain, 50)
            .await
            .unwrap();
        assert_eq!(outcome.new_events.len(), 1);
        assert_eq!(outcome.new_events[0].kind, MaintEventKind::PrMerged);
    }

    #[tokio::test]
    async fn already_stale_does_not_emit_duplicate() {
        let gh = FakeGh::default();
        gh.push_pr(pr_state("aaa", "2026-05-01T00:00:00Z"));
        let rec = MaintainReconciler {
            gh: &gh,
            settings: &MaintainSettings::default(),
            now: parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
        };
        let maintain = MaintLedgerReport {
            events: vec![
                event(MaintEventKind::PrOpen, Some("aaa"), "open"),
                event(MaintEventKind::PrStale, Some("aaa"), "open_stale"),
            ],
            skipped: vec![],
        };
        let outcome = rec
            .reconcile(&[session()], &maintain, 50)
            .await
            .unwrap();
        assert!(outcome.new_events.is_empty());
    }

    #[tokio::test]
    async fn already_branch_deleted_does_not_emit_duplicate() {
        let gh = FakeGh::default();
        let mut state = pr_state("aaa", "2026-06-12T00:00:00Z");
        state.head_ref_deleted = true;
        gh.push_pr(state);
        let rec = MaintainReconciler {
            gh: &gh,
            settings: &MaintainSettings::default(),
            now: parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
        };
        let maintain = MaintLedgerReport {
            events: vec![
                event(MaintEventKind::PrOpen, Some("aaa"), "open"),
                event(MaintEventKind::PrBranchDeleted, Some("aaa"), "open_branch_deleted"),
            ],
            skipped: vec![],
        };
        let outcome = rec
            .reconcile(&[session()], &maintain, 50)
            .await
            .unwrap();
        assert!(outcome.new_events.is_empty());
    }

    #[tokio::test]
    async fn force_pushed_then_stable_does_not_emit_duplicate() {
        let gh = FakeGh::default();
        gh.push_pr(pr_state("bbb", "2026-06-12T00:00:00Z"));
        let rec = MaintainReconciler {
            gh: &gh,
            settings: &MaintainSettings::default(),
            now: parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
        };
        let maintain = MaintLedgerReport {
            events: vec![
                event(MaintEventKind::PrOpen, Some("aaa"), "open"),
                event(MaintEventKind::PrForcePushed, Some("bbb"), "open"),
            ],
            skipped: vec![],
        };
        let outcome = rec
            .reconcile(&[session()], &maintain, 50)
            .await
            .unwrap();
        assert!(outcome.new_events.is_empty());
    }

    #[tokio::test]
    async fn force_push_resets_stale_flag_then_re_stales() {
        let gh = FakeGh::default();
        gh.push_pr(pr_state("bbb", "2026-05-01T00:00:00Z"));
        let rec = MaintainReconciler {
            gh: &gh,
            settings: &MaintainSettings::default(),
            now: parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
        };
        let maintain = MaintLedgerReport {
            events: vec![
                event(MaintEventKind::PrOpen, Some("aaa"), "open"),
                event(MaintEventKind::PrStale, Some("aaa"), "open_stale"),
                event(MaintEventKind::PrForcePushed, Some("bbb"), "open"),
            ],
            skipped: vec![],
        };
        let outcome = rec
            .reconcile(&[session()], &maintain, 50)
            .await
            .unwrap();
        assert_eq!(outcome.new_events.len(), 1);
        assert_eq!(outcome.new_events[0].kind, MaintEventKind::PrStale);
    }

    #[tokio::test]
    async fn rate_limit_floor_defers_current_and_remaining_artifacts_without_event() {
        let gh = FakeGh::default();
        gh.push_pr_low_rate(pr_state("aaa", "2026-06-12T00:00:00Z"));
        let session = session_with_targets(vec![
            TargetRecord {
                id: "target-a".to_owned(),
                family_id: "family-a".to_owned(),
                bucket: "block_processing".to_owned(),
                delivery_mode: DeliveryMode::NormalPr,
                status: TargetStatus::Accepted,
                status_stage: None,
                reason_code: None,
                head_sha: Some("aaa".to_owned()),
                pr_url: Some("https://github.com/owner/repo/pull/1".to_owned()),
                issue_url: None,
                bench: None,
            },
            TargetRecord {
                id: "target-b".to_owned(),
                family_id: "family-a".to_owned(),
                bucket: "block_processing".to_owned(),
                delivery_mode: DeliveryMode::NormalPr,
                status: TargetStatus::Accepted,
                status_stage: None,
                reason_code: None,
                head_sha: Some("bbb".to_owned()),
                pr_url: Some("https://github.com/owner/repo/pull/2".to_owned()),
                issue_url: None,
                bench: None,
            },
        ]);
        let rec = MaintainReconciler {
            gh: &gh,
            settings: &MaintainSettings::default(),
            now: parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
        };
        let outcome = rec
            .reconcile(&[session], &MaintLedgerReport::default(), 50)
            .await
            .unwrap();
        assert!(outcome.new_events.is_empty());
        assert_eq!(outcome.queried, 1);
        assert_eq!(outcome.deferred.len(), 2);
        assert!(
            outcome
                .deferred
                .iter()
                .all(|d| d.reason == "rate-limit-floor")
        );
    }

    #[tokio::test]
    async fn limit_defers_artifacts_after_query_budget() {
        let gh = FakeGh::default();
        gh.push_pr(pr_state("aaa", "2026-06-12T00:00:00Z"));
        gh.push_pr(pr_state("bbb", "2026-06-12T00:00:00Z"));
        let session = session_with_targets(vec![
            TargetRecord {
                id: "target-a".to_owned(),
                family_id: "family-a".to_owned(),
                bucket: "block_processing".to_owned(),
                delivery_mode: DeliveryMode::NormalPr,
                status: TargetStatus::Accepted,
                status_stage: None,
                reason_code: None,
                head_sha: Some("aaa".to_owned()),
                pr_url: Some("https://github.com/owner/repo/pull/1".to_owned()),
                issue_url: None,
                bench: None,
            },
            TargetRecord {
                id: "target-b".to_owned(),
                family_id: "family-a".to_owned(),
                bucket: "block_processing".to_owned(),
                delivery_mode: DeliveryMode::NormalPr,
                status: TargetStatus::Accepted,
                status_stage: None,
                reason_code: None,
                head_sha: Some("bbb".to_owned()),
                pr_url: Some("https://github.com/owner/repo/pull/2".to_owned()),
                issue_url: None,
                bench: None,
            },
        ]);
        let rec = MaintainReconciler {
            gh: &gh,
            settings: &MaintainSettings::default(),
            now: parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
        };
        let outcome = rec
            .reconcile(&[session], &MaintLedgerReport::default(), 1)
            .await
            .unwrap();
        assert_eq!(outcome.queried, 1);
        assert_eq!(outcome.new_events.len(), 1);
        assert_eq!(outcome.deferred.len(), 1);
        assert_eq!(outcome.deferred[0].reason, "limit");
    }
}
