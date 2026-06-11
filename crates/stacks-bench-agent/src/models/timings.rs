//! `<session>/results/timings.json` — per-session wall-clock phase
//! timings, written incrementally during a full-pipeline run.
//!
//! Each phase wrapper records its elapsed wall-clock seconds via
//! [`PhaseTimingsRecorder::record`] **after** the phase completes,
//! which atomically rewrites the on-disk file. A session that
//! crashes mid-pipeline leaves a partial `timings.json` carrying
//! entries for every phase that finished before the crash — useful
//! for triaging hung or aborted sessions without having to
//! reconstruct timing from stderr.
//!
//! Sister field:
//! [`crate::models::session_record::SessionRecord::phase_durations_secs`]
//! — populated by archive from this file.
//!
//! Schema: v1, content is `{schema_version: 1, durations: {phase →
//! seconds_f64}}`. `durations` is a `BTreeMap` so the serialized
//! JSON is deterministic.
//!
//! Canonical phase keys (per
//! [`crate::models::session_record::SessionRecord::phase_durations_secs`]):
//! `baseline`, `triage`, `analysis`, `merge`, `optimize`, `bench`,
//! `finalize`, `publish`. Sub-phases that share a name (e.g. 0a + 0b
//! → `baseline`, 1.8 calibration → `bench`, 3.5 results-analyzer →
//! `finalize`) accumulate via `record`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::SchemaVersionV1;

/// v1 of the timings record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Timings {
    /// Constant: 1.
    pub schema_version: SchemaVersionV1,
    /// Wall-clock seconds per phase. Keys: see module doc.
    pub durations: BTreeMap<String, f64>,
}

impl Timings {
    /// Read + parse `<session>/results/timings.json`. Returns `None`
    /// (not an error) when the file doesn't exist, so callers like
    /// archive can fall back to an empty map for legacy sessions
    /// that predate this contract.
    pub fn read_optional(path: &Path) -> Result<Option<Self>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(anyhow::Error::new(e))
                    .with_context(|| format!("reading timings.json at {}", path.display()));
            }
        };
        let parsed: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parsing timings.json at {}", path.display()))?;
        Ok(Some(parsed))
    }

    /// Atomic rewrite via temp-file-then-rename. Unlike
    /// [`crate::models::source::SourceJson::write`], this is **not**
    /// write-once — each phase wrapper rewrites the file with the
    /// updated map. Same-dir temp + rename is atomic on every POSIX
    /// fs we care about, so a reader interrupting a writer always
    /// sees the previous coherent snapshot (or the new one), never
    /// a partial write.
    pub fn write_atomic(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .with_context(|| format!("timings.json path has no parent: {}", path.display()))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating timings.json parent {}", parent.display()))?;
        let pretty = serde_json::to_string_pretty(self).context("serializing Timings to JSON")?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".timings.json.")
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
        // `persist` (not `persist_noclobber`) — rewrites on every
        // phase completion.
        tmp.persist(path)
            .map_err(|e| {
                anyhow::anyhow!("persisting timings.json {}: {}", path.display(), e.error)
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_json() {
        let mut t = Timings::default();
        t.durations
            .insert("baseline".to_owned(), 295.6);
        t.durations
            .insert("triage".to_owned(), 12.4);
        let json = serde_json::to_string(&t).unwrap();
        let back: Timings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = r#"{"schema_version":1,"durations":{},"bogus":42}"#;
        assert!(serde_json::from_str::<Timings>(bad).is_err());
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let bad = r#"{"schema_version":2,"durations":{}}"#;
        let err = serde_json::from_str::<Timings>(bad).unwrap_err();
        assert!(format!("{err}").contains("schema_version"));
    }

    #[test]
    fn read_optional_returns_none_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp
            .path()
            .join("does-not-exist.json");
        assert!(
            Timings::read_optional(&missing)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn write_then_read_round_trips_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("timings.json");
        let mut original = Timings::default();
        original
            .durations
            .insert("baseline".to_owned(), 100.0);
        original
            .write_atomic(&path)
            .unwrap();

        let read_back = Timings::read_optional(&path)
            .unwrap()
            .unwrap();
        assert_eq!(read_back, original);
    }

    /// Incremental writes: simulate a phase wrapper rewriting the
    /// file after each phase completes. The file must always reflect
    /// the latest snapshot — including a phase that crashes after
    /// recording leaves the previous-phase entries on disk.
    #[test]
    fn write_atomic_supports_incremental_rewrites() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("timings.json");

        let mut t = Timings::default();
        t.durations
            .insert("baseline".to_owned(), 100.0);
        t.write_atomic(&path).unwrap();

        t.durations
            .insert("triage".to_owned(), 50.0);
        t.write_atomic(&path).unwrap();

        let read_back = Timings::read_optional(&path)
            .unwrap()
            .unwrap();
        assert_eq!(read_back.durations.len(), 2);
        assert_eq!(read_back.durations["baseline"], 100.0);
        assert_eq!(read_back.durations["triage"], 50.0);
    }
}
