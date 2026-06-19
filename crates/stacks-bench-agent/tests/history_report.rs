//! Integration tests for `sbagent history report`.
//!
//! Covers v17 Phase 3 acceptance:
//!
//! - stdout default emits markdown with the documented sections.
//! - `--out <path>` writes the file, creates missing parent directories, and
//!   keeps stdout empty.
//! - `--since` filters by ISO date and ISO week id, identically to `history
//!   list`.
//! - Invalid `--since` exits non-zero with a clear diagnostic.
//! - Empty ledger renders the `no sessions archived yet` notice.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

fn exec_report(config_path: &Path, extra_args: &[&str]) -> Output {
    let mut args: Vec<&str> = vec!["-c", config_path.to_str().unwrap(), "history", "report"];
    args.extend_from_slice(extra_args);
    Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn sbagent history report")
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "sbagent history report failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Hand-rolled session JSON line matching the shape `sessions.jsonl`
/// carries in real ledgers (kind=session_completed, schema_version=3).
fn session_line(id: &str, started_at: &str, status: &str) -> String {
    format!(
        r#"{{"kind":"session_completed","schema_version":3,"id":"{id}","artifact_branch":"session/{id}","artifact_sha":"cafebabecafebabecafebabecafebabecafebabe","started_at":"{started_at}","finished_at":"{started_at}","status":"{status}","sbagent_version":"0.3.0","range":{{"network":"mainnet"}},"baseline_run_ids":[],"phase_durations_secs":{{}},"targets":[]}}"#,
    )
}

#[test]
fn history_report_stdout_default_prints_markdown_digest() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    // Two sessions in the same ISO week so the default cutoff picks
    // both up. ISO 2026-W23 = Mon 2026-06-01..Sun 2026-06-07.
    write_sessions_jsonl(
        &operator,
        &[
            session_line("a", "2026-06-02T00:00:00Z", "succeeded"),
            session_line("b", "2026-06-03T00:00:00Z", "failed"),
        ],
    );

    let out = exec_report(&config_path, &[]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");

    assert!(stdout.starts_with("# sbagent history report:"), "stdout missing title:\n{stdout}",);
    assert!(stdout.contains("## Summary"), "stdout missing Summary section:\n{stdout}");
    assert!(stdout.contains("## Sessions"), "stdout missing Sessions section:\n{stdout}");
    // Default cutoff label appears when --since is absent.
    assert!(
        stdout.contains("(default: latest ISO week)"),
        "stdout missing default-cutoff label:\n{stdout}",
    );
    for b in stdout.as_bytes() {
        assert!(*b < 0x80, "non-ASCII byte 0x{b:02x} in stdout");
    }
    assert!(!stdout.contains("\x1b["), "ANSI escape leaked into stdout");
}

#[test]
fn history_report_out_writes_file_and_keeps_stdout_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &[session_line("a", "2026-06-02T00:00:00Z", "succeeded")]);

    // Nested path proves `create_dir_all` runs before the write.
    let out_path = tmp
        .path()
        .join("reports")
        .join("2026-W23")
        .join("digest.md");
    let out =
        exec_report(&config_path, &["--out", out_path.to_str().unwrap(), "--since", "2026-06-01"]);
    assert_success(&out);
    assert!(
        out.stdout.is_empty(),
        "stdout should be empty when --out is set; got: {}",
        String::from_utf8_lossy(&out.stdout),
    );
    assert!(out_path.exists(), "--out file was not created: {}", out_path.display());

    let body = std::fs::read_to_string(&out_path).expect("read --out file");
    assert!(
        body.starts_with("# sbagent history report: since 2026-06-01\n"),
        "out file missing explicit-cutoff title:\n{body}",
    );
    assert!(body.contains("## Summary"), "out file missing Summary:\n{body}");
}

