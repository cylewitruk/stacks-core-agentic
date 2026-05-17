-- Transactions calling a specific contract function in one run.
--
-- Purpose
--   Drill-down companion to `top_contract_calls.sql`. Once that query has
--   surfaced a hot contract.function pair, this lists the actual transactions
--   that called it in this run, ordered by duration. The result returns
--   both `stacks_tx_id` (DB-local) and `tx_hash` (globally stable); pass
--   the `tx_hash` value as the `:stacks_tx_hash` input to
--   `profiler_trace_tx.sql` to inspect a single hot call's full trace.
--
--   This query exposes all five Clarity cost columns from `stacks_tx_stats`
--   (`clarity_runtime`, `clarity_read_count`, `clarity_read_length`,
--   `clarity_write_count`, `clarity_write_length`) — see `README.md`
--   ("Clarity cost columns") for their exact semantics. They are
--   deterministic Clarity budget units, NOT timings, NOT bytes-on-disk, NOT
--   raw operation counts.
--
-- Parameters
--   :run_id            benchmark_run.id
--   :issuer_address    e.g. 'SP2VCQJGH7PHP2DJK7Z0V48AGBHQAW3R3ZW1QF4N'
--   :contract_name     e.g. 'borrow-helper-v2-1-7'
--   :function_name     e.g. 'liquidation-call' (use '%' to match any)
--   :limit             max rows
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :issuer_address 'SP2VCQJGH7PHP2DJK7Z0V48AGBHQAW3R3ZW1QF4N'" \
--     ".parameter set :contract_name 'borrow-helper-v2-1-7'" \
--     ".parameter set :function_name 'liquidation-call'" \
--     ".parameter set :limit 25" \
--     ".read $QUERIES_DIR/txs_for_contract.sql"

SELECT
  sts.stacks_tx_id                          AS stacks_tx_id,
  tx.tx_hash_hex                            AS tx_hash,
  sts.synthetic_block_id                    AS synthetic_block_id,
  sb.height                                 AS stacks_block_height,
  ROUND(sts.duration_us / 1000.0, 3)        AS duration_ms,
  sts.clarity_runtime,
  sts.clarity_read_count,
  sts.clarity_read_length,
  sts.clarity_write_count,
  sts.clarity_write_length
FROM stacks_tx_stats AS sts
JOIN stacks_tx       AS tx ON tx.id = sts.stacks_tx_id
JOIN contract        AS c  ON c.id = tx.contract_id
JOIN principal       AS p  ON p.id = c.issuer_principal_id
LEFT JOIN contract_fn AS cf ON cf.id = tx.contract_fn_id
JOIN synthetic_block AS synth ON synth.id = sts.synthetic_block_id
JOIN stacks_block    AS sb ON sb.id = synth.stacks_block_id
WHERE sts.benchmark_run_id = :run_id
  AND tx.stacks_tx_type_id  = 2  -- Contract Call
  AND p.address  = :issuer_address
  AND c.name     = :contract_name
  AND COALESCE(cf.name, '') LIKE :function_name
ORDER BY sts.duration_us DESC
LIMIT MIN(:limit, 200);
