# Decision 0002: Finalize Sources Results From The Results-Analyzer Verdict

- **status:** accepted
- **date:** 2026-06

## Decision

Finalize does not mechanically aggregate benchmark means into a verdict.
Instead, Phase 3.5 writes `analyze/<target>/results-analysis.json`, and
finalize sources `Experiment.status`, `Experiment.improvement_pct`, caveats,
and PR-result prose from that typed verdict.

## Rationale

Per-invocation measurements need interpretation. A simple mean can hide a
mechanism mismatch, cache-priming artifact, or workload-specific tradeoff.

## Consequences

- Missing or invalid verdicts produce an aborted experiment row.
- Rejected verdicts do not publish.
- Accepted and mixed verdicts can publish only if they also pass the Phase 5
  confidence floor.
- `pr_body_summary` is the canonical benchmark-result prose for normal PRs.
