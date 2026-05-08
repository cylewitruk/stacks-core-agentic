-- Top Clarity-budget consumers per (issuer, contract, function) in one run.
--
-- Purpose
--   The throughput-lens entry point for triage. Surfaces which contract
--   functions burn the most deterministic Clarity budget across the run, on
--   each of the five axes that gate per-tenure tx capacity. Distinct from
--   `top_contract_calls.sql` (which ranks by wall time) — a contract that
--   runs cheaply per call but consumes a large share of the runtime/read/
--   write budget is exactly the kind of family the throughput lens needs to
--   surface, and the wall-time ranking will miss.
--
--   See `README.md` ("Clarity cost columns") for the EXACT semantics of
--   `clarity_runtime`, `clarity_read_count`, `clarity_read_length`,
--   `clarity_write_count`, `clarity_write_length`. They are deterministic
--   Clarity budget units, NOT timings, NOT bytes-on-disk, NOT raw MARF/
--   SQLite operation counts. A tenure ends when the first of these five
--   budgets hits its block cap, so the *binding* axis (the one closest to
--   its cap) is what actually limits throughput.
--
--   `max_block_*` columns are the worst-block aggregate consumption per
--   axis — i.e. for the block where this contract.function consumed the
--   most of that axis, what was the SUM across all calls in that block.
--   Critical: this is computed from per-block sums, NOT per-tx maxima.
--   A contract called 10 times in one block (each costing 100 runtime
--   units) contributes 1000 to that block's runtime aggregate, not 100.
--   Compare against `stacks_block_stats` aggregates to identify which axis
--   is binding for the worst blocks; the analyzer will validate this in
--   code.
--
--   `max_axis_share_pct` is the maximum of the five `pct_run_*` columns —
--   i.e. the most-consumed axis as a share of run total. Rows are ordered
--   by this descending so write-heavy contracts (high pct_run_write_length
--   but low pct_run_runtime) surface alongside runtime-heavy ones rather
--   than being hidden by `LIMIT`. Tiebreak on sum_runtime for stability.
--
--   Cross-epoch caveat: aggregates over Clarity cost columns may span
--   Stacks epoch boundaries with different cost weights — see README.md.
--
-- Parameters
--   :run_id  benchmark_run.id
--   :limit   max rows
--
-- Invocation
--   sqlite3 -header -csv "$DB" \
--     ".parameter set :run_id 1" \
--     ".parameter set :limit 25" \
--     ".read $QUERIES_DIR/top_clarity_consumers_by_contract.sql"

WITH per_block AS (
  -- Aggregate to (contract.function, block) FIRST so MAX over blocks below
  -- represents the worst-block sum, not the worst-tx single-call cost.
  SELECT
    p.address                         AS issuer,
    c.name                            AS contract,
    COALESCE(cf.name, '(unknown)')    AS function,
    sts.synthetic_block_id            AS synthetic_block_id,
    SUM(sts.clarity_runtime)          AS block_runtime,
    SUM(sts.clarity_read_count)       AS block_read_count,
    SUM(sts.clarity_read_length)      AS block_read_length,
    SUM(sts.clarity_write_count)      AS block_write_count,
    SUM(sts.clarity_write_length)     AS block_write_length,
    COUNT(*)                          AS block_calls
  FROM stacks_tx_stats AS sts
  JOIN stacks_tx       AS tx ON tx.id = sts.stacks_tx_id
                            AND tx.stacks_tx_type_id = 2  -- Contract Call
  JOIN contract        AS c  ON c.id = tx.contract_id
  JOIN principal       AS p  ON p.id = c.issuer_principal_id
  LEFT JOIN contract_fn AS cf ON cf.id = tx.contract_fn_id
  WHERE sts.benchmark_run_id = :run_id
  GROUP BY p.address, c.name, cf.name, sts.synthetic_block_id
),
per_contract AS (
  -- Roll up per-block aggregates to (contract.function), with MAX now
  -- correctly representing the worst-block aggregate.
  SELECT
    issuer, contract, function,
    SUM(block_calls)         AS call_count,
    SUM(block_runtime)       AS sum_runtime,
    SUM(block_read_count)    AS sum_read_count,
    SUM(block_read_length)   AS sum_read_length,
    SUM(block_write_count)   AS sum_write_count,
    SUM(block_write_length)  AS sum_write_length,
    MAX(block_runtime)       AS max_block_runtime,
    MAX(block_read_count)    AS max_block_read_count,
    MAX(block_read_length)   AS max_block_read_length,
    MAX(block_write_count)   AS max_block_write_count,
    MAX(block_write_length)  AS max_block_write_length
  FROM per_block
  GROUP BY issuer, contract, function
),
run_totals AS (
  SELECT
    SUM(clarity_runtime)      AS total_runtime,
    SUM(clarity_read_count)   AS total_read_count,
    SUM(clarity_read_length)  AS total_read_length,
    SUM(clarity_write_count)  AS total_write_count,
    SUM(clarity_write_length) AS total_write_length
  FROM stacks_tx_stats
  WHERE benchmark_run_id = :run_id
),
ranked AS (
  SELECT
    pc.issuer,
    pc.contract,
    pc.function,
    pc.call_count,
    pc.sum_runtime,
    ROUND(100.0 * pc.sum_runtime      / NULLIF(rt.total_runtime, 0), 2)      AS pct_run_runtime,
    pc.sum_read_count,
    ROUND(100.0 * pc.sum_read_count   / NULLIF(rt.total_read_count, 0), 2)   AS pct_run_read_count,
    pc.sum_read_length,
    ROUND(100.0 * pc.sum_read_length  / NULLIF(rt.total_read_length, 0), 2)  AS pct_run_read_length,
    pc.sum_write_count,
    ROUND(100.0 * pc.sum_write_count  / NULLIF(rt.total_write_count, 0), 2)  AS pct_run_write_count,
    pc.sum_write_length,
    ROUND(100.0 * pc.sum_write_length / NULLIF(rt.total_write_length, 0), 2) AS pct_run_write_length,
    pc.max_block_runtime,
    pc.max_block_read_count,
    pc.max_block_read_length,
    pc.max_block_write_count,
    pc.max_block_write_length
  FROM per_contract AS pc
  CROSS JOIN run_totals AS rt
)
SELECT
  *,
  -- Max share across the five axes. Triage's throughput lens ranks by this
  -- so write-heavy contracts (high pct_run_write_length, low pct_run_runtime)
  -- surface alongside runtime-heavy ones.
  ROUND(MAX(
    COALESCE(pct_run_runtime,      0),
    COALESCE(pct_run_read_count,   0),
    COALESCE(pct_run_read_length,  0),
    COALESCE(pct_run_write_count,  0),
    COALESCE(pct_run_write_length, 0)
  ), 2) AS max_axis_share_pct
FROM ranked
ORDER BY max_axis_share_pct DESC, sum_runtime DESC
LIMIT MIN(:limit, 200);
