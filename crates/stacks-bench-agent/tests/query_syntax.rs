//! Syntax probes for paired results-analyzer SQL.
//!
//! These tests do not need a real stacks-bench DB. They create the minimal
//! table shapes referenced by the query files and ask the sqlite3 CLI to parse
//! and execute each query against an empty in-memory database.

use std::io::Write as _;
use std::process::{Command, Stdio};

fn minimal_schema() -> &'static str {
    r#"
CREATE TABLE benchmark_run (
  id INTEGER PRIMARY KEY,
  run_name TEXT,
  start_time TEXT,
  end_time TEXT,
  git_commit_hash BLOB,
  git_branch TEXT,
  git_dirty INTEGER,
  build_profile TEXT,
  build_opt_level TEXT,
  build_rustc_version TEXT,
  args_json TEXT
);
CREATE TABLE stacks_block_stats (
  benchmark_run_id INTEGER,
  setup_duration_us INTEGER,
  execution_duration_us INTEGER,
  commit_duration_us INTEGER,
  commit_overhead_baseline_us INTEGER,
  total_duration_us INTEGER
);
CREATE TABLE stacks_tx_stats (
  benchmark_run_id INTEGER
);
CREATE TABLE profiler_record (
  benchmark_run_id INTEGER
);
CREATE TABLE profiler_span (
  id INTEGER PRIMARY KEY,
  context TEXT,
  name TEXT
);
CREATE TABLE profiler_span_summary (
  benchmark_run_id INTEGER,
  profiler_span_id INTEGER,
  self_wall_time_us INTEGER,
  total_wall_time_us INTEGER,
  call_count INTEGER
);
"#
}

fn run_query(path: &str, extra_params: &[(&str, &str)]) {
    let query_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("queries")
        .join(path);
    let mut script = String::from(minimal_schema());
    script.push_str("\n.parameter init\n");
    script.push_str(".parameter set :baseline_run_id 1\n");
    script.push_str(".parameter set :candidate_run_id 2\n");
    for (key, value) in extra_params {
        script.push_str(&format!(".parameter set {key} {value}\n"));
    }
    script.push_str(&format!(".read {}\n", query_path.display()));

    let mut child = Command::new("sqlite3")
        .arg(":memory:")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sqlite3");
    child
        .stdin
        .as_mut()
        .expect("sqlite stdin")
        .write_all(script.as_bytes())
        .expect("write sqlite script");
    let output = child
        .wait_with_output()
        .expect("wait sqlite3");
    assert!(
        output.status.success(),
        "{path} failed sqlite syntax probe\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn paired_results_analyzer_queries_parse_under_sqlite() {
    run_query("compare_run_summary.sql", &[]);
    run_query("compare_block_timing_between_runs.sql", &[]);
    run_query("compare_spans_between_runs.sql", &[(":span_name", "'RollbackWrapper::lookup'")]);
}
