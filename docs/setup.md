# Setup

One-time bootstrap for an agent VM. Run these steps before the first
optimization session.

## Prerequisites (manual, off the agent VM)

These three things `sbagent init` does NOT create — they're owned
externally and need to exist before any local step:

1. **Bot GitHub account** that will own the stacks-core fork + the
   operator repo, and whose name will appear as the commit author /
   PR opener for autonomously-generated work.
2. **Two repos on the bot's account:**
   - `<bot>/stacks-core` — fork of `stacks-network/stacks-core` (the
    target codebase). `feat/stacks-bench` doesn't need to exist on
    this fork yet; `sbagent init --seed-from` will push it.
   - `<bot>/<operator>` — empty repo for the operator dir (no README /
    `.gitignore` — init writes those). Any name; commonly something
    like `<bot>-autopilot` or `stacks-bench-agentic-operator`.
3. **Fine-grained PAT** scoped to those two repos with: Contents R+W,
   Pull requests R+W, Issues R+W, Metadata (auto). Workflow permission
   only if the bot needs to touch `.github/`.

## 1. Install `sbagent`

```bash
git clone git@github.com:cylewitruk/stacks-bench-agent.git \
  "$HOME/Code/stacks-bench-agent"
cd "$HOME/Code/stacks-bench-agent"
just install   # → ~/.cargo/bin/sbagent
```

Every example in these docs assumes `sbagent` resolves on `PATH`. Re-run
`just install` after a `git pull`, then run `sbagent sync` in each
operator dir to pull the new version's bundled prompts/schemas/queries.

## 2. Place the bot's PAT

The PAT MUST live outside the operator/tool tree — `sbagent` enforces
this at preflight to keep Codex's `--add-dir` scope from reaching it.

```bash
mkdir -p ~/.config/sbagent
install -m 0600 /tmp/your-pat ~/.config/sbagent/gh_token
```

## 3. Write the operator config

Operator config lives at `~/.config/sbagent/config.toml` by default
(XDG-compliant). Copy the annotated template from the tool repo:

```bash
cp ~/Code/stacks-bench-agent/assets/example.config.toml \
   ~/.config/sbagent/config.toml
chmod 600 ~/.config/sbagent/config.toml
```

Edit the minimum required fields:

- `base = "repos/stacks-core"` (submodule path inside the operator dir)
- `base_repo_url = "https://github.com/<bot>/stacks-core.git"`
- `publish_base_branch = "feat/stacks-bench"`
- `prompt_overrides_dir = ".sbagent/prompts"` — sibling
  `.sbagent/schemas/` and `.sbagent/queries/` auto-derive from this
