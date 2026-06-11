-- Baseline-vs-candidate coarse run summary.
--
-- Purpose
--   Results-analyzer first-pass comparison. Confirms both runs exist and
--   compares coarse total work before drilling into span / block / Clarity
--   evidence. Positive `improvement_pct` means candidate lower / faster.
--
-- Parameters
--   :baseline_run_id   benchmark_run.id for the Phase 1.8 baseline invocation
--   :candidate_run_id  benchmark_run.id for the matching Phase 3 invocation
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :baseline_run_id 101" \
--     ".parameter set :candidate_run_id 202" \
--     ".read queries/compare_run_summary.sql"

WITH run_metrics AS (
  SELECT
    CASE br.id
      WHEN :baseline_run_id THEN 'baseline'
      WHEN :candidate_run_id THEN 'candidate'
    END AS side,
    CAST((julianday(br.end_time) - julianday(br.start_time)) * 1000000 AS INTEGER) AS wall_us,
    (SELECT SUM(total_duration_us)
       FROM stacks_block_stats
      WHERE benchmark_run_id = br.id) AS block_total_us,
    (SELECT COUNT(*)
       FROM stacks_block_stats
      WHERE benchmark_run_id = br.id) AS blocks_processed,
    (SELECT COUNT(*)
       FROM stacks_tx_stats
      WHERE benchmark_run_id = br.id) AS txs_processed,
    (SELECT COUNT(*)
       FROM profiler_record
      WHERE benchmark_run_id = br.id) AS profiler_records
  FROM benchmark_run AS br
  WHERE br.id IN (:baseline_run_id, :candidate_run_id)
),
metrics(metric, baseline_value, candidate_value) AS (
  SELECT
    'run_wall_us',
    MAX(CASE WHEN side = 'baseline' THEN wall_us END),
    MAX(CASE WHEN side = 'candidate' THEN wall_us END)
  FROM run_metrics
  UNION ALL
  SELECT
    'block_total_us',
    MAX(CASE WHEN side = 'baseline' THEN block_total_us END),
    MAX(CASE WHEN side = 'candidate' THEN block_total_us END)
  FROM run_metrics
  UNION ALL
  SELECT
    'blocks_processed',
    MAX(CASE WHEN side = 'baseline' THEN blocks_processed END),
    MAX(CASE WHEN side = 'candidate' THEN blocks_processed END)
  FROM run_metrics
  UNION ALL
  SELECT
    'txs_processed',
    MAX(CASE WHEN side = 'baseline' THEN txs_processed END),
    MAX(CASE WHEN side = 'candidate' THEN txs_processed END)
  FROM run_metrics
  UNION ALL
  SELECT
    'profiler_records',
    MAX(CASE WHEN side = 'baseline' THEN profiler_records END),
    MAX(CASE WHEN side = 'candidate' THEN profiler_records END)
  FROM run_metrics
)
SELECT
  metric,
  baseline_value,
  candidate_value,
  candidate_value - baseline_value AS delta,
  ROUND(100.0 * (baseline_value - candidate_value) / NULLIF(baseline_value, 0), 3)
    AS improvement_pct
FROM metrics;
