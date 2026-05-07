-- Top contract-call functions by total wall time in one run.
--
-- Purpose
--   Surfaces which Clarity contract functions dominate execution time, so the
--   triage agent can decide whether the run's hot Clarity-VM spans
--   (lookup_variable, lookup_function, etc.) are concentrated in a small set
--   of contracts (worth a per-contract optimization) or spread broadly (worth
--   a generic VM-path optimization).
--
-- Parameters
--   :run_id  benchmark_run.id
--   :limit   max rows
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :limit 20" \
--     ".read queries/top_contract_calls.sql"

SELECT
  p.address                                  AS issuer,
  c.name                                     AS contract,
  COALESCE(cf.name, '(unknown)')             AS function,
  COUNT(*)                                   AS call_count,
  ROUND(SUM(sts.duration_us) / 1000.0, 2)    AS total_ms,
  ROUND(AVG(sts.duration_us) / 1000.0, 3)    AS avg_ms
FROM stacks_tx_stats AS sts
JOIN stacks_tx       AS tx  ON tx.id = sts.stacks_tx_id
                            AND tx.stacks_tx_type_id = 2  -- Contract Call
JOIN contract        AS c   ON c.id  = tx.contract_id
JOIN principal       AS p   ON p.id  = c.issuer_principal_id
LEFT JOIN contract_fn AS cf ON cf.id = tx.contract_fn_id
WHERE sts.benchmark_run_id = :run_id
GROUP BY p.address, c.name, cf.name
ORDER BY total_ms DESC
LIMIT MIN(:limit, 200);
