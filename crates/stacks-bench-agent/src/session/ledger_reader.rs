//! Typed reader for `<operator>/sessions.jsonl`.
//!
//! Single source-of-truth for `sbagent history list`, `sbagent history
//! show`, and any future cross-session tooling (e.g. `maintain`).
//!
//! **Default behavior is lossy.** Malformed lines land in
//! [`LedgerReadReport::skipped`] rather than failing the whole read, so
//! a long-running operator ledger stays readable even if a hand edit
//! or older tooling left one bad line behind. Callers that want strict
//! parse semantics check `report.skipped.is_empty()` after the call.
//!
//! Skipped-line warning emission is the CLI's responsibility, NOT the
//! reader's — `history list` / `history show` decide when to
//! `eprintln!`. Library consumers (future `maintain`, batch tooling)
//! stay quiet. The reader function does NOT touch stderr.

use std::path::Path;
use std::{fs, io};

use anyhow::Result;

use crate::models::session_record::SessionRecord;

/// Result of one ledger read. Always returned by value (no streaming);
/// operator ledgers stay small in practice.
#[derive(Debug, Default)]
pub struct LedgerReadReport {
    /// Records that parsed cleanly, in file order.
    pub records: Vec<SessionRecord>,
    /// One entry per non-blank line that failed to parse.
    pub skipped: Vec<SkippedLine>,
}

/// One unparseable line, captured so the CLI can warn about it.
#[derive(Debug, Clone)]
pub struct SkippedLine {
    /// 1-indexed position in the source file (matches editor jumps).
    pub line_number: usize,
    /// Underlying parse error rendered via `{:#}` (anyhow chain).
    pub error: String,
}

/// Read every record from `path`. Blank lines are silently skipped
/// (they aren't errors). Unparseable lines populate
/// [`LedgerReadReport::skipped`].
///
/// Missing file ⇒ empty report (not an error). This matches the
/// `ledger_contains_id` precedent in [`crate::session::archive`] and
/// gives `history list` a clean "no sessions archived yet" path.
///
/// Returns `Err` only for I/O failures other than `NotFound`
/// (permission denied, unreadable inode, etc.) — situations where
/// the operator would want a loud failure rather than an empty list.
pub fn read_all(path: &Path) -> Result<LedgerReadReport> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(LedgerReadReport::default());
        }
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("reading sessions.jsonl at {}", path.display())));
        }
    };

    let mut report = LedgerReadReport::default();
    for (idx, line) in raw.lines().enumerate() {
        let line_number = idx + 1;
        if line.trim().is_empty() {
            continue;
        }
        match SessionRecord::from_ledger_line(line) {
            Ok(rec) => report.records.push(rec),
            Err(e) => report
                .skipped
                .push(SkippedLine {
                    line_number,
                    error: format!("{e:#}"),
                }),
        }
    }
    Ok(report)
}

/// Total session wall-clock: the sum of `phase_durations_secs` values.
///
/// Tiny one-liner, but it gives every CLI command and future tool one
/// shared definition. v5 introduced the per-phase fields; v6's
/// `history list` `wall-clock` column and `history show`'s header
/// timing line both derive from this same sum.
pub fn session_total_secs(record: &SessionRecord) -> f64 {
    record
        .phase_durations_secs
        .values()
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_total_secs_sums_phase_durations() {
        // Use the v1 fixture from session_record's own tests as a
        // minimal valid record; mutate phase_durations to assert
        // the sum.
        let line = r#"{
            "kind":"session_completed","schema_version":3,
            "id":"x","artifact_branch":"session/x",
            "artifact_sha":"cafebabecafebabecafebabecafebabecafebabe",
            "started_at":"2026-06-09T12:00:00Z",
            "finished_at":"2026-06-09T12:15:00Z",
            "status":"succeeded","sbagent_version":"0.3.0",
            "range":{"network":"mainnet"},
            "baseline_run_ids":[],
            "phase_durations_secs":{"baseline":120.5,"triage":30.25,"optimize":600.0},
            "targets":[]
        }"#;
        let rec = SessionRecord::from_ledger_line(line).unwrap();
        let total = session_total_secs(&rec);
        assert!((total - 750.75).abs() < 1e-9, "total={total}");
    }

    #[test]
    fn session_total_secs_is_zero_for_empty_phase_map() {
        let line = r#"{
            "kind":"session_completed","schema_version":3,
            "id":"x","artifact_branch":"session/x",
            "artifact_sha":"cafebabecafebabecafebabecafebabecafebabe",
            "started_at":"2026-06-09T12:00:00Z",
            "finished_at":"2026-06-09T12:00:00Z",
            "status":"succeeded","sbagent_version":"0.3.0",
            "range":{"network":"mainnet"},
            "baseline_run_ids":[],
            "phase_durations_secs":{},
            "targets":[]
        }"#;
        let rec = SessionRecord::from_ledger_line(line).unwrap();
        assert_eq!(session_total_secs(&rec), 0.0);
    }
}
