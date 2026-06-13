//! Autonomy safety gates shared by session-start preflight and tests.
//!
//! These checks are deliberately read-only except for the circuit breaker,
//! which writes `.sbagent/pause` when recent completed sessions show a
//! repeated zero-accepted pattern.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};

use crate::models::maintain_event::{MaintEvent, MaintEventKind};
use crate::models::session_record::{SessionRecord, SessionStatus, TargetStatus};
use crate::session::ledger_reader::read_all as read_sessions;
use crate::session::maintain_ledger::read_all as read_maintain;
use crate::settings::AutonomySettings;

/// One blocking autonomy gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomyBlock {
    /// Stable gate id for preflight rendering/tests.
    pub gate: &'static str,
    /// Human-readable block reason.
    pub message: String,
    /// Operator-facing remediation.
    pub remediation: String,
}

/// Resolve the operator pause file.
pub fn pause_file(operator_repo_root: &Path) -> PathBuf {
    operator_repo_root
        .join(".sbagent")
        .join("pause")
}

/// Read operator ledgers and evaluate every autonomy gate.
pub fn check_operator_gates(
    operator_repo_root: &Path,
    settings: &AutonomySettings,
    now: SystemTime,
) -> Result<Vec<AutonomyBlock>> {
    check_operator_gates_inner(operator_repo_root, settings, now, BreakerAction::ReportOnly)
}

/// Read operator ledgers, evaluate every autonomy gate, and write
/// `.sbagent/pause` if the zero-accepted circuit breaker trips.
pub fn enforce_operator_gates(
    operator_repo_root: &Path,
    settings: &AutonomySettings,
    now: SystemTime,
) -> Result<Vec<AutonomyBlock>> {
    check_operator_gates_inner(operator_repo_root, settings, now, BreakerAction::WritePause)
}

#[derive(Debug, Clone, Copy)]
enum BreakerAction {
    ReportOnly,
    WritePause,
}

fn check_operator_gates_inner(
    operator_repo_root: &Path,
    settings: &AutonomySettings,
    now: SystemTime,
    breaker_action: BreakerAction,
) -> Result<Vec<AutonomyBlock>> {
    let mut blocks = Vec::new();
    check_pause_file(operator_repo_root, &mut blocks);

    let sessions_path = operator_repo_root.join("sessions.jsonl");
    let maintain_path = operator_repo_root.join("maintain.jsonl");
    let sessions = read_sessions(&sessions_path)?;
    let maintain = read_maintain(&maintain_path)?;

    check_open_pr_limit(settings, &sessions.records, &maintain.events, &mut blocks);
    check_session_interval(settings, &sessions.records, now, &mut blocks);
    check_zero_accepted_breaker(
        operator_repo_root,
        settings,
        &sessions.records,
        now,
        breaker_action,
        &mut blocks,
    )?;
    Ok(blocks)
}

fn check_pause_file(operator: &Path, blocks: &mut Vec<AutonomyBlock>) {
    let path = pause_file(operator);
    if path.exists() {
        blocks.push(AutonomyBlock {
            gate: "pause-file",
            message: format!("operator pause file exists at {}", path.display()),
            remediation: format!(
                "inspect {}, then remove it to allow `sbagent session run` again; `sbagent \
                 maintain` remains allowed while paused",
                path.display(),
            ),
        });
    }
}

fn check_open_pr_limit(
    settings: &AutonomySettings,
    sessions: &[SessionRecord],
    maintain: &[MaintEvent],
    blocks: &mut Vec<AutonomyBlock>,
) {
    let open = open_pr_urls(sessions, maintain);
    if open.len() >= settings.max_open_agent_prs {
        blocks.push(AutonomyBlock {
            gate: "max-open-agent-prs",
            message: format!(
                "{} open bot PR(s) meets/exceeds autonomy.max_open_agent_prs={}",
                open.len(),
                settings.max_open_agent_prs,
            ),
            remediation: "merge, close, or maintain-reconcile existing bot PRs before starting \
                          another session"
                .to_owned(),
        });
    }
}

