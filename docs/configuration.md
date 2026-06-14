# Configuration

Where settings live, what each path means, and how Codex needs to be
configured to run subagents.

## Single source of truth: `config.toml`

All long-lived settings live in a single TOML file. Resolution order
(first match wins):

1. `-c <path>` / `--config-path <path>` flag.
2. `$XDG_CONFIG_HOME/sbagent/config.toml`.
3. `~/.config/sbagent/config.toml` (the recommended default).

The XDG/HOME path means operators don't have to commit
machine-specific paths into their operator repo. Per-invocation
overrides live as explicit clap flags; the only env var that backs a
flag is `SBAGENT_SESSION_ID` (on `--session-id`). No `.env` file.

The annotated template is checked in at
[assets/example.config.toml](../assets/example.config.toml) — copy it
to `~/.config/sbagent/config.toml` and edit. Settings are grouped into
stanzas (`[layout]`, `[source]`, `[stacks_bench]`, `[triage]`,
`[analyzer]`, `[optimizer]`, `[results_analysis]`, `[codex]`,
`[publish]`, `[maintain]`, `[autonomy]`, `[preflight]`, `[git]`); each stanza's fields are
documented inline in that file. Notable Pass 1c knobs:

- `analyzer.max_invocations_per_target` (default `8`, schema hard max
  `16`) — operator cap on `verification_replay.invocations[]` length
  per analyzer-emitted target. Enforced at analyzer-output
  validation, BEFORE Phase 1.8 runs any stacks-bench command.
- `results_analysis.confidence_floor` (default `"medium"`, values
  `"high" | "medium" | "low"`) — minimum confidence required for a
  `normal_pr` target to publish in Phase 5. Verdicts below the floor
  hold for operator review.
- `maintain.stale_after_days` (default `14`) and
  `maintain.secondary_rate_limit_floor_pct` (default `10`) — thresholds for
  `sbagent maintain` PR/issue lifecycle reconciliation.
- `autonomy.max_open_agent_prs` (default `10`),
  `autonomy.min_session_interval_hours` (default `144`), and
  `autonomy.zero_accepted_circuit_breaker` (default `3`) — fail-closed
  session-start gates for unattended operation.
  `autonomy.dedup_failure_threshold` (default `3`) blocks exact fix
  signatures from re-entering optimizer fan-out after that many unsuccessful
  archived attempts. They block `sbagent session run` before source
  materialization / baseline work; `sbagent maintain` remains allowed while
  paused.
- `preflight.min_free_gib` (default unset / `None`) — session-start
  free-disk floor on the filesystem holding
  `layout.agent_workspace_root`. When set, the preflight emits a hard
  `Fail` if available space is below the threshold and surfaces the
  exact `sbagent workspace prune` invocation in the error body. When
  unset, the preflight emits a `Warn` only below the conservative
  10 GiB warn floor — pick a real value once you've measured a
  session's peak per-target disk. See [operations.md](operations.md)
  for the `workspace prune` recipe.

Prefer `--count` on `session baseline run` (the Phase 0 discovery pass) for
bounded demo runs.
Avoid `--with-pre-naka` unless benchmarking pre-Nakamoto data is
intentional, because it can add significant chainstate copy time.

## Canonical paths (operator-side)

```text
<operator>                          # the dir `sbagent init` creates
  .sbagent/
    prompts/                        # MiniJinja agent templates (triage,
                                    # analyzer, merge, optimizer,
                                    # results-analyzer, pr-writer,
                                    # issue-writer). Operator-tunable
                                    # (autoresearch `program.md` model);
                                    # `sbagent check` warns on drift vs
                                    # binary bundle.
    schemas/                        # JSON Schemas — mirror of binary
                                    # bundle. DO NOT EDIT; `sbagent check`
                                    # fails on drift, `sbagent sync`
                                    # restores from binary.
    queries/                        # Triage/analyzer SQL — same contract
                                    # as schemas (mirror, do not edit).
    context/                        # Reference docs the agents read at
                                    # absolute paths (non-targets.md,
                                    # bucket-anchors.md,
                                    # stacks-domain-context.md). Tunable
                                    # with sidecar TOML declaring which
                                    # phases require each doc.
  memory/                           # Cross-session bot memory
                                    # (analyzed-rejections ledger). Lifts
                                    # out of .sbagent/ by default so it
                                    # sits next to the bundle dirs.
  sessions/<session-id>/results/    # per-session artifacts (resolves to
                                    # <agent_workspace_root>/sessions/<id>
                                    # when workspace_root is set, or
                                    # <operator>/sessions/<id> on the
                                    # legacy fallback)
  events/                           # append-only event log (operator-side)
```

