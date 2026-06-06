# GitHub setup — `stacks-bench-bot` publish topology

Concrete map of the repos, forks, and branches sbagent reads from and writes to, plus the GitHub-side setup needed before Phase 5 publish can run.

The topology has **two phases**: a **pilot** phase (now — keeps everything inside the bot's own fork) and an **upstream** phase (later — promotes PRs to the canonical repo once stacks-bench lands in `stacks-network/stacks-core`). The transition between them is a config change, not a code change.

---

## Repos in play

```text
GitHub                                  Local
─────────────────────────────────────   ──────────────────────────────────────

stacks-network/stacks-core              (canonical upstream — never directly
└─ main                                  pushed to from sbagent; future PR target)
└─ feat/stacks-bench (NOT YET — once
   you upstream stacks-bench)

cylewitruk/stacks-core                  (your personal fork — the substrate
└─ feat/stacks-bench  ◄── you push      authoring tree where YOU evolve
   stacks-bench commits here             stacks-bench. sbagent never pushes here)

stacks-bench-bot/stacks-core            <operator>/repos/stacks-core/  ← git submodule
└─ feat/stacks-bench  ◄── seeded from   ├─ HEAD detached at pinned SHA
│  cylewitruk/feat/stacks-bench;        ├─ remotes:
│  must be manually re-pushed when you  │   • origin    → cylewitruk/stacks-core
│  advance stacks-bench                 │   • bot       → stacks-bench-bot/stacks-core
└─ agentic/<session>/<target>           │   • upstream  → stacks-network/stacks-core
   ◄── pushed by sbagent on each        │     (optional, for future swap)
   successful publish                   └─ feat/stacks-bench (local ref, kept in
                                            sync with origin/feat/stacks-bench)

cylewitruk/stacks-bench-agent           ~/Code/.../stacks-bench-agent/
(this code — the sbagent CLI tool)      └─ committable, your own dev repo

cylewitruk/stacks-bench-agentic-        ~/Code/.../stacks-bench-agentic-operator/
operator                                ├─ committable (sessions/, events/,
(operator state — sessions, roadmap,    │  example.config.toml, roadmap)
etc.)                                   ├─ repos/stacks-core/   (submodule)
                                        └─ sessions/<id>/results/  (durable artifacts)

                                        ~/.config/sbagent/config.toml
                                        └─ per-user config (outside operator repo,
                                           machine-local)

                                        /private/tmp/sbagent-workspaces/
                                        └─ optimizers/<session>/<target>/
                                           (mutable agent scratch: clones +
                                            target/ build caches; outside
                                            operator repo)
```

**Roles**:

- **`stacks-network/stacks-core`** — the canonical upstream. sbagent never pushes here directly. PRs land here only after the upstream-phase swap (see below).
- **`cylewitruk/stacks-core` (your fork)** — where you evolve `stacks-bench` itself (the bench harness in `stacks-bench/`). The operator's submodule pins commits from this fork. sbagent never pushes here either.
- **`stacks-bench-bot/stacks-core` (bot fork)** — the bot's working fork. During pilot, this is both the **head** repo (where bot-generated branches live) AND the **base** repo (where bot PRs/issues land). After upstream-swap, it remains the head but the base moves to `stacks-network/stacks-core`.
- **`cylewitruk/stacks-bench-agent` (the tool)** — sbagent's Rust source. Released via `cargo install`. No operational state.
- **`cylewitruk/stacks-bench-agentic-operator` (this repo)** — operator state: session results, events, roadmap, config templates. The runtime sbagent reads here.

---

## What happens during a session (pilot mode)

```text
Phase 0 baseline
     │   sbagent runs stacks-bench against the binary built at
     │   <operator>/repos/stacks-core/target/release/stacks-bench
     │   (operator's checkout, pinned to feat/stacks-bench tip).
     ▼
Phase 1-2 triage + analysis + merge
     │   codex-driven; produces optimization-targets.json.
     ▼
Phase 2 (optimize) — for each target T:
     │
     │   sbagent (coordinator)         git clone --reference --local
     │   creates per-target clone:     <operator>/repos/stacks-core
     │                                 → /private/tmp/sbagent-workspaces/
     │                                   optimizers/<sid>/<T>/
     │                                 (origin → cylewitruk/stacks-core,
     │                                  bot    → stacks-bench-bot/stacks-core,
     │                                  upstream → stacks-network/stacks-core
     │                                  — all replicated from base)
     │
     │                                 new branch in clone:
     │                                   agent/<sid>/<T>   (off feat/stacks-bench)
     │
     │   codex agent edits source,     ← inside the clone's working tree
     │   fmt + clippy + nextest        ← inside the clone's working tree
     │   writes implementation.md      ← inside <operator>/sessions/<sid>/
     │                                   results/experiments/<T>/
     │
     │   sbagent (coordinator)         git -C <clone> add -A
     │   commits as bot:               git -C <clone> commit -m "perf: optimize <T>"
     │                                   (authored as stacks-bench-bot via env-vars)
     ▼
Phase 3 bench — coordinator-owned (sbagent runs stacks-bench against the
     │   committed binary at <clone>/target/release/stacks-bench). Functional
     │   in principle; in the operator's current setup the bench's shadow
     │   tempdir resolves under /Volumes/Extern which the sandbox blocks for
     │   codex but is writable for the coordinator process itself. The
     │   *separate* optimizer inner-loop bench that lived inside codex was
     │   removed in pass-b.1 (sandbox can't write there); coordinator-driven
     │   per-attempt bench + multi-attempt orchestration returns in pass-b.2.
Phase 4 finalize (noise / regression check)
     ▼
Phase 5 publish — for each kept + accepted target T:
     │
     │   sbagent (coordinator)         git -C <clone> push -u bot
     │                                   agent/<sid>/<T>:agentic/<sid>/<T>
     │                                 ⇒ branch lands on stacks-bench-bot/stacks-core
     │                                   as `agentic/<sid>/<T>`
     │
     │                                 gh PR create (via octocrab)
     │                                   head: stacks-bench-bot:agentic/<sid>/<T>
     │                                   base: stacks-bench-bot:feat/stacks-bench
     │                                 ⇒ draft PR lives in stacks-bench-bot/stacks-core
```

The agent's branch (`agent/<sid>/<T>` — singular) and the published branch (`agentic/<sid>/<T>` — prefix from `publish.branch_prefix`) are intentionally different names. The agent works on `agent/`; publish does a fresh push to `agentic/` so the bot fork's pushed-branch namespace stays distinct from any in-flight per-session local branches.

---

## Phase-swap config

Only **`publish.base_repo`** and **`publish.base_branch`** change between phases. Everything else stays put.

| Stage | `publish.remote` | `publish.base_repo` | `publish.base_branch` | `publish.head_owner` | Trigger to swap |
| ----- | ---------------- | ------------------- | --------------------- | -------------------- | ---------------- |
| **Pilot (now)** | `bot` | `stacks-bench-bot/stacks-core` | `feat/stacks-bench` | `stacks-bench-bot` | Bot fork exists + substrate seeded |
| **Upstream-ready** | `bot` | `stacks-network/stacks-core` | `main` (or canonical target) | `stacks-bench-bot` | `stacks-bench` lands in `stacks-network`; manual draft-PR confirms bot PAT has cross-owner permission |

---

## Pilot setup (one-time, before first publish)

### 1. Fork `stacks-network/stacks-core` as `stacks-bench-bot`

Log in as `stacks-bench-bot` on GitHub → "Fork" on `stacks-network/stacks-core` → owner = `stacks-bench-bot`. End state: `https://github.com/stacks-bench-bot/stacks-core` exists.

### 2. Seed the bot fork's `feat/stacks-bench` from your fork

```bash
cd <operator>/repos/stacks-core
# Add the bot remote if not already present:
git remote add bot https://github.com/stacks-bench-bot/stacks-core.git
# Verify the fetch URL:
git remote -v

# Push your fork's feat/stacks-bench to the bot fork:
git fetch origin feat/stacks-bench
git push bot refs/remotes/origin/feat/stacks-bench:refs/heads/feat/stacks-bench
```

Verify on GitHub UI that `stacks-bench-bot/stacks-core` now has a `feat/stacks-bench` branch matching `cylewitruk`'s tip.

### 3. Create the bot's PAT (Personal Access Token)

Logged in as `stacks-bench-bot`:

- **Settings → Developer settings → Personal access tokens → Fine-grained tokens**
- **Resource owner**: `stacks-bench-bot`
- **Repository access**: select only `stacks-bench-bot/stacks-core` (least-privilege)
- **Permissions**:
  - `Contents`: Read & write
  - `Pull requests`: Read & write
  - `Issues`: Read & write
  - `Metadata`: Read-only (auto-included)
- **Expiry**: 90 days (set a calendar reminder to rotate)
- **No `workflow` scope** for pilot — only needed if the bot starts touching `.github/workflows/`

Copy the token immediately (it's shown once).

### 4. Place the PAT on disk

```bash
mkdir -p ~/.config/sbagent
echo "<the-bot-pat>" > ~/.config/sbagent/gh_token
chmod 600 ~/.config/sbagent/gh_token
```

The operator's `config.toml` already points `publish.token_file` at this path. sbagent enforces it lives **outside** the framework root (so codex's `--add-dir` paths never expose it).

### 5. Verify the operator's local stacks-core checkout has the `bot` remote

```bash
git -C <operator>/repos/stacks-core remote -v
```

Should show three remotes:

```text
origin    https://github.com/cylewitruk/stacks-core.git    (fetch + push)
upstream  https://github.com/stacks-network/stacks-core.git (fetch + push)
bot       https://github.com/stacks-bench-bot/stacks-core.git (fetch + push)
```

`recreate_checkout` replicates every remote from this base into per-target clones, so `bot` must be present here for clones to inherit it. (Added in step 2.)

### 6. Sanity check

```bash
cd <operator>
sbagent check       # config + paths + binary all OK
sbagent prompt lint # template render OK
```

Both should print `OK`.

### Pre-publish verification (run this before EVERY publish-enabled session)

The setup steps above are one-time, but a single missed step silently breaks publish — `recreate_checkout` replicates remotes from the operator's `repos/stacks-core` into per-target clones, so if the `bot` remote isn't on the base checkout, the per-target clones don't get it either, and Phase 5's `git push -u bot <branch>` fails. Run this short check before any session that ends with `--publish-accepted-prs`:

```bash
cd <operator>

# 1. Operator checkout has the bot remote.
git -C repos/stacks-core remote get-url bot \
  || { echo "MISSING: git remote add bot https://github.com/stacks-bench-bot/stacks-core.git"; exit 1; }

# 2. Bot fork's substrate is non-empty (the seeded feat/stacks-bench tip exists).
git -C repos/stacks-core ls-remote bot refs/heads/feat/stacks-bench | grep -q . \
  || { echo "MISSING: push refs/remotes/origin/feat/stacks-bench:refs/heads/feat/stacks-bench to bot"; exit 1; }

# 3. Token file present + 0600.
test -f ~/.config/sbagent/gh_token && [ "$(stat -f '%Lp' ~/.config/sbagent/gh_token)" = "600" ] \
  || { echo "MISSING or world-readable: ~/.config/sbagent/gh_token (chmod 600)"; exit 1; }

# 4. sbagent's own preflight (validates token + token file is outside framework dir).
sbagent check
```

All four should pass silently / print `OK`.

---

## Substrate-sync workflow (ongoing)

Each time you push a new commit to `cylewitruk/feat/stacks-bench`:

```bash
cd <operator>/repos/stacks-core
git fetch origin feat/stacks-bench
git push bot refs/remotes/origin/feat/stacks-bench:refs/heads/feat/stacks-bench
```

Without this, the bot fork's substrate goes stale and new sessions' agent branches diff against an old `feat/stacks-bench`. Eventually this becomes a Layer 3A `sbagent maintain` step that runs automatically before each session; for pilot it's manual.

---

## Upstream-swap procedure (when `stacks-bench` lands in `stacks-network/stacks-core`)

### 1. Validate cross-owner PR permissions (one-time, manual)

```bash
cd <operator>/repos/stacks-core

# Create a tiny no-op branch on the bot fork, rooted on the bot's
# upstream-tracking branch (whatever `publish.base_branch` is going
# to be after the swap — main in this example). The PR needs SOME
# diff to exist or `gh pr create` will refuse.
git fetch upstream main
git checkout -B test-noop upstream/main
echo "test: bot permissions check" > .sbagent-permission-test.md
git add .sbagent-permission-test.md
GIT_CONFIG_COUNT=4 \
  GIT_CONFIG_KEY_0=user.name      GIT_CONFIG_VALUE_0=stacks-bench-bot \
  GIT_CONFIG_KEY_1=user.email     GIT_CONFIG_VALUE_1="283996447+stacks-bench-bot@users.noreply.github.com" \
  GIT_CONFIG_KEY_2=commit.gpgsign GIT_CONFIG_VALUE_2=false \
  GIT_CONFIG_KEY_3=tag.gpgsign    GIT_CONFIG_VALUE_3=false \
  git commit -m "test: bot permissions"
git push bot test-noop

# Now attempt the cross-owner draft PR using the bot's PAT.
GH_TOKEN=$(cat ~/.config/sbagent/gh_token) gh pr create \
  --repo stacks-network/stacks-core --draft \
  --head stacks-bench-bot:test-noop --base main \
  --title 'test: bot permissions' --body 'will close immediately'
```

- **403 / permission denied**: fix PAT scopes. Cross-owner PRs into a public repo via a fine-grained PAT may require the `stacks-network` org to allow third-party app actions, OR a classic (not fine-grained) PAT with the `public_repo` scope. Diagnose + retry.
- **PR created**: close it via the GitHub UI, then locally delete the test branch:

  ```bash
  # Detach from test-noop first; `git branch -D <branch>` refuses to
  # delete the currently checked-out branch. Detaching to upstream/main
  # leaves you in a sensible read-only state for inspection.
  git checkout --detach upstream/main
  git push bot :test-noop          # delete remote branch on bot fork
  git branch -D test-noop          # delete local branch
  ```

  You're cleared.

### 2. Flip two config keys in `~/.config/sbagent/config.toml`

```toml
[publish]
base_repo   = "stacks-network/stacks-core"  # was stacks-bench-bot/stacks-core
base_branch = "main"                        # or whatever upstream target is correct
# publish.remote + publish.head_owner unchanged
```

### 3. Adjust substrate-sync (probably no-op if upstream uses `main`)

Now the bot fork should sync from `stacks-network/stacks-core:main` (or the canonical target), not `cylewitruk/feat/stacks-bench`. Update your sync script accordingly:

```bash
cd <operator>/repos/stacks-core
git fetch upstream main
git push bot refs/remotes/upstream/main:refs/heads/main
```

### 4. Done

Next session's PRs go cross-owner: `stacks-bench-bot:agentic/<sid>/<T>` → `stacks-network/stacks-core:main`. Bot still owns its branches; only review target moves.

---

## What the pilot validates (and what it doesn't)

| Path | Validated by pilot? |
| ---- | ------------------- |
| Bot PAT authenticates to GitHub | ✓ |
| `git push -u bot <branch>` from per-target clone | ✓ |
| PR creation via octocrab with branch + body templates | ✓ |
| Issue creation (`consensus_issue` route) | ✓ |
| Bot identity on commits + PRs (env-var overrides) | ✓ |
| Branch + body label rendering | ✓ |
| **Cross-owner PR permissions (bot → stacks-network)** | **No — step 1 of upstream-swap covers it** |

The unvalidated piece is deliberately deferred — better to flush it out via a single manual draft-PR right before swap than to discover it mid-session.

---

## Branch cleanup (future)

Over time the bot fork accumulates `agentic/<session>/<target>` branches. Two options for handling this:

- **Manual**: periodically delete merged/abandoned branches on the bot fork via `gh api -X DELETE` or the GitHub UI.
- **Layer 3A automation**: `sbagent maintain` reads PR state from GitHub, calls `git push bot :branch` for every branch whose PR is `merged` or `closed_unmerged`. Roadmapped, not pilot-blocking.

Not urgent for pilot — at session-end, the operator can run `sbagent session optimize clean` which removes the per-target clones locally (and the agent branches inside them); the GitHub-side branches stay until cleaned up.

---

## File locations (quick reference)

| What | Where |
| ---- | ----- |
| Operator config | `~/.config/sbagent/config.toml` (per-user, outside the operator repo) |
| Operator config template | `<operator>/.sbagent/example.config.toml` (committed) |
| Bot PAT | `~/.config/sbagent/gh_token` (mode 0600) |
| Operator's stacks-core submodule | `<operator>/repos/stacks-core/` |
| Per-target agent clones (mutable) | `/private/tmp/sbagent-workspaces/optimizers/<sid>/<target>/` |
| Per-session durable artifacts | `<operator>/sessions/<sid>/results/` |
| Current planning docs | `<tool-repo>/planning/` |
| Historical autonomous roadmap | `<tool-repo>/assets/autonomous-roadmap.md` |
| This document | `<operator>/gh-setup.md` |
