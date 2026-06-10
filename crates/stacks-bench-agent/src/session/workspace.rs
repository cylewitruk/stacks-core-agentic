//! Workspace hygiene — prune stale per-session scratch under
//! `agent_workspace_root/sessions/`.
//!
//! Two durable signals drive prune safety:
//! 1. **`sessions.jsonl` on operator main** — terminal-state ledger. A session
//!    id present in the ledger is archived/finalized.
//! 2. **`<session_dir>/.run.pid`** — best-effort live-session marker written by
//!    `session run`. See [`crate::session::run_pid`].
//!
//! See [`docs/operations.md`](../../../docs/operations.md) for operator
//! usage.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use std::{fs, io};

use anyhow::{Context as _, Result};

use crate::models::session_record::SessionRecord;
use crate::session::run_pid;

/// User-facing options coming from the CLI.
#[derive(Debug, Clone, Copy, Default)]
pub struct PruneOptions {
    /// Only prune sessions older than this. None ⇒ no age threshold.
    pub older_than: Option<Duration>,
    /// Require presence in `sessions.jsonl` (terminal/archived).
    pub archived_only: bool,
    /// Print decisions, do not remove anything.
    pub dry_run: bool,
}

/// PID liveness probe — injectable so tests can simulate
/// live/stale/no-pid sessions without spawning real children.
pub type LivenessProbe = fn(u32) -> bool;

/// Why a candidate was skipped. Used both for the dry-run report and
/// to explain decisions to the operator after a real apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// `.run.pid` was readable and the PID is currently alive.
    LiveSession { pid: u32 },
    /// `--archived-only` set and this session id isn't in `sessions.jsonl`.
    NotArchived,
    /// `--older-than` set and this session is younger than the threshold.
    YoungerThanThreshold { age: Duration, threshold: Duration },
    /// Neither `--archived-only` nor `--older-than` were provided —
    /// refuse to remove anything to keep the destructive path explicit.
    NoFilter,
}

/// One candidate considered for pruning.
#[derive(Debug, Clone)]
pub struct PruneCandidate {
    pub id: String,
    pub path: PathBuf,
    pub age: Duration,
    pub archived: bool,
    /// `Some(pid)` if a PID file was present and parseable.
    pub pid: Option<u32>,
    /// `Some(reason)` ⇒ this candidate is NOT prunable. `None` ⇒ prunable.
    pub skip: Option<SkipReason>,
    /// Best-effort size of the session dir tree (bytes).
    pub bytes: u64,
}

/// Outcome of one prune invocation.
#[derive(Debug, Clone, Default)]
pub struct PruneReport {
    pub candidates: Vec<PruneCandidate>,
    /// Sum of `bytes` across actually-removed candidates. For
    /// `dry_run = true` this is the "would-be-freed" figure.
    pub freed_bytes: u64,
    /// IDs successfully removed this call (empty for dry-run).
    pub removed: Vec<String>,
    pub dry_run: bool,
}

/// Inputs to a prune invocation. Carries every injectable seam
/// (sessions root, ledger path, clock, liveness probe) so tests can
/// drive the full decision matrix in-process.
pub struct PruneInputs<'a> {
    /// `<agent_workspace_root>/sessions/`.
    pub sessions_root: &'a Path,
    /// Path to `sessions.jsonl` on operator main, or `None` when the
    /// operator hasn't archived yet / `archived_only` doesn't apply.
    pub operator_ledger: Option<&'a Path>,
    pub options: PruneOptions,
    /// Current time. Injectable so tests can simulate aged sessions.
    pub now: SystemTime,
    /// PID liveness probe. Production runs pass [`run_pid::is_live`];
    /// tests pass a closure that returns a deterministic verdict.
    pub liveness: LivenessProbe,
}

