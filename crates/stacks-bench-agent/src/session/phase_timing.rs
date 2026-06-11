//! Phase timing recorder used by `cli/session/run.rs`.
//!
//! Wrap each phase call with a `let t = Instant::now(); /* phase */;
//! recorder.record("baseline", t.elapsed())?;` pair. The recorder
//! accumulates per-canonical-key durations and atomically rewrites
//! `<session>/results/timings.json` after each `record` call, so a
//! session that crashes mid-pipeline leaves a partial file carrying
//! every phase that completed before the crash.
//!
//! Multiple `record` calls on the same key accumulate (Phase 0a + 0b
//! both record under `baseline`; Phase 1.8 calibration records under
//! `bench`; Phase 3.5 results-analyzer records under `finalize`). The
//! canonical key set is enforced by
//! [`crate::models::session_record::SessionRecord::phase_durations_secs`].

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use crate::models::timings::Timings;

/// Phase-timing recorder.
///
/// Owns the path to `<session>/results/timings.json` and an
/// in-memory map of durations. Each `record` call accumulates into
/// the map and rewrites the file atomically.
pub struct PhaseTimingsRecorder {
    path: PathBuf,
    state: Timings,
}

impl PhaseTimingsRecorder {
    /// Construct a recorder for one session. Does NOT touch the
    /// filesystem yet; the file is created on the first successful
    /// `record` call. (A session that crashes before any phase
    /// completes leaves no `timings.json`, matching the read path's
    /// "absent file → empty map" expectation.)
    pub fn new(timings_json: PathBuf) -> Self {
        Self {
            path: timings_json,
            state: Timings::default(),
        }
    }

    /// Add `duration` (wall-clock elapsed) to the entry for `phase`,
    /// then atomically rewrite the on-disk file.
    ///
    /// Multiple calls on the same `phase` accumulate. The
    /// `f64`-seconds representation is what
    /// `SessionRecord.phase_durations_secs` carries, so the units
    /// match end-to-end without conversion at archive time.
    pub fn record(&mut self, phase: &str, duration: Duration) -> Result<()> {
        let secs = duration.as_secs_f64();
        self.state
            .durations
            .entry(phase.to_owned())
            .and_modify(|d| *d += secs)
            .or_insert(secs);
        self.state
            .write_atomic(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_first_call_creates_file_with_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("timings.json");
        let mut rec = PhaseTimingsRecorder::new(path.clone());
        rec.record("baseline", Duration::from_secs_f64(12.3))
            .unwrap();

        let on_disk = Timings::read_optional(&path)
            .unwrap()
            .unwrap();
        assert_eq!(on_disk.durations.len(), 1);
        assert!((on_disk.durations["baseline"] - 12.3).abs() < 1e-9);
    }

    #[test]
    fn record_accumulates_same_phase_across_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("timings.json");
        let mut rec = PhaseTimingsRecorder::new(path.clone());
        // Phase 0a + 0b both record under "baseline".
        rec.record("baseline", Duration::from_secs_f64(5.0))
            .unwrap();
        rec.record("baseline", Duration::from_secs_f64(7.0))
            .unwrap();

        let on_disk = Timings::read_optional(&path)
            .unwrap()
            .unwrap();
        assert!((on_disk.durations["baseline"] - 12.0).abs() < 1e-9);
    }

    #[test]
    fn record_distinct_phases_land_as_separate_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("timings.json");
        let mut rec = PhaseTimingsRecorder::new(path.clone());
        rec.record("baseline", Duration::from_secs_f64(10.0))
            .unwrap();
        rec.record("triage", Duration::from_secs_f64(3.0))
            .unwrap();

        let on_disk = Timings::read_optional(&path)
            .unwrap()
            .unwrap();
        assert_eq!(on_disk.durations.len(), 2);
    }

    /// Simulates a session that records two phases then "crashes"
    /// (drops the recorder). The on-disk file must carry both
    /// phases — that's the crash-partial behavior the iteration
    /// spec required.
    #[test]
    fn crashed_session_leaves_partial_file_with_completed_phases() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("timings.json");
        {
            let mut rec = PhaseTimingsRecorder::new(path.clone());
            rec.record("baseline", Duration::from_secs_f64(10.0))
                .unwrap();
            rec.record("triage", Duration::from_secs_f64(3.0))
                .unwrap();
            // "Crash" — recorder dropped before any later phase records.
        }
        let on_disk = Timings::read_optional(&path)
            .unwrap()
            .unwrap();
        assert_eq!(on_disk.durations.len(), 2);
        assert!(
            on_disk
                .durations
                .contains_key("baseline")
        );
        assert!(
            on_disk
                .durations
                .contains_key("triage")
        );
        // The phase that was running when the "crash" happened is
        // absent — there's no half-finished entry.
        assert!(
            !on_disk
                .durations
                .contains_key("analysis")
        );
    }
}
