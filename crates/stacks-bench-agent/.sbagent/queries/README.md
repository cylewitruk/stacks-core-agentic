# Benchmark DB Query Catalog

A small library of SQLite queries agents can run against the `stacks-bench`
SQLite database. Triage and analyzer agents use single-run discovery-pass
queries to refine candidate mechanisms; the results-analyzer uses paired
target-calibration-baseline vs verification-bench queries to verify whether the
optimizer moved the predicted mechanism. Each file is parameterized with
sqlite3 named placeholders (`:run_id`, `:span_id`, `:baseline_run_id`, ...).
The paired-query parameter name `:baseline_run_id` is legacy terminology for
the target calibration baseline run id.

## Source

These queries were extracted from a Metabase question dump captured during
earlier interactive analysis, then refactored to:

- replace Metabase template syntax (`{{run_id}}`, `[[OR ...]]`) with sqlite3
  named bindings;
- fix schema drift (e.g. `profiler_record.synthetic_block_id`, not
  `stacks_block_id`; no `stacks_tx_stats.estimated_commit_impact_us`);
- prefer the pre-aggregated `profiler_span_summary` /
  `profiler_span_block_summary` tables where possible;
- drop SQLite-incompatible features (`FULL OUTER JOIN`, etc.).

The original Metabase dump lives in [dump.json](dump.json) for reference.

## Invocation pattern

```bash
DB="$STACKS_BENCH_DATA_DIR/appdata/stacks-bench.db"
sqlite3 -header -csv "$DB" \
  ".parameter set :run_id 1" \
  ".parameter set :limit 25" \
  ".read $QUERIES_DIR/top_spans_by_self_wall.sql"
```

`$QUERIES_DIR` is exposed to the triage agent via the rendered prompt; in
manual invocations from the framework checkout it is `$FRAMEWORK_ROOT/queries`.
Each query's header comment lists its parameters and a runnable example.

## Recommended triage flow

1. **Orient.** [`run_summary.sql`](run_summary.sql) — confirm the run, the
   commit, the args, the workload size.
2. **Characterize the workload.** [`tx_type_distribution.sql`](tx_type_distribution.sql),
   [`block_timing_breakdown.sql`](block_timing_breakdown.sql),
   [`baseline_empty_block_breakdown.sql`](baseline_empty_block_breakdown.sql) —
   know which phase / tx type dominates before picking spans.
3. **Rank candidate spans.** [`top_spans_by_self_wall.sql`](top_spans_by_self_wall.sql)
   first; cross-check with [`top_spans_by_call_count.sql`](top_spans_by_call_count.sql)
   for high-frequency low-cost spans the wall-time ranking misses.
