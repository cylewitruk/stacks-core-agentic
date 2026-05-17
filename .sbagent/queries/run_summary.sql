-- Run summary: provenance + workload size for one benchmark run.
--
-- Purpose
--   First-look orientation. Confirms which run is being analyzed, the source
--   commit + build profile, the args used, and how much work was profiled
--   (block/tx/profiler-record counts). Use this to sanity-check that the run
--   you intend to triage is actually the one you have.
--
-- Parameters
--   :run_id  benchmark_run.id
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".read queries/run_summary.sql"

SELECT
  br.id                                                                     AS run_id,
  br.run_name,
  br.start_time,
  br.end_time,
  CAST((julianday(br.end_time) - julianday(br.start_time)) * 86400 AS INTEGER) AS duration_secs,
  hex(br.git_commit_hash)                                                   AS git_commit_hash,
  br.git_branch,
  br.git_dirty,
  br.build_profile,
  br.build_opt_level,
  br.build_rustc_version,
  br.args_json,
  (SELECT COUNT(*) FROM stacks_block_stats   WHERE benchmark_run_id = br.id)  AS blocks_processed,
  (SELECT COUNT(*) FROM stacks_tx_stats      WHERE benchmark_run_id = br.id)  AS txs_processed,
  (SELECT COUNT(*) FROM profiler_record      WHERE benchmark_run_id = br.id)  AS profiler_records,
  (SELECT COUNT(DISTINCT profiler_span_id)
     FROM profiler_span_summary              WHERE benchmark_run_id = br.id)  AS distinct_spans,
  (SELECT SUM(total_duration_us) / 1000.0
     FROM stacks_block_stats                 WHERE benchmark_run_id = br.id)  AS total_block_duration_ms
FROM benchmark_run AS br
WHERE br.id = :run_id;
