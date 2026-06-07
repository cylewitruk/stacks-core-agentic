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
stanzas (`[layout]`, `[stacks_core]`, `[stacks_bench]`, `[triage]`,
`[analyzer]`, `[optimizer]`, `[results_analysis]`, `[codex]`,
`[publish]`, `[preflight]`, `[git]`); each stanza's fields are
documented inline in that file. Notable Pass 1c knobs:

- `analyzer.max_invocations_per_target` (default `8`, schema hard max
  `16`) — operator cap on `verification_replay.invocations[]` length
  per analyzer-emitted target. Enforced at analyzer-output
  validation, BEFORE Phase 1.8 runs any stacks-bench command.
- `results_analysis.confidence_floor` (default `"medium"`, values
  `"high" | "medium" | "low"`) — minimum confidence required for a
  `normal_pr` target to publish in Phase 5. Verdicts below the floor
  hold for operator review.
- `preflight.min_free_gib` (default unset / `None`) — session-start
  free-disk floor on the filesystem holding
  `layout.agent_workspace_root`. When set, the preflight emits a hard
  `Fail` if available space is below the threshold and surfaces the
  exact `sbagent workspace prune` invocation in the error body. When
  unset, the preflight emits a `Warn` only below the conservative
  10 GiB warn floor — pick a real value once you've measured a
  session's peak per-target disk. See [operations.md](operations.md)
  for the `workspace prune` recipe.

Prefer `--count` on `session baseline run` for bounded demo runs.
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
  repos/
    stacks-core/                    # submodule, tracks publish.base_branch
  sessions/<session-id>/results/    # per-session artifacts
  events/                           # append-only event log (operator-side)
```

```text
<layout.agent_workspace_root>       # default /private/tmp/sbagent-workspaces
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

`sbagent init --push` / `--seed-from` use an `http.<prefix>.extraheader`
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

[projects."/absolute/path/to/<bot>/<operator>/repos/stacks-core"]
trust_level = "trusted"

[projects."/private/tmp/sbagent-workspaces"]
trust_level = "trusted"
```

The framework checkout no longer needs `writable_roots` entries —
operator deployments don't read from it at runtime (prompts /
schemas / queries are bundled).

The `sbagent-workspaces` trust entry covers session bulk
(`/private/tmp/sbagent-workspaces/sessions/<id>/`) AND per-target
optimizer clones (`/private/tmp/sbagent-workspaces/optimizers/<id>/
<target>/`) AND transient archive worktrees in one rule — every
mutable subagent scratch dir lives under
`layout.agent_workspace_root` by design. Adjust the path to match
whatever you set `layout.agent_workspace_root` to.

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
**pre-built** binary so MCP startup doesn't pay first-build cost
(`cargo run` would invoke a build check on every startup, which can
blow past `startup_timeout_sec`).

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.stacks_bench]
command = "/absolute/path/to/<bot>/<operator>/repos/stacks-core/target/release/stacks-bench"
args    = ["--db", "/absolute/path/to/data/stacks-bench", "mcp"]
startup_timeout_sec = 30
tool_timeout_sec    = 600
enabled = true
```

`--db` points at `stacks_bench.data_dir` from your config.toml. The
bootstrap step that pre-builds `stacks-bench` (see
[setup.md](setup.md)) is what makes this safe. If you ever wipe
`target/release/`, re-run `cargo stacks-bench --help >/dev/null` from
the submodule before launching Codex to repopulate the binary.

For the demo, keep MCP optional. The direct command-line flow is
easier to debug.
