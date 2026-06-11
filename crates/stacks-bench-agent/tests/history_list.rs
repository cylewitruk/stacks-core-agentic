//! Integration tests for `sbagent history list`.
//!
//! Drive the real `CARGO_BIN_EXE_sbagent` binary against a tempdir
//! operator with a hand-rolled `sessions.jsonl`. Covers the v6
//! Phase 3 acceptance:
//!
//! - 3-session fixture renders three rows with the canonical column set. Total
//!   wall-clock matches the sum of `phase_durations_secs` per session.
//! - Output is pure ASCII (no glyphs, no Unicode box-drawing, no ANSI escape
//!   codes when stdout is piped). Asserted by byte-for-byte equality against a
//!   checked-in expected string.
//! - `--limit 2` truncates to the two most-recent sessions.
//! - `--since 2026-W23` ISO-week filter narrows to one row.
//! - `--since 2026-06-01` calendar-date filter narrows to one row.
//! - Empty ledger prints `no sessions archived yet` and exits 0.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Build a config.toml setting `layout.operator_repo_root` to
/// `operator`. The history command resolves the ledger path off this
/// field via `Layout::require_operator_repo_root`.
fn write_config(workspace: &Path, operator: &Path) -> PathBuf {
    let config_path = workspace.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[layout]\noperator_repo_root = \"{}\"\nagent_workspace_root = \"{}\"\n",
            operator.display(),
            workspace.display(),
        ),
    )
    .unwrap();
    config_path
}

fn write_sessions_jsonl(operator: &Path, lines: &[String]) {
    std::fs::create_dir_all(operator).unwrap();
    let path = operator.join("sessions.jsonl");
    let mut body = String::new();
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    std::fs::write(path, body).unwrap();
}

fn exec_history(config_path: &Path, args: &[&str]) -> Output {
    let mut cli = vec!["-c", config_path.to_str().unwrap(), "history", "list"];
    cli.extend_from_slice(args);
    Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(&cli)
        // Force the no-color path even if some hypothetical test
        // runner attaches a PTY. Belt-and-braces — Command::output()
        // already pipes stdout, but the explicit env makes the
        // intent obvious in the test.
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn sbagent history list")
}