No `repos/` subtree at the operator root — per-v3 there is no
operator-side source submodule. Source materializes at session start
under `<workspace>/sessions/<id>/repos/<cache_id>/` from `[source].url` +
`[source].branch`; see [git-topology.md](git-topology.md) §2.

```text
<layout.agent_workspace_root>       # recommended /private/tmp/sbagent-workspaces
  cache/<cache_id>.git/             # shared bare clone of [source].url
                                    # (one per upstream; reused across
                                    # sessions). `sbagent source cache-id`
                                    # prints the derived id.
  sessions/<session-id>/
    repos/<cache_id>/               # per-session source checkout, forked
                                    # from <cache_id>.git via
                                    # `git clone --reference --local`
  optimizers/<session-id>/<target>/ # mutable per-target git clones
                                    # — NOT inside the operator repo, so
                                    # `git status` stays clean and Codex
                                    # has a sandbox-friendly scratch root.
```

The bundle dirs (`layout.schemas_dir`, `layout.queries_dir`,
`layout.context_overrides_dir`) auto-derive from
`layout.prompt_overrides_dir`'s parent when their explicit config keys
are unset. With `prompt_overrides_dir = ".sbagent/prompts"` (the
conventional setting), the three bundle dirs land under `.sbagent/`
with no extra config.

`layout.memory_dir` is different: it holds accumulated bot knowledge
(the analyzed-rejections ledger today), not bundled/synced state. When
unset, the derived path LIFTS out of `.sbagent/` and lands at
`<operator>/memory/` — so the operator sees memory next to `.sbagent/`,
not inside it. Setting `memory_dir` explicitly is honored verbatim;
`.sbagent/memory` is a legitimate operator choice if you want it
co-located with the bundle dirs.

## Bundle lifecycle (`sbagent sync`)

Prompts, JSON schemas, and SQL queries are all embedded in the
`sbagent` binary via `include_str!`. They get to disk in two ways:

- **`sbagent init`** seeds `.sbagent/{prompts,schemas,queries,context}/`
  with don't-replace semantics. Re-runs are no-ops.
- **`sbagent sync`** (after an `sbagent` upgrade): rewrites ALL bundles
  (schemas, queries, prompts, context) unconditionally — the bundled
  versions are the contract surface. Pass `--keep-tunables` to preserve
  operator-edited prompts + context docs while still refreshing schemas
  and queries. The legacy `--force-tunables` / `--force-prompts` flags
  are accepted as deprecated no-op aliases for one release.

`sbagent check` enforces the contract:

- Schemas drift on disk vs bundle → **fail** (operator validates
  agent output against the wrong contract otherwise).
- Queries drift on disk vs bundle → **fail** (stale column ordering
  silently breaks the typed candidates/analysis pipeline).
- Prompts drift on disk vs bundle → **fail** for the load-bearing
  `optimizer.md` (orchestrator's typed-report gate depends on bundled
  contract); **warn** for analyzer/triage/merge-analyses (still
  operator-tunable). Fix with `sbagent sync` (or `sbagent sync
  --keep-tunables` to preserve other tunes).

## Forge-agnostic auth

`sbagent init --push` uses an `http.<prefix>.extraheader`
config override (injected via `GIT_CONFIG_COUNT` env-vars, never
persisted) to attach a Basic credential to git pushes. Two settings
control it:

- `git.auth_username` — defaults to `x-access-token` (GitHub
  fine-grained PATs). Set to `oauth2` for GitLab project tokens,
  the Bitbucket account username for Bitbucket app passwords,
  `git` for self-hosted Gitea / Forgejo.
