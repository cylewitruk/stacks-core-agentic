# Bucket anchors

Every accepted optimization target lives structurally under exactly one of two
work buckets. The bucket is determined by the **nearest `Segment: ...` ancestor
of the `target_span`** in the trace tree. This is a classification mechanism,
not an exclusion mechanism — both buckets are valid optimization surfaces, but
they have very different cost profiles and the analyzer + merge phases need to
distinguish them.

## The two buckets

### `block_processing`

Per-tx work that runs during block production. Anchor segments:

- `Segment: Tx Execution`
- `Transaction`

Typical hot work in this bucket:

- Clarity VM interpretation, type checking, cost tracking
- MARF reads via `RollbackWrapper`
- per-tx Clarity value serialization / deserialization
- per-tx allocations and clones in the contract-call hot path

This bucket is where **latency** improvements (faster tx execution) and
**throughput** improvements (lower per-tx Clarity-cost consumption, freeing
tenure-budget headroom) are typically found.

### `block_commit`

Block-boundary work that runs once per block, after the last tx in the block
has been executed. Anchor segments:

- `Segment: Finalize (merkle+seal)`
- `Segment: Clarity State Commit`
- `Segment: Advance Chain Tip`
- `Segment: Index Commit`

Typical hot work in this bucket:

- MARF trie hashing and ancestor-link computation
- bulk MARF / SQLite write flushes (the deferred-write payoff path)
- state-root construction and validation
- SQLite commit + fsync

This bucket is where **commit-time** improvements (faster block-boundary work)
are typically found.

## Classification rule

`bucket` = the nearest `Segment: ...` ancestor of `target_span`, matched
against the anchor lists above.

## Anchors are not valid target_span values

The anchor spans **classify** the bucket; they are not themselves valid
optimization handles. They are wrappers — `Segment: Tx Execution`, the four
commit segments, `Transaction` — whose self-time is the sum of their
descendants' work. Choosing one as a `target_span` is the wrapper-level
selection the analyzer is meant to drill past.

If your investigation lands on an anchor as the candidate `target_span`,
either descend further to a real handle inside its subtree, or reject the
family. The same is true of any obvious alias for an anchor (e.g. a textual
variant pointing at the same wrapper). Treat anchors with the same span-level
exclusion logic as `non-targets.md`: forbidden as the target itself,
permitted as an ancestor on the hot path.

## Excluded anchors

These segments are NOT bucket anchors. Targets under them are out of scope for
the optimization pipeline (see `non-targets.md`):

- `Segment: Setup` — benchmark harness setup, not chain processing.
- `Segment` (the bare top-level wrapper) — benchmark harness root.

A target whose nearest `Segment: ...` ancestor is `Segment: Setup` should not
be promoted at all.

## Cross-bucket coupling: deferred writes

The two buckets are causally coupled, not independent. MARF writes and some
SQLite writes are buffered through `RollbackWrapper` during `Segment: Tx
Execution` and only materialized at `Segment: Clarity State Commit` (and the
later commit segments). So a fix that reduces *write volume* during execution
will reduce both `block_processing` cost (the staging-buffer push, small) and
`block_commit` cost (the bulk flush, large).

This means a `block_processing`-bucket target can legitimately reduce
`block_commit` wall time as a side effect, and a `block_commit`-bucket target
might be addressable from either side (reduce upstream write volume vs.
optimize the flush path). These are usually distinct fixes — they should NOT
be merged just because they share an aggregate timing effect — but the
analyzer should be aware of the coupling when reasoning about a fix's full
impact.
