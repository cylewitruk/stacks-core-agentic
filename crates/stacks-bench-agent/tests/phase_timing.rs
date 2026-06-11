//! Phase-timing flow integration tests.
//!
//! The unit-test surface for `PhaseTimingsRecorder` + the `Timings`
//! model lives in those modules' inline tests. This file covers the
//! flow between them and the archive consumer:
//!
//! - A mock pipeline that records via `PhaseTimingsRecorder` writes a
//!   `timings.json` that round-trips through `Timings::read_optional`.
//! - The same `timings.json` is what `archive::build_session_record` reads to
//!   populate `SessionRecord.phase_durations_secs` — but that wiring is
//!   asserted in `tests/archive.rs` against a pre-staged fixture file.
//!
//! These tests don't drive `cli/session/run` end-to-end because that
//! would require a real session pipeline (chainstate, codex, etc.);
//! the contract between the recorder and archive is small enough to
//! cover with a fixture instead.

use std::time::Duration;

use stacks_bench_agent::models::timings::Timings;
use stacks_bench_agent::session::phase_timing::PhaseTimingsRecorder;

/// Mock pipeline: record canonical phases in the order
/// `cli/session/run.rs` calls them, then read the file back and
/// assert the on-disk shape matches the recorded sequence.
#[test]
fn mock_pipeline_round_trips_through_timings_json() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("timings.json");

    let mut rec = PhaseTimingsRecorder::new(path.clone());
    // Phase 0a + 0b → baseline (accumulates)
    rec.record("baseline", Duration::from_secs_f64(60.0))
        .unwrap();
    rec.record("baseline", Duration::from_secs_f64(235.0))
        .unwrap();
    // Phase 1
    rec.record("triage", Duration::from_secs_f64(12.0))
        .unwrap();
    // Phase 1.5
    rec.record("analysis", Duration::from_secs_f64(45.0))
        .unwrap();
    // Phase 1.7
    rec.record("merge", Duration::from_secs_f64(3.5))
        .unwrap();
    // Phase 1.8 + 3 → bench (accumulates)
    rec.record("bench", Duration::from_secs_f64(20.0))
        .unwrap();
    rec.record("bench", Duration::from_secs_f64(180.0))
        .unwrap();
    // Phase 2
    rec.record("optimize", Duration::from_secs_f64(900.0))
        .unwrap();
    // Phase 3.5 + 4 → finalize (accumulates)
    rec.record("finalize", Duration::from_secs_f64(30.0))
        .unwrap();
    rec.record("finalize", Duration::from_secs_f64(2.0))
        .unwrap();
    // Phase 5 (optional)
    rec.record("publish", Duration::from_secs_f64(5.0))
        .unwrap();

    let on_disk = Timings::read_optional(&path)
        .unwrap()
        .unwrap();
    // Canonical 8 keys present.
    let expected_keys =
        ["baseline", "triage", "analysis", "merge", "bench", "optimize", "finalize", "publish"];
    for k in &expected_keys {
        assert!(
            on_disk
                .durations
                .contains_key(*k),
            "expected canonical key `{k}` in timings.json; got {:?}",
            on_disk
                .durations
                .keys()
                .collect::<Vec<_>>(),
        );
    }
    // Accumulated keys carry the sum, not just the last write.
    assert!((on_disk.durations["baseline"] - 295.0).abs() < 1e-9);
    assert!((on_disk.durations["bench"] - 200.0).abs() < 1e-9);
    assert!((on_disk.durations["finalize"] - 32.0).abs() < 1e-9);
}

/// Crash-partial behavior: a recorder that exits after only the
/// first three phases leaves a `timings.json` carrying exactly
/// those three. The phase that was running at "crash time" doesn't
/// appear as a half-finished entry.
#[test]
fn crashed_pipeline_leaves_partial_timings_json_with_completed_phases() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("timings.json");
    {
        let mut rec = PhaseTimingsRecorder::new(path.clone());
        rec.record("baseline", Duration::from_secs_f64(100.0))
            .unwrap();
        rec.record("triage", Duration::from_secs_f64(10.0))
            .unwrap();
        rec.record("analysis", Duration::from_secs_f64(45.0))
            .unwrap();
        // Recorder dropped here — simulates a panic / OOM during
        // Phase 1.7 merge, before merge completes and records.
    }
    let on_disk = Timings::read_optional(&path)
        .unwrap()
        .unwrap();
    assert_eq!(on_disk.durations.len(), 3);
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
    assert!(
        on_disk
            .durations
            .contains_key("analysis")
    );
    // The phase that was running when the "crash" happened doesn't
    // produce an entry — there's no half-finished timing.
    assert!(
        !on_disk
            .durations
            .contains_key("merge")
    );
}

/// A session that never recorded any phase (crashed before Phase 0a
/// returned) writes nothing — and `read_optional` returns `None`,
/// which archive translates to an empty `phase_durations_secs` map
/// without erroring out.
#[test]
fn session_with_no_recorded_phases_has_no_timings_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("timings.json");

    // Construct the recorder but never call `record`.
    let _rec = PhaseTimingsRecorder::new(path.clone());
    drop(_rec);

    assert!(!path.exists(), "no record() calls → no file on disk");
    assert!(
        Timings::read_optional(&path)
            .unwrap()
            .is_none()
    );
}
