# Stacks Domain Context

Last verified: 2026-05

Use this file to calibrate scale, terminology, and performance magnitude. It
does not override session artifacts, profiler traces, schemas, or benchmark
results. If context conflicts with current run data, trust the run data and
mention the conflict.

## Chain Identity

- Mainnet has roughly 8M Stacks blocks as of 2026.
- Mainnet burnchain is Bitcoin.
- Nakamoto activated at Bitcoin block 867,867 / Stacks block 171,833.
- Pre-Nakamoto: one Stacks block per Bitcoin block, roughly 10 min cadence.
- Nakamoto-era: Stacks blocks are produced with a target cadence of 5s.
- In Nakamoto, a miner produces Stacks blocks during a tenure; signers validate
  blocks before acceptance.

## Height Namespaces

Do not confuse:

- Burn[chain] height: Bitcoin block height.
- Burn[chain] hash: Bitcoin block hash.
- Stacks block height: Height of a Stacks block on the Stacks chain, independent
  of Bitcoin/burnchain.
- `stacks_block_hash`: Analogous to Stacks' `block_hash`.
- `block_hash`: For Bitcoin, a block's unique hash. For Stacks, a block's hash,
  but NOT guaranteed unique across forks.
- `index_block_hash`: globally/fork-unique Stacks block hash, used internally by the
  node; not user-facing and NOT the same as `block_hash`.
- `tx_hash`: canonical transaction hash.
- `stacks-bench` synthetic block id: local integer id in one stacks-bench DB; not
  stable across data dirs, imports, or runs.

In artifacts, emit schema-named hashes — `tx_hash` for transactions and
`stacks_block_hash` for Stacks blocks (the user-visible block hash, not
`index_block_hash`). Never emit heights or `stacks-bench` synthetic ids.
Never emit `index_block_hash` — it's an internal node identifier, not the
user-visible replay form.

## Codebase Terms

- `burnchain`: generalized code term; mainnet burnchain is Bitcoin.
- `chainstate`: persistent node state, including MARF and SQLite side stores.
- `tenure`: period where one miner produces Stacks blocks, bounded by Bitcoin
  burnchain progress.
- `Clarity`: Stacks smart contract language.
- `MARF`: Merkleized Adaptive Radix Forest; the authenticated trie structure
  storing Stacks chainstate.

## Stacks Epochs

Stacks has versioned epochs, such as 2.0, 2.05, 2.1, 2.5, and 3.0, activated at
specific burnchain heights. Clarity cost weights can change at epoch
transitions. A benchmark slice that spans an epoch boundary mixes cost regimes;
cost-aggregate evidence from such a slice may be recalibration noise, not a
structural finding.

## Transaction Types

Stacks transactions include `contract-call`, `smart-contract` deploys,
`token-transfer`, `coinbase`, and a few others. `contract-call` is where most
recurring Clarity execution cost lives. `smart-contract` deploys can be
variable, but usually form a different workload family; token transfers and
coinbase are closer to fixed-cost. `tx_type_distribution.sql` shows the mix for
a run.

## Performance Calibration

- Tx execution is millisecond-scale work.
- Simple contract calls should usually be ms to tens of ms.
- 100ms+ tx execution deserves investigation.
- 1s+ tx execution is catastrophic unless explained by an unusual contract or
  benchmark artifact.
- Block commit / seal / chainstate flush should be relatively low ms on a tuned
  node, however can be overrepresented in blocks with few, small transactions.
- A few ms can matter when repeated across many txs or every block.
- Full LTO is used for release builds, so favor structural wins: fewer calls,
  fewer allocations, less/faster serialization, better batching, fewer writes,
  cheaper cacheable lookups, less iteration, etc.

## Clarity Costs

Per-tenure capacity is gated by five Clarity axes:

- `runtime`: A unit of measurement for computed resource usage (NOT a unit of
  time),
- `read_count`/`write_count`: The number of read/write operations performed, and
- `read_length`/`write_length`: The number of contract-visible bytes read/written.

A fix improves throughput only if it reduces the binding or near-binding axis.
Cost-weight recalibration is consensus-breaking and HIP-class. Useful findings
compare resource usage, wall time, and charged cost: overcharged cheap work and
undercharged expensive work are both signals.

## Benchmarking

Benchmarking is performed using the `stacks-bench` tooling, which replays Stacks
blocks and commits them into micro-forks ("synthetic blocks") against a real
Stacks mainnet chainstate snapshot.

### Benchmark Slice Context

A session often benchmarks 25k-50k block samples out of an 8M+ block history,
well under 1% of mainnet. Slice coverage is not global production frequency.

- High slice coverage means broad relevance in the sampled range.
- Medium coverage can still be production-relevant for common tx/contract
  shapes.
- Low coverage is not automatic rejection; it may identify sparse expensive
  work.
- Outlier checks matter: one-tx or one-block families should be narrowed or
  caveated.

Different Nakamoto-era ranges expose different contract, tx, and commit mixes.
Do not assume one range represents all mainnet behavior.

### What stacks-bench Exercises

`stacks-bench` primarily exercises mining / block production:

- block construction and append paths
- transaction execution, including Clarity VM contract evaluation
- MARF and SQLite state updates
- block commit / chainstate flush / seal work

It does not fully exercise:

- block validation paths
- signer-side logic
- p2p networking
- mempool admission
- operational behavior not reached by replayed blocks

"No trace evidence" can mean "not reached by this benchmark", not "cold in
production." Validation-only optimizations need separate evidence and should not
be routed as normal benchmarked PRs.