#[test]
fn history_report_since_filters_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(
        &operator,
        &[
            session_line("older", "2026-05-01T00:00:00Z", "succeeded"),
            session_line("newer", "2026-06-10T00:00:00Z", "succeeded"),
        ],
    );

    let out = exec_report(&config_path, &["--since", "2026-06-01"]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");

    assert!(stdout.contains("newer"), "stdout missing newer session id:\n{stdout}");
    assert!(!stdout.contains("older"), "stdout should not include older session id:\n{stdout}");

    // ISO week form should select the same Monday.
    let out_week = exec_report(&config_path, &["--since", "2026-W23"]);
    assert_success(&out_week);
    let stdout_week = String::from_utf8(out_week.stdout).expect("stdout is utf-8");
    assert!(
        stdout_week.contains("newer") && !stdout_week.contains("older"),
        "ISO week form did not produce the same selection:\n{stdout_week}",
    );
}

#[test]
fn history_report_invalid_since_exits_nonzero_with_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &[session_line("a", "2026-06-02T00:00:00Z", "succeeded")]);

    let out = exec_report(&config_path, &["--since", "garbage"]);
    assert!(!out.status.success(), "expected non-zero exit for invalid --since");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a recognized form"),
        "stderr missing diagnostic phrase:\n{stderr}",
    );
    assert!(
        stderr.contains("YYYY-MM-DD") && stderr.contains("YYYY-Www"),
        "stderr missing format hints:\n{stderr}",
    );
}

/// Hand-rolled session JSON with explicit `phase_durations_secs` so
/// the rendered wall-clock is deterministic for byte-equality
/// fixtures. 60 + 600 = 660s → `11:00`.
const FIXED_SESSION_LINE: &str = r#"{"kind":"session_completed","schema_version":3,"id":"fix-id","artifact_branch":"session/fix-id","artifact_sha":"cafebabecafebabecafebabecafebabecafebabe","started_at":"2026-06-02T00:00:00Z","finished_at":"2026-06-02T00:11:00Z","status":"succeeded","sbagent_version":"0.3.0","range":{"network":"mainnet"},"baseline_run_ids":[],"phase_durations_secs":{"baseline":60.0,"optimize":600.0},"targets":[]}"#;

/// Byte-for-byte expected output for [`fixed_session_line`] under an
/// explicit `--since 2026-06-01` cutoff. The same string drives both
/// the stdout byte-equality check and the `--out` byte-equality
/// check below, so stdout and file paths cannot drift.
#[rustfmt::skip]
const EXPECTED_REPORT: &str = "\
# sbagent history report: since 2026-06-01

## Summary

- Total sessions: 1 (succeeded: 1, failed: 0, aborted: 0)
- Targets: accepted 0 / rejected 0 / aborted 0
- Total wall-clock: 11:00

## Sessions

| id     | started_at | status    | targets | wall-clock | prs  | issues |
| ------ | ---------- | --------- | ------- | ---------- | ---- | ------ |
| fix-id | 2026-06-02 | succeeded | 0/0/0   | 11:00      | 0    | 0      |
";

#[test]
fn history_report_stdout_matches_byte_equality_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &[FIXED_SESSION_LINE.to_owned()]);

    let out = exec_report(&config_path, &["--since", "2026-06-01"]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert_eq!(stdout, EXPECTED_REPORT, "stdout did not match expected fixture");
}

#[test]
fn history_report_out_file_matches_byte_equality_fixture_and_stdout_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &[FIXED_SESSION_LINE.to_owned()]);

    let out_path = tmp
        .path()
        .join("reports")
        .join("digest.md");
    let out =
        exec_report(&config_path, &["--since", "2026-06-01", "--out", out_path.to_str().unwrap()]);
    assert_success(&out);
    assert!(
        out.stdout.is_empty(),
        "stdout should be empty when --out is set; got: {}",
        String::from_utf8_lossy(&out.stdout),
    );
    let body = std::fs::read_to_string(&out_path).expect("read --out file");
    assert_eq!(body, EXPECTED_REPORT, "--out file did not match expected fixture");
}

#[test]
fn history_report_empty_ledger_renders_no_sessions_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    // Operator dir exists with an empty sessions.jsonl.
    write_sessions_jsonl(&operator, &[]);
    let config_path = write_config(tmp.path(), &operator);

    let out = exec_report(&config_path, &[]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert!(stdout.contains("empty ledger"), "stdout missing empty-ledger title:\n{stdout}");
    assert!(stdout.contains("no sessions archived yet"), "stdout missing empty notice:\n{stdout}",);
}
