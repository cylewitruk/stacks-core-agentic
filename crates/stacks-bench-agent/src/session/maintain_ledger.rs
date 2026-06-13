//! Typed reader/writer for `<operator>/maintain.jsonl`.
//!
//! Mirrors [`crate::session::ledger_reader`] for `sessions.jsonl`:
//! reads are lossy by default (bad lines land in-band), and warning
//! emission is the CLI's job. Writes are append-only compact JSONL.

use std::path::Path;
use std::{fs, io};

use anyhow::{Context as _, Result};

use crate::models::maintain_event::MaintEvent;
use crate::models::{ToJson, ValidateModel};

/// Result of one maintain-ledger read.
#[derive(Debug, Default)]
pub struct MaintLedgerReport {
    /// Events that parsed cleanly, in file order.
    pub events: Vec<MaintEvent>,
    /// One entry per non-blank line that failed to parse.
    pub skipped: Vec<SkippedMaintLine>,
}

/// One unparseable line, captured so callers can warn without the
/// reader touching stderr.
#[derive(Debug, Clone)]
pub struct SkippedMaintLine {
    /// 1-indexed position in the source file.
    pub line_number: usize,
    /// Rendered parse/validation error.
    pub error: String,
}

/// Read every event from `path`. Missing file ⇒ empty report.
pub fn read_all(path: &Path) -> Result<MaintLedgerReport> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(MaintLedgerReport::default());
        }
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("reading maintain.jsonl at {}", path.display())));
        }
    };

    let mut report = MaintLedgerReport::default();
    for (idx, line) in raw.lines().enumerate() {
        let line_number = idx + 1;
        if line.trim().is_empty() {
            continue;
        }
        match MaintEvent::from_ledger_line(line) {
            Ok(event) => report.events.push(event),
            Err(e) => report
                .skipped
                .push(SkippedMaintLine {
                    line_number,
                    error: format!("{e:#}"),
                }),
        }
    }
    Ok(report)
}

/// Append one validated compact JSONL record, creating the file and
/// parent directory if needed.
pub fn append_event(path: &Path, event: &MaintEvent) -> Result<()> {
    event
        .validate_model()
        .context("validating maintain event before append")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating maintain.jsonl parent {}", parent.display()))?;
    }
    let mut line = event
        .to_json()
        .context("serializing maintain event")?;
    line.push('\n');
    use std::io::Write as _;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening maintain.jsonl for append at {}", path.display()))?;
    f.write_all(line.as_bytes())
        .with_context(|| format!("appending maintain event to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::common::SchemaVersionV1;
    use crate::models::maintain_event::MaintEventKind;

    fn event(kind: MaintEventKind, state: &str) -> MaintEvent {
        MaintEvent {
            schema_version: SchemaVersionV1,
            kind,
            observed_at: "2026-06-13T12:00:00Z".to_owned(),
            session_id: "20260611-172955".to_owned(),
            target_id: Some("target-a".to_owned()),
            family_id: Some("family-a".to_owned()),
            fix_signature: Some("target-a".to_owned()),
            pr_url: Some("https://github.com/owner/repo/pull/1".to_owned()),
            issue_url: None,
            prior_state: None,
            new_state: state.to_owned(),
            head_sha: Some("abc123".to_owned()),
        }
    }

    #[test]
    fn missing_file_returns_empty_report() {
        let tmp = tempfile::tempdir().unwrap();
        let report = read_all(
            &tmp.path()
                .join("maintain.jsonl"),
        )
        .unwrap();
        assert!(report.events.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn append_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("maintain.jsonl");
        let first = event(MaintEventKind::PrOpen, "open");
        let second = event(MaintEventKind::PrMerged, "merged");
        append_event(&path, &first).unwrap();
        append_event(&path, &second).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 2);
        let report = read_all(&path).unwrap();
        assert_eq!(report.events, vec![first, second]);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn lossy_reader_reports_bad_nonblank_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("maintain.jsonl");
        let good = event(MaintEventKind::PrOpen, "open");
        std::fs::write(&path, format!("{}\n\nnot-json\n", serde_json::to_string(&good).unwrap()))
            .unwrap();
        let report = read_all(&path).unwrap();
        assert_eq!(report.events, vec![good]);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].line_number, 3);
    }

    #[test]
    fn append_rejects_invalid_event() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("maintain.jsonl");
        let mut bad = event(MaintEventKind::PrOpen, "open");
        bad.pr_url = None;
        let err = append_event(&path, &bad).unwrap_err();
        assert!(format!("{err:#}").contains("validating maintain event"));
        assert!(!path.exists());
    }
}
