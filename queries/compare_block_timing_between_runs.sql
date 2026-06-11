-- Baseline-vs-candidate block timing breakdown.
--
-- Purpose
--   Results-analyzer comparison for block-level expected signals. Compares
--   average setup / execution / commit / total time per measured block.
--   Positive `improvement_pct` means candidate lower / faster.
--
-- Parameters
--   :baseline_run_id   benchmark_run.id for the Phase 1.8 baseline invocation
--   :candidate_run_id  benchmark_run.id for the matching Phase 3 invocation
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :baseline_run_id 101" \
--     ".parameter set :candidate_run_id 202" \
--     ".read queries/compare_block_timing_between_runs.sql"

WITH per_run AS (
  SELECT
    CASE benchmark_run_id
      WHEN :baseline_run_id THEN 'baseline'
      WHEN :candidate_run_id THEN 'candidate'
    END AS side,
    AVG(setup_duration_us) AS setup_us,
    AVG(execution_duration_us) AS execution_us,
    AVG(commit_duration_us) AS commit_us,
    AVG(commit_overhead_baseline_us) AS commit_overhead_baseline_us,
    AVG(total_duration_us) AS total_us,
    COUNT(*) AS blocks
  FROM stacks_block_stats
  WHERE benchmark_run_id IN (:baseline_run_id, :candidate_run_id)
  GROUP BY benchmark_run_id
),
metrics(metric, baseline_value, candidate_value) AS (
  SELECT
    'setup_us_per_block',
    MAX(CASE WHEN side = 'baseline' THEN setup_us END),
    MAX(CASE WHEN side = 'candidate' THEN setup_us END)
  FROM per_run
  UNION ALL
  SELECT
    'execution_us_per_block',
    MAX(CASE WHEN side = 'baseline' THEN execution_us END),
    MAX(CASE WHEN side = 'candidate' THEN execution_us END)
  FROM per_run
  UNION ALL
  SELECT
    'commit_us_per_block',
    MAX(CASE WHEN side = 'baseline' THEN commit_us END),
    MAX(CASE WHEN side = 'candidate' THEN commit_us END)
  FROM per_run
  UNION ALL
  SELECT
    'commit_overhead_baseline_us_per_block',
    MAX(CASE WHEN side = 'baseline' THEN commit_overhead_baseline_us END),
    MAX(CASE WHEN side = 'candidate' THEN commit_overhead_baseline_us END)
  FROM per_run
  UNION ALL
  SELECT
    'total_us_per_block',
    MAX(CASE WHEN side = 'baseline' THEN total_us END),
    MAX(CASE WHEN side = 'candidate' THEN total_us END)
  FROM per_run
  UNION ALL
  SELECT
    'blocks',
    MAX(CASE WHEN side = 'baseline' THEN blocks END),
    MAX(CASE WHEN side = 'candidate' THEN blocks END)
  FROM per_run
)
SELECT
  metric,
  ROUND(baseline_value, 3) AS baseline_value,
  ROUND(candidate_value, 3) AS candidate_value,
  ROUND(candidate_value - baseline_value, 3) AS delta,
  ROUND(100.0 * (baseline_value - candidate_value) / NULLIF(baseline_value, 0), 3)
    AS improvement_pct
FROM metrics;
