//! Integration tests for [`stacks_bench_agent::session::ledger_reader`].
//!
//! Three v6-Phase-2 acceptance round-trips:
//!
//! - 5 valid records → `records.len() == 5, skipped.len() == 0`.
//! - 3 valid + 2 malformed → `records.len() == 3, skipped.len() == 2` with each
//!   `line_number` matching the actual 1-indexed source position; `error`
//!   carries the underlying parse message.
//! - v1 + v2 + v3 records on the same file → all three read transparently
//!   (regression coverage for the schema-version compat pattern v4
//!   established).
//!
//! Plus the two side guarantees the reader contract specifies:
//!
//! - Missing file ⇒ empty report (not Err).
//! - The reader function emits NO stderr on its own. The skipped-line warning
//!   is the CLI's job (Phase 3 / Phase 4).

use std::fs;

use stacks_bench_agent::session::ledger_reader::{LedgerReadReport, read_all, session_total_secs};

/// Render a synthetic v3 session record line. Keeps the fixture
/// hand-rolled JSON close to one place so each test reads cleanly.
fn v3_line(id: &str, baseline_secs: f64) -> String {
    format!(
        r#"{{"kind":"session_completed","schema_version":3,"id":"{id}","artifact_branch":"session/{id}","artifact_sha":"cafebabecafebabecafebabecafebabecafebabe","started_at":"2026-06-09T12:00:00Z","finished_at":"2026-06-09T12:15:00Z","status":"succeeded","sbagent_version":"0.3.0","range":{{"network":"mainnet"}},"baseline_run_ids":[],"phase_durations_secs":{{"baseline":{baseline_secs}}},"targets":[]}}"#,
    )
}

#[test]
fn read_all_missing_file_returns_empty_report() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("sessions.jsonl");
    let report = read_all(&path).expect("missing file is not an error");
    assert!(report.records.is_empty());
    assert!(report.skipped.is_empty());
}

#[test]
fn read_all_five_valid_records_returns_five_records_zero_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("sessions.jsonl");
    let mut body = String::new();
    for i in 0..5 {
        body.push_str(&v3_line(&format!("20260609-{i:06}"), 100.0 + i as f64));
        body.push('\n');
    }
    fs::write(&path, body).unwrap();

    let LedgerReadReport { records, skipped } = read_all(&path).unwrap();
    assert_eq!(records.len(), 5);
    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(records[0].id, "20260609-000000");
    assert_eq!(records[4].id, "20260609-000004");
}

