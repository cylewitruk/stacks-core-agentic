# Phase 5: Autonomous publishing — PRs and issues

After `finalize/summary.json` is written, the coordinator can optionally
publish autonomous-run artifacts to GitHub. The router branches per
target's `delivery_mode`:

- `normal_pr` → draft PR (or non-draft per `publish.draft_prs`) with
  operator-configured labels — **only when the full Pass 1c gate
  chain passes**: (a) `summary.experiments[].status == Accepted`,
  (b) `analyze/<target>/results-analysis.json` is on disk and
  context-valid (`session_id` + `target_id` match), (c) the
  verdict is `accepted` or `mixed` (`rejected` is skipped), and
  (d) the verdict's `confidence` is at or above
  `results_analysis.confidence_floor` (default `medium`). Any
  failing condition results in `Skip` with an explicit reason —
  `push` never sees a generated PR body for a target that didn't
  clear the gate.
- `consensus_poc_pr` → draft PR ALWAYS, with operator labels plus the
  hardcoded safety set `consensus-change,needs-HIP,do-not-merge`. No
  benchmark / verdict required — gated only on the typed optimizer
  report's `outcome=implemented`.
- `consensus_issue` → GitHub issue with `consensus-change,needs-HIP`
  labels. The optimizer never produced an implementation; the issue
  body comes from the analyzer's `consensus_writeup`.

## Architecture

Phase 5 runs entirely in-process under the agent user. There is no
sudo, no separate publisher account, and no `gh` CLI dependency. PRs
and issues are created via the GitHub REST API (`octocrab`); the
worktree → branch → push hop still shells `git`. See
[git-topology.md](git-topology.md) §5a for the per-target clone /
branch / push lifecycle in context.

| Subcommand | What it does |
| ---------- | ------------ |
| `sbagent publish generate` | Iterates `merge/optimization-targets.json` and dispatches per `delivery_mode` after running the gate chain above. For PR modes, runs `pr-writer.md` and writes `optimize/<target>/pr-title.txt` + `pr-body.md`; the prompt branches on `delivery_mode` so consensus PoC PRs frame benchmark-skipped/scoped-tests/HIP-coordination explicitly. For `normal_pr`, the prompt also receives the canonical Phase 3.5 verdict (`results_analysis_json`) — `pr-writer.md` pastes `pr_body_summary` verbatim into the PR body's Result section, appends the per-invocation breakdown table, and surfaces `caveats[]`. For `consensus_issue`, runs `issue-writer.md` and writes `optimize/<target>/issue-title.txt` + `issue-body.md` from the analyzer's `consensus_writeup`. Section validators enforce the required body shape per mode. The token is never inlined into any rendered prompt. |
| `sbagent publish push` | Reads `publish.token_file` into memory at call time, builds an authenticated `octocrab` client, and dispatches per `delivery_mode`. PR modes: switches the worktree to `agentic/<session>/<target>`, stages tracked-file modifications only (`git add -u`), commits, pushes, then creates a draft PR via the API — `consensus_poc_pr` is forced draft and gets the safety label set. `consensus_issue`: no branch / no commit / no push; creates an issue with a hidden trace tag (`<!-- agentic-<session>-<target> -->`) in the body for idempotent re-runs. Skips on existing PR/issue. |

## Threat model for the GitHub token

The token sits at `<publish.token_file>` (default
`${HOME}/.config/sbagent/gh_token`), mode 0600, owned by the agent
user. Codex never sees it because:

1. **The token lives outside whatever `publish generate` exposes to
   Codex via `--add-dir`.** Recommended location: the user's
   `~/.config/sbagent/` (the default), which is outside every path
   the publish prompts ever pass through `--add-dir`. The
   `sbagent` hard guard fires only when `dev.framework_root` is set
   (tool-developer mode) and refuses to start Phase 5 if the token
   sits inside that framework checkout. For operator deployments
   (the default — `dev.framework_root` unset) there is no automatic
   guard against putting the token inside the operator dir, so
   keep it outside the operator dir and outside
   `layout.agent_workspace_root` by convention.
2. **The token's directory is not in
   `[sandbox_workspace_write].writable_roots`** in
   `~/.codex/config.toml`.
3. **`sbagent` reads the token into its own process memory**; Codex
   runs in a separate process and cannot inspect that memory.

If you change Codex's sandbox to grant a broader read scope, move
`publish.token_file` somewhere outside the new scope, or revoke and
rotate the token.

## One-time setup

Place the bot's PAT at the default location:

```bash
mkdir -p "$HOME/.config/sbagent"
install -m 0600 /tmp/your-token "$HOME/.config/sbagent/gh_token"
```

