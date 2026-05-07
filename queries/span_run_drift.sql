-- Run-over-run drift for the top-N spans across the most-recent runs.
--
-- Purpose
--   When the DB contains 2+ runs, this surfaces spans whose self-wall time
--   has changed materially across recent runs. Useful for triage to:
--     - reject spans whose recent baseline is *already* dropping (someone
--       else is fixing it);
--     - prioritize spans that have grown noticeably (regression candidates).
--
-- Behavior with one run
--   `runs >= 2` filter excludes single-run cases, so this query returns no
--   rows on a fresh DB. That is expected — the triage agent should still
--   call it once to confirm there is nothing to learn from history.
--
-- Parameters
--   :recent_runs  how many recent runs to compare across (e.g. 5)
--   :limit        max spans to return
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :recent_runs 5" \
--     ".parameter set :limit 20" \
--     ".read queries/span_run_drift.sql"

WITH recent AS (
  SELECT id FROM benchmark_run
  ORDER BY id DESC
  LIMIT :recent_runs
),
span_runs AS (
  SELECT
    pss.profiler_span_id,
    pss.benchmark_run_id,
    pss.self_wall_time_us,
    pss.call_count
  FROM profiler_span_summary AS pss
  JOIN recent r ON r.id = pss.benchmark_run_id
)
SELECT
  ps.id                                                                  AS span_id,
  ps.context,
  ps.name,
  GROUP_CONCAT(
    sr.benchmark_run_id || ':' || ROUND(sr.self_wall_time_us / 1000.0, 1) || 'ms',
    ' | '
  )                                                                       AS per_run_self_wall_ms,
  ROUND(MIN(sr.self_wall_time_us) / 1000.0, 1)                            AS min_ms,
  ROUND(MAX(sr.self_wall_time_us) / 1000.0, 1)                            AS max_ms,
  ROUND(
    100.0 * (MAX(sr.self_wall_time_us) - MIN(sr.self_wall_time_us))
          / NULLIF(MIN(sr.self_wall_time_us), 0),
    1)                                                                    AS spread_pct,
  COUNT(DISTINCT sr.benchmark_run_id)                                     AS runs
FROM span_runs sr
JOIN profiler_span ps ON ps.id = sr.profiler_span_id
GROUP BY ps.id
HAVING runs >= 2
ORDER BY MAX(sr.self_wall_time_us) DESC
LIMIT MIN(:limit, 200);
