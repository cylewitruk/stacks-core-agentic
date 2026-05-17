-- Top profiler spans by exclusive (self) wall time.
--
-- Purpose
--   Rank the spans that consumed the most exclusive wall-clock time in one run,
--   with sampling rate, expanded estimates, CPU/wait split, and per-call cost.
--   This is the primary hotspot ranking the triage agent should consult first.
--
-- Why prefer this over the JSON output of `bench show --profiler-hot`
--   - The JSON is capped at the requested top-N. This query lets you scan
--     deeper or filter by `:limit`.
--   - The JSON does not expose the CPU vs wait split; this does.
--   - Returns both inclusive `wall_ms` and exclusive `self_wall_ms`. A span
--     ranked by `self_wall_ms` may be a leaf hotspot (`wall_ms ≈ self_wall_ms`)
--     or an aggregator whose subtree carries most of the cost
--     (`wall_ms ≫ self_wall_ms`); the optimization shape differs.
--   - Reads the pre-aggregated `profiler_span_summary` table, so it is fast
--     even on multi-million-record runs.
--
-- Heuristics for triage
--   - High `self_wall_ms` AND high `call_count` → potential cache / batching win.
--   - High `self_wall_ms` AND low  `call_count` → per-call cost; redesign target.
--   - `self_wait_ms ≈ self_wall_ms` (CPU close to 0) → I/O- or lock-bound.
--   - `sampling_rate < 0.1` and few records → estimate may be noisy; consult
--     `span_per_sample_distribution.sql` before trusting the rank.
--
-- Parameters
--   :run_id  benchmark_run.id
--   :limit   max rows (15–50 is typical)
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :limit 25" \
--     ".read queries/top_spans_by_self_wall.sql"

SELECT
  pss.profiler_span_id                                                AS span_id,
  ps.context,
  ps.name,
  pss.call_count,
  pss.sample_count,
  ROUND(pss.sample_count * 1.0 / NULLIF(pss.call_count, 0), 4)        AS sampling_rate,
  ROUND(COALESCE(pss.est_wall_us,      pss.wall_time_us)      / 1000.0, 2) AS wall_ms,
  ROUND(COALESCE(pss.est_self_wall_us, pss.self_wall_time_us) / 1000.0, 2) AS self_wall_ms,
  ROUND(COALESCE(pss.est_self_cpu_us,  pss.self_cpu_time_us)  / 1000.0, 2) AS self_cpu_ms,
  ROUND(
    (COALESCE(pss.est_self_wall_us, pss.self_wall_time_us)
       - COALESCE(pss.est_self_cpu_us, pss.self_cpu_time_us)) / 1000.0,
    2)                                                                AS self_wait_ms,
  ROUND(
    COALESCE(pss.est_self_wall_us, pss.self_wall_time_us)
      * 1.0 / NULLIF(pss.call_count, 0),
    3)                                                                AS avg_self_wall_us
FROM profiler_span_summary AS pss
JOIN profiler_span         AS ps ON ps.id = pss.profiler_span_id
WHERE pss.benchmark_run_id = :run_id
ORDER BY COALESCE(pss.est_self_wall_us, pss.self_wall_time_us) DESC
LIMIT MIN(:limit, 200);