- `git.auth_url_prefix` — defaults to `https://github.com/`. Set to
  your forge's HTTPS root for non-GitHub hosts (e.g.
  `https://gitlab.com/`). Trailing slash is normalized internally
  so `https://gitlab.com` and `https://gitlab.com/` resolve
  identically — defeats typosquat hosts like
  `https://gitlab.com.evil.example/`.

Non-empty prefixes that aren't `https://...` are rejected at config
load (`http://` would leak the PAT over plaintext; `git@host:` /
`ssh://` URLs ignore the extraheader entirely). Setting the prefix
to `""` is **expert / advanced mode**: the auth header is attached
unqualified and sent to **any** HTTPS remote git contacts during the
invocation. Use only after auditing every remote.

## Tool-developer mode (optional `dev.framework_root`)

`dev.framework_root` was required pre-bundle. With prompts / schemas /
queries embedded, operator deployments leave it unset. The remaining
consumers:

- `sbagent schema export` defaults `--out` to `<framework>/schemas/`
  so a tool dev regenerating schemas writes back to the source tree.
- `sbagent check`'s typed-model-vs-committed-schema drift gate runs
  only when `dev.framework_root` is set (catches "edited a Rust model,
  forgot to commit the regenerated schema"; irrelevant for operators).

Set `dev.framework_root` to your `stacks-bench-agent` source checkout
if you're iterating on the tool itself. Otherwise omit it.

## Recommended Codex config

Create `~/.codex/config.toml` inside the agent VM:

```toml
# Replace the absolute paths below with your actual checkout root.

model = "gpt-5.5"

approval_policy = "never"
sandbox_mode = "workspace-write"
web_search = "cached"

[sandbox_workspace_write]
network_access = true
writable_roots = [
  "/absolute/path/to/<bot>/<operator>",
  "/private/tmp/sbagent-workspaces",   # match `layout.agent_workspace_root`
]

[projects."/private/tmp/sbagent-workspaces"]
trust_level = "trusted"
```

The framework checkout no longer needs `writable_roots` entries —
operator deployments don't read from it at runtime (prompts /
schemas / queries are bundled).

The `sbagent-workspaces` trust entry covers EVERY mutable subagent
scratch dir in one rule:

- session bulk + per-session source checkout
  (`/private/tmp/sbagent-workspaces/sessions/<id>/`)
- the shared bare cache
  (`/private/tmp/sbagent-workspaces/cache/<cache_id>.git/`)
- per-target optimizer clones
  (`/private/tmp/sbagent-workspaces/optimizers/<id>/<target>/`)
- transient archive worktrees

Adjust the path to match whatever you set `layout.agent_workspace_root`
to.

Trust the worktree root once at bootstrap so newly created
session-scoped worktrees inherit trust without per-experiment config
edits. That entry is recursive in practice (Codex matches the
longest path prefix), but if a future Codex version tightens that,
render a per-session entry into `~/.codex/config.toml.d/` from the
session bootstrap step.

Set permissions:

```bash
chmod 700 ~/.codex
chmod 600 ~/.codex/config.toml
```

## Optional MCP configuration for stacks-bench

Useful after the direct CLI loop works. Point Codex at the
**archived** `stacks-bench` binary that Phase 0a writes per session — it
lives outside the per-session source checkout and persists for the
lifetime of the session dir, so MCP startup doesn't pay a build
cost.

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.stacks_bench]
command = "/private/tmp/sbagent-workspaces/sessions/<SESSION_ID>/results/baseline/bin/stacks-bench"
args    = ["--db", "/absolute/path/to/data/stacks-bench", "mcp"]
startup_timeout_sec = 30
tool_timeout_sec    = 600
enabled = true
```

`--db` points at `stacks_bench.data_dir` from your config.toml. The
archived binary path is per-session (rewrite the `<SESSION_ID>`
component when switching sessions, or leave MCP off if you're not
using it).

For the demo, keep MCP optional. The direct command-line flow is
easier to debug.