/// Run a prune pass. Pure of side effects when `options.dry_run` is true.
pub fn prune(inputs: &PruneInputs<'_>) -> Result<PruneReport> {
    let archived = match inputs.operator_ledger {
        Some(p) => read_archived_ids(p)?,
        None => HashSet::new(),
    };

    let entries = match fs::read_dir(inputs.sessions_root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(PruneReport {
                dry_run: inputs.options.dry_run,
                ..Default::default()
            });
        }
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("reading {}", inputs.sessions_root.display())));
        }
    };

    let mut candidates: Vec<PruneCandidate> = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let id = match entry
            .file_name()
            .into_string()
        {
            Ok(s) => s,
            Err(_) => continue,
        };

        let age = session_age(&path, inputs.now)?;
        let pid = run_pid::read(&path)?;
        let is_archived = archived.contains(&id);
        let bytes = best_effort_dir_size(&path);

        let skip = decide_skip(pid, inputs.liveness, &inputs.options, is_archived, age);
        candidates.push(PruneCandidate {
            id,
            path,
            age,
            archived: is_archived,
            pid,
            skip,
            bytes,
        });
    }

    let mut report = PruneReport {
        candidates,
        dry_run: inputs.options.dry_run,
        ..Default::default()
    };

    if !inputs.options.dry_run {
        for c in &report.candidates {
            if c.skip.is_some() {
                continue;
            }
            fs::remove_dir_all(&c.path)
                .with_context(|| format!("removing {}", c.path.display()))?;
            report
                .removed
                .push(c.id.clone());
            report.freed_bytes += c.bytes;
        }
    } else {
        for c in &report.candidates {
            if c.skip.is_none() {
                report.freed_bytes += c.bytes;
            }
        }
    }

    Ok(report)
}

/// Decide whether a candidate is prunable given the options + live
/// signals. `None` ⇒ prunable. Encodes the precedence:
/// LiveSession > NoFilter > NotArchived > YoungerThanThreshold.
fn decide_skip(
    pid: Option<u32>,
    liveness: LivenessProbe,
    options: &PruneOptions,
    archived: bool,
    age: Duration,
) -> Option<SkipReason> {
    if let Some(pid) = pid
        && liveness(pid)
    {
        return Some(SkipReason::LiveSession { pid });
    }
    // Refuse to remove anything when no filter is set — destructive
    // intent must be explicit.
    if !options.archived_only && options.older_than.is_none() {
        return Some(SkipReason::NoFilter);
    }
    if options.archived_only && !archived {
        return Some(SkipReason::NotArchived);
    }
    if let Some(threshold) = options.older_than
        && age < threshold
    {
        return Some(SkipReason::YoungerThanThreshold { age, threshold });
    }
    None
}

/// Read every session id from `sessions.jsonl`. Missing file ⇒ empty
/// set. Malformed lines are skipped silently (the prune path doesn't
/// want a hand-edited ledger to lock the operator out of pruning).
fn read_archived_ids(path: &Path) -> Result<HashSet<String>> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
        }
    };
    let mut ids = HashSet::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // `from_ledger_line` accepts every supported schema version
        // (v1 / v2 / v3), so workspace prune's archived-set lookup
        // against a long-running `sessions.jsonl` doesn't silently
        // treat historical sessions as "not archived" and offer to
        // prune them.
        let Ok(rec) = SessionRecord::from_ledger_line(line) else {
            continue;
        };
        ids.insert(rec.id);
    }
    Ok(ids)
}

/// Session age via the session dir's mtime. Best-effort: a clock skew
/// from `now` ⇒ saturating zero, so we never panic on a fresh dir
/// whose mtime is fractionally ahead.
fn session_age(path: &Path, now: SystemTime) -> Result<Duration> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let modified = meta
        .modified()
        .with_context(|| format!("mtime {}", path.display()))?;
    Ok(now
        .duration_since(modified)
        .unwrap_or_default())
}

/// Walk `path` and sum up file sizes. Best-effort: skips paths the
/// process can't stat and counts symlinks by their own size, not the
/// target's, to avoid double-counting cross-session shared trees.
fn best_effort_dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let meta = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            if let Ok(rd) = fs::read_dir(&p) {
                for entry in rd.flatten() {
                    stack.push(entry.path());
                }
            }
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

/// Parse a humantime-ish duration: `<n><suffix>` where suffix is one
/// of `s` `m` `h` `d` `w`. Multi-component strings (`1d12h`) aren't
/// supported — operators almost always want a single unit, and the
/// extra parser surface isn't worth the dep.
pub fn parse_duration(raw: &str) -> Result<Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        anyhow::bail!("duration is empty");
    }
    let (digits, suffix) = match raw.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => raw.split_at(i),
        None => (raw, "s"),
    };
    let n: u64 = digits
        .parse()
        .with_context(|| format!("parsing duration leading number from `{raw}`"))?;
    let secs = match suffix {
        "s" => n,
        "m" => n.saturating_mul(60),
        "h" => n.saturating_mul(60 * 60),
        "d" => n.saturating_mul(60 * 60 * 24),
        "w" => n.saturating_mul(60 * 60 * 24 * 7),
        other => anyhow::bail!(
            "unrecognized duration suffix `{other}` in `{raw}` (use one of s/m/h/d/w)",
        ),
    };
    Ok(Duration::from_secs(secs))
}

