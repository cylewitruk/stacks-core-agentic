-- Baseline-vs-candidate profiler span comparison.
--
-- Purpose
--   Results-analyzer mechanism check for analyzer-named spans. Compares the
--   same span name/context between one baseline invocation run and its
--   matching candidate invocation run. Positive `improvement_pct` means
--   candidate lower / faster on exclusive wall time.
--
-- Parameters
--   :baseline_run_id   benchmark_run.id for the Phase 1.8 baseline invocation
--   :candidate_run_id  benchmark_run.id for the matching Phase 3 invocation
--   :span_name         span name or context to compare (exact match)
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :baseline_run_id 101" \
--     ".parameter set :candidate_run_id 202" \
--     ".parameter set :span_name 'RollbackWrapper::lookup'" \
--     ".read queries/compare_spans_between_runs.sql"

WITH span_rows AS (
  SELECT
    CASE pss.benchmark_run_id
      WHEN :baseline_run_id THEN 'baseline'
      WHEN :candidate_run_id THEN 'candidate'
    END AS side,
    ps.context,
    ps.name,
    SUM(pss.self_wall_time_us) AS self_wall_us,
    SUM(pss.total_wall_time_us) AS total_wall_us,
    SUM(pss.call_count) AS calls
  FROM profiler_span_summary AS pss
  JOIN profiler_span AS ps ON ps.id = pss.profiler_span_id
  WHERE pss.benchmark_run_id IN (:baseline_run_id, :candidate_run_id)
    AND (ps.name = :span_name OR ps.context = :span_name)
  GROUP BY pss.benchmark_run_id, ps.context, ps.name
),
span_keys AS (
  SELECT context, name
  FROM span_rows
  GROUP BY context, name
),
paired AS (
  SELECT
    k.context,
    k.name,
    MAX(CASE WHEN r.side = 'baseline' THEN r.self_wall_us END) AS baseline_self_wall_us,
    MAX(CASE WHEN r.side = 'candidate' THEN r.self_wall_us END) AS candidate_self_wall_us,
    MAX(CASE WHEN r.side = 'baseline' THEN r.total_wall_us END) AS baseline_total_wall_us,
    MAX(CASE WHEN r.side = 'candidate' THEN r.total_wall_us END) AS candidate_total_wall_us,
    MAX(CASE WHEN r.side = 'baseline' THEN r.calls END) AS baseline_calls,
    MAX(CASE WHEN r.side = 'candidate' THEN r.calls END) AS candidate_calls
  FROM span_keys AS k
  LEFT JOIN span_rows AS r ON r.context = k.context AND r.name = k.name
  GROUP BY k.context, k.name
)
SELECT
  context,
  name,
  baseline_self_wall_us,
  candidate_self_wall_us,
  candidate_self_wall_us - baseline_self_wall_us AS self_wall_delta_us,
  ROUND(
    100.0 * (baseline_self_wall_us - candidate_self_wall_us)
          / NULLIF(baseline_self_wall_us, 0),
    3
  ) AS self_wall_improvement_pct,
  baseline_total_wall_us,
  candidate_total_wall_us,
  candidate_total_wall_us - baseline_total_wall_us AS total_wall_delta_us,
  ROUND(
    100.0 * (baseline_total_wall_us - candidate_total_wall_us)
          / NULLIF(baseline_total_wall_us, 0),
    3
  ) AS total_wall_improvement_pct,
  baseline_calls,
  candidate_calls,
  candidate_calls - baseline_calls AS call_delta
FROM paired
ORDER BY ABS(COALESCE(self_wall_delta_us, 0)) DESC;
