//! Integration tests for `sbagent history show <session-id>`.
//!
//! Covers v6 Phase 4 acceptance:
//!
//! - Known id renders the three sections in order with the canonical layout.
//!   Byte-for-byte equality against a checked-in expected string (proves
//!   pure-ASCII, no Unicode, no ANSI).
//! - Unknown id exits 1 with the documented error message.
//! - `NO_COLOR=1` suppresses ANSI escape codes (we can't simulate a real TTY in
//!   `Command::output()`, but the explicit env tightens the contract).
//! - Phase-duration bars are proportional by hash-character count (not by
//!   visual eyeball): a 1200:295 ratio yields ~4x bar.

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

fn exec_show(config_path: &Path, session_id: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(["-c", config_path.to_str().unwrap(), "history", "show", session_id])
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn sbagent history show")
}

/// Hand-rolled fixture session designed for predictable output:
///
/// - Three phase durations with a 1200:295:0.5 ratio. The bar-chart acceptance
///   check asserts the optimize bar is roughly 4x the baseline bar; 0.5s
///   renders as `< 1s`.
/// - Three targets covering accepted (with bench + pr_url), rejected (bench, no
///   urls), and aborted (no bench, no urls).
fn fixture_session_line() -> String {
    let targets = [
        // Accepted target with bench: improvement +5.32%, candidate
        // 135s wall (= 2:15), pr_url set.
        r#"{"id":"target-1","family_id":"f","bucket":"block_processing","delivery_mode":"normal_pr","status":"accepted","pr_url":"https://example.com/pr/1","bench":{"baseline_run_ids":[1],"candidate_run_ids":[2],"baseline_total_us":140000000,"candidate_total_us":135000000,"improvement_pct":5.32,"passes_noise_floor":true}}"#,
        // Rejected: improvement -1.25%, candidate 90s wall (= 1:30),
        // no urls.
        r#"{"id":"target-2","family_id":"f","bucket":"block_processing","delivery_mode":"normal_pr","status":"rejected","status_stage":"bench","bench":{"baseline_run_ids":[1],"candidate_run_ids":[3],"baseline_total_us":89000000,"candidate_total_us":90000000,"improvement_pct":-1.25,"passes_noise_floor":false}}"#,
        // Aborted: no bench, no urls.
        r#"{"id":"target-3","family_id":"f","bucket":"block_processing","delivery_mode":"normal_pr","status":"aborted","status_stage":"merge"}"#,
    ];
    let tgs = targets.join(",");
    format!(
        r#"{{"kind":"session_completed","schema_version":3,"id":"20260609-120000-show-fixture","artifact_branch":"session/20260609-120000-show-fixture","artifact_sha":"cafebabecafebabecafebabecafebabecafebabe","started_at":"2026-06-09T12:00:00Z","finished_at":"2026-06-09T12:25:00Z","status":"succeeded","sbagent_version":"0.3.0","range":{{"network":"mainnet"}},"baseline_run_ids":[],"phase_durations_secs":{{"baseline":295.0,"optimize":1200.0,"triage":0.5}},"targets":[{tgs}]}}"#,
    )
}

// Hand-rolled expected output. Uses a raw string so rustfmt can't
// inject line-continuations or trailing whitespace mid-literal —
// byte-equality is the test's whole point.
#[rustfmt::skip]
const EXPECTED_SHOW_OUTPUT: &str = r#"Session 20260609-120000-show-fixture
  started_at -> finished_at:  2026-06-09T12:00:00Z -> 2026-06-09T12:25:00Z
  status:                     succeeded

Phase durations
  optimize  1200.00s  ############################################################
  baseline   295.00s  ###############
  triage       0.50s  < 1s

Targets
  id        status    delivery_mode  improvement_pct  bench  url
  target-1  accepted  normal_pr      +5.32%           2:15   https://example.com/pr/1
  target-2  rejected  normal_pr      -1.25%           1:30   -
  target-3  aborted   normal_pr      -                -      -
"#;

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "sbagent history show failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn history_show_renders_three_sections_as_ascii() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &[fixture_session_line()]);

    let out = exec_show(&config_path, "20260609-120000-show-fixture");
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");

    assert_eq!(stdout, EXPECTED_SHOW_OUTPUT, "stdout did not match expected fixture");

    for b in stdout.as_bytes() {
        assert!(*b < 0x80, "non-ASCII byte 0x{b:02x} in stdout");
    }
    assert!(!stdout.contains("\x1b["), "found ANSI escape in stdout");
}

#[test]
fn history_show_unknown_id_exits_nonzero_with_documented_message() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &[fixture_session_line()]);

    let out = exec_show(&config_path, "20260101-000000-nope");
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no such session id `20260101-000000-nope`"),
        "stderr missing id reference:\n{stderr}",
    );
    assert!(stderr.contains("sbagent history list"), "stderr missing remediation hint:\n{stderr}");
}

#[test]
fn history_show_phase_bars_are_proportional_by_hash_count() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &[fixture_session_line()]);

    let out = exec_show(&config_path, "20260609-120000-show-fixture");
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");

    let bar_chars = |needle: &str| -> usize {
        let line = stdout
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line for {needle} in:\n{stdout}"));
        line.chars()
            .filter(|c| *c == '#')
            .count()
    };

    let optimize_bar = bar_chars("optimize");
    let baseline_bar = bar_chars("baseline");
    assert!(optimize_bar > 0, "optimize bar empty");
    assert!(baseline_bar > 0, "baseline bar empty");

    // 1200:295 ≈ 4.068. With BAR_WIDTH=60 the bars are 60 and 15
    // respectively → exact ratio of 4.0. Allow ±0.5 for rounding
    // robustness in case BAR_WIDTH ever shifts.
    let ratio = optimize_bar as f64 / baseline_bar as f64;
    assert!(
        (3.5..=4.5).contains(&ratio),
        "expected optimize/baseline bar ratio ~4 (1200s/295s); got {optimize_bar}/{baseline_bar} \
         = {ratio}",
    );
}

#[test]
fn history_show_no_color_suppresses_ansi_escapes() {
    // Best-effort assertion: `Command::output()` already pipes stdout
    // so use_color is false regardless. Setting NO_COLOR=1 nails the
    // contract bullet ("NO_COLOR=1 suppresses color escape codes even
    // on a TTY") at the env-var layer; the TTY-layer gate is covered
    // by the same code path.
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    let config_path = write_config(tmp.path(), &operator);
    write_sessions_jsonl(&operator, &[fixture_session_line()]);

    let out = exec_show(&config_path, "20260609-120000-show-fixture");
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert!(!stdout.contains("\x1b["), "ANSI escape leaked despite NO_COLOR=1");
}
