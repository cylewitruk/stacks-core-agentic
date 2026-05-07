-- Average block-level timing breakdown for one run.
--
-- Purpose
--   Splits per-block wall time into setup / execution / commit phases (plus
--   the empty-block commit overhead baseline). Tells the triage agent which
--   high-level phase the time is in before diving into per-span data.
--
-- Heuristics for triage
--   - If `Commit` dominates → look at MARF, sqlite, file-write spans.
--   - If `Execution` dominates → look at Clarity VM spans (`clarity::vm::*`).
--   - If `Setup` dominates → block-init / parent-tip resolution path.
--   - `Commit (baseline overhead)` is the irreducible cost of committing an
--     empty block; subtract it from `Commit` to estimate tx-driven commit work.
--
-- Parameters
--   :run_id  benchmark_run.id
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".read queries/block_timing_breakdown.sql"

WITH agg AS (
  SELECT
    AVG(setup_duration_us)            / 1000.0 AS setup_ms,
    AVG(execution_duration_us)        / 1000.0 AS execution_ms,
    AVG(commit_duration_us)           / 1000.0 AS commit_ms,
    AVG(commit_overhead_baseline_us)  / 1000.0 AS commit_overhead_baseline_ms,
    AVG(total_duration_us)            / 1000.0 AS total_ms,
    COUNT(*)                                   AS blocks
  FROM stacks_block_stats
  WHERE benchmark_run_id = :run_id
)
SELECT 'Setup'                       AS phase, ROUND(setup_ms, 3)                   AS avg_ms_per_block, blocks FROM agg
UNION ALL SELECT 'Execution',                  ROUND(execution_ms, 3),                                  blocks FROM agg
UNION ALL SELECT 'Commit',                     ROUND(commit_ms, 3),                                     blocks FROM agg
UNION ALL SELECT 'Commit (baseline overhead)', ROUND(commit_overhead_baseline_ms, 3),                   blocks FROM agg
UNION ALL SELECT 'Total',                      ROUND(total_ms, 3),                                      blocks FROM agg;
