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
    target codebase). `feat/stacks-bench` must exist on this fork at
    session-start time — sbagent fetches it from `[source].url` into
    the shared bare cache. If the fork is brand-new, seed the branch
    manually before the first session: `git push <fork> <branch>` from
    any clone that already carries it.
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

Edit the minimum required fields (stanza shape — every key sits under
its `[section]`):

- `[source] url = "https://github.com/<bot>/stacks-core.git"` and
  `branch = "feat/stacks-bench"` — sbagent materializes a per-session
  source checkout from this at session start (no operator submodule).
  Optional `id = "..."` pins the bare-cache dir name; otherwise sbagent
  derives one deterministically (see `sbagent source cache-id`).
- `[publish] base_branch = "feat/stacks-bench"`,
  `token_file` (absolute path to step 2's token), plus `remote`,
  `base_repo`, `head_owner` (required — names the GitHub owner whose
  fork holds the bot's `agentic/<id>/<target>` branches; there is no
  fallback derivation post-v3), `branch_prefix`, `draft_prs`
- `[layout] prompt_overrides_dir = ".sbagent/prompts"` — sibling
  `.sbagent/schemas/`, `.sbagent/queries/`, `.sbagent/context/`,
  and top-level `<operator>/memory/` auto-derive from this
- `[layout] agent_workspace_root = "/private/tmp/sbagent-workspaces"` on
  macOS (or `/var/tmp/sbagent-workspaces/` on Linux) — mutable scratch
  state lives here, NOT in the operator repo
- `[git] author_name`, `[git] author_email` (the bot's identity)
- `[stacks_bench] source_dir`, `start_at`, `count` — required by
  `session baseline run`. See [configuration.md](configuration.md)
  for tuning.

`[dev] framework_root` is OPTIONAL; leave it unset for operator deployments.
The bundled prompts / schemas / queries inside the `sbagent` binary
are seeded to `.sbagent/` automatically; no separate framework
checkout is needed at runtime.

`sbagent` auto-resolves `~/.config/sbagent/config.toml`. To use a
different location pass `-c <path>` on every invocation.

## 4. Bootstrap the operator dir

`sbagent init` is a one-shot bootstrap that seeds
`.sbagent/{prompts,schemas,queries,context}/` from the binary, writes
a `.gitignore`, and produces an initial commit authored as the bot.
No source submodule is added — `[source]` config drives a per-session
source checkout under `<workspace>` at session start.

```bash
mkdir -p ~/operator && cd ~/operator
git init -b main
git remote add origin https://github.com/<bot>/<operator-repo>.git

sbagent init --push
```

`git init` first so `git remote add origin` has a `.git/` to record
into; `sbagent init` is then a no-op for the init step and proceeds
with bundle seeding + initial commit + push.

`--push` lands the initial commit on `origin/main` using a PAT-via-env
mechanism (token never enters argv, `.git/config`, or shell history).
The HTTPS origin is validated against `git.auth_url_prefix` (defaults
to `https://github.com/`); SSH / other-prefix URLs error up-front
rather than silently bypassing the header.

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
`<publish.token_file>` non-empty + readable (and outside
`dev.framework_root` when that setting is set — see
[publishing.md](publishing.md#threat-model-for-the-github-token) for
the full token-location rules), `publish.base_repo` reachable via
the GitHub API with that token, publish remote URL resolves to
github.com.

## 5. Configure Codex

If Codex has not been initialized yet, run it once interactively so
`~/.codex` exists. Then edit `~/.codex/config.toml` per
[configuration.md](configuration.md#recommended-codex-config) — at
minimum, trust the `<workspace>/sessions/<id>/repos/<cache_id>/` checkout (where
Phase 0a builds `stacks-bench`) and the sessions root.

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

## 7. Pre-build `stacks-bench` (optional)

Phase 0a now builds `stacks-bench` inside the per-session source
checkout under `<workspace>/sessions/<id>/repos/<cache_id>/`. There is no
operator-side checkout to warm up — the cost of the first session's
Phase 0a build is paid once per session.

Across sessions targeting the same upstream `[source]`, the shared
bare cache at `<workspace>/cache/<cache_id>.git/` makes subsequent
`git clone` steps near-instant (hardlinked objects), but the cargo
build is per-session because each session's
`repos/<cache_id>/target/` is independent.

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

Add `--with-publish` to also probe Phase 5 wiring: `publish.token_file`
is readable + non-empty, and `publish.base_repo` is reachable via the
GitHub API with that token. See [docs/publishing.md](publishing.md) for
the full Phase 5 setup.

## Updating

**Tool side** — after a `git pull` in your `stacks-bench-agent`
checkout, re-install + refresh the bundled prompts/schemas/queries in
every operator dir on the host:

```bash
cd $HOME/Code/stacks-bench-agent
git pull
just install                              # rebuilds ~/.cargo/bin/sbagent

cd ~/operator
sbagent sync                              # rewrite ALL .sbagent/ bundles
                                          # (schemas, queries, prompts, context)
                                          # — `--keep-tunables` preserves
                                          # operator-edited prompts/context
sbagent check                             # confirm no drift remains
```

`sbagent sync --push` chains the commit + push if the operator dir
should track upgrades in its own git history.

**Source side** — there is no operator-side source checkout to
advance. Every `sbagent session run` fetches `[source].branch` into
the bare cache and re-clones into the per-session checkout, so the
session always runs against the `[source].branch` tip on the
configured `[source].url` at session start. For publish sessions that
URL is the bot's writable fork (see
[Prerequisites](#prerequisites-manual-off-the-agent-vm)) — operators
keep its `[source].branch` in sync with the canonical upstream
out-of-band (`git fetch upstream && git push origin`). To pin to a
specific SHA, point `[source].branch` at a tag or named ref.

## Migrating a pre-v3 operator dir (one-time)

Run this once on an operator dir that still carries the legacy
`repos/stacks-core` submodule + `[stacks_core]` config. Each step is
an explicit shell command so you can stop and inspect at any point.

```bash
# 1. Confirm clean state on operator main and inside the submodule.
git -C ~/operator status
git -C ~/operator/repos/stacks-core status

# 2. Update config.toml: remove [stacks_core], add [source]. Use the
#    annotated template as reference.
$EDITOR ~/.config/sbagent/config.toml
#   - delete the [stacks_core] stanza entirely
#   - add [source] url + branch (typically matches the old
#     base_repo_url + publish.base_branch)

# 3. Seed the bare cache from the existing submodule (fast — local).
mkdir -p /private/tmp/sbagent-workspaces/cache      # or your workspace root
CACHE_ID="$(sbagent source cache-id)"
git clone --bare --local \
  ~/operator/repos/stacks-core \
  "/private/tmp/sbagent-workspaces/cache/${CACHE_ID}.git"

# 4. Remove the submodule from the operator dir.
git -C ~/operator submodule deinit -f repos/stacks-core
git -C ~/operator rm -rf repos/stacks-core
rm -rf ~/operator/.git/modules/repos/stacks-core
rm -f ~/operator/.gitmodules                   # if no other submodules remain
rmdir ~/operator/repos 2>/dev/null || true     # if no sibling subdirs remain

# 5. Commit the removal on operator main as the bot identity.
#    (Use the same identity sbagent uses for the seeded initial commit.)
git -C ~/operator -c user.name="Stacks BenchBot" \
    -c user.email="<bot-email>" \
    commit -m "migrate: drop repos/stacks-core submodule (v3 cutover)"

# 6. Sanity-check the new shape.
sbagent check
sbagent source cache-id     # confirms the helper resolves the same id
```

Existing archived `session/<id>` branches are unaffected — those are
write-once and keep their pre-v3 layout. Only post-migration sessions
materialize the new `<workspace>/sessions/<id>/repos/<cache_id>/` + `source.json`
shape.

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
   <layout.agent_workspace_root>/sessions/<session-id>/results/
   (per-phase subdirs: baseline/, triage/, analysis/, merge/,
   verify/, optimize/, analyze/, finalize/). The workspace path
   lives outside the operator repo by design — `sbagent session
   archive` is the boundary that commits sessions into the operator
   repo's permanent history.
7. Preserve <stacks_bench.data_dir> as the shared benchmark app-data
   dir (operators typically set this outside the operator repo for
   the same isolation reason).
8. Destroy VM and delete disposable run dir.
```

For the demo, skip this and run directly inside the agent VM.