#[test]
fn read_all_mixed_valid_and_malformed_records_skips_with_correct_line_numbers() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("sessions.jsonl");

    // Layout:
    // line 1: valid v3
    // line 2: garbage (not JSON)
    // line 3: valid v3
    // line 4: JSON object with an unknown top-level field (rejected by
    // deny_unknown_fields) line 5: valid v3
    // Empty/whitespace-only lines should NOT count as skipped.
    let mut body = String::new();
    body.push_str(&v3_line("20260609-000001", 100.0));
    body.push('\n');
    body.push_str("this is not valid json\n");
    body.push_str(&v3_line("20260609-000003", 200.0));
    body.push('\n');
    // Unsupported schema_version trips from_ledger_line's bail!.
    body.push_str(
        r#"{"kind":"session_completed","schema_version":99,"id":"future","artifact_branch":"x","artifact_sha":"x","started_at":"x","finished_at":"x","status":"succeeded","sbagent_version":"x","range":{"network":"mainnet"},"baseline_run_ids":[],"phase_durations_secs":{},"targets":[]}"#,
    );
    body.push('\n');
    body.push_str(&v3_line("20260609-000005", 300.0));
    body.push('\n');
    fs::write(&path, body).unwrap();

    let LedgerReadReport { records, skipped } = read_all(&path).unwrap();
    assert_eq!(
        records.len(),
        3,
        "records={:?}",
        records
            .iter()
            .map(|r| &r.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(skipped.len(), 2, "skipped={skipped:?}");

    assert_eq!(skipped[0].line_number, 2);
    assert!(!skipped[0].error.is_empty(), "skipped[0].error empty");
    assert_eq!(skipped[1].line_number, 4);
    assert!(
        skipped[1]
            .error
            .contains("schema_version"),
        "skipped[1].error = {:?}",
        skipped[1].error,
    );

    // Record IDs preserve file order.
    assert_eq!(records[0].id, "20260609-000001");
    assert_eq!(records[1].id, "20260609-000003");
    assert_eq!(records[2].id, "20260609-000005");
}

#[test]
fn read_all_handles_v1_v2_and_v3_records_on_the_same_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("sessions.jsonl");

    // Hand-rolled v1: no source_* fields.
    let v1 = r#"{"kind":"session_completed","schema_version":1,"id":"v1-record","artifact_branch":"session/v1-record","artifact_sha":"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef","started_at":"2026-05-18T19:03:21Z","finished_at":"2026-05-18T19:11:42Z","status":"succeeded","sbagent_version":"0.1.0","range":{"network":"mainnet"},"baseline_run_ids":[],"phase_durations_secs":{},"targets":[]}"#;

    // Hand-rolled v2: carries stacks_core_base_sha (stripped by from_ledger_line).
    let v2 = r#"{"kind":"session_completed","schema_version":2,"id":"v2-record","artifact_branch":"session/v2-record","artifact_sha":"abcabcabcabcabcabcabcabcabcabcabcabcabca","started_at":"2026-06-07T10:44:00Z","finished_at":"2026-06-07T11:00:00Z","status":"succeeded","sbagent_version":"0.2.0","stacks_core_base_sha":"f4cab0a011e273e1f1f9b5249afee2fba6123da5","range":{"network":"mainnet"},"baseline_run_ids":[42],"phase_durations_secs":{"baseline":300.0},"targets":[],"source_url":"https://github.com/stacks-network/stacks-core.git","source_branch":"feat/stacks-bench","source_sha":"0ad33704c259da4102b5f195617760003ac89c18","source_fetched_at":"2026-06-07T10:43:59Z"}"#;

    // Native v3 carrying source_* fields (the test specifically
    // exercises the v3 fields land in `records[2]`; the bare
    // `v3_line` helper omits them).
    let v3 = r#"{"kind":"session_completed","schema_version":3,"id":"v3-record","artifact_branch":"session/v3-record","artifact_sha":"cafebabecafebabecafebabecafebabecafebabe","started_at":"2026-06-09T12:00:00Z","finished_at":"2026-06-09T12:15:00Z","status":"succeeded","sbagent_version":"0.3.0","range":{"network":"mainnet"},"baseline_run_ids":[100],"phase_durations_secs":{"baseline":280.0},"targets":[],"source_url":"https://github.com/cylewitruk/stacks-core.git","source_branch":"feat/stacks-bench","source_sha":"1234567890abcdef1234567890abcdef12345678","source_fetched_at":"2026-06-09T11:59:59Z"}"#.to_owned();

    let mut body = String::new();
    body.push_str(v1);
    body.push('\n');
    body.push_str(v2);
    body.push('\n');
    body.push_str(&v3);
    body.push('\n');
    fs::write(&path, body).unwrap();

    let LedgerReadReport { records, skipped } = read_all(&path).unwrap();
    assert_eq!(records.len(), 3);
    assert!(skipped.is_empty(), "{skipped:?}");

    // All three promoted to v3 in-memory.
    assert!(
        records
            .iter()
            .all(|r| r.schema_version.get() == 3)
    );
    assert_eq!(records[0].id, "v1-record");
    assert_eq!(records[1].id, "v2-record");
    assert_eq!(records[2].id, "v3-record");

    // v1 has no source fields; v2 + v3 do.
    assert!(
        records[0]
            .source_url
            .is_none()
    );
    assert_eq!(
        records[1]
            .source_url
            .as_deref(),
        Some("https://github.com/stacks-network/stacks-core.git"),
    );
    assert!(
        records[2]
            .source_url
            .is_some()
    );
}

#[test]
fn session_total_secs_sums_phase_durations_values() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("sessions.jsonl");
    // Custom multi-phase line to exercise the sum.
    let line = r#"{"kind":"session_completed","schema_version":3,"id":"multi-phase","artifact_branch":"session/multi-phase","artifact_sha":"cafebabecafebabecafebabecafebabecafebabe","started_at":"2026-06-09T12:00:00Z","finished_at":"2026-06-09T12:15:00Z","status":"succeeded","sbagent_version":"0.3.0","range":{"network":"mainnet"},"baseline_run_ids":[],"phase_durations_secs":{"baseline":120.5,"triage":30.25,"optimize":600.0},"targets":[]}"#;
    fs::write(&path, format!("{line}\n")).unwrap();

    let report = read_all(&path).unwrap();
    let total = session_total_secs(&report.records[0]);
    assert!((total - 750.75).abs() < 1e-9, "total={total}");
}

#[test]
fn read_all_blank_lines_do_not_count_as_skipped() {
    // Blank-line tolerance is part of the contract: an editor that
    // adds a trailing newline shouldn't show up as a "1 skipped line"
    // warning in `history list`. Same for hand-edited blank lines
    // between records.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("sessions.jsonl");
    let body = format!("\n{}\n   \n{}\n\n", v3_line("first", 100.0), v3_line("second", 200.0),);
    fs::write(&path, body).unwrap();

    let LedgerReadReport { records, skipped } = read_all(&path).unwrap();
    assert_eq!(records.len(), 2);
    assert!(skipped.is_empty(), "{skipped:?}");
}
