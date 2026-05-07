-- Top profiler spans by call count.
--
-- Purpose
--   Spans with very high call counts are a different class of optimization
--   opportunity from spans with high per-call cost. Even a few-µs span can
--   dominate a run if it's called millions of times — those are typically
--   addressable via memoization, batching, or removing redundant lookups.
--
-- Heuristics for triage
--   - High `call_count`, low `avg_self_wall_us`, but non-trivial `self_wall_ms`
--     → strong cache / dedup candidate.
--   - High `call_count` AND high `avg_self_wall_us` → already triaged via
--     `top_spans_by_self_wall.sql`; this query won't add new information.
--
-- Parameters
--   :run_id  benchmark_run.id
--   :limit   max rows
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :limit 20" \
--     ".read queries/top_spans_by_call_count.sql"

SELECT
  pss.profiler_span_id                                                  AS span_id,
  ps.context,
  ps.name,
  pss.call_count,
  pss.sample_count,
  ROUND(pss.sample_count * 1.0 / NULLIF(pss.call_count, 0), 4)          AS sampling_rate,
  ROUND(COALESCE(pss.est_wall_us,      pss.wall_time_us)      / 1000.0, 2) AS wall_ms,
  ROUND(COALESCE(pss.est_self_wall_us, pss.self_wall_time_us) / 1000.0, 2) AS self_wall_ms,
  ROUND(COALESCE(pss.est_self_cpu_us,  pss.self_cpu_time_us)  / 1000.0, 2) AS self_cpu_ms,
  ROUND(COALESCE(pss.est_self_wall_us, pss.self_wall_time_us) * 1.0
        / NULLIF(pss.call_count, 0), 4)                                 AS avg_self_wall_us,
  ROUND(COALESCE(pss.est_self_cpu_us,  pss.self_cpu_time_us)  * 1.0
        / NULLIF(pss.call_count, 0), 4)                                 AS avg_self_cpu_us
FROM profiler_span_summary AS pss
JOIN profiler_span         AS ps ON ps.id = pss.profiler_span_id
WHERE pss.benchmark_run_id = :run_id
ORDER BY pss.call_count DESC
LIMIT MIN(:limit, 200);