/// Pretty-print a report on stdout. One line per candidate plus a
/// totals footer in bytes / GiB. Operators see exactly which sessions
/// were pruned and which were retained and why.
pub fn print_report(report: &PruneReport, sessions_root: &Path) {
    let kept = report
        .candidates
        .iter()
        .filter(|c| c.skip.is_some())
        .count();
    let prunable = report
        .candidates
        .iter()
        .filter(|c| c.skip.is_none())
        .count();

    println!(
        "workspace prune ({}): {} prunable, {} kept in {}",
        if report.dry_run { "dry-run" } else { "applied" },
        prunable,
        kept,
        sessions_root.display(),
    );
    for c in &report.candidates {
        let bytes_h = humanize_bytes(c.bytes);
        match &c.skip {
            None => {
                if report.dry_run {
                    println!("  WOULD PRUNE {} ({bytes_h}, age {})", c.id, humanize(c.age));
                } else if report.removed.contains(&c.id) {
                    println!("  PRUNED      {} ({bytes_h}, age {})", c.id, humanize(c.age));
                }
            }
            Some(SkipReason::LiveSession { pid }) => {
                println!("  KEPT        {} (live session pid={pid})", c.id);
            }
            Some(SkipReason::NotArchived) => {
                println!("  KEPT        {} (not in sessions.jsonl)", c.id);
            }
            Some(SkipReason::YoungerThanThreshold { age, threshold }) => {
                println!(
                    "  KEPT        {} (age {} < threshold {})",
                    c.id,
                    humanize(*age),
                    humanize(*threshold),
                );
            }
            Some(SkipReason::NoFilter) => {
                println!(
                    "  KEPT        {} (no --older-than / --archived-only — destructive intent \
                     must be explicit)",
                    c.id,
                );
            }
        }
    }
    println!(
        "  total: {} {} across {} session(s)",
        humanize_bytes(report.freed_bytes),
        if report.dry_run { "would be freed" } else { "freed" },
        if report.dry_run { prunable } else { report.removed.len() },
    );
}

