-- Top transactions in one run, ranked by duration.
--
-- Purpose
--   General drill-down without a contract pre-filter: surfaces the heaviest
--   individual transactions in a run so the agent can pick one to inspect
--   with `profiler_trace_tx.sql`. Useful when triage suspects a hotspot is
--   dominated by a small number of pathological txs rather than a contract
--   pattern.
--
--   This query exposes all five Clarity cost columns from `stacks_tx_stats`
--   (`clarity_runtime`, `clarity_read_count`, `clarity_read_length`,
--   `clarity_write_count`, `clarity_write_length`) — see `README.md`
--   ("Clarity cost columns") for their exact semantics. They are
--   deterministic Clarity budget units, NOT timings, NOT bytes-on-disk, NOT
--   raw operation counts.
--
-- Parameters
--   :run_id  benchmark_run.id
--   :limit   max rows
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :limit 20" \
--     ".read $QUERIES_DIR/top_txs_by_duration.sql"

SELECT
  sts.stacks_tx_id                                     AS stacks_tx_id,
  tx.tx_hash_hex                                       AS tx_hash,
  tt.name                                              AS tx_type,
  sts.synthetic_block_id                               AS synthetic_block_id,
  sb.height                                            AS stacks_block_height,
  COALESCE(p.address, '')                              AS contract_issuer,
  COALESCE(c.name, '')                                 AS contract,
  COALESCE(cf.name, '')                                AS function,
  ROUND(sts.duration_us / 1000.0, 3)                   AS duration_ms,
  sts.clarity_runtime,
  sts.clarity_read_count,
  sts.clarity_read_length,
  sts.clarity_write_count,
  sts.clarity_write_length
FROM stacks_tx_stats AS sts
JOIN stacks_tx       AS tx  ON tx.id = sts.stacks_tx_id
JOIN stacks_tx_type  AS tt  ON tt.id = tx.stacks_tx_type_id
JOIN synthetic_block AS synth ON synth.id = sts.synthetic_block_id
JOIN stacks_block    AS sb    ON sb.id    = synth.stacks_block_id
LEFT JOIN contract   AS c   ON c.id   = tx.contract_id
LEFT JOIN principal  AS p   ON p.id   = c.issuer_principal_id
LEFT JOIN contract_fn AS cf ON cf.id  = tx.contract_fn_id
WHERE sts.benchmark_run_id = :run_id
ORDER BY sts.duration_us DESC
LIMIT MIN(:limit, 200);