/// Build a fixture session line:
///
/// - `started_at` controls sort order + `--since` filtering.
/// - `wall_secs` populates `phase_durations_secs.baseline` (so the wall-clock
///   column sums to that value).
/// - `targets` is `(accepted, rejected, aborted, failed, prs, issues)`.
fn fixture_line(
    id: &str,
    started_at: &str,
    status: &str,
    wall_secs: f64,
    targets: (u32, u32, u32, u32, u32, u32),
) -> String {
    let (acc, rej, abrt, fail, prs, issues) = targets;
    let mut tgs = Vec::new();
    let mut pr_left = prs;
    let mut issue_left = issues;
    let mut emit = |status_str: &str, stage: Option<&str>, kind: char, idx: u32| {
        let stage_field = stage
            .map(|s| format!(r#","status_stage":"{s}""#))
            .unwrap_or_default();
        let pr_field = if pr_left > 0 {
            pr_left -= 1;
            r#","pr_url":"https://example.com/pr/1""#
        } else {
            ""
        };
        let issue_field = if issue_left > 0 {
            issue_left -= 1;
            r#","issue_url":"https://example.com/i/1""#
        } else {
            ""
        };
        tgs.push(format!(
            r#"{{"id":"t-{kind}-{idx}","family_id":"f","bucket":"block_processing","delivery_mode":"normal_pr","status":"{status_str}"{stage_field}{pr_field}{issue_field}}}"#,
        ));
    };
    for i in 0..acc {
        emit("accepted", None, 'a', i);
    }
    for i in 0..rej {
        emit("rejected", Some("bench"), 'r', i);
    }
    for i in 0..abrt {
        emit("aborted", Some("merge"), 'b', i);
    }
    for i in 0..fail {
        emit("failed", Some("bench"), 'f', i);
    }
    let targets_json = tgs.join(",");
    format!(
        r#"{{"kind":"session_completed","schema_version":3,"id":"{id}","artifact_branch":"session/{id}","artifact_sha":"cafebabecafebabecafebabecafebabecafebabe","started_at":"{started_at}","finished_at":"2026-06-09T13:00:00Z","status":"{status}","sbagent_version":"0.3.0","range":{{"network":"mainnet"}},"baseline_run_ids":[],"phase_durations_secs":{{"baseline":{wall_secs}}},"targets":[{targets_json}]}}"#,
    )
}

/// Three sessions with deterministic values designed for byte-equality
/// checking. Sorted ascending here; the command sorts descending.
fn three_session_fixture() -> Vec<String> {
    vec![
        // Oldest: aborted, 1 target (aborted), 30s wall, no urls.
        fixture_line(
            "20260528-180000",
            "2026-05-28T18:00:00Z",
            "aborted",
            30.0,
            (0, 0, 1, 0, 0, 0),
        ),
        // Mid: failed, 2 targets (2 aborted via Failed), 1 pr, 1 issue,
        // 1h23m wall.
        fixture_line(
            "20260605-090000",
            "2026-06-05T09:00:00Z",
            "failed",
            4980.0, // 1:23:00
            (0, 0, 0, 2, 1, 1),
        ),
        // Newest: succeeded, 3 targets (2 acc, 1 rej), 2 prs, 0 issues,
        // 5m wall.
        fixture_line(
            "20260609-120000",
            "2026-06-09T12:00:00Z",
            "succeeded",
            300.0,
            (2, 1, 0, 0, 2, 0),
        ),
    ]
}

const EXPECTED_DEFAULT_TABLE: &str = "\
id               started_at  status     targets  wall-clock  prs  issues
20260609-120000  2026-06-09  succeeded  2/1/0    5:00        2    0
20260605-090000  2026-06-05  failed     0/0/2    1:23:00     1    1
20260528-180000  2026-05-28  aborted    0/0/1    0:30        0    0
";

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "sbagent history list failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn history_list_renders_three_session_fixture_as_ascii() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &three_session_fixture());

    let out = exec_history(&config_path, &[]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert_eq!(stdout, EXPECTED_DEFAULT_TABLE, "stdout did not match expected fixture");

    // Pure ASCII contract: every byte fits the 7-bit range and there
    // are no ANSI escape codes (CSI introducer `\x1b[`).
    for b in stdout.as_bytes() {
        assert!(*b < 0x80, "non-ASCII byte 0x{b:02x} in stdout");
    }
    assert!(!stdout.contains("\x1b["), "found ANSI escape in stdout");
}

#[test]
fn history_list_limit_truncates_to_most_recent() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &three_session_fixture());

    let out = exec_history(&config_path, &["--limit", "2"]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    let data_lines: Vec<&str> = stdout
        .lines()
        .skip(1) // header
        .collect();
    assert_eq!(data_lines.len(), 2);
    assert!(data_lines[0].starts_with("20260609-120000"), "got: {}", data_lines[0]);
    assert!(data_lines[1].starts_with("20260605-090000"), "got: {}", data_lines[1]);
    // The oldest session must NOT appear.
    assert!(!stdout.contains("20260528-180000"));
}

#[test]
fn history_list_since_iso_week_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &three_session_fixture());

    // 2026-W24 starts Monday 2026-06-08 → drops the 2026-06-05 and
    // 2026-05-28 sessions; keeps the 2026-06-09 one.
    let out = exec_history(&config_path, &["--since", "2026-W24"]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert!(stdout.contains("20260609-120000"));
    assert!(!stdout.contains("20260605-090000"));
    assert!(!stdout.contains("20260528-180000"));

    // Sanity: 2026-W23 (Monday 2026-06-01) keeps both 06-09 and 06-05,
    // drops only the 05-28 session. Confirms the boundary date is
    // inclusive ("on or after").
    let out = exec_history(&config_path, &["--since", "2026-W23"]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert!(stdout.contains("20260609-120000"));
    assert!(stdout.contains("20260605-090000"));
    assert!(!stdout.contains("20260528-180000"));
}

#[test]
fn history_list_since_calendar_date_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &three_session_fixture());

    // --since 2026-06-01 keeps the 2026-06-09 and 2026-06-05
    // sessions; drops the 2026-05-28 one.
    let out = exec_history(&config_path, &["--since", "2026-06-01"]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert!(stdout.contains("20260609-120000"));
    assert!(stdout.contains("20260605-090000"));
    assert!(!stdout.contains("20260528-180000"));
}

#[test]
fn history_list_empty_ledger_prints_friendly_message_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    // Empty file (not missing — exercises both the "file exists but
    // empty" and "no records survived" paths).
    std::fs::create_dir_all(&operator).unwrap();
    std::fs::write(operator.join("sessions.jsonl"), "").unwrap();

    let out = exec_history(&config_path, &[]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert_eq!(stdout.trim(), "no sessions archived yet");
}

#[test]
fn history_list_missing_ledger_prints_friendly_message_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    // Operator dir exists but no sessions.jsonl yet. (Simulates a
    // fresh operator who hasn't archived any sessions.)
    std::fs::create_dir_all(&operator).unwrap();

    let out = exec_history(&config_path, &[]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert_eq!(stdout.trim(), "no sessions archived yet");
}