- `publish_token_file` (absolute path to step 2's token)
- `publish_remote`, `publish_base_repo`, `publish_head_owner`,
  `publish_branch_prefix`, `publish_draft_prs`
- `git_author_name`, `git_author_email` (the bot's identity)
- `agent_workspace_root = "/private/tmp/sbagent-workspaces"` on macOS
  (or `/var/tmp/sbagent-workspaces/` on Linux) — mutable scratch state
  lives here, NOT in the operator repo.
- `source_dir`, `stacks_bench_start_at`, `stacks_bench_count` —
  required by `session baseline run`. See
  [configuration.md](configuration.md) for tuning.

`framework_root` is OPTIONAL; leave it unset for operator deployments.
The bundled prompts / schemas / queries inside the `sbagent` binary
are seeded to `.sbagent/` automatically; no separate framework
checkout is needed at runtime.

`sbagent` auto-resolves `~/.config/sbagent/config.toml`. To use a
different location pass `-c <path>` on every invocation.

## 4. Bootstrap the operator dir

`sbagent init` is a one-shot bootstrap that adds the stacks-core
submodule, seeds `.sbagent/{prompts,schemas,queries}/` from the
binary, writes a `.gitignore`, and produces an initial commit
authored as the bot. On a brand-new bot fork (no `feat/stacks-bench`
branch yet), pass `--seed-from <your-fork-url>` so init pushes the
substrate branch first.

```bash
mkdir -p ~/operator && cd ~/operator
git init -b main
git remote add origin https://github.com/<bot>/<operator-repo>.git

sbagent init \
  --seed-from https://github.com/<your-fork>/stacks-core.git \
  --push
```

`--push` lands the initial commit on `origin/main` using the same
PAT-via-env mechanism (token never enters argv, `.git/config`, or
shell history). The HTTPS origin is validated against
`git_auth_url_prefix` (defaults to `https://github.com/`); SSH /
other-prefix URLs error up-front rather than silently bypassing the
header.

Re-running `init` on the same dir is safe — prompt / schema / query
seeding is don't-replace, and the commit step skips when nothing is
newly staged.

## 5. Verify

```bash
cd ~/operator
sbagent check --with-publish
```

Probes (all in-process): bundle drift for schemas + queries (fail on
mismatch with the running binary), prompt drift (warn-only),
`<publish_token_file>` non-empty + readable (and outside
`framework_root` when that setting is set — see
[publishing.md](publishing.md#threat-model-for-the-github-token) for
the full token-location rules), `publish_base_repo` reachable via
the GitHub API with that token, publish remote URL resolves to
github.com.

## 5. Configure Codex

If Codex has not been initialized yet, run it once interactively so
`~/.codex` exists. Then edit `~/.codex/config.toml` per
[configuration.md](configuration.md#recommended-codex-config) — at
minimum, trust the submodule checkout and the sessions root.

## 6. Apply benchmark host tuning

Run once on the agent VM, before the first benchmark. These reduce
micro-architectural noise. Values reset on reboot, so re-apply if the
VM is rebooted.

```bash
# Pin all cores to the performance governor.
for c in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
  echo performance | sudo tee "$c" >/dev/null
done

# Disable SMT/HT for stable single-thread timings.
# Skip if the kernel doesn't expose this knob (some VM kernels don't).
if [ -w /sys/devices/system/cpu/smt/control ]; then
  echo off | sudo tee /sys/devices/system/cpu/smt/control >/dev/null
fi

# Disable ASLR for benchmarking only. Re-enable (echo 2) before doing
# anything security-sensitive on this VM.
echo 0 | sudo tee /proc/sys/kernel/randomize_va_space >/dev/null
```

Record the applied tuning state into a sibling file so a future audit
can review what host-level state was active when a session ran:

```bash
{
  echo "GOVERNOR=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)"
  echo "SMT=$(cat /sys/devices/system/cpu/smt/control 2>/dev/null || echo unknown)"
  echo "ASLR=$(cat /proc/sys/kernel/randomize_va_space)"
} > ~/.config/sbagent/.tuning
```

## 7. Pre-build `stacks-bench`

Pre-build the release binary in the operator's stacks-core checkout
before the first session so the baseline run and any optional MCP
usage do not pay first-build cost:

```bash
cd "$HOME/stacks-bench-agentic-operator/repos/stacks-core"
cargo build --release -p stacks-bench
```

## 8. Build cache

Each worktree uses its own `./target/` directory (the default). This
is required for parallel agents — sharing `CARGO_TARGET_DIR` across
worktrees would serialize builds and break concurrent experiments.
The cost is disk: ~10–20 GB per active worktree.

`sccache` is the only acceptable cross-worktree caching layer. It is
best-effort: previous attempts to use it with `stacks-core` did not
work cleanly, so do not block the demo on it. If you want to retry:

```bash
# Optional, may not work with stacks-core. If it fails, unset and rebuild.
export RUSTC_WRAPPER=sccache
export SCCACHE_DIR="$HOME/stacks-core-agentic/cache/sccache"
export SCCACHE_CACHE_SIZE=50G
mkdir -p "$SCCACHE_DIR"
```

Do **not** set `CARGO_INCREMENTAL=1` — it interferes with full LTO
release builds, which is the build profile benchmarks use.

The `RUST_BACKTRACE=1` export remains useful:

```bash
echo 'export RUST_BACKTRACE=1' >> ~/.bashrc
```

## 9. Verify

For a quick environment sanity check before the demo, run:

```bash
sbagent check
```

Add `--with-publish` to also probe Phase 5 wiring: `publish_token_file`
is readable + non-empty, and `publish_base_repo` is reachable via the
GitHub API with that token. See [docs/publishing.md](publishing.md) for
the full Phase 5 setup.

## Updating

Two repos can advance independently: the tool (`sbagent` binary) and
the operator dir (which pins a stacks-core submodule).

**Tool side** — after a `git pull` in your `stacks-bench-agent`
checkout, re-install + refresh the bundled prompts/schemas/queries in
every operator dir on the host:

```bash
cd $HOME/Code/stacks-bench-agent
git pull
just install                              # rebuilds ~/.cargo/bin/sbagent

cd ~/operator
sbagent sync                              # rewrite .sbagent/{schemas,queries}
sbagent check                             # confirm no drift remains
```

`sbagent sync --push` chains the commit + push if the operator dir
should track upgrades in its own git history.

**Operator side** — advance the stacks-core submodule from inside the
operator dir:

```bash
cd ~/operator
git submodule update --remote repos/stacks-core
# inspect, run sbagent check, commit the new pointer
```

## Later benchmark-VM replacement

After the demo, introduce a small host-side benchmark orchestrator
only for VM lifecycle. Keep the same logical command model:

```text
1. Build stacks-bench in the agent VM or inside the benchmark VM.
2. Create per-benchmark-run VM OS overlay.
3. Reflink clone chainstate.raw.
4. Boot benchmark VM.
5. Run the same cargo stacks-bench commands with the same --db,
   --json, --source, and block-range arguments.
6. Copy JSON/stderr/optimization-session artifacts back to
   <operator>/sessions/<session-id>/results/ (per-phase subdirs:
   baseline/, triage/, analysis/, merge/, optimize/, finalize/).
7. Preserve <stacks_bench_data_dir> (default
   <operator>/data/stacks-bench) as the shared benchmark app-data dir.
8. Destroy VM and delete disposable run dir.
```

For the demo, skip this and run directly inside the agent VM.
