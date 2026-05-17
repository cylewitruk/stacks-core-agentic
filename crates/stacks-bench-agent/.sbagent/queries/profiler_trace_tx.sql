-- Recursive profiler trace tree for ONE transaction in one run.
--
-- Purpose
--   Drill-down inspection: shows the full hierarchical span tree captured for
--   a single transaction, indented by depth. Use this after the high-level
--   triage queries have surfaced a suspect tx (via `top_txs_by_duration.sql`,
--   `txs_for_contract.sql`, or by joining a hot span back to its
--   `stacks_tx_id`s).
--
-- Filtering
--   `:min_wall_ms` prunes spans whose total wall time is below the threshold
--   *before* the recursion runs, so children of pruned ancestors are also
--   dropped. Pass 0 to keep the full tree. A typical first pass uses 5–10ms
--   to avoid hundreds of low-cost rows.
--
-- Notes
--   - Ordering follows execution order (`child_index`), not sort by cost.
--   - Includes the `tag` text from `profiler_tag` (often a contract:function)
--     and the source `file:line` from `profiler_location`.
--   - `depth` and `parent_record_id` make the tree machine-addressable: the
--     consumer can rebuild parent/child relationships exactly without parsing
--     a visual indent. `span` is the bare span name; reconstruct hierarchy
--     from `depth` (sort key) and `parent_record_id` (FK to `record_id`).
--   - `cpu_ms` (the inclusive CPU total) is intentionally omitted: it duplicates
--     information already conveyed by `wall_ms` plus `self_cpu_ms`, and trace
--     output tends to be the largest result the agent reads.
--
-- Parameters
--   :run_id           benchmark_run.id
--   :stacks_tx_hash   stacks_tx.tx_hash_hex (0x-prefixed 64-hex-char hash).
--                     Resolved to stacks_tx.id via a one-row dim lookup so
--                     the fact-table scan filters on the indexed FK
--                     `profiler_record.stacks_tx_id`, not on the
--                     unindexed hash column.
--   :min_wall_ms      threshold (0 disables the filter)
--   :max_rows         LIMIT to keep the result bounded
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :stacks_tx_hash '0xabcdef...'" \
--     ".parameter set :min_wall_ms 5" \
--     ".parameter set :max_rows 200" \
--     ".read $QUERIES_DIR/profiler_trace_tx.sql"

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
  WHERE pr.benchmark_run_id = :run_id
    AND pr.stacks_tx_id     = (
      SELECT id FROM stacks_tx WHERE tx_hash_hex = :stacks_tx_hash
    )
    AND COALESCE(pr.est_wall_us, pr.wall_time_us) >= :min_wall_ms * 1000.0
),
trace_tree AS (
  -- Anchors: rows in scope whose parent is NOT in scope (true roots, or roots
  -- whose true parent was pruned by the min_wall_ms filter).
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
  t.call_count,
  t.sample_count,
  ROUND(COALESCE(t.est_wall_us,      t.wall_time_us)      / 1000.0, 3) AS wall_ms,
  ROUND(COALESCE(t.est_self_wall_us, t.self_wall_time_us) / 1000.0, 3) AS self_wall_ms,
  ROUND(t.self_cpu_time_us / 1000.0, 3)                             AS self_cpu_ms
FROM trace_tree t
JOIN profiler_span         s  ON s.id  = t.profiler_span_id
LEFT JOIN profiler_location pl ON pl.id = t.profiler_location_id
LEFT JOIN profiler_tag      pt ON pt.id = t.profiler_tag_id
ORDER BY t.sort_path
LIMIT MIN(:max_rows, 2000);
