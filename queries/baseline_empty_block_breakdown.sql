-- Baseline (empty-block) processing cost breakdown.
--
-- Purpose
--   Each run measures how long it takes to process an empty block (no txs)
--   for warmup + measured iterations. The result is a per-stage average that
--   approximates the irreducible block-processing overhead. The triage agent
--   should treat any tx-driven span whose total cost is *below* this baseline
--   as low-priority — there is no way to make a tx-bearing block faster than
--   the empty-block path.
--
-- Parameters
--   :run_id  benchmark_run.id
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".read queries/baseline_empty_block_breakdown.sql"

SELECT
  'Setup'           AS phase,
  ROUND(avg_setup_us           / 1000.0, 3) AS avg_ms,
  warmup_blocks,
  measured_blocks
FROM block_processing_baseline
WHERE benchmark_run_id = :run_id

UNION ALL
SELECT 'Finalize',          ROUND(avg_finalize_us       / 1000.0, 3), warmup_blocks, measured_blocks
FROM block_processing_baseline WHERE benchmark_run_id = :run_id

UNION ALL
SELECT 'Clarity Commit',    ROUND(avg_clarity_commit_us / 1000.0, 3), warmup_blocks, measured_blocks
FROM block_processing_baseline WHERE benchmark_run_id = :run_id

UNION ALL
SELECT 'Advance Tip',       ROUND(avg_advance_tip_us    / 1000.0, 3), warmup_blocks, measured_blocks
FROM block_processing_baseline WHERE benchmark_run_id = :run_id

UNION ALL
SELECT 'Index Commit',      ROUND(avg_index_commit_us   / 1000.0, 3), warmup_blocks, measured_blocks
FROM block_processing_baseline WHERE benchmark_run_id = :run_id;
