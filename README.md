# Stacks Core Agentic Experiments

This repository contains the autonomous optimization framework for
`stacks-core`. The framework lives here; the target codebase lives as a git
submodule at [repos/stacks-core](repos/stacks-core).

That split is intentional:

- this repo can be checked out anywhere
- the framework can be updated with a normal `git pull`
- the `stacks-core` revision used by the framework is pinned by the submodule
- optimizer worktrees can still be created from `repos/stacks-core`

## Repository Layout

```text
<FRAMEWORK_ROOT>
  README.md
  overview.md
  prompts/
  schemas/
  scripts/
  sessions/
  data/
  repos/
    stacks-core/   # git submodule
```

The detailed workflow, phase contracts, and artifact model live in
[overview.md](overview.md). This README focuses only on initial setup and
updating the checkout.

In the scripts and env file, `FRAMEWORK_ROOT` means the root of this checkout.
It is derived automatically at runtime, so `/work` is now only an example
deployment location, not a requirement.

## Initial Setup

Clone this repository wherever you like, then initialize the
`repos/stacks-core` submodule:

```bash
git clone git@github.com:cylewitruk/stacks-core-agentic.git "$HOME/stacks-core-agentic"
cd "$HOME/stacks-core-agentic"
git submodule update --init --recursive
export FRAMEWORK_ROOT="$PWD"
```

The `stacks-core` submodule is expected to live at:

```text
$FRAMEWORK_ROOT/repos/stacks-core
```

That path is used by the scripts and by the default `BASE` entry in
[scripts/env.example](scripts/env.example).

## Configure The Submodule Checkout

The framework expects the `stacks-core` checkout to track the benchmark branch
you are using for experiments:

```bash
cd "$FRAMEWORK_ROOT/repos/stacks-core"
git status -sb
git remote -v
```

If you want the standard upstream wiring used by the runbook:

```bash
git remote add upstream git@github.com:stacks-network/stacks-core.git
git remote set-url --delete --push upstream '.*' || true
git fetch origin
git fetch upstream
git switch feat/stacks-bench
git branch --set-upstream-to=origin/feat/stacks-bench
```

## Agent VM Bootstrap

Create the working directories and environment file expected by the scripts:

```bash
mkdir -p "$FRAMEWORK_ROOT/data/stacks-bench" "$FRAMEWORK_ROOT/sessions"
cp "$FRAMEWORK_ROOT/scripts/env.example" "$FRAMEWORK_ROOT/.env"
```

Then edit `$FRAMEWORK_ROOT/.env` for your VM:

- confirm `BASE=$FRAMEWORK_ROOT/repos/stacks-core`
- set `SOURCE_DIR` to a directory containing `chainstate/`
- set the benchmark range you want to use
- optionally tune `CODEX_MODEL`, parallelism, and cache settings

If Codex has not been initialized yet, run it once interactively so `~/.codex`
exists. The phase scripts also expect `~/.codex` to be writable by the current
user.

Add Codex trust entries for the actual checkout location, not a hardcoded
`/work` path. At minimum, trust the submodule checkout and the sessions root:

```toml
[projects."/absolute/path/to/stacks-core-agentic/repos/stacks-core"]
trust_level = "trusted"

[projects."/absolute/path/to/stacks-core-agentic/sessions"]
trust_level = "trusted"
```

If you cloned the repo and exported `FRAMEWORK_ROOT="$PWD"` as shown above,
substitute that absolute path into the config.

For a quick environment sanity check before the demo, run:

```bash
bash "$FRAMEWORK_ROOT/scripts/preflight.sh"
```

## Build The Benchmark Binary

Pre-build `stacks-bench` before the first session so the baseline run and any
optional MCP usage do not pay first-build cost:

```bash
cd "$FRAMEWORK_ROOT/repos/stacks-core"
cargo build --release -p stacks-bench
```

## Updating The Framework

To update the framework repo and the pinned `stacks-core` revision together:

```bash
cd "$FRAMEWORK_ROOT"
git pull
git submodule update --init --recursive
```

If you are intentionally advancing the submodule while developing the
framework, update the submodule checkout, test the scripts, and then commit the
new submodule pointer in this repo.

## Running The Workflow

The two main entrypoints are:

- [scripts/run-bench-agent-coordinator.sh](scripts/run-bench-agent-coordinator.sh)
  for the full automated flow
- the individual phase scripts in [scripts/](scripts/) for a hand-driven demo
  against an imported baseline

For the step-by-step orchestration model, benchmark assumptions, and expected
artifacts, see [overview.md](overview.md).
