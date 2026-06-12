//! Syntax probes for paired results-analyzer SQL.
//!
//! These tests do not need a real stacks-bench DB. They create the production
//! table shapes referenced by the paired query files and ask the sqlite3 CLI
//! to parse and execute each query against an empty in-memory database.

use std::io::Write as _;
use std::process::{Command, Stdio};

fn production_schema_subset() -> &'static str {
    r#"
CREATE TABLE benchmark_run (
  id INTEGER PRIMARY KEY NOT NULL,
  run_name TEXT,
  chainstate_id INTEGER NOT NULL,
  git_commit_hash BLOB NOT NULL,
  start_time TIMESTAMP NOT NULL,
  end_time TIMESTAMP,
  args_json TEXT NOT NULL,
  build_profile TEXT NOT NULL DEFAULT '',
  build_opt_level TEXT NOT NULL DEFAULT '',
  build_debug_assertions BOOLEAN NOT NULL DEFAULT 0,
  build_overflow_checks BOOLEAN NOT NULL DEFAULT 0,
  build_target_triple TEXT NOT NULL DEFAULT '',
  build_rustc_version TEXT NOT NULL DEFAULT '',
  git_branch TEXT,
  git_dirty BOOLEAN,
  CHECK(length(git_commit_hash) IN (20, 32))
);
CREATE TABLE stacks_block_stats (
  benchmark_run_id INTEGER NOT NULL,
  synthetic_block_id INTEGER NOT NULL,
  total_duration_us INTEGER NOT NULL,
  setup_duration_us INTEGER NOT NULL,
  execution_duration_us INTEGER NOT NULL,
  commit_duration_us INTEGER NOT NULL,
  commit_overhead_baseline_us INTEGER NOT NULL,
  clarity_write_length INTEGER NOT NULL,
  clarity_write_count INTEGER NOT NULL,
  clarity_read_length INTEGER NOT NULL,
  clarity_read_count INTEGER NOT NULL,
  clarity_runtime INTEGER NOT NULL,
  total_storage_delta INTEGER NOT NULL,
  PRIMARY KEY (benchmark_run_id, synthetic_block_id)
) WITHOUT ROWID;
CREATE TABLE stacks_tx_stats (
  benchmark_run_id INTEGER NOT NULL,
  stacks_tx_id INTEGER NOT NULL,
  synthetic_block_id INTEGER NOT NULL,
  duration_us INTEGER NOT NULL,
  clarity_write_length INTEGER NOT NULL,
  clarity_write_count INTEGER NOT NULL,
  clarity_read_length INTEGER NOT NULL,
  clarity_read_count INTEGER NOT NULL,
  clarity_runtime INTEGER NOT NULL,
  PRIMARY KEY (benchmark_run_id, synthetic_block_id, stacks_tx_id)
) WITHOUT ROWID;
CREATE TABLE profiler_record (
  id INTEGER PRIMARY KEY NOT NULL,
  benchmark_run_id INTEGER NOT NULL,
  parent_id INTEGER,
  profiler_span_id INTEGER NOT NULL,
  profiler_tag_id INTEGER,
  profiler_location_id INTEGER NOT NULL,
  child_index INTEGER NOT NULL,
  depth INTEGER NOT NULL,
  synthetic_block_id INTEGER NOT NULL,
  stacks_tx_id INTEGER,
  wall_time_us INTEGER NOT NULL,
  cpu_time_us INTEGER NOT NULL,
  self_wall_time_us INTEGER NOT NULL,
  self_cpu_time_us INTEGER NOT NULL,
  call_count INTEGER NOT NULL,
  sample_count INTEGER NOT NULL,
  expand_factor REAL GENERATED ALWAYS AS (
    CASE
      WHEN sample_count > 0 THEN (call_count * 1.0 / sample_count)
      ELSE NULL
    END
  ) VIRTUAL,
  est_wall_us REAL GENERATED ALWAYS AS (
    CASE WHEN sample_count > 0 THEN wall_time_us * (call_count * 1.0 / sample_count) END
  ) VIRTUAL,
  est_self_wall_us REAL GENERATED ALWAYS AS (
    CASE WHEN sample_count > 0 THEN self_wall_time_us * (call_count * 1.0 / sample_count) END
  ) VIRTUAL,
  est_cpu_us REAL GENERATED ALWAYS AS (
    CASE WHEN sample_count > 0 THEN cpu_time_us * (call_count * 1.0 / sample_count) END
  ) VIRTUAL,
  est_self_cpu_us REAL GENERATED ALWAYS AS (
    CASE WHEN sample_count > 0 THEN self_cpu_time_us * (call_count * 1.0 / sample_count) END
  ) VIRTUAL
);
CREATE TABLE profiler_span (
  id INTEGER PRIMARY KEY NOT NULL,
  context TEXT,
  name TEXT NOT NULL,
  UNIQUE(context, name)
);
CREATE TABLE profiler_span_summary (
  benchmark_run_id INTEGER NOT NULL,
  profiler_span_id INTEGER NOT NULL,
  record_count INTEGER NOT NULL,
  call_count INTEGER NOT NULL,
  sample_count INTEGER NOT NULL,
  wall_time_us INTEGER NOT NULL,
  self_wall_time_us INTEGER NOT NULL,
  cpu_time_us INTEGER NOT NULL,
  self_cpu_time_us INTEGER NOT NULL,
  expand_factor REAL GENERATED ALWAYS AS (
    CASE
      WHEN sample_count > 0 THEN (call_count * 1.0 / sample_count)
      ELSE NULL
    END
  ) VIRTUAL,
  est_wall_us REAL GENERATED ALWAYS AS (
    CASE WHEN sample_count > 0 THEN wall_time_us * (call_count * 1.0 / sample_count) END
  ) VIRTUAL,
  est_self_wall_us REAL GENERATED ALWAYS AS (
    CASE WHEN sample_count > 0 THEN self_wall_time_us * (call_count * 1.0 / sample_count) END
  ) VIRTUAL,
  est_cpu_us REAL GENERATED ALWAYS AS (
    CASE WHEN sample_count > 0 THEN cpu_time_us * (call_count * 1.0 / sample_count) END
  ) VIRTUAL,
  est_self_cpu_us REAL GENERATED ALWAYS AS (
    CASE WHEN sample_count > 0 THEN self_cpu_time_us * (call_count * 1.0 / sample_count) END
  ) VIRTUAL,
  PRIMARY KEY (benchmark_run_id, profiler_span_id)
) WITHOUT ROWID;
"#
}

fn schema_column_guard() -> &'static str {
    r#"
SELECT wall_time_us FROM profiler_span_summary LIMIT 0;
SELECT self_wall_time_us FROM profiler_span_summary LIMIT 0;
"#
}

fn run_query(path: &str, extra_params: &[(&str, &str)]) {
    let query_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("queries")
        .join(path);
    let mut script = String::from(production_schema_subset());
    script.push_str(schema_column_guard());
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