Or override `publish.token_file` in `config.toml` to a different
absolute path. Keep it outside the operator dir and outside
`layout.agent_workspace_root` by convention — `sbagent`'s hard preflight
guard only catches tokens inside `dev.framework_root` (tool-developer
mode), so operator deployments rely on you to pick a sane location.
Whatever path you choose, drop the token there with mode 0600 owned
by the agent user.

**Recommended: GitHub fine-grained PAT** scoped to the two repos
(`<bot>/stacks-core` for optimizer PRs + `<bot>/<operator>` for
`sbagent init --push` / event-log pushes). Repository permissions:

- Contents: Read & write
- Pull requests: Read & write
- Issues: Read & write
- Metadata: Read-only (auto-included)
- Workflow: only if the bot needs to touch `.github/`

For upstream-mode (cross-owner PRs to `stacks-network/stacks-core`),
a classic PAT with the `public_repo` scope may be required —
verify with a manual draft-PR test before flipping `publish.base_repo`.

## Forge-agnostic auth

By default `publish push` (and `sbagent init --push`) builds a Basic
credential pair `x-access-token:<token>` and attaches it via
`http.https://github.com/.extraheader`. Two settings cover non-GitHub
forges:

- `git.auth_username` — defaults to `x-access-token` (GitHub
  fine-grained PATs accept this magic name). Set to `oauth2` for
  GitLab PATs, the Bitbucket username for Bitbucket Cloud app
  passwords, or `git` for self-hosted Gitea / Forgejo.
- `git.auth_url_prefix` — defaults to `https://github.com/`. Set to
  the forge's HTTPS root (with or without trailing slash —
  normalized internally) to scope the credential header to that host.

Non-HTTPS prefixes are rejected at config load. See
[configuration.md](configuration.md#forge-agnostic-auth) for the
expert / unqualified-mode escape hatch.

## Enabling Phase 5

Pass `--publish-accepted-prs` to `sbagent session run`, and configure
publishing targets in `config.toml`:

```toml
[publish]
draft_prs   = true                         # false to publish ready-for-review PRs
base_repo   = "cylewitruk/stacks-core"     # default — your fork, low blast radius
base_branch = "feat/stacks-bench"
remote      = "origin"
pr_labels   = ["needs-bench-review"]       # optional
# token_file = "/abs/path/outside/operator"  # default ${HOME}/.config/sbagent/gh_token
```

The default `publish.base_repo` targets your fork, not
`stacks-network/stacks-core`, so a runaway autonomous flow lands PRs
in your own UI rather than upstream. Override only when you've
reviewed a session and want to escalate.

## Verifying the publish wiring

```bash
sbagent check --with-publish
```

Probes (all in-process):

- `<publish.token_file>` lives outside `dev.framework_root` when that
  setting is in use (hard guard; tool-developer mode only). For
  operator deployments the check is informational — keep the token
  outside the operator dir by convention.
- `<publish.token_file>` is non-empty and readable.
- `octocrab.repos(owner, repo).get()` succeeds against
  `publish.base_repo` with that token (catches a wrong repo or an
  unauthorized token before any PR is opened).
- `publish.head_owner` is set explicitly, AND its value matches the
  owner segment of `[source].url`. Per-session source checkouts
  rewrite `origin` to `[source].url` at materialization; per-target
  clones inherit only that one remote; Phase 5 runs `git push origin
  <branch>`. So for publish sessions `[source].url` MUST be the
  writable bot fork — `https://github.com/<publish.head_owner>/...`.
  The preflight emits a `Fail` when the URL's owner segment doesn't
  match `publish.head_owner` (defense in depth against an operator
  configuring the canonical upstream as `[source].url` and silently
  pushing to the wrong place). Operators on the legacy pre-v3 layout
  (operator submodule + `[stacks_core]` config) had `head_owner`
  derived from the submodule's `git remote get-url` output and a
  separate `bot` remote replicated into per-target clones; the v3
  cutover removed both, in favor of the single-`origin` model.
  `publish.remote` is therefore constrained to `"origin"` (the
  default); preflight emits a `Fail` for any other value because no
  other remote exists in the per-target clone for Phase 5 to push
  to. A future tunable hook may let operators install an additional
  remote URL into per-target clones; until that ships, non-`origin`
  values are unsupported.

`sbagent session run --publish-accepted-prs` runs the same probes
upfront, so a misconfigured Phase 5 fails before Phases 0-4 burn
compute.

## Re-running Phase 5

`sbagent publish push` checks for an existing PR (head + base) and
existing issue (trace tag in body) before any git or API mutations,
and skips the target entirely if one is found. Re-running with the
same session id is idempotent for any target whose PR/issue already
exists. New accepted targets in subsequent sessions get their own
branches via the `agentic/<session>/<target>` naming.

`sbagent publish clean` clears per-target PR/issue artifacts so the
next `publish generate` re-renders from scratch. It does NOT touch
already-pushed PRs/issues on GitHub.