fn humanize(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 * 60 * 24 {
        format!("{}d", secs / (60 * 60 * 24))
    } else if secs >= 60 * 60 {
        format!("{}h", secs / (60 * 60))
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn humanize_bytes(b: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;
    if b >= GIB {
        format!("{:.1} GiB", b as f64 / GIB as f64)
    } else if b >= MIB {
        format!("{:.1} MiB", b as f64 / MIB as f64)
    } else if b >= KIB {
        format!("{:.1} KiB", b as f64 / KIB as f64)
    } else {
        format!("{b} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dead(_pid: u32) -> bool {
        false
    }
    fn always_live(_pid: u32) -> bool {
        true
    }

    fn seed_session(root: &Path, id: &str, pid_file: Option<u32>) -> PathBuf {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("results"), b"").ok();
        if let Some(pid) = pid_file {
            fs::write(run_pid::path_for_session(&dir), format!("{pid}\n")).unwrap();
        }
        dir
    }

    #[test]
    fn parse_duration_handles_supported_suffixes() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(5 * 60));
        assert_eq!(parse_duration("7d").unwrap(), Duration::from_secs(7 * 86400));
        assert_eq!(parse_duration("2w").unwrap(), Duration::from_secs(2 * 7 * 86400));
        assert!(parse_duration("nope").is_err());
        assert!(parse_duration("7x").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn no_filter_protects_every_session_in_dry_run() {
        let tmp = tempfile::tempdir().unwrap();
        seed_session(tmp.path(), "20260601-000000", None);
        seed_session(tmp.path(), "20260602-000000", None);

        let report = prune(&PruneInputs {
            sessions_root: tmp.path(),
            operator_ledger: None,
            options: PruneOptions {
                older_than: None,
                archived_only: false,
                dry_run: true,
            },
            now: SystemTime::now(),
            liveness: dead,
        })
        .unwrap();

        assert_eq!(report.candidates.len(), 2);
        assert!(
            report
                .candidates
                .iter()
                .all(|c| matches!(c.skip, Some(SkipReason::NoFilter)))
        );
        assert_eq!(report.freed_bytes, 0);
        assert!(report.removed.is_empty());
    }

    #[test]
    fn live_pid_blocks_prune_even_with_filters_set() {
        let tmp = tempfile::tempdir().unwrap();
        seed_session(tmp.path(), "live-session", Some(12345));

        let report = prune(&PruneInputs {
            sessions_root: tmp.path(),
            operator_ledger: None,
            options: PruneOptions {
                older_than: Some(Duration::from_secs(0)),
                archived_only: false,
                dry_run: false,
            },
            now: SystemTime::now(),
            liveness: always_live,
        })
        .unwrap();

        assert_eq!(report.candidates.len(), 1);
        assert!(matches!(report.candidates[0].skip, Some(SkipReason::LiveSession { pid: 12345 })));
        assert!(report.removed.is_empty());
        assert!(
            tmp.path()
                .join("live-session")
                .exists()
        );
    }

    #[test]
    fn stale_pid_falls_through_to_age_and_archive_filters() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = seed_session(tmp.path(), "stale-session", Some(99999));
        // Past-dated mtime so the age threshold catches it.
        let one_year_ago = SystemTime::now() - Duration::from_secs(365 * 86400);

        let report = prune(&PruneInputs {
            sessions_root: tmp.path(),
            operator_ledger: None,
            options: PruneOptions {
                older_than: Some(Duration::from_secs(86400)),
                archived_only: false,
                dry_run: false,
            },
            // Mock `now` so the just-created dir reads as old.
            now: one_year_ago + Duration::from_secs(2 * 365 * 86400),
            liveness: dead,
        })
        .unwrap();

        assert_eq!(report.removed.len(), 1, "stale PID must NOT block prune");
        assert_eq!(report.removed[0], "stale-session");
        assert!(!dir.exists());
    }

    #[test]
    fn archived_only_requires_ledger_match() {
        use std::collections::BTreeMap;

        use crate::models::ToJson;
        use crate::models::common::SchemaVersionV3;
        use crate::models::session_record::{
            SessionRange, SessionRecord, SessionRecordKind, SessionStatus,
        };

        let tmp = tempfile::tempdir().unwrap();
        seed_session(tmp.path(), "20260601-archived", None);
        seed_session(tmp.path(), "20260602-not-archived", None);

        let ledger = tmp
            .path()
            .join("sessions.jsonl");
        let rec = SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: SchemaVersionV3,
            id: "20260601-archived".to_owned(),
            artifact_branch: "session/20260601-archived".to_owned(),
            artifact_sha: "deadbeef".to_owned(),
            artifact_url: None,
            started_at: "2026-06-01T00:00:00Z".to_owned(),
            finished_at: "2026-06-01T00:30:00Z".to_owned(),
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
            phase_durations_secs: BTreeMap::new(),
            targets: vec![],
            source_url: None,
            source_branch: None,
            source_sha: None,
            source_fetched_at: None,
        };
        let line = rec.to_json().unwrap();
        fs::write(&ledger, format!("{line}\n")).unwrap();

        let report = prune(&PruneInputs {
            sessions_root: tmp.path(),
            operator_ledger: Some(&ledger),
            options: PruneOptions {
                older_than: None,
                archived_only: true,
                dry_run: false,
            },
            now: SystemTime::now(),
            liveness: dead,
        })
        .unwrap();

        assert_eq!(report.removed, vec!["20260601-archived"]);
        let candidates_by_id: std::collections::HashMap<_, _> = report
            .candidates
            .iter()
            .map(|c| (c.id.clone(), c))
            .collect();
        assert!(matches!(
            candidates_by_id["20260602-not-archived"].skip,
            Some(SkipReason::NotArchived)
        ));
        assert!(
            tmp.path()
                .join("20260602-not-archived")
                .exists()
        );
    }

    #[test]
    fn missing_sessions_root_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let report = prune(&PruneInputs {
            sessions_root: &tmp
                .path()
                .join("does-not-exist"),
            operator_ledger: None,
            options: PruneOptions {
                older_than: None,
                archived_only: false,
                dry_run: false,
            },
            now: SystemTime::now(),
            liveness: dead,
        })
        .unwrap();
        assert!(report.candidates.is_empty());
        assert!(report.removed.is_empty());
    }

    #[test]
    fn dry_run_never_removes_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = seed_session(tmp.path(), "20260601-old", None);

        let report = prune(&PruneInputs {
            sessions_root: tmp.path(),
            operator_ledger: None,
            options: PruneOptions {
                older_than: Some(Duration::from_secs(0)),
                archived_only: false,
                dry_run: true,
            },
            now: SystemTime::now() + Duration::from_secs(86400),
            liveness: dead,
        })
        .unwrap();

        assert!(report.dry_run);
        assert!(report.removed.is_empty());
        assert!(dir.exists(), "dry-run must not remove anything");
        // Freed-bytes is the "would-be-freed" figure even in dry-run.
        assert_eq!(
            report
                .candidates
                .iter()
                .filter(|c| c.skip.is_none())
                .count(),
            1
        );
    }
}