fn check_session_interval(
    settings: &AutonomySettings,
    sessions: &[SessionRecord],
    now: SystemTime,
    blocks: &mut Vec<AutonomyBlock>,
) {
    if settings.min_session_interval_hours == 0 {
        return;
    }
    let Some((session_id, ts)) = latest_session_time(sessions) else {
        return;
    };
    let elapsed = now
        .duration_since(ts)
        .unwrap_or(Duration::ZERO);
    let min = Duration::from_secs(
        settings
            .min_session_interval_hours
            .saturating_mul(3600),
    );
    if elapsed < min {
        blocks.push(AutonomyBlock {
            gate: "min-session-interval",
            message: format!(
                "latest archived session `{session_id}` is {}h old; \
                 autonomy.min_session_interval_hours requires {}h",
                elapsed.as_secs() / 3600,
                settings.min_session_interval_hours,
            ),
            remediation: "wait until the interval elapses or lower \
                          autonomy.min_session_interval_hours deliberately"
                .to_owned(),
        });
    }
}

fn check_zero_accepted_breaker(
    operator: &Path,
    settings: &AutonomySettings,
    sessions: &[SessionRecord],
    now: SystemTime,
    action: BreakerAction,
    blocks: &mut Vec<AutonomyBlock>,
) -> Result<()> {
    let n = settings.zero_accepted_circuit_breaker;
    if n == 0 {
        return Ok(());
    }
    let mut completed: Vec<&SessionRecord> = sessions
        .iter()
        .filter(|s| s.status != SessionStatus::Aborted)
        .collect();
    completed.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
    });
    if completed.len() < n {
        return Ok(());
    }
    let window = &completed[..n];
    if window
        .iter()
        .any(|s| session_has_accepted_target(s))
    {
        return Ok(());
    }

    let path = pause_file(operator);
    if path.exists() {
        blocks.push(AutonomyBlock {
            gate: "zero-accepted-circuit-breaker",
            message: format!(
                "last {n} completed session(s) had zero accepted targets; existing pause file \
                 preserved at {}",
                path.display(),
            ),
            remediation: format!("inspect {} before removing it", path.display()),
        });
        return Ok(());
    }

    if matches!(action, BreakerAction::ReportOnly) {
        blocks.push(AutonomyBlock {
            gate: "zero-accepted-circuit-breaker",
            message: format!(
                "last {n} completed session(s) had zero accepted targets; session run would write \
                 pause file {}",
                path.display(),
            ),
            remediation: "inspect recent sessions before starting another run, or disable \
                          autonomy.zero_accepted_circuit_breaker deliberately"
                .to_owned(),
        });
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating pause-file parent {}", parent.display()))?;
    }
    let ids: Vec<&str> = window
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    let body = format!(
        "sbagent paused at {}\nreason: last {n} completed sessions had zero accepted \
         targets\nsessions:\n{}\n\nRemove this file after review to allow `sbagent session run`.\n",
        format_system_time(now),
        ids.iter()
            .map(|id| format!("- {id}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    std::fs::write(&path, body)
        .with_context(|| format!("writing pause file {}", path.display()))?;
    blocks.push(AutonomyBlock {
        gate: "zero-accepted-circuit-breaker",
        message: format!(
            "last {n} completed session(s) had zero accepted targets; wrote pause file {}",
            path.display(),
        ),
        remediation: format!("inspect {}, then remove it to resume sessions", path.display()),
    });
    Ok(())
}

/// Return PR URLs currently considered open by sessions + maintain projection.
pub fn open_pr_urls(sessions: &[SessionRecord], maintain: &[MaintEvent]) -> BTreeSet<String> {
    let mut urls = BTreeSet::new();
    for session in sessions {
        for target in &session.targets {
            if let Some(url) = &target.pr_url {
                urls.insert(url.clone());
            }
        }
    }
    let terminal = terminal_pr_urls(maintain);
    for url in terminal {
        urls.remove(&url);
    }
    urls
}

fn terminal_pr_urls(events: &[MaintEvent]) -> BTreeSet<String> {
    let mut latest = BTreeMap::<String, MaintEventKind>::new();
    for event in events {
        if let Some(url) = &event.pr_url {
            latest.insert(url.clone(), event.kind);
        }
    }
    latest
        .into_iter()
        .filter_map(|(url, kind)| {
            matches!(kind, MaintEventKind::PrMerged | MaintEventKind::PrClosedUnmerged)
                .then_some(url)
        })
        .collect()
}

fn latest_session_time(sessions: &[SessionRecord]) -> Option<(&str, SystemTime)> {
    sessions
        .iter()
        .filter_map(|s| {
            parse_isoish_utc(&s.finished_at)
                .or_else(|| parse_isoish_utc(&s.started_at))
                .map(|ts| (s.id.as_str(), ts))
        })
        .max_by_key(|(_, ts)| *ts)
}

fn session_has_accepted_target(session: &SessionRecord) -> bool {
    session
        .targets
        .iter()
        .any(|t| t.status == TargetStatus::Accepted)
}

fn parse_isoish_utc(s: &str) -> Option<SystemTime> {
    let s = s
        .strip_suffix('Z')
        .unwrap_or(s);
    let date_time: Vec<&str> = s.split('T').collect();
    if date_time.len() != 2 {
        return None;
    }
    let date: Vec<i64> = date_time[0]
        .split('-')
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if date.len() != 3 {
        return None;
    }
    let time_head = date_time[1]
        .split('.')
        .next()
        .unwrap_or(date_time[1]);
    let time: Vec<i64> = time_head
        .split(':')
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if time.len() != 3 {
        return None;
    }
    let days = days_from_civil(date[0], date[1], date[2]);
    if days < 0 {
        return None;
    }
    let secs = days
        .checked_mul(86_400)?
        .checked_add(time[0].checked_mul(3600)?)?
        .checked_add(time[1].checked_mul(60)?)?
        .checked_add(time[2])?;
    u64::try_from(secs)
        .ok()
        .map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

fn format_system_time(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + (m <= 2) as i64;
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::models::common::{DeliveryMode, SchemaVersionV1, SchemaVersionV3};
    use crate::models::maintain_event::MaintEventKind;
    use crate::models::session_record::{
        SessionRange, SessionRecordKind, TargetRecord, TargetStatusStage,
    };

    fn target(id: &str, status: TargetStatus, pr: Option<&str>) -> TargetRecord {
        TargetRecord {
            id: id.to_owned(),
            family_id: "family-a".to_owned(),
            bucket: "block_processing".to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            status,
            status_stage: (status != TargetStatus::Accepted).then_some(TargetStatusStage::Bench),
            reason_code: None,
            head_sha: None,
            pr_url: pr.map(str::to_owned),
            issue_url: None,
            bench: None,
        }
    }

    fn session(
        id: &str,
        status: SessionStatus,
        started: &str,
        targets: Vec<TargetRecord>,
    ) -> SessionRecord {
        SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: SchemaVersionV3,
            id: id.to_owned(),
            artifact_branch: format!("session/{id}"),
            artifact_sha: "cafebabecafebabecafebabecafebabecafebabe".to_owned(),
            artifact_url: None,
            started_at: started.to_owned(),
            finished_at: started.to_owned(),
            status,
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
            phase_durations_secs: BTreeMap::new(),
            targets,
            source_url: None,
            source_branch: None,
            source_sha: None,
            source_fetched_at: None,
        }
    }

    fn event(kind: MaintEventKind, url: &str) -> MaintEvent {
        MaintEvent {
            schema_version: SchemaVersionV1,
            kind,
            observed_at: "2026-06-13T00:00:00Z".to_owned(),
            session_id: "s1".to_owned(),
            target_id: Some("target-a".to_owned()),
            family_id: Some("family-a".to_owned()),
            fix_signature: Some("target-a".to_owned()),
            pr_url: Some(url.to_owned()),
            issue_url: None,
            prior_state: None,
            new_state: "open".to_owned(),
            head_sha: Some("aaa".to_owned()),
        }
    }

    #[test]
    fn open_pr_urls_subtracts_terminal_maintain_events() {
        let pr1 = "https://github.com/owner/repo/pull/1";
        let pr2 = "https://github.com/owner/repo/pull/2";
        let sessions = vec![session(
            "s1",
            SessionStatus::Succeeded,
            "2026-06-10T00:00:00Z",
            vec![
                target("a", TargetStatus::Accepted, Some(pr1)),
                target("b", TargetStatus::Accepted, Some(pr2)),
            ],
        )];
        let maintain = vec![event(MaintEventKind::PrMerged, pr1)];
        let open = open_pr_urls(&sessions, &maintain);
        assert_eq!(open, BTreeSet::from([pr2.to_owned()]));
    }

    #[test]
    fn open_pr_limit_blocks_at_threshold() {
        let pr1 = "https://github.com/owner/repo/pull/1";
        let pr2 = "https://github.com/owner/repo/pull/2";
        let sessions = vec![session(
            "s1",
            SessionStatus::Succeeded,
            "2026-06-10T00:00:00Z",
            vec![
                target("a", TargetStatus::Accepted, Some(pr1)),
                target("b", TargetStatus::Accepted, Some(pr2)),
            ],
        )];
        let settings = AutonomySettings {
            max_open_agent_prs: 2,
            ..AutonomySettings::default()
        };
        let mut blocks = Vec::new();
        check_open_pr_limit(&settings, &sessions, &[], &mut blocks);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].gate, "max-open-agent-prs");
    }

    #[test]
    fn terminal_maintain_events_remove_prs_from_limit_count() {
        let pr1 = "https://github.com/owner/repo/pull/1";
        let pr2 = "https://github.com/owner/repo/pull/2";
        let sessions = vec![session(
            "s1",
            SessionStatus::Succeeded,
            "2026-06-10T00:00:00Z",
            vec![
                target("a", TargetStatus::Accepted, Some(pr1)),
                target("b", TargetStatus::Accepted, Some(pr2)),
            ],
        )];
        let settings = AutonomySettings {
            max_open_agent_prs: 2,
            ..AutonomySettings::default()
        };
        let mut blocks = Vec::new();
        check_open_pr_limit(
            &settings,
            &sessions,
            &[event(MaintEventKind::PrClosedUnmerged, pr2)],
            &mut blocks,
        );
        assert!(blocks.is_empty());
    }

    #[test]
    fn cadence_gate_blocks_recent_session() {
        let settings = AutonomySettings {
            min_session_interval_hours: 144,
            ..AutonomySettings::default()
        };
        let sessions = vec![session(
            "s1",
            SessionStatus::Succeeded,
            "2026-06-12T00:00:00Z",
            vec![target("a", TargetStatus::Accepted, None)],
        )];
        let mut blocks = Vec::new();
        check_session_interval(
            &settings,
            &sessions,
            parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
            &mut blocks,
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].gate, "min-session-interval");
        assert!(
            blocks[0]
                .message
                .contains("144h")
        );
    }

    #[test]
    fn queue_and_cadence_gates_both_report() {
        let pr1 = "https://github.com/owner/repo/pull/1";
        let settings = AutonomySettings {
            max_open_agent_prs: 1,
            min_session_interval_hours: 144,
            ..AutonomySettings::default()
        };
        let sessions = vec![session(
            "s1",
            SessionStatus::Succeeded,
            "2026-06-12T00:00:00Z",
            vec![target("a", TargetStatus::Accepted, Some(pr1))],
        )];
        let mut blocks = Vec::new();
        check_open_pr_limit(&settings, &sessions, &[], &mut blocks);
        check_session_interval(
            &settings,
            &sessions,
            parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
            &mut blocks,
        );
        let gates: BTreeSet<&str> = blocks
            .iter()
            .map(|b| b.gate)
            .collect();
        assert_eq!(gates, BTreeSet::from(["max-open-agent-prs", "min-session-interval"]));
    }

    #[test]
    fn fewer_than_breaker_window_does_not_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = AutonomySettings {
            zero_accepted_circuit_breaker: 3,
            ..AutonomySettings::default()
        };
        let sessions = vec![
            session("s1", SessionStatus::Succeeded, "2026-06-10T00:00:00Z", vec![]),
            session("s2", SessionStatus::Succeeded, "2026-06-11T00:00:00Z", vec![]),
        ];
        let mut blocks = Vec::new();
        check_zero_accepted_breaker(
            tmp.path(),
            &settings,
            &sessions,
            parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
            BreakerAction::WritePause,
            &mut blocks,
        )
        .unwrap();
        assert!(blocks.is_empty());
        assert!(!pause_file(tmp.path()).exists());
    }

    #[test]
    fn aborted_sessions_do_not_count_toward_breaker_window() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = AutonomySettings {
            zero_accepted_circuit_breaker: 3,
            ..AutonomySettings::default()
        };
        let sessions = vec![
            session("s1", SessionStatus::Aborted, "2026-06-12T00:00:00Z", vec![]),
            session("s2", SessionStatus::Succeeded, "2026-06-11T00:00:00Z", vec![]),
            session("s3", SessionStatus::Succeeded, "2026-06-10T00:00:00Z", vec![]),
        ];
        let mut blocks = Vec::new();
        check_zero_accepted_breaker(
            tmp.path(),
            &settings,
            &sessions,
            parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
            BreakerAction::WritePause,
            &mut blocks,
        )
        .unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn zero_accepted_breaker_writes_pause_file() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = AutonomySettings {
            zero_accepted_circuit_breaker: 3,
            ..AutonomySettings::default()
        };
        let sessions = vec![
            session("s3", SessionStatus::Succeeded, "2026-06-12T00:00:00Z", vec![]),
            session("s2", SessionStatus::Succeeded, "2026-06-11T00:00:00Z", vec![]),
            session("s1", SessionStatus::Succeeded, "2026-06-10T00:00:00Z", vec![]),
        ];
        let mut blocks = Vec::new();
        check_zero_accepted_breaker(
            tmp.path(),
            &settings,
            &sessions,
            parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
            BreakerAction::WritePause,
            &mut blocks,
        )
        .unwrap();
        assert_eq!(blocks.len(), 1);
        let body = std::fs::read_to_string(pause_file(tmp.path())).unwrap();
        assert!(body.contains("last 3 completed sessions had zero accepted targets"));
        assert!(body.contains("- s3"));
    }

    #[test]
    fn accepted_session_prevents_breaker() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = AutonomySettings {
            zero_accepted_circuit_breaker: 3,
            ..AutonomySettings::default()
        };
        let sessions = vec![
            session("s3", SessionStatus::Succeeded, "2026-06-12T00:00:00Z", vec![]),
            session(
                "s2",
                SessionStatus::Succeeded,
                "2026-06-11T00:00:00Z",
                vec![target("a", TargetStatus::Accepted, None)],
            ),
            session("s1", SessionStatus::Succeeded, "2026-06-10T00:00:00Z", vec![]),
        ];
        let mut blocks = Vec::new();
        check_zero_accepted_breaker(
            tmp.path(),
            &settings,
            &sessions,
            parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
            BreakerAction::WritePause,
            &mut blocks,
        )
        .unwrap();
        assert!(blocks.is_empty());
        assert!(!pause_file(tmp.path()).exists());
    }

    #[test]
    fn zero_accepted_breaker_report_only_does_not_write_pause_file() {
        let tmp = tempfile::tempdir().unwrap();
        let settings = AutonomySettings {
            zero_accepted_circuit_breaker: 3,
            ..AutonomySettings::default()
        };
        let sessions = vec![
            session("s3", SessionStatus::Succeeded, "2026-06-12T00:00:00Z", vec![]),
            session("s2", SessionStatus::Succeeded, "2026-06-11T00:00:00Z", vec![]),
            session("s1", SessionStatus::Succeeded, "2026-06-10T00:00:00Z", vec![]),
        ];
        let mut blocks = Vec::new();
        check_zero_accepted_breaker(
            tmp.path(),
            &settings,
            &sessions,
            parse_isoish_utc("2026-06-13T00:00:00Z").unwrap(),
            BreakerAction::ReportOnly,
            &mut blocks,
        )
        .unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].gate, "zero-accepted-circuit-breaker");
        assert!(!pause_file(tmp.path()).exists());
    }
}
