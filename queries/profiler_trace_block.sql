-- Recursive profiler trace tree for ONE synthetic block in one run.
--
-- Purpose
--   Block-level analogue of `profiler_trace_tx.sql`. Inspect the hierarchical
--   span tree across all txs and block-level work (setup / commit / finalize)
--   for a single synthetic block. Useful when the agent needs to understand
--   block-level overhead (commit, MARF, sqlite) rather than per-tx behavior,
--   or wants to see how a high-recurrence span like `walk_backptr` is
--   distributed across the block's call sites.
--
-- Filtering
--   `:min_wall_ms` prunes spans whose total wall is below threshold before
--   recursion. Block traces can be much larger than tx traces (hundreds of
--   txs per block + block plumbing), so a non-zero floor is strongly
--   recommended (10–25ms is typical for triage).
--
-- Notes
--   - `depth` and `parent_record_id` make the tree machine-addressable; `span`
--     is the bare name. Reconstruct hierarchy from those two columns rather
--     than from any visual indent. `cpu_ms` is omitted (kept `self_cpu_ms`
--     only) to keep output dense for the LLM consumer.
--
-- Parameters
--   :run_id              benchmark_run.id
--   :stacks_block_hash   stacks_block.block_hash_hex (0x-prefixed 64-hex-char
--                        index block hash). Resolved to
--                        `synthetic_block.id` for the current run via a
--                        one-row dim lookup so the fact-table scan
--                        filters on the indexed FK
--                        `profiler_record.synthetic_block_id`.
--   :min_wall_ms         threshold (0 disables the filter)
--   :max_rows            LIMIT
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :stacks_block_hash '0xabcdef...'" \
--     ".parameter set :min_wall_ms 10" \
--     ".parameter set :max_rows 300" \
--     ".read $QUERIES_DIR/profiler_trace_block.sql"

WITH RECURSIVE
scoped AS (
  SELECT
    pr.id, pr.parent_id, pr.profiler_span_id, pr.profiler_tag_id,
    pr.profiler_location_id, pr.child_index, pr.depth,
    pr.synthetic_block_id, pr.stacks_tx_id,
    pr.call_count, pr.sample_count,
    pr.wall_time_us, pr.self_wall_time_us,
    pr.cpu_time_us,  pr.self_cpu_time_us,
    pr.est_wall_us,  pr.est_self_wall_us
  FROM profiler_record pr
  WHERE pr.benchmark_run_id    = :run_id
    AND pr.synthetic_block_id  = (
      SELECT synth.id
        FROM synthetic_block synth
        JOIN stacks_block    sb ON sb.id = synth.stacks_block_id
       WHERE synth.benchmark_run_id = :run_id
         AND sb.block_hash_hex = :stacks_block_hash
    )
    AND COALESCE(pr.est_wall_us, pr.wall_time_us) >= :min_wall_ms * 1000.0
),
trace_tree AS (
  SELECT s.*, printf('%09d', s.id) AS sort_path, 0 AS lvl
  FROM scoped s
  WHERE s.parent_id IS NULL
     OR NOT EXISTS (SELECT 1 FROM scoped p WHERE p.id = s.parent_id)

  UNION ALL

  SELECT c.*, parent.sort_path || '.' || printf('%04d', c.child_index), parent.lvl + 1
  FROM scoped c
  JOIN trace_tree parent ON c.parent_id = parent.id
)
SELECT
  t.id                                                              AS record_id,
  t.parent_id                                                       AS parent_record_id,
  t.lvl                                                             AS depth,
  s.name                                                            AS span,
  s.context,
  COALESCE(pl.file || ':' || pl.line, '')                           AS location,
  pt.tag,
  COALESCE(tx.tx_hash_hex, '')                                      AS tx_hash,
  t.call_count,
  t.sample_count,
  ROUND(COALESCE(t.est_wall_us,      t.wall_time_us)      / 1000.0, 3) AS wall_ms,
  ROUND(COALESCE(t.est_self_wall_us, t.self_wall_time_us) / 1000.0, 3) AS self_wall_ms,
  ROUND(t.self_cpu_time_us / 1000.0, 3)                             AS self_cpu_ms
FROM trace_tree t
JOIN profiler_span         s  ON s.id  = t.profiler_span_id
LEFT JOIN profiler_location pl ON pl.id = t.profiler_location_id
LEFT JOIN profiler_tag      pt ON pt.id = t.profiler_tag_id
LEFT JOIN stacks_tx         tx ON tx.id = t.stacks_tx_id
ORDER BY t.sort_path
LIMIT MIN(:max_rows, 2000);
