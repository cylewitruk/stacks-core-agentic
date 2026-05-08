-- Span recurrence across blocks and transactions in one run.
--
-- Purpose
--   For each span, compute how broadly it appears across the run's workload:
--     - distinct synthetic blocks it shows up in (and % of all blocks);
--     - distinct stacks transactions it shows up in (and % of all txs).
--   This is the strongest single signal the triage agent has for separating
--   "real, broadly-distributed hotspot" from "spike driven by a few outliers".
--
-- Heuristics for triage
--   `pct_blocks` is a PRIORITY signal, not a rejection signal — the run is a
--   slice of the chain, so a real bug may only show up in a fraction of these
--   blocks. The "is this an outlier vs a consistent pattern" decision lives
--   in `span_per_block_distribution.sql` / `top_blocks_for_span.sql`, not here.
--
--   - `pct_blocks` ≥ 70% → broad workload signal; standard priority.
--   - `pct_blocks` 30–70% → workload-conditional; scale each axis of
--     `expected_improvement` proportionally and note the coverage caveat in
--     rationale.
--   - `pct_blocks` < 30% → narrow but possibly real. Validate via the
--     per-block distribution before promoting; if the cost is consistent
--     across the blocks it does touch, accept at lower priority.
--   - `distinct_txs = 0` and `distinct_blocks` is high → block-level work
--     (commit/setup/finalize), not tx-level.
--
-- Note
--   Walks `profiler_record` directly (not `profiler_span_summary`) because the
--   summary table aggregates over blocks and loses the recurrence dimension.
--   Returns ALL spans (typically a few hundred) so a single call answers the
--   recurrence question for any candidate the agent considers — including
--   those surfaced by `top_spans_by_call_count.sql` that are not in the top
--   self-wall ranking. Sums use `est_*` virtual columns for consistency with
--   `top_spans_by_self_wall.sql` so ranking and validation share units.
--
-- Parameters
--   :run_id  benchmark_run.id
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".read queries/span_recurrence.sql"

WITH per_span AS (
  SELECT
    pr.profiler_span_id,
    ps.context,
    ps.name,
    SUM(COALESCE(pr.est_wall_us,      pr.wall_time_us))      AS total_wall_us,
    SUM(COALESCE(pr.est_self_wall_us, pr.self_wall_time_us)) AS total_self_wall_us,
    SUM(pr.call_count)                                       AS total_calls,
    COUNT(DISTINCT pr.synthetic_block_id) AS distinct_blocks,
    COUNT(DISTINCT pr.stacks_tx_id)       AS distinct_txs
  FROM profiler_record AS pr
  JOIN profiler_span   AS ps ON ps.id = pr.profiler_span_id
  WHERE pr.benchmark_run_id = :run_id
  GROUP BY pr.profiler_span_id, ps.context, ps.name
),
run_extent AS (
  SELECT
    COUNT(DISTINCT synthetic_block_id) AS blocks_in_run,
    COUNT(DISTINCT stacks_tx_id)       AS txs_in_run
  FROM profiler_record
  WHERE benchmark_run_id = :run_id
)
SELECT
  ps.profiler_span_id                                                       AS span_id,
  ps.context,
  ps.name,
  ROUND(ps.total_wall_us      / 1000.0, 2)                                  AS wall_ms,
  ROUND(ps.total_self_wall_us / 1000.0, 2)                                  AS self_wall_ms,
  ps.total_calls,
  ps.distinct_blocks,
  ROUND(100.0 * ps.distinct_blocks / NULLIF(re.blocks_in_run, 0), 1)         AS pct_blocks,
  ps.distinct_txs,
  ROUND(100.0 * ps.distinct_txs / NULLIF(re.txs_in_run, 0), 1)               AS pct_txs
FROM per_span AS ps
CROSS JOIN run_extent AS re
ORDER BY ps.total_self_wall_us DESC;
