# Triage Queries

A small library of SQLite queries the triage agent can run against the
`stacks-bench` SQLite database to refine signal beyond the top-50 profiler
JSON snapshot. Each file is parameterized with sqlite3 named placeholders
(`:run_id`, `:span_id`, `:limit`, ...).

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
   generic VM-path fix.
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
   runs exist, surfaces spans whose recent baseline is moving.

## Query catalog

| File                                  | Purpose                                                                                  | Params                                                                     |
| ------------------------------------- | ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `run_summary.sql`                     | Run provenance + workload counts.                                                        | `:run_id`                                                                  |
| `top_spans_by_self_wall.sql`          | Primary hotspot ranking; CPU vs wait split; per-call avg.                                | `:run_id`, `:limit`                                                        |
| `span_recurrence.sql`                 | % of blocks / txs in which each span appears (returns all spans, no limit).              | `:run_id`                                                                  |
| `top_spans_by_call_count.sql`         | High-frequency spans (cache / dedup candidates).                                         | `:run_id`, `:limit`                                                        |
| `block_timing_breakdown.sql`          | Avg setup / execution / commit per block; commit-overhead baseline.                      | `:run_id`                                                                  |
| `baseline_empty_block_breakdown.sql`  | Avg per-stage cost of processing an empty block (irreducible floor).                     | `:run_id`                                                                  |
| `tx_type_distribution.sql`            | Cheap workload context: tx-type counts and total time.                                   | `:run_id`                                                                  |
| `top_contract_calls.sql`              | Top Clarity contract-functions by total wall time.                                       | `:run_id`, `:limit`                                                        |
| `span_per_sample_distribution.sql`    | Sample-weighted per-call wall-time shape (min/max/avg/p50/p95/p99) for ONE span.         | `:run_id`, `:span_id`                                                      |
| `span_per_block_distribution.sql`     | Per-block exclusive-wall percentiles + `top1/top3_share_pct` for ONE span.               | `:run_id`, `:span_id`                                                      |
| `txs_for_contract.sql`                | List the transactions calling a specific contract.function pair in one run.              | `:run_id`, `:issuer_address`, `:contract_name`, `:function_name`, `:limit` |
| `top_txs_by_duration.sql`             | Heaviest transactions in one run, with their contract/block context.                     | `:run_id`, `:limit`                                                        |
| `top_blocks_for_span.sql`             | Synthetic blocks where a span is most expensive (drill from hot span → blocks).          | `:run_id`, `:span_id`, `:limit`                                            |
| `profiler_trace_tx.sql`               | Recursive span tree for ONE transaction, indented; `:min_wall_ms` prunes noise.          | `:run_id`, `:stacks_tx_id`, `:min_wall_ms`, `:max_rows`                    |
| `profiler_trace_block.sql`            | Recursive span tree for ONE synthetic block (txs + block plumbing).                      | `:run_id`, `:synthetic_block_id`, `:min_wall_ms`, `:max_rows`              |
| `span_run_drift.sql`                  | Cross-run spread for top spans across the most-recent N runs.                            | `:recent_runs`, `:limit`                                                   |

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
  per benchmark run).
