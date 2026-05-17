# Stacks Core Agentic Experiments

Autonomous, benchmark-driven optimization framework for `stacks-core`.

`sbagent` runs a four-tier Codex-agent pipeline (triage → analyze →
merge → optimize) over a `stacks-bench` baseline, fans out optimizers
into per-target git clones, serially benchmarks each candidate under
a cross-process lock, and (optionally) ships the accepted results as
draft PRs or issues on GitHub. **This repo is the tool**; the
operational lifecycle (target stacks-core pin, schedule, event log,
summaries) lives in a separate operator repo bootstrapped by
`sbagent init`. The reference operator at
[`stacks-bench-agentic-operator`](https://github.com/cylewitruk/stacks-bench-agentic-operator)
shows the shape; the bot's own operator dir is created the same way.

That split is intentional:

- the tool repo ships versioned binary releases via `cargo install`
- operator repos own their target stacks-core pin (allowing third
  parties to run against any fork)
- per-session state (events, summaries) accumulates in the operator
  repo, keeping the tool's `git log` clean
- multiple operators can consume the same tool version

## Features

- **Four-tier agent pipeline** (schema v2): triage picks workload
  families, analyzers commit target spans per family, merge dedupes
  convergent fixes, optimizers implement in isolated worktrees.
- **Three delivery modes**: `normal_pr` (perf fix), `consensus_poc_pr`
  (deliberately consensus-breaking PoC, draft + safety labels), and
  `consensus_issue` (HIP-class change, no implementation).
- **Hand-driven or one-shot**: every phase is an idempotent `sbagent`
  subcommand; `sbagent session run` chains them.
- **Per-phase clean**: every phase that writes artifacts has a
  matching `clean` to reset state.
- **Cross-process locking** of `bench run` / `bench rerun` /
  `cargo nextest` via `fd-lock` (no shell `flock` dependency).
- **Tail with late-arrival semantics**: `sbagent session tail`
  multiplexes every JSONL/stderr stream as it appears.
- **One-shot operator bootstrap**: `sbagent init` materializes a
  fresh operator dir (submodule, prompts/schemas/queries bundles,
  `.gitignore`, initial commit authored as the bot). Optional
  `--push` + `--seed-from` flags handle the PAT-via-env auth so the
  token never enters argv, `.git/config`, or shell history.
- **Self-contained operator dirs**: prompts, JSON schemas, and SQL
  query bundles are embedded in the `sbagent` binary and seeded to
  `<operator>/.sbagent/{prompts,schemas,queries}/`. `sbagent sync`
  refreshes them after an upgrade; `sbagent check` fails on schemas/
  queries drift and warns on prompt drift (operator edits are
  legitimate tuning per autoresearch's `program.md` model).
- **MiniJinja-on-disk prompts**: operator-editable templates render
  in strict mode at runtime — no rebuild required to retune.
- **Forge-agnostic auth**: `git_auth_username` + `git_auth_url_prefix`
  default to GitHub but cover GitLab / Bitbucket / self-hosted forges.
  Trailing-slash normalization defends against typosquat hosts.
- **In-process GitHub publishing**: PRs and issues created via the
  GitHub REST API (`octocrab`) directly from `sbagent`; no sudo, no
  separate publisher user, no `gh` CLI dependency. Token sits at
  `<publish_token_file>` (default `${HOME}/.config/sbagent/gh_token`,
  mode 0600); `sbagent` enforces that it lives outside the framework
  root so Codex's `--add-dir` scope can never reach it.
- **TOML-only configuration**: a single `config.toml` (default
  `~/.config/sbagent/config.toml`); no `.env` files, no shell-level
  env layering.

## Repository Layout

### This repo (tool side)

```text
<this-repo>
  README.md
  docs/                       # see "Documentation" below
  prompts/                    # bundle sources (non-targets.md, bucket-anchors.md)
  schemas/                    # bundle sources (generated from typed models)
  queries/                    # bundle sources (triage/analyzer SQL)
  templates/                  # bundle sources (MiniJinja prompt templates)
  assets/                     # example.config.toml, sccache.service
  crates/stacks-bench-agent/  # the `sbagent` binary
```

All four bundle dirs are embedded in the `sbagent` binary at compile
time via `include_str!`. Operators don't read them from this repo —
they're seeded into the operator dir by `sbagent init` / `sbagent sync`.

### Operator dir (created by `sbagent init`)

```text
<operator>
  config.toml               # optional; user-level config typically lives at
                            #   ~/.config/sbagent/config.toml instead
  .sbagent/
    prompts/                # MiniJinja templates + reference docs (tunable)
    schemas/                # JSON Schemas (mirror of binary bundle, do not edit)
    queries/                # triage/analyzer SQL (mirror of binary bundle, do not edit)
  repos/
    stacks-core/            # submodule, tracks `publish_base_branch`
  sessions/<id>/results/    # per-session artifacts
  events/                   # append-only event log (operator-side only)
```

Mutable agent scratch state (per-target git clones during a session) lives
under `agent_workspace_root` — defaults to `/private/tmp/sbagent-workspaces/`
on macOS — NOT under the operator dir. Keeps `git status` clean and avoids
embedded-repo warnings.

## CLI overview

```text
sbagent init [--push] [--seed-from URL]    # bootstrap a fresh operator dir
sbagent sync [--force-prompts]             # refresh .sbagent/{schemas,queries}
                                           # (and --force-prompts) from binary
sbagent check [--with-publish]             # preflight: tools, codex compat,
                                           # bundle drift, optional publish probe

sbagent prompt lint                        # validate on-disk templates
sbagent prompt sync --force                # legacy alias; prefer `sbagent sync`

sbagent schema export                      # tool-dev: regenerate schemas/

sbagent session run                        # full pipeline (mints session id)
sbagent session validate --session-id ID
sbagent session tail [--session-id ID]

sbagent session baseline run|import|clean  # phase 0
sbagent session triage   run|clean         # phase 1
sbagent session analysis run|merge|clean   # phase 1.5 + 1.7
sbagent session optimize run|clean         # phase 2
sbagent session bench    run|clean         # phase 3
sbagent session finalize run|clean         # phase 4

sbagent publish generate                   # phase 5: render PR/issue text
sbagent publish push                       # phase 5: push to GitHub via REST API
sbagent publish clean                      # clear publish artifacts
```

Config resolution: `-c <path>` wins, else `./config.toml` in cwd, else
`$XDG_CONFIG_HOME/sbagent/config.toml`, else `~/.config/sbagent/config.toml`.

## Quick start

```bash
# 1. Build + install the tool.
git clone git@github.com:cylewitruk/stacks-bench-agent.git \
  "$HOME/Code/stacks-bench-agent"
cd "$HOME/Code/stacks-bench-agent"
just install                                # → ~/.cargo/bin/sbagent

# 2. Drop the bot's GitHub PAT (fine-grained, scoped to the two repos
#    with Contents/PRs/Issues R+W).
mkdir -p ~/.config/sbagent
install -m 0600 /tmp/your-pat ~/.config/sbagent/gh_token

# 3. Write the operator config (see assets/example.config.toml for the
#    full annotated template).
cat >~/.config/sbagent/config.toml <<'TOML'
base                  = "repos/stacks-core"
base_repo_url         = "https://github.com/<bot>/stacks-core.git"
publish_base_branch   = "feat/stacks-bench"
prompt_overrides_dir  = ".sbagent/prompts"

publish_token_file    = "/Users/me/.config/sbagent/gh_token"
publish_remote        = "bot"
publish_base_repo     = "<bot>/stacks-core"
publish_head_owner    = "<bot>"
publish_branch_prefix = "agentic"
publish_draft_prs     = true

git_author_name       = "<bot>"
git_author_email      = "<NUM>+<bot>@users.noreply.github.com"
agent_workspace_root  = "/private/tmp/sbagent-workspaces"

# Required by `session baseline run`:
source_dir            = "/mnt/chainstate/mainnet"
stacks_bench_start_at = 5_000_000
stacks_bench_count    = 25_000
TOML

# 4. Bootstrap a fresh operator dir end-to-end.
mkdir -p ~/operator && cd ~/operator
git init -b main
git remote add origin https://github.com/<bot>/<operator-repo>.git
sbagent init --seed-from https://github.com/<your-fork>/stacks-core.git --push

# 5. Smoke-test.
sbagent check --with-publish
sbagent session run --publish-accepted-prs
```

For everything quick start glosses over (Codex config, host tuning,
build cache, MCP, etc.), see [docs/setup.md](docs/setup.md). For the
config schema, see [assets/example.config.toml](assets/example.config.toml).

## Documentation

The detailed contracts are split across topical docs under
[docs/](docs/). Read them in this rough order:

- **[docs/architecture.md](docs/architecture.md)** — How the four
  agent tiers fit together, what each prompt template owns, the
  `${VAR}` exposed per phase, and the optimization guardrails.
  Start here if you want to understand *why* the pipeline is
  shaped this way.
- **[docs/configuration.md](docs/configuration.md)** — `config.toml`
  fields, canonical paths under the operator dir, bundle lifecycle,
  forge-agnostic auth, recommended Codex settings, and optional MCP
  wiring.
- **[docs/setup.md](docs/setup.md)** — One-time agent-VM bootstrap:
  install `sbagent`, place the PAT, write config, `sbagent init`,
  Codex config, host tuning, pre-built `stacks-bench`, `sbagent check`.
- **[docs/workflow.md](docs/workflow.md)** — Phase-by-phase contract
  (reads/writes per phase), the hand-driven walkthrough, the
  orchestrator, and the `summary.json` schema-v2 shape.
- **[docs/publishing.md](docs/publishing.md)** — Phase 5: PR / PoC-PR
  / issue routing per `delivery_mode`, in-process token handling,
  and the re-run / idempotency rules.
- **[docs/operations.md](docs/operations.md)** — tmux launch,
  `session tail`, `session validate`, recovery from missing
  artifacts, the no-targets recovery flow, raw `stacks-bench` CLI
  reference, and bench-lock / test-lock semantics.
