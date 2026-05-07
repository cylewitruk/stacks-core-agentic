-- Per-block self-wall-time distribution for ONE span in one run.
--
-- Purpose
--   For a given span, computes the distribution of *per-block* exclusive wall
--   time (min/max/avg/p50/p95/p99). Complements `span_per_sample_distribution`:
--   a span with uniform per-call cost can still be skewed across blocks if
--   only a fraction of blocks exercise it.
--
-- Outlier-vs-pattern columns
--   `top1_share_pct` and `top3_share_pct` give the share of total span cost
--   carried by the single hottest block / hottest three blocks. These let
--   step 2 of the triage validation procedure run with just this query, no
--   need to join `top_blocks_for_span.sql` separately for a quick check.
--
-- Heuristics for triage
--   - `top3_share_pct` > 50 → outlier-driven; the span's headline cost is
--     coming from a tiny number of pathological blocks, not a broad pattern.
--   - `max_block_ms` >> ~10 × `p95_block_ms` → long tail of one-shot spikes.
--   - `min_block_ms` near 0 with high `max_block_ms` → highly skewed; cross-
--     check `top1_share_pct` before trusting the headline.
--   - p50 close to avg, top3 share modest (e.g. < 25%) → uniform; promote.
--
-- Sampling expansion
--   Uses `est_self_wall_us` (sampling-expanded) consistently with
--   `top_spans_by_self_wall.sql`, so ranking and validation share the same
--   units even when `sample_count < call_count`.
--
-- Parameters
--   :run_id   benchmark_run.id
--   :span_id  profiler_span.id
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :span_id 12" \
--     ".read queries/span_per_block_distribution.sql"

WITH per_block AS (
  SELECT
    psbs.synthetic_block_id,
    COALESCE(psbs.est_self_wall_us, psbs.self_wall_time_us) AS self_wall_us,
    psbs.call_count
  FROM profiler_span_block_summary AS psbs
  WHERE psbs.benchmark_run_id = :run_id
    AND psbs.profiler_span_id = :span_id
),
totals AS (
  SELECT SUM(self_wall_us) AS total_self_wall_us FROM per_block
),
top1 AS (
  SELECT MAX(self_wall_us) AS top1_self_wall_us FROM per_block
),
top3 AS (
  SELECT SUM(self_wall_us) AS top3_self_wall_us FROM (
    SELECT self_wall_us FROM per_block ORDER BY self_wall_us DESC LIMIT 3
  )
)
SELECT
  COUNT(*)                                                          AS blocks,
  SUM(call_count)                                                   AS total_calls,
  ROUND((SELECT total_self_wall_us FROM totals) / 1000.0, 3)        AS total_self_wall_ms,
  ROUND(MIN(self_wall_us) / 1000.0, 3)                              AS min_block_ms,
  ROUND(MAX(self_wall_us) / 1000.0, 3)                              AS max_block_ms,
  ROUND(AVG(self_wall_us) / 1000.0, 3)                              AS avg_block_ms,
  (SELECT ROUND(self_wall_us / 1000.0, 3) FROM per_block ORDER BY self_wall_us
     LIMIT 1 OFFSET CAST(0.50 * (SELECT COUNT(*)-1 FROM per_block) AS INTEGER)) AS p50_block_ms,
  (SELECT ROUND(self_wall_us / 1000.0, 3) FROM per_block ORDER BY self_wall_us
     LIMIT 1 OFFSET CAST(0.95 * (SELECT COUNT(*)-1 FROM per_block) AS INTEGER)) AS p95_block_ms,
  (SELECT ROUND(self_wall_us / 1000.0, 3) FROM per_block ORDER BY self_wall_us
     LIMIT 1 OFFSET CAST(0.99 * (SELECT COUNT(*)-1 FROM per_block) AS INTEGER)) AS p99_block_ms,
  ROUND(100.0 * (SELECT top1_self_wall_us FROM top1)
              / NULLIF((SELECT total_self_wall_us FROM totals), 0), 1)         AS top1_share_pct,
  ROUND(100.0 * (SELECT top3_self_wall_us FROM top3)
              / NULLIF((SELECT total_self_wall_us FROM totals), 0), 1)         AS top3_share_pct
FROM per_block;
