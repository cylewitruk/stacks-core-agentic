# Superseded: Preflight V2

- **id:** `0021-preflight-v2`
- **status:** `superseded`
- **completed:** `2026-06-11`
- **superseded by:** v3 per-session source clone
  ([crates/stacks-bench-agent/src/source/repo.rs](../../../crates/stacks-bench-agent/src/source/repo.rs))
- **audit:** [v6 Phase 1](../../archive/completed/v6-observability-surface.md#phase-1-0021-preflight-v2-prologue-rescope)

## Original Problem

Session-start preflight v1 caught installed-binary drift, load-bearing prompt
drift, and local submodule reachability. Two drift classes remained:

1. **Branch-ref divergence** — local branch ref behind a detached-HEAD bump.
2. **Network-fetch freshness** — local-only reachability missing unfetched
   upstream state.

Both targeted the legacy shared-submodule layout (`<operator>/repos/<base>`)
under which optimizer per-target clones inherited whatever stale state the
operator's working tree held.

## Why Superseded

v3 replaced the shared submodule with a per-session source clone. Every
`sbagent session run` materializes a fresh checkout under
`<agent_workspace_root>/sessions/<id>/repos/<cache_id>/`, populated by a
two-stage flow ([source/repo.rs](../../../crates/stacks-bench-agent/src/source/repo.rs)):

1. `ensure_cache` runs
   `git fetch <source_url> +refs/heads/<branch>:refs/heads/<branch>` against
   the bare object cache — explicit refspec, fresh on every session start.
2. `clone_session_checkout` runs `git clone --branch <branch>` against the
   just-updated cache, then `resolve_head_sha` pins the resolved SHA into
   `source.json` (write-once at session start).

Mapping to `0021`'s two checks:

- **Branch-ref divergence** is structurally impossible: there is no
  operator-side branch ref to fall behind. The bare cache's
  `refs/heads/<branch>` is force-updated against the upstream URL every
  session start (the `+` prefix on the refspec), and the session checkout
  clones from that just-refreshed ref. `source.json.sha` is the durable
  pinning record.
- **Network-fetch freshness** is structurally impossible: `ensure_cache`
  runs the fetch unconditionally on every session start. The "operator
  forgot to fetch + bump" mode `0021` named has no equivalent in v3 —
  there is no operator-side fetch step to forget.

The `check_source_config` preflight check
([session/preflight.rs:93](../../../crates/stacks-bench-agent/src/session/preflight.rs#L93))
covers the residual configuration drift class (missing `[source].url`,
`[source].branch`, or `layout.agent_workspace_root`) that v3 introduced as
prerequisites for the per-session clone.

## Outcome

No code, no shrunken design. Both remaining checks are obsoleted by v3's
architecture. Backlog entry removed; this archive note preserves the
audit trail.
