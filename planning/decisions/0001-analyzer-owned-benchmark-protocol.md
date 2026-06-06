# Decision 0001: Analyzer Owns The Benchmark Protocol

- **status:** accepted
- **date:** 2026-06

## Decision

For every non-consensus optimization target, the analyzer emits
`verification_replay.invocations[]`. Each invocation is a self-contained
stacks-bench measurement intent: samples, warmup, repetitions, profiler mode,
and expected signal.

## Rationale

The analyzer is the phase that understands the workload shape and proposed
mechanism. Hard-coding cold/warm or txid/block behavior in the coordinator
would miss targets such as batching, lazy init, or predicate-conditioned
aliases.

## Consequences

- `verification_replay` is required on bench-eligible targets.
- Phase 1.8 and Phase 3 mirror the exact invocation protocol.
- The operator can cap invocation count with
  `analyzer.max_invocations_per_target`.
- Bad protocols fail before benchmark time is spent.
