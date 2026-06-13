//! Integration tests for `sbagent maintain`.
//!
//! The GitHub-querying reconciler is covered in-process with `FakeGh`.
//! These binary tests cover process-boundary behavior that does not need
//! live GitHub: argument wiring, config loading, ledger projection, and
//! the all-terminal no-op path.

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

fn write_jsonl(path: &Path, lines: &[&str]) {
    let mut body = String::new();
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    std::fs::write(path, body).unwrap();
}

fn exec_maintain(config_path: &Path, args: &[&str]) -> Output {
    let mut cli = vec!["-c", config_path.to_str().unwrap(), "maintain"];
    cli.extend_from_slice(args);
    Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(&cli)
        .output()
        .expect("spawn sbagent maintain")
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "sbagent maintain failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn maintain_all_terminal_artifacts_exits_zero_without_publish_token() {
    let tmp = tempfile::tempdir().unwrap();
    let operator = tmp.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    let config_path = write_config(tmp.path(), &operator);

    write_jsonl(
        &operator.join("sessions.jsonl"),
        &[
            r#"{"kind":"session_completed","schema_version":3,"id":"20260609-120000-maintain","artifact_branch":"session/20260609-120000-maintain","artifact_sha":"cafebabecafebabecafebabecafebabecafebabe","started_at":"2026-06-09T12:00:00Z","finished_at":"2026-06-09T12:25:00Z","status":"succeeded","sbagent_version":"0.3.0","range":{"network":"mainnet"},"baseline_run_ids":[],"phase_durations_secs":{"baseline":1.0},"targets":[{"id":"target-1","family_id":"f","bucket":"block_processing","delivery_mode":"normal_pr","status":"accepted","pr_url":"https://github.com/stacks-bench-bot/stacks-core/pull/1"}]}"#,
        ],
    );
    write_jsonl(
        &operator.join("maintain.jsonl"),
        &[
            r#"{"schema_version":1,"kind":"pr_merged","observed_at":"2026-06-10T10:00:00Z","session_id":"20260609-120000-maintain","target_id":"target-1","family_id":"f","fix_signature":"target-1","pr_url":"https://github.com/stacks-bench-bot/stacks-core/pull/1","prior_state":"open","new_state":"merged","head_sha":"aaa"}"#,
        ],
    );

    let out = exec_maintain(&config_path, &["--dry-run"]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    assert_eq!(stdout, "no maintenance events; all observed artifacts terminal\n");
    assert!(
        !operator
            .join("maintain.jsonl.lock")
            .exists(),
        "no-op dry-run should not create lock sidecars"
    );
}