4. **Validate one span before promoting.** [`span_recurrence.sql`](span_recurrence.sql)
   to confirm broad distribution; [`span_per_sample_distribution.sql`](span_per_sample_distribution.sql)
   to detect long tails (treat as a shape signal, not a literal per-call
   latency — see the file's header for the per-sample-vs-per-call caveat);
   [`span_per_block_distribution.sql`](span_per_block_distribution.sql)
   to detect outlier blocks.
5. **Enrich Clarity-VM spans.** [`top_contract_calls.sql`](top_contract_calls.sql) —
   identifies which contracts/functions the Clarity-VM hot spans are running
   for, in case a per-contract optimization is more targeted than a
   generic VM-path fix. For the **throughput lens** specifically — finding
   contracts that consume disproportionate Clarity budget regardless of wall
   time — use [`top_clarity_consumers_by_contract.sql`](top_clarity_consumers_by_contract.sql)
   instead; it ranks by Clarity-cost units and surfaces near-binding axes
   per contract.
6. **Drill down when aggregates are insufficient.** Two paths:
   - **By contract/function:** [`txs_for_contract.sql`](txs_for_contract.sql)
     lists the actual transactions calling a hot contract.function pair; pick
     a heavy `stacks_tx_id` and pass it to
     [`profiler_trace_tx.sql`](profiler_trace_tx.sql) to inspect the full
     hierarchical span tree.
   - **By outlier block:** [`top_blocks_for_span.sql`](top_blocks_for_span.sql)
     surfaces the synthetic blocks where a hot span is concentrated; feed the
     `synthetic_block_id` to [`profiler_trace_block.sql`](profiler_trace_block.sql)
     to see the full block trace including block-level (non-tx) work.
   - **By raw cost:** [`top_txs_by_duration.sql`](top_txs_by_duration.sql) is
     the contract-agnostic version of step 6a — useful when the agent
     suspects a small set of pathological txs rather than a broad pattern.
   - All trace queries take a `:min_wall_ms` filter; start at 5–10ms for txs
     and 10–25ms for blocks, then lower if the result is too sparse.
7. **Cross-run trend.** [`span_run_drift.sql`](span_run_drift.sql) — when 2+
   discovery-pass runs exist, surfaces spans whose recent profile is moving.

## Results-analyzer paired comparisons

Phase 3.5 receives one target calibration baseline run id and one verification
bench run id per analyzer invocation. Use the paired queries below before
judging `matches_expected_signal`:

1. **Envelope sanity.** [`compare_run_summary.sql`](compare_run_summary.sql) —
   confirms both runs exist and compares coarse wall / block / tx totals. Treat
   this as the run envelope, not the mechanism proof.
2. **Wall-time axes.**
   [`compare_spans_between_runs.sql`](compare_spans_between_runs.sql) compares
   analyzer-named spans and is the primary mechanism check for `tx_latency` and
   most `commit_time` findings.
3. **Block-phase axes.**
   [`compare_block_timing_between_runs.sql`](compare_block_timing_between_runs.sql)
   compares setup / execution / commit / total time per block. Use it to
   corroborate commit-bucket or whole-block hypotheses.

All paired queries use the same sign convention as `results-analysis.json`:
positive `improvement_pct` means the verification bench is faster / cheaper
than the target calibration baseline.

## Query catalog

| File                                    | Purpose                                                                                  | Params                                                                     |
| --------------------------------------- | ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `run_summary.sql`                       | Run provenance + workload counts.                                                        | `:run_id`                                                                  |
| `top_spans_by_self_wall.sql`            | Primary hotspot ranking; CPU vs wait split; per-call avg.                                | `:run_id`, `:limit`                                                        |
| `span_recurrence.sql`                   | % of blocks / txs in which each span appears (returns all spans, no limit).              | `:run_id`                                                                  |
| `top_spans_by_call_count.sql`           | High-frequency spans (cache / dedup candidates).                                         | `:run_id`, `:limit`                                                        |
| `block_timing_breakdown.sql`            | Avg setup / execution / commit per block; discovery-pass commit-overhead context.         | `:run_id`                                                                  |
| `baseline_empty_block_breakdown.sql`    | Avg per-stage cost of processing an empty block (irreducible floor).                     | `:run_id`                                                                  |
| `compare_run_summary.sql`               | Target-calibration-baseline vs verification-bench coarse run envelope comparison.         | `:baseline_run_id`, `:candidate_run_id`                                     |
| `compare_spans_between_runs.sql`        | Target-calibration-baseline vs verification-bench profiler-span comparison.              | `:baseline_run_id`, `:candidate_run_id`, `:span_name`                       |
| `compare_block_timing_between_runs.sql` | Target-calibration-baseline vs verification-bench setup / execution / commit comparison. | `:baseline_run_id`, `:candidate_run_id`                                     |
| `tx_type_distribution.sql`              | Cheap workload context: tx-type counts and total time.                                   | `:run_id`                                                                  |
| `top_contract_calls.sql`                | Top Clarity contract-functions by total wall time.                                       | `:run_id`, `:limit`                                                        |
| `top_clarity_consumers_by_contract.sql` | Top Clarity-budget consumers per contract.function (5-axis breakdown + per-block max).   | `:run_id`, `:limit`                                                        |
| `span_per_sample_distribution.sql`      | Sample-weighted per-call wall-time shape (min/max/avg/p50/p95/p99) for ONE span.         | `:run_id`, `:span_id`                                                      |
| `span_per_block_distribution.sql`       | Per-block exclusive-wall percentiles + `top1/top3_share_pct` for ONE span.               | `:run_id`, `:span_id`                                                      |
| `txs_for_contract.sql`                  | List the transactions calling a specific contract.function pair in one run.              | `:run_id`, `:issuer_address`, `:contract_name`, `:function_name`, `:limit` |
| `top_txs_by_duration.sql`               | Heaviest transactions in one run, with their contract/block context.                     | `:run_id`, `:limit`                                                        |
| `top_blocks_for_span.sql`               | Synthetic blocks where a span is most expensive (drill from hot span → blocks).          | `:run_id`, `:span_id`, `:limit`                                            |
| `profiler_trace_tx.sql`                 | Recursive span tree for ONE transaction, indented; `:min_wall_ms` prunes noise.          | `:run_id`, `:stacks_tx_id`, `:min_wall_ms`, `:max_rows`                    |
| `profiler_trace_block.sql`              | Recursive span tree for ONE synthetic block (txs + block plumbing).                      | `:run_id`, `:synthetic_block_id`, `:min_wall_ms`, `:max_rows`              |
| `span_run_drift.sql`                    | Cross-run spread for top spans across the most-recent N runs.                            | `:recent_runs`, `:limit`                                                   |

## Clarity cost columns

A few queries surface Clarity cost columns from `stacks_tx_stats`
(`clarity_runtime`, `clarity_read_count`, `clarity_read_length`,
`clarity_write_count`, `clarity_write_length`). LLMs consistently misread
these — they are not timings, not bytes-on-disk, and not raw operation counts.
The exact semantics:

- **`clarity_runtime`** — *deterministic CPU + memory cost units*, derived from
  Clarity's per-function benchmark calibration. NOT wall-clock time, NOT µs,
  NOT a profiler measurement. Comparable across runs by construction;
  consumed against the tenure's `runtime` budget.
- **`clarity_read_count` / `clarity_write_count`** — number of *Clarity-level*
  read / write operations the VM observed. NOT the number of underlying MARF
  or SQLite operations (which can be amplified by tries, indexes, etc.).
- **`clarity_read_length` / `clarity_write_length`** — number of bytes
  *from the Clarity perspective* — i.e. the size of the values passed to
  `var-set`, `map-set`, etc. NOT bytes-on-disk, NOT serialized representation
  size with overhead.

These five values are consensus-critical, deterministic budget units. They
gate per-tenure tx capacity: a tenure ends when the first of the five budgets
hits its block cap. So "this contract consumes 30% of runtime budget across
the worst block" is a meaningful throughput finding even when the wall-time
spent on it is small, because it caps how many more txs the tenure could fit.

### Deferred-write coupling

MARF writes and some SQLite writes are buffered through `RollbackWrapper`
during tx execution and only materialized at block commit. So `write_count`
and `write_length` recorded against tx execution describe work whose actual
wall-time cost is paid in `Segment: Clarity State Commit` (and the index
commit phases). A fix that reduces Clarity write volume during execution
amortizes through to commit-time savings.

Aggregates over `clarity_runtime`, `clarity_read_count`, `clarity_read_length`,
`clarity_write_count`, `clarity_write_length` across multiple blocks may span
Stacks epoch boundaries, and cost weights change between epochs. Cross-epoch
aggregates can be artifacts of recalibration rather than structural cost.
Per-block epoch metadata is not currently available in the bench data; until
it is, treat run-wide aggregates with the caveat that they may mix
incompatible cost regimes.

## Schema reference

The authoritative schema is [stacks-bench/migrations/](../repos/stacks-core/stacks-bench/migrations/).
Notable tables these queries depend on:

- `profiler_span_summary` (rolled up by `(benchmark_run_id, profiler_span_id)`);
- `profiler_span_block_summary` (rolled up by
  `(benchmark_run_id, synthetic_block_id, profiler_span_id)`);
- `profiler_record` (per-record raw data with `synthetic_block_id`,
  `stacks_tx_id`, hierarchical `parent_id`);
- `stacks_block_stats`, `stacks_tx_stats` (per-block / per-tx wall-time facts);
- `block_processing_baseline` (empty-block per-stage averages, one row
  per benchmark run; table name is legacy upstream terminology).
