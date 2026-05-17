-- Top synthetic blocks for a given span, ranked by self-wall time.
--
-- Purpose
--   When a span is concentrated in a small number of blocks (low pct_blocks
--   from `span_recurrence.sql`, or skewed distribution from
--   `span_per_block_distribution.sql`), this lists the specific blocks
--   where it dominates. The result returns both `synthetic_block_id`
--   (DB-local) and `stacks_block_hash` (globally stable); pass the
--   `stacks_block_hash` value as the `:stacks_block_hash` input to
--   `profiler_trace_block.sql` to see which parents and siblings drive
--   the cost in those blocks.
--
-- Parameters
--   :run_id   benchmark_run.id
--   :span_id  profiler_span.id
--   :limit    max rows
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :span_id 12" \
--     ".parameter set :limit 20" \
--     ".read $QUERIES_DIR/top_blocks_for_span.sql"

SELECT
  psbs.synthetic_block_id                                                  AS synthetic_block_id,
  sb.height                                                                AS stacks_block_height,
  sb.block_hash_hex                                                        AS stacks_block_hash,
  ROUND(COALESCE(psbs.est_self_wall_us, psbs.self_wall_time_us) / 1000.0, 3) AS self_wall_ms,
  ROUND(COALESCE(psbs.est_wall_us,      psbs.wall_time_us)      / 1000.0, 3) AS wall_ms,
  psbs.call_count,
  psbs.sample_count,
  ROUND(COALESCE(psbs.est_self_wall_us, psbs.self_wall_time_us) * 1.0
        / NULLIF(psbs.call_count, 0), 4)                                   AS avg_self_wall_us
FROM profiler_span_block_summary AS psbs
JOIN synthetic_block             AS synth ON synth.id = psbs.synthetic_block_id
JOIN stacks_block                AS sb    ON sb.id    = synth.stacks_block_id
WHERE psbs.benchmark_run_id  = :run_id
  AND psbs.profiler_span_id  = :span_id
ORDER BY COALESCE(psbs.est_self_wall_us, psbs.self_wall_time_us) DESC
LIMIT MIN(:limit, 200);
