# Completed: Preflight, Archive, And Provenance Hardening

- **rollup:** `preflight-archive-hardening`
- **status:** `superseded`
- **completed:** `2026-05`
- **archive kind:** historical completed-item rollup
- **split into:** [0011](../completed/0011-coordinator-provenance-sidecar.md),
  [0012](../completed/0012-flag-symmetry.md),
  [0013](../completed/0013-preflight-v1.md),
  [0014](../completed/0014-sync-refresh-by-default.md),
  [0015](../completed/0015-archive-head-sha-propagation.md),
  [0016](../completed/0016-db-artifact-consistency.md)

## Shipped

- Session-start preflight v1: installed binary drift, load-bearing prompt drift,
  and submodule reachability.
- `sbagent sync` refreshes schemas, queries, prompts, and context by default;
  `--keep-tunables` opts out.
- DB vs artifact run-id consistency warnings before finalize/archive.
- Coordinator provenance sidecar with base/head SHA and resume/finalize checks.
- `head_sha` propagation into the archive ledger.
- Flag symmetry between baseline and candidate benches.

## Follow-Up

- Branch-ref and network-fetch freshness checks were tracked as
  [`0021-preflight-v2`](0021-preflight-v2.md) and closed as
  superseded by v3's per-session source clone (v6 prologue, 2026-06-11).
- Full per-session source clone redesign remains a P2 candidate.
