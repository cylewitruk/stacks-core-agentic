-- Transaction type distribution for one run.
--
-- Purpose
--   Cheap workload context: how many txs of each type were processed and
--   how much wall time they collectively consumed. Helps the triage agent
--   decide whether the run's hotspots are likely Clarity-VM driven (Contract
--   Call dominates) or token-/coinbase-driven.
--
-- Parameters
--   :run_id  benchmark_run.id
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".read queries/tx_type_distribution.sql"

SELECT
  tt.name                                              AS tx_type,
  COUNT(DISTINCT sts.stacks_tx_id)                     AS tx_count,
  ROUND(SUM(sts.duration_us) / 1000.0, 2)              AS total_duration_ms,
  ROUND(AVG(sts.duration_us) / 1000.0, 3)              AS avg_duration_ms
FROM stacks_tx_stats AS sts
JOIN stacks_tx       AS tx ON tx.id = sts.stacks_tx_id
JOIN stacks_tx_type  AS tt ON tt.id = tx.stacks_tx_type_id
WHERE sts.benchmark_run_id = :run_id
GROUP BY tt.name
ORDER BY total_duration_ms DESC;
