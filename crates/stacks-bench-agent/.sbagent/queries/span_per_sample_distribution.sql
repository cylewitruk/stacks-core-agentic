-- Per-sample wall-time distribution for ONE span in one run.
--
-- Purpose
--   For a given span, computes a SHAPE summary (min/max/avg/p50/p95/p99) of
--   per-sample exclusive wall time (`self_wall_time_us / sample_count`),
--   sample-weighted across all `profiler_record` rows. Use this to decide
--   whether a hot span has a uniform shape (steady, addressable hotspot) or
--   whether its total is dominated by a long tail (one-off outliers; harder
--   to optimize).
--
-- Sample weighting
--   Percentiles and `avg_us` are weighted by `sample_count`: a record with
--   1000 samples contributes 1000 units to the cumulative distribution, a
--   record with 1 sample contributes 1. This matches the intuition of "what
--   does the per-call cost look like" rather than "what does the per-record
--   mean look like." Without weighting, a single noisy 1-sample record could
--   distort the p99 as much as a heavily-sampled steady record.
--
-- Caveat — per-sample vs per-call
--   When `sampling_rate ≈ 1.0` (see `top_spans_by_self_wall.sql`'s
--   `sampling_rate` column), per-sample is effectively per-call.
--   When `sampling_rate < 1.0`, the distribution still reflects shape but
--   does NOT report literal per-call latency — treat it as a steady-vs-tail
--   signal, not a benchmark figure.
--
-- Heuristics for triage
--   - p99 / p50 ratio < ~5 → uniform cost; safe optimization target.
--   - p99 / p50 ratio > ~20 → long tail; investigate WHY before accepting
--     (a call-site bug, a single pathological input, GC pause, etc.).
--   This is supporting evidence, not a hard rule. Combine with
--   `span_per_block_distribution.sql` before rejecting on shape alone.
--
-- Parameters
--   :run_id   benchmark_run.id
--   :span_id  profiler_span.id (find via top_spans_by_self_wall.sql)
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :span_id 15" \
--     ".read queries/span_per_sample_distribution.sql"

WITH per_record AS (
  SELECT
    CAST(pr.self_wall_time_us * 1.0 / NULLIF(pr.sample_count, 0) AS REAL) AS x,
    pr.call_count,
    pr.sample_count                                                       AS w
  FROM profiler_record AS pr
  WHERE pr.benchmark_run_id = :run_id
    AND pr.profiler_span_id = :span_id
    AND pr.sample_count > 0
),
ordered AS (
  SELECT
    x,
    w,
    SUM(w) OVER ()           AS total_w,
    SUM(w) OVER (ORDER BY x) AS cum_w
  FROM per_record
)
SELECT
  (SELECT COUNT(*)        FROM per_record)                              AS records,
  (SELECT SUM(call_count) FROM per_record)                              AS total_calls,
  (SELECT SUM(w)          FROM per_record)                              AS total_samples,
  ROUND((SELECT MIN(x) FROM per_record), 2)                             AS min_us,
  ROUND((SELECT MAX(x) FROM per_record), 2)                             AS max_us,
  ROUND((SELECT SUM(x * w) * 1.0 / NULLIF(SUM(w), 0) FROM per_record), 2) AS avg_us,
  (SELECT ROUND(x, 2) FROM ordered WHERE cum_w >= 0.50 * total_w
     ORDER BY cum_w LIMIT 1)                                            AS p50_us,
  (SELECT ROUND(x, 2) FROM ordered WHERE cum_w >= 0.95 * total_w
     ORDER BY cum_w LIMIT 1)                                            AS p95_us,
  (SELECT ROUND(x, 2) FROM ordered WHERE cum_w >= 0.99 * total_w
     ORDER BY cum_w LIMIT 1)                                            AS p99_us
;
