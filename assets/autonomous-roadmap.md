# Autonomous closed-loop roadmap

> **Historical source.** This document has been consolidated into the canonical
> repo-local planning system under [`planning/`](../planning/). Current selected
> work lives in [`planning/iterations/`](../planning/iterations/) and unscheduled
> items live in [`planning/backlog.md`](../planning/backlog.md); detailed
> autonomous-loop items imported from here are numbered `0030`-`0037` with matching docs under
> [`planning/design/`](../planning/design/). Keep this file for archaeology,
> not as the source of truth.

> **Maintenance contract for agents working on this roadmap:**
> After every iteration that touches an item below, the agent MUST:
>
> 1. Update the **Status** field of every item touched (legend at the top of each Layer).
> 2. Append an entry to the **Change log** at the bottom of this document recording: what was completed, what deviated from the plan and why, and any follow-up items that surfaced.
> 3. If an item is being broken up, restructured, or descoped, edit the item body — don't silently let the plan drift out of sync with reality.
>
> Historical note: this was the source of truth before the `planning/`
> restructure. New work should reference the numbered planning items instead.

---

## Status legend

- `[ ]` — Not started
- `[~]` — In progress
- `[x]` — Done
- `[skip]` — Intentionally skipped (record reason in item body + change log)
- `[!]` — Deviated from plan (record what changed + why in item body + change log)

## Goal

Reach a state where `sbagent` runs as a closed-loop autonomous service: scheduled cadence, persistent cross-session memory, GitHub PR lifecycle reconciliation, safe operator-free operation, with full audit trail in git.

## Architectural shape (decided)

- **State of record**: append-only JSONL event log at `events/*.jsonl`, committed to repo.
- **Read-side cache**: SQLite projection at `<layout.sessions_root>/.cache/history.db`, gitignored, rebuilt by replaying events.
- **Per-target branches**: `agent/<session-id>/<target-id>` (already implemented).
- **PR lifecycle**: reconciliation loop via `sbagent maintain`, polling GitHub and emitting state-change events.
- **Schedule**: GitHub Actions cron + concurrency-gated, bot identity for commits.
- **Audit**: per-target writeup committed as `.sbagent/optimizations/<target-id>.md` on each branch (single file, not full artifact tree).

Reference: this is event-sourcing + GitOps + per-target PR-branches, adapted for LLM-driven code modification. Confirmed alignment with [karpathy/autoresearch](https://github.com/karpathy/autoresearch) at the inner-loop level (commit-or-reset pattern, "NEVER STOP" prompt framing, single-line append-only attempt log).

---

## Architectural decision — tool vs operator split

**Status:** Decided 2026-05-12 — split taken. Tool: [`cylewitruk/stacks-bench-agent`](https://github.com/cylewitruk/stacks-bench-agent). Operator: this repo, [`cylewitruk/stacks-bench-agentic-operator`](https://github.com/cylewitruk/stacks-bench-agentic-operator).

> Historical note: this split decision is retained here as source context. The
> current backlog/iteration entries are in `planning/`.

The current proposal puts the autonomous lifecycle (events, schedule, secrets, target submodule, operator config) in *this* repo (`stacks-bench-agent`). Industry pattern for long-lived autonomous services (Renovate, Dependabot, Atlantis, Flux/ArgoCD) consistently separates **tool** (versioned releases, no state) from **operator** (state, schedule, config, audit log). The autoresearch repo is itself an operator, not a tool — it does not generalize across multiple training setups.

### Reasons to split

1. `git log` on tool repo stays clean — code history not interleaved with weekly operational commits.
2. Semver releases of `sbagent` are unambiguous; each session records `sbagent_version` for reproducibility.
3. Forkability — third-party operators can run `sbagent` against their own stacks-core fork without forking the tool.
4. Cleaner security boundary — secrets and bot commit history live with operator state, not tool source.
5. Audit-trail integrity — autonomous-loop commits visually separated from human tool-dev commits.
6. Prompts can become operator-tunable overrides on bundled defaults (matches the autoresearch "iterate on `program.md`" insight).

### Proposed shape (if split is taken)

**This repo (`stacks-bench-agent`)** stays a tool:

- Rust crate + CLI (binary released via cargo install / semver tags).
- Bundled default prompt templates.
- Tests, fixtures, docs.
- **Loses**: `sessions/` archive, `events/`, `.github/workflows/sbagent-*.yml`, operator config, `autonomous-roadmap.md` (mostly).
- **Gains**: a `layout.prompt_overrides_dir` config knob so operators can override bundled templates.

**New sister repo (working name `stacks-bench-operator`)** holds the autonomous lifecycle:

```text
stacks-bench-operator/
├── .github/workflows/      sbagent-maintain.yml
├── .sbagent/
│   ├── pause               presence blocks runs
│   ├── config.toml         rate limits, model, paths, sbagent version pin
│   └── prompts/            optional overrides of bundled templates
├── events/                 append-only JSONL, per-session + per-maintain-run
├── sessions/               summary-only committed snapshots
├── reports/                optional weekly digests
├── repos/stacks-core/      submodule of the target
└── autonomous-roadmap.md   this doc (mostly)
```

### Layer allocation if split is taken

- **Stays in `stacks-bench-agent` (tool)**: Layer 0 (already done), Layer 1A (schema + analyzer prompt + caller branch), Layer 1B (optimizer inner-loop mode), the `layout.prompt_overrides_dir` mechanism.
- **Moves to operator repo**: Layer 2 (event log, projection cache, dedup, commit/push), Layer 3 (`sbagent maintain`, Actions, hygiene, observability), the roadmap doc itself, `sessions/` archive, scheduling config.

### Decisions taken

- **Split: yes.** Tool (`stacks-bench-agent`) vs operator (`stacks-bench-agentic-operator`).
- **Timing: option (1)** — split now, before Layer 1A starts and before any Layer 2 state has been written anywhere.
- **Operator repo name:** `stacks-bench-agentic-operator`.
- **`feature-requests-stacks-bench.md` ownership:** stays in `stacks-bench-agent` (tool-side). Rationale: FRs are coordination between sbagent and stacks-bench, not operational state.
- **`autonomous-roadmap.md` ownership:** moves to operator repo as authoritative; this repo retains a short stub linking back.

---

## Layer 0 — Substrate

Pre-conditions outside sbagent's repo.

| Status | Item | Notes |
| ------ | ---- | ----- |
| `[x]` | stacks-bench targeted-replay (FR-1 multi-`--txid` + FR-2 `--block`) | Landed upstream as `6ec953ee941019a5eb0821713ed61962bf47691a` on 2026-05-11. Submodule (now in this operator repo at `repos/stacks-core`) pinned to that commit, release binary builds, `--txid` / `--block` / `--repetitions` verified on the CLI. |

## Tool-side infrastructure (cross-cutting)

Foundational mechanisms in `stacks-bench-agent` that aren't tied to a single Layer but are required for Layers 1-3 to land cleanly.

| Status | Item | Notes |
| ------ | ---- | ----- |
| `[x]` | Operator repo created + bootstrapped | `cylewitruk/stacks-bench-agentic-operator` bootstrapped 2026-05-12 with skeleton (README, `.gitignore`, `.sbagent/example.config.toml`, `events/`, `sessions/`, `reports/`, `.github/workflows/` placeholders). |
| `[x]` | Submodule migrated tool → operator | `repos/stacks-core` removed from tool repo on 2026-05-12; lives in operator repo, pinned to `6ec953ee94`. Tool's `Settings::base` made lazy (`Option<PathBuf>` + `Layout::require_base()`); ~25 callsites updated. |
| `[x]` | Disk-first prompt templates + `layout.prompt_overrides_dir` mechanism | Templates now live on disk in `<settings.prompt_overrides_dir>/`. Tool seeds bundled defaults on startup with don't-replace-if-exists. Askama replaced by MiniJinja (runtime parser, strict-undefined mode, keeps trailing newlines). Reference docs (`non-targets.md`, `bucket-anchors.md`) seed alongside templates — symmetric tuning surface. |
| `[x]` | `sbagent prompt lint` / `sbagent prompt sync --force` | Lint dry-renders every disk template against the matching prompt struct's field-complete synthetic context; exits non-zero on any finding. Sync force-rewrites every template from bundled defaults (refuses without `--force`). End-to-end verified on 2026-05-12. |

---

## Layer 1 — Better single-session output

Status: `[ ]`

Makes individual sessions produce higher-quality work efficiently. Unlocked by Layer 0.

### 1A. Wire targeted replay into sbagent

Status: `[x]` Done 2026-05-12 (~1 day, bundled with the unstable-id cleanup) · Depends on: Layer 0

- Add `verification_replay: { txids?: string[], blocks?: string[], repetitions: u32, rationale: string }` to `optimization-targets.schema.json` (optional field; absence falls back to current full-range bench).
  - `txids[]` entries: 0x-prefixed 64-hex-char transaction hashes (schema `pattern: "^0x[0-9a-fA-F]{64}$"`).
  - `blocks[]` entries: 0x-prefixed 64-hex-char **stacks index block hashes** (same pattern). Hash-only — never heights or synthetic ids. Rationale: integers in this field would be ambiguous between stacks height / burn height / synthetic id; hashes are globally unique and self-describing, so the artifact stays readable months after the session ran.
- Update `templates/analyzer.md` so the analyzer emits this field. Guidance:
  - `txids` for per-tx hotspots (Clarity runtime, lookups, tuple ops).
  - `blocks` for block-context paths (seal, commit, side-store flush, MARF backptr).
  - Leave null when neither cleanly applies; coordinator falls back to full-range.
  - Always emit the index block hash from `get_block_stats` / drilldown queries — never the height column.
- Branch in `crates/stacks-bench-agent/src/session/bench_experiments.rs`: if `verification_replay` is present, build `--txid X --txid Y` (or `--block <hash> --block <hash>`) with `--repetitions K` instead of `--start-at`/`--count`/`--filter`. (stacks-bench's `--block` accepts hex hashes or heights; we always pass hashes from the artifact, but the CLI side itself doesn't change.)
  - **Naming nit**: clap-side flag is singular (`--block`); JSON field stays plural (`blocks`) since it's an array.
- Update `analysis.schema.json` similarly so per-family analyzer outputs can carry `verification_replay` on each target; merge phase unions/picks the most specific recipe.
- Test fixtures + a snapshot test that an example targeted-replay recipe renders into the right CLI invocation.

**Effect**: Phase 3 wall-time drops from ~30 min/run to a few minutes/run for most targets; substrate for 1B.

### 1B. Optimizer inner-loop mode

Status: `[~]` In progress — prompt + settings + wiring landed 2026-05-12; deferred items below. · Depends on: 1A

- Re-cast `templates/optimizer.md` from one-shot task to autoresearch-style loop:
  - implement variant → `cargo fmt-stacks` → `cargo clippy-stacks`/`stackslib` (for `normal_pr`) → `cargo nextest run` → targeted-replay bench → keep (commit) or revert (`git reset --hard HEAD~1`).
  - "NEVER STOP until budget exhausted" framing, lifted from autoresearch's `program.md`.
- Coordinator-level config: `--optimizer-attempts N` and `--optimizer-budget-minutes M` flags; defaults e.g. 5 attempts or 60 min (whichever first).
- Hard per-attempt timeout that terminates the codex child (not just logs).
- Emit one event per attempt: `attempt_started`, `attempt_kept`, `attempt_reverted`, `attempt_crashed` (events introduced in Layer 2A; for now write to a per-target `attempts.jsonl` and migrate when 2A lands).
- The optimizer writes `implementation.md` describing the **final kept commit's** writeup, not the journey.

**Effect**: per-accepted-target quality jumps materially. Per-session wall time roughly unchanged because the cheap-signal substrate (1A) is in place.

---

## Layer 2 — Cross-session memory

Status: `[ ]`

Without this, every session restarts from zero; sbagent re-proposes things it already tried, ignores things it already merged. **Required for "closed loop."**

### 2A. Event log skeleton

Status: `[ ]` · Estimate: ~1 day

- Define event types as a Rust enum + schemars-emitted JSON Schema. Initial set:
  - Session-level: `session_started`, `session_finalized`.
  - Triage: `candidate_proposed`, `candidate_skipped_by_dedup` (filled in by 2B).
  - Analysis: `analysis_accepted`, `analysis_rejected`.
  - Merge: `target_merged`, `target_rejected_by_merge`.
  - Optimize: `attempt_started`, `attempt_kept`, `attempt_reverted`, `attempt_crashed` (from 1B).
  - Bench: `bench_completed`, `bench_failed`.
  - Finalize: `experiment_accepted`, `experiment_rejected`, `experiment_aborted`.
  - Publish (Phase 5): `pr_opened`, `pr_opened_failed`, `issue_opened`.
- Every event carries `event_version: 1` (versioning enforced from day one).
- Writer: append-only JSONL to `events/<session-id>.jsonl`. Use `OpenOptions::append(true)` so concurrent writes can't corrupt.
- Replay: `crates/stacks-bench-agent/src/session/history.rs` (new module) reads all `events/*.jsonl` and builds a `<layout.sessions_root>/.cache/history.db` SQLite projection. Cache is gitignored, disposable.
- Projection schema: one indexed row per `(fix_signature, session_id)` with current PR state, baseline_head, improvement_pct, latest event timestamp.
- Subcommand: `sbagent history show [--format=markdown|tsv]` renders the projection.

### 2B. Triage / merge dedup filter

Status: `[x]` · Shipped in
[`0031-triage-merge-dedup-filter`](../planning/archive/completed/0031-triage-merge-dedup-filter.md)

- Phase 1.7 (merge) reads projection at start.
- For each candidate target, look up `fix_signature` in projection. Drop if:
  - Status is `pr_open` and PR is not stale (define stale via config: e.g. `pr_stale_after_days = 30`).
  - Status is `issue_open`.
  - Status is `pr_merged` (upstream already has it).
  - Status is `tried_n_times_no_signal` with N ≥ `dedup_failure_threshold` (config; default 3).
- v12 implements this against `sessions.jsonl` + `maintain.jsonl` rather than
  waiting for the deferred 2A unified event log. Dedup skips are recorded as
  deterministic `rejected_by_merge` rows with closed `dedup:` reasons
  (`dedup:open-pr`, `dedup:open-issue`, `dedup:merged`,
  `dedup:repeated-failure`).

### 2C. Advisory optimizer memory

Status: `[x]` · Shipped in
[`0028-optimizer-memory`](../planning/archive/completed/0028-optimizer-memory.md)

- After triage identifies current families, `sbagent session run` writes
  `results/optimizer-memory.json` from `sessions.jsonl` + `maintain.jsonl`.
- The memory is compact and exact: last 5 attempts per signature, last 3
  sibling signatures per family, plus latest lifecycle URL/state and
  `source_sha` when available.
- Analyzer, merge, and optimizer prompts receive the memory as advisory
  context. It can shape recommendations and implementation choices, but only
  v12 dedup produces deterministic hard skips.

### 2D. Per-session commit + push

Status: `[ ]` · Estimate: ~½ day · Depends on: 2A

- End of `sbagent session run`: `git add events/<session-id>.jsonl sessions/<session-id>/{summary.md,targets.md,summary.json}` (summary-only; raw artifacts stay off the main branch — see L1 / L3 decisions).
- Commit message template: `session <id>: <accepted>↑ <rejected>↓ <aborted>✗ (<dur>)`.
- Push to `origin/main` (or configured branch).
- Skip if events file is empty (failed before any event emitted).
- Loop-detection guard: do not run `sbagent` recursively from a workflow triggered by its own commit.

**Effect after Layer 2**: sessions form a coherent series. Re-running doesn't redo the same work. `git log` is a readable audit trail. Manual triggering still required.

---

## Layer 3 — Autonomy / lifecycle

Status: `[ ]`

Required for "just runs and runs." Each item independently shippable but 3A and 3C should land before 3B (no scheduled cron without lifecycle awareness + hygiene).

### 3A. `sbagent maintain` subcommand (reconciliation loop)

Status: `[x]` · Shipped in
[`0033-maintain-command`](../planning/archive/completed/0033-maintain-command.md)

- Reads `sessions.jsonl` and `maintain.jsonl`.
- Queries GitHub for non-terminal PR/issue artifacts.
- Emits typed lifecycle events (`pr_open`, `pr_merged`,
  `pr_closed_unmerged`, `pr_stale`, `pr_force_pushed`,
  `pr_branch_deleted`, `issue_open`, `issue_closed`) into
  `maintain.jsonl`.
- Commits + pushes `maintain.jsonl` on real runs.
- No code modifications. This is pure observation + state delta; future
  iterations can add PR mutations if needed.

### 3B. GitHub Actions wiring

Status: `[x]` · Shipped in
[`v11-autonomy-safety-and-maintain-schedule`](../planning/archive/completed/v11-autonomy-safety-and-maintain-schedule.md)

- Operator-template workflow for `.github/workflows/sbagent-maintain.yml` only:
  - copied from
    `assets/operator-templates/.github/workflows/sbagent-maintain.yml`.
  - `schedule: cron: <daily>`, `workflow_dispatch`, and `sessions.jsonl`
    push trigger.
  - Shared `concurrency` group reserved for autonomy jobs.
  - Bot identity configured early.
- Loop-detection guard: `if: github.actor != 'stacks-bench-bot'`.
- `sbagent session run` is not scheduled in GitHub-hosted CI. Benchmark
  sessions need a dedicated host with chainstate/data mounts and should use
  local scheduling, with systemd templates shipped in
  [`0050-local-session-cron`](../planning/archive/completed/0050-local-session-cron.md).

### 3C. Hygiene (required, not optional)

Status: `[x]` · Shipped in
[`v11-autonomy-safety-and-maintain-schedule`](../planning/archive/completed/v11-autonomy-safety-and-maintain-schedule.md)

The pause file + rate limits + circuit breaker are not nice-to-haves — they're the difference between "runs unattended" and "runs unattended *without burning down the review queue / codex budget*." Same line item as 3A.

- **Pause file**: `.sbagent/pause` in repo blocks `session run` from starting. Use during release freezes / incidents. `maintain` keeps running (read-only).
- **Rate limits** in `config.toml`:
  - `max_open_agent_prs` (config; default 10) — block `session run` if exceeded.
  - `min_session_interval_hours` (default 144) — block if last session was too recent.
- **Circuit breaker**: if last K sessions had zero `experiment_accepted` events, set `pause` file automatically and require manual reset (K = 3 default).
- Later hardening can add weekly bench-hour budgets, event-version
  enforcement, GitHub idempotency keys, and signed commits.

### 3D. Observability surface

Status: `[ ]` · Estimate: ~1 day · Depends on: 2A, 3A

- `sbagent history report --format=markdown` — human-readable weekly digest: sessions count, PRs opened/merged/closed, top fix_signatures by attempts, agent token spend if tracked, time-to-merge distribution.
- Optional: commit that report to `reports/<iso-week>.md` weekly so the repo itself serves as a dashboard.
- Skip if no usage interest — defer to a v2 if running manually for a while suffices.

---

## Future proposals (out of v1 scope; record now, evaluate later)

### Triage-emitted anchor benchmarks

**Status:** `[ ]` Proposed 2026-05-12. Defer until current Layer 1B (single-tx
targeted replay + warmup defaults) is validated end-to-end; revisit if
inner-loop signal proves too noisy or cache-warmness drift between phases
matters for production fidelity.

**Idea:** triage runs `--txid` + `--block` benchmarks for promoted
candidates with a large `--warmup`, stores `anchor_run_id` per
representative on `candidates.json`. Analyzer drilldowns, optimizer local
baseline (Phase 2 Step 0), and Phase 3 verification ALL re-use those
anchor runs instead of running their own benchmarks. Same recipe,
same cache state, same `total_duration_us` semantics across every
measurement point.

**Pros:**

- Eliminates the per-target Step 0 local-baseline bench (~12 min × N
  targets saved).
- Makes the cache-warmness profile consistent end-to-end — every
  improvement_pct comparison uses the same cold-fork-then-warmup baseline.
- Phase 3 verification becomes a same-recipe comparison against the
  anchor instead of a fresh bench, which simplifies finalize.

**Cons:**

- Triage's job grows from ~10 min LLM call to LLM + per-representative
  bench runs. With ~20 representatives and 7 min/bench, ~2+ hours added
  upstream.
- Schema changes ripple: `candidates.json` needs `anchor_run_id` per
  representative; analyzer / merge / Phase 2 / Phase 3 need to consume it.
- Adds upstream complexity before we've validated the downstream signal.

**Trigger to revisit:** observed run-to-run variance > noise floor in
Layer 1B inner loops despite reasonable warmup, OR a kept attempt fails
Phase 3 verification because cache-warmness flipped the keep/revert
decision between phases.

## Out of scope / future

- **Multi-branch / staging**: run a fork of `sbagent` against a non-production stacks-core fork as a staging environment before promoting prompt-template changes to the prod loop. Worth considering once Layer 3 is stable.
- **Snapshot pruning**: event log grows linearly; when replay exceeds ~30s, write a periodic compacted snapshot and replay only from there. Defer until measured replay time becomes a problem.
- **Multi-target inner loop coordination**: today the optimizer fans out N targets in parallel, each in its own worktree, each running its own inner loop independently. Joint optimization (target A's win affects target B's measurement) is a separate problem; explicitly NOT planned.
- **Feedback ingestion from PR comments**: when a reviewer comments on an agent PR, auto-queue a fresh session targeting that fix_signature with the comment as additional optimizer context. Powerful but a big design lift. Defer.
- **Full artifact tree on per-target branches**: discussed and rejected — would pollute diffs. Single `.sbagent/optimizations/<target-id>.md` audit file is the agreed-upon compromise (decision recorded in Change log).

---

## Recommended start sequence

If only one thing ships next: **Layer 1A** (`verification_replay` wiring). ½ day, dramatically faster Phase 3, dependency for everything else.

After 1A lands and one session has run successfully through the new code path:

1. **Layer 1A → 1B** as a unit. Ship a session, verify quality jump.
2. **Layer 2A → 2B → 2C** as a cohesive bundle (partial state is worse than no state).
3. **Layer 3A → 3C → 3B**, in that order. Lifecycle awareness + hygiene before scheduled cron.
4. **Layer 3D** last (or skip until needed).

Total rough effort: 6-8 days to "autonomous closed-loop running on a schedule, safely."

---

## Change log

Entries are reverse-chronological. Each entry: date, short summary, what items moved, deviations, follow-ups.

### 2026-05-13 (Layer 1B v2 pass-b.1 — coordinator commit)

Pass-(a) + (a.5) test-drive surfaced what we already suspected: codex's `workspace-write` sandbox + macOS Seatbelt deny writes to `.git/` even when the clone's `.git/` directory lives inside the agent's cwd (observed via the `com.apple.provenance` xattr behavior). Switching from linked worktrees to clones moved git state into cwd, but the sandbox still wouldn't let the agent commit. The agent diagnosed this correctly, ran fmt + clippy + nextest cleanly, wrote `implementation.md`, but had no way to produce the commit object the rest of the pipeline depends on.

The lesson is the same as for `/Volumes/Extern/.stacks-bench-shadows`: **anything requiring trusted filesystem mutation outside ordinary source edits should leave codex.** Pass-(b) moves both `git commit` and `stacks-bench` to the coordinator process (which runs outside any sandbox). We split delivery into b.1 (commit) and b.2 (bench + multi-attempt orchestration) — independent, ordered.

**Shipped (pass-b.1)**:

- `templates/optimizer.md` rewritten as a **single-shot, no-git, no-bench** prompt. Steps reduce to: hypothesize + implement → `cargo fmt-stacks` → `cargo clippy-stacks` + `cargo clippy-stackslib` (normal_pr only) → `cargo nextest run --retries 2` → `cargo build --release -p stacks-bench` → write `implementation.md` (kept) or `abort.md` (any gate failed). The agent is explicitly told **not to touch `.git/` or run `stacks-bench`**. From ~200 prompt lines down to ~100; massively simpler mental model for the agent.
- Coordinator-side commit in `optimizers::run_one`, post-codex, pre-HEAD-advance gate. Two new helpers in `optimizers.rs`:
  - `coordinator_commit_if_kept(exp_dir, checkout, target_id, settings)` — runs the strict verification contract:
    1. `implementation.md` present AND `abort.md` absent (else demote)
    2. `git status --porcelain` reports non-empty output (else demote — "agent wrote marker but did nothing")
    3. `git add -A && git commit -m "perf: optimize <target>"` succeeds with `optimizer_git_env` env-vars applied (else demote with diagnostic)
  - `demote_kept_to_abort(exp_dir, target_id, reason)` — moves `implementation.md` → `.demoted`, writes `abort.md` with the diagnostic. Shared shape with the existing `verify_kept_or_demote` demotion mechanics.
- `verify_kept_or_demote` becomes defense-in-depth — `coordinator_commit_if_kept` catches most failure modes pre-emptively; the HEAD-advance gate still runs after to catch any path that produced `implementation.md` without a corresponding HEAD advance.
- `optimizer_attempts` config + CLI flag preserved (surface stability) but effectively clamped to 1; emits `tracing::warn!` if operator set > 1. Multi-attempt orchestration returns coordinator-driven in pass-b.2.
- Tests: four new unit tests on `coordinator_commit_if_kept` (happy path, clean-tree demotion, both-markers demotion, no-marker no-op); existing `tests/optimizers.rs` + `tests/orchestrator_chain.rs` updated so `FakeGit` does `git init --bare` style setup and `FakeHarness`/`ChainHarness` simulate the agent's source edit. New `optimizer_prompt_forbids_git_and_bench_operations` test in `prompts.rs` locks the b.1 guardrails ("must not touch `.git/`", no `git commit` instructions, no `stacks-bench bench run`, no `--shadow-dir-root` in the rendered prompt).
- Four obsolete prompt-rendering tests removed (`optimizer_prompt_omits/emits_network/shadow_dir_root` — those asserted on bench-command rendering that's no longer in the template).

**What this fixes**:

- `.git/` sandbox-deny issue: the agent never tries `git commit`, so it can't get blocked. The coordinator's commit runs outside codex, where `git commit` works as expected.
- "Agent wrote implementation.md but didn't actually commit anything" false-positives (the failure mode that triggered the HEAD-advance gate v1.3): now impossible because the coordinator commit happens or doesn't happen at a well-defined point, not the agent's interpretation.
- Prompt complexity: the autoresearch-style "NEVER STOP" inner-loop discipline (`LAST_KEPT_SHA`, `git reset --hard`, `git clean -fd`, attempt budgeting) is gone — none of it was reachable inside the sandbox anyway.

**What this does NOT fix**:

- Bench is still inside codex, still blocked by `/Volumes/Extern` write denial. Pass-b.2 moves bench to the coordinator. Until then: the agent's keep/abort decision is based on fmt + clippy + nextest only; no local improvement_pct signal.
- Multi-attempt loop: gone temporarily; coordinator-driven multi-attempt orchestration returns in pass-b.2.

105 tests pass · `just lint` clean · `sbagent check` + `prompt lint` OK from operator.

### 2026-05-13 (Layer 1B v2 pass-a.5 — agent workspace root)

Followup to pass-(a) (linked worktrees → per-target clones): the clones still lived UNDER the operator repo at `sessions/<id>/worktrees/<target>/`, which is a poor fit for the "operator = durable records, workspace = mutable execution state" separation. Each clone's `target/` is ~25-30GB post-build; embedded inside the operator tree it polluted `du`, Spotlight, Time Machine, and produced "embedded git repo" warnings on every `git status`.

**Shipped**:

- New `Settings::agent_workspace_root: Option<PathBuf>` — generic root for mutable agent scratch state, NOT optimizer-specific. Today only the optimizer phase populates it; future phases (analyzers, merge, publish, future per-phase scratch state) get sibling subdirs as needed without rearranging the operator's setup.
- Layout method `Layout::session_optimizer_checkouts_dir(id)` encapsulates the resolution:
  - Set: `<agent_workspace_root>/optimizers/<session_id>/<target_id>/`
  - Unset (legacy default): `<layout.sessions_root>/<id>/worktrees/<target>/`
- Every callsite that previously joined `session_worktrees_dir` for optimizer checkouts now goes through `session_optimizer_checkouts_dir`: `optimizers::run_one`, `cli/session/optimize/clean.rs`, `cli/session/run.rs` session-end prune + Phase 3 chained bench, `cli/session/bench/run.rs`, `publish.rs` push + render. Single point of truth — pass-b can read the same method for coordinator-side bench.
- macOS recommendation documented as `/private/tmp/sbagent-workspaces` (cleared on boot, outside Spotlight + Time Machine, codex Seatbelt writes there cleanly). NOT platform-magic — operator-configured.
- Three new path-resolution tests: unset = legacy path, set = workspace root, relative root gets absolutized.
- Operator `config.toml` already updated with the recommended macOS path.

**Effect**: optimizer clones move outside the operator repo. No more embedded-repo warnings. The operator's `du -sh .` no longer counts agent build caches. Pass-b's coordinator-side bench has a stable, documented path to read the clone's binary from.

### 2026-05-13 (Layer 1B v2 — worktrees → clones; bench moves back to coordinator)

End-to-end test-drives in v1.2 and v1.3 surfaced two architectural mismatches that v1.x kept patching around:

1. **Linked worktrees + submodule git-dir is hostile to per-target sandbox isolation.** An `agent/<session>/<target>` worktree lives at `<operator>/sessions/<id>/worktrees/<target>`, but its `.git` is a file pointing back to `<operator>/.git/modules/repos/stacks-core/worktrees/<target>/`. Every `git commit` / `git reset` from inside the worktree writes the index lock + refs to that out-of-cwd path. We tried adding the submodule's `--git-common-dir` to codex `--add-dir`; macOS Seatbelt still blocked the writes. Patching `--add-dir` for every git-internal path is whack-a-mole.

2. **Bench-from-inside-codex pulls a privileged host operation across the trust boundary.** Layer 1B v1 put a per-attempt targeted-replay bench into the optimizer prompt's inner loop ("apples-to-apples local baseline"). That works fine when the source dir + shadow root live on the operator's home filesystem, but fails when they're on `/Volumes/Extern` (the realistic deployment shape): macOS Seatbelt denies writes to external-drive paths even with `--add-dir`. The coordinator already owns the trusted bench-runner role — moving bench *out* of codex is the architecturally correct fix, not adding more `--add-dir` paths.

#### Plan

Three changes; (a) and (b) ship as separate passes since they're logically independent.

**(a) Replace per-target linked worktrees with per-target local clones.**

- `git clone --reference <base> --branch <base_branch> --local <base> <clone>` (or equivalent) so the clone shares the base repo's object store (no extra GB on disk) but has its OWN `.git/` directory inside the agent's cwd. Sandbox writes never leave the agent's checkout.
- Branch creation moves inside the clone (`git -C <clone> switch -c agent/<session>/<target>`); the agent's commits land on that branch local to the clone.
- Phase 5 publish: fetch the agent's branch from the clone back to the operator's `repos/stacks-core` checkout, then push from there to the bot's GitHub fork. (The clone never talks to GitHub directly.)
- Teardown becomes `rm -rf <clone>` — no `git worktree remove` / `git worktree prune` / `git branch -D` dance. `prune_aborted_experiments` simplifies accordingly.
- Disk economy preserved via `--reference`: the base's object store is reused, only working-tree files + per-clone refs are duplicated. Per-target `target/` cache survives unchanged (it lives in the clone's cwd, same as before).

**Effect**: `.git/modules/...` sandbox-deny issue goes away entirely — every `.git/` write is inside the cwd, which `workspace-write` allows by default. No `git_common_dir` `--add-dir` plumbing needed. The branch-cleanup sweep simplifies from "remove worktree, then drop branch, in that order" to "rm -rf the clone."

**(b) Move bench execution back to the coordinator.**

- Strip Step 0 local baseline + per-attempt targeted-replay bench from `optimizer.md`. The agent's keep/discard decision becomes fmt + lint + nextest only.
- Coordinator runs the bench: pre-codex (against the clone's binary at `base_branch` tip = within-session local baseline), then per-kept-attempt (against the clone's binary at the agent's commit SHA). Same apples-to-apples property as the v1 inner-loop bench, but the bench runs in the trusted coordinator process with normal host permissions.
- Coordinator feeds the improvement_pct back to the agent via a marker file the agent re-reads at attempt N+1 — OR the coordinator drives the loop itself (codex emits a candidate commit, coordinator decides keep/discard based on its own bench). The latter is cleaner; either is a Layer 1B v2 design call.
- All `/Volumes/Extern` + shadow-dir + stacks-bench DB writes now happen outside codex. `--add-dir` for shadow-dir-root + git-common-dir can be removed.

**Effect**: the operator's `/Volumes/Extern` setup works as-is; no sandbox bypass needed. The agent's prompt shrinks. The coordinator's `bench_experiments.rs` gets the inner-loop work moved into it.

**(c) Keep — these survive the refactor unchanged**: HEAD-advance gate, `LAST_KEPT_SHA` discipline, marker-file gating (`implementation.md` vs `abort.md`), `--retries 2` on nextest, env-var git identity (`GIT_AUTHOR_*` + `GIT_CONFIG_COUNT` overrides), `--shadow-dir-root` plumbing (still needed at the coordinator), the demotion-on-no-commit logic. All architecture-orthogonal — they apply equally well to clones-with-coordinator-bench as to worktrees-with-inner-loop-bench.

#### Deferred / removed

- `git_common_dir(base)` helper + its `--add-dir` entry — no longer needed once `.git/` is inside cwd.
- `git branch -D` step in `prune_aborted_experiments` — clones are `rm -rf`'d wholesale.
- (Pass-b only) Step 0 baseline + per-attempt bench shell snippets in `optimizer.md`.
- (Pass-b only) `bench_shadow_dir_root` field on `OptimizerPrompt` (still on `Settings` / `Layout` for coordinator-side use).

#### Ordering

- **Pass (a) ships first.** It fixes the `.git/modules/...` sandbox issue on its own; with `(a)` shipped, the inner-loop bench still works as-is for any operator whose source dir isn't on an external drive (the test-drive can re-run on a `/Users/...`-rooted source as a smoke test of the clone refactor).
- **Pass (b) ships next.** Re-architects the inner loop around coordinator-owned bench.
- **Layer 2A (event log)** still gates on Layer 1B being end-to-end demonstrated. That demonstration now requires `(a)` + at minimum a successful test-drive on a non-Extern source, OR `(b)` to run against the current setup.

### 2026-05-12 (Layer 1B v1.3 — env-var git identity + HEAD-advance demotion gate)

Two follow-up rounds on v1.2's signing/identity story uncovered the actual `git commit` failure mode and produced the correctness gate that caught a false-positive "landed":

- **`apply_worktree_identity` was not actually worktree-local** (Codex review). `git -C <worktree> config <key> <value>` for a linked worktree writes to the SHARED repo config in the common-dir, not a per-worktree file — so my "worktree-local override" was silently mutating the operator's `repos/stacks-core/` checkout for every subsequent operation. Replaced with `optimizer_git_env(settings)` returning `GIT_AUTHOR_*` / `GIT_COMMITTER_*` / `GIT_CONFIG_COUNT` env entries plumbed through new `InvokeInputs::extra_env`; codex inherits + propagates them to every git invocation in its process tree. Zero git-config-file mutation. Two unit tests for the env shape (configured identity + defaults).
- **Coordinator-side HEAD-advance gate** ([optimizers.rs `verify_kept_or_demote`](crates/stacks-bench-agent/src/session/optimizers.rs)): captures the worktree's initial HEAD at creation, and when `implementation.md` is present at exit, demands HEAD has advanced past that SHA. Otherwise the agent claimed kept-attempts but never committed — silent sandbox/signing failure. Demotes to `abort.md` (preserving the agent's writeup as `implementation.md.demoted` for diagnosis) so Phase 3 + Phase 5 correctly skip the target. Three real-git tests covering unchanged-HEAD, advanced-HEAD, and no-marker paths.
- **HEAD-advance gate proved its worth on the v1.3 test-drive**: 1 target, fmt + 2 clippy aliases + full nextest (10,490 passed) all green, but `git commit` was sandbox-blocked → demoted correctly. The substrate diagnosed the actual blocker (the `.git/modules/...` write deny + `/Volumes/Extern` write deny) cleanly and saved the agent's diff as `attempt-1.patch`. Those two sandbox issues are what triggers v2's architectural pivot above.

98 tests pass · `just lint` clean · `sbagent check` + `prompt lint` OK.

### 2026-05-12 (Layer 1B v1.2 — test-drive findings + sandbox plumbing)

End-to-end Layer 1B test-drive against `sqlite-side-store-batched-replace` (single target, hand-staged `verification_replay` recipe, `repetitions=10`/`warmup=10`) surfaced three real framework gaps + one infrastructure bug. All five fixes shipped together:

1. **Path-resolution bug at the `Layout` boundary** (uncovered by relative `base` / `sessions_root` in the operator config). `git -C <base> worktree add <relative-wt>` resolved the worktree under `repos/stacks-core/` (because `-C` cd's into base first), but codex `--cd <relative-wt>` resolved against sbagent's CWD (the operator root) — two different absolute paths for the same logical worktree. Codex crashed with `No such file or directory` before emitting any event. **Fix**: new `absolutize` helper in [layout.rs:218](crates/stacks-bench-agent/src/layout.rs#L218) called on every relative path at Layout construction time (framework, sessions_root, stacks_bench_data_dir, lock_dir, base). Single boundary, every consumer agrees.

2. **`stacks-bench` shadow tempdir blocked by codex sandbox**. stacks-bench creates a reflink shadow beside the source dir; for sources on `/Volumes/Extern`, the codex `workspace-write` sandbox refused writes. **Fix (multi-repo)**: upstream stacks-bench landed `--shadow-dir-root <DIR>` (`b2ea69397c`); sbagent now plumbs `Settings::stacks_bench_shadow_dir` → `Layout::stacks_bench_shadow_dir` → `BenchRange::shadow_dir_root` (Phase 0/3) AND `OptimizerPrompt::bench_shadow_dir_root` (inner loop). When set, the optimizer adds the dir to codex `--add-dir` so writes succeed. Schema constraint: must be on the same filesystem as source (reflinks). Two new snapshot tests lock the emission.

3. **Submodule git-dir outside the sandbox**. The agent's `git reset --hard` inside its worktree tried to write to `<operator>/.git/modules/repos/stacks-core/...`; sandbox blocked it. Agent worked around with `apply_patch`, but the workaround isn't reliable. **Fix**: `git_common_dir()` shells `git rev-parse --git-common-dir` against `base`; result added to codex `--add-dir` for every optimizer invocation. Works for both regular checkouts and submodules.

4. **Flake-induced revert** of a working change (`stacks-signer chainstate::tests::v1::check_tenure_extend_unsupported_cause`, SQLite `database is locked` — passed on v2 of the same test). **Fix**: `cargo nextest run --retries 2` (3 total per test) added to the optimizer prompt's test step. A test that fails 3× in a row is genuinely broken; flakes get suppressed.

5. **Mandatory revert step dropped** — the worktree+branch is the deliverable boundary, the marker file is the publish gate. **Fix**: optimizer prompt rewritten to use `kept` / `discarded` per-attempt semantics: discard = log it, don't commit. The agent MAY `git reset --hard HEAD~1` between attempts for a clean diff, but it's no longer mandatory. Exit gating still binary on `implementation.md` vs `abort.md`.

6. **Session-end branch cleanup** ([optimizers.rs:`prune_aborted_branches`](crates/stacks-bench-agent/src/session/optimizers.rs)). After Phase 5 publish, `sbagent session run` walks `experiments/*/`, drops `agent/<session>/<target>` for every dir without `implementation.md` (abort.md or crash). Kept branches survive — publish owns them.

Verification: `just build` clean · `just test` 92/92 pass (4 new) · `just lint` clean · `sbagent check` OK against the operator config. End-to-end re-run of the test-drive recipe is the next step.

**Test-drive observations not turned into fixes** (worth noting for context):

- Cold-cache LTO build of stacks-bench inside the worktree is ~10 min; nextest workspace pass is ~15 min. Warm cache cuts the bench-build to seconds. With `verification_replay` (single tx, 10+10 reps), per-attempt wall time is dominated by nextest, not bench.
- The agent correctly identified the flaky nextest failure as unrelated and chose to abort rather than re-try — pre-`--retries 2` behavior. Worth re-testing with retries enabled to confirm the suppression works end-to-end.

### 2026-05-12 (Layer 1B v1.1 — Codex review fixes)

Codex review of v1 raised five findings; addressed:

1. **Parallel-agents=1 is now hard-enforced** at `optimizers::run` for any session with normal_pr targets (clamp + `tracing::warn!`). The roadmap claim no longer relies on operator discipline.
2. **Two-step bench-then-show pattern** in the prompt: `bench run --json` returns only `run_id`; `total_duration_us` is fetched via `bench show --run-id <id>`. Eliminates the silent "agent has no real number" failure.
3. **Source/network/warmup/filter plumbed** through `OptimizerPrompt` (new fields `bench_source_dir`, `bench_network`, `bench_warmup`, `bench_filter`). The prompt no longer has a TODO marker for these.
4. **`--optimizer-attempts` and `--optimizer-budget-minutes` CLI flags** added to both `sbagent session run` and `sbagent session optimize run`. CLI overrides Settings.
5. **`attempt_started` event emitted FIRST** in each attempt, before any work. Provides a crash breadcrumb if codex dies mid-attempt (nextest hang, OOM, outer timeout).

Verification: `just test` 79/79 pass · `just lint` clean · `sbagent check` OK · `sbagent prompt lint` against a fresh seed dir OK.

Per-attempt hard timeout (Codex finding 5b) remains v2 work — codex doesn't expose nested timeouts; outer `codex_exec_timeout_sec` is the hard kill. Documented in v1 deferred items.

**Follow-up (Codex review of v1.1):** empty `bench_network` rendered as `--network ""` in the prompt. Fixed by (a) defaulting to `"mainnet"` at the optimizer callsite (matches Phase 0/3) and (b) wrapping `--network` in a Jinja `{% if bench_network %}...{% endif %}` conditional in the prompt. Same conditional applied to the `--warmup`/`--filter` value listings in the Bench-environment section. Two new unit tests (`optimizer_prompt_omits_network_flag_when_empty` / `..._emits_network_flag_when_set`) lock the conditional behavior so it can't regress silently — the synthetic lint context wouldn't otherwise exercise the empty-string path. 81/81 tests pass.

### 2026-05-12 (Layer 1B v1 — prompt + plumbing)

- **`templates/optimizer.md` rewritten** as an autoresearch-style inner loop:
  - Step 0 establishes a per-target local baseline by benching the unmodified worktree (so per-attempt comparisons are apples-to-apples within the session — Phase 0's global baseline was built from a different commit/cache state).
  - Steps 1..N (where N = `optimizer_attempts`): hypothesize → implement → fmt-stacks → clippy-stacks/stackslib (normal_pr only) → nextest → commit → targeted-replay bench → keep (HEAD stays) or revert (`git reset --hard HEAD~1`). Per-attempt event appended to `attempts.jsonl`.
  - "NEVER STOP until budget exhausted" framing, lifted from autoresearch's `program.md`.
  - Delivery-mode split: `normal_pr` uses bench-based keep/revert; `consensus_poc_pr` uses scoped-tests-pass as the keep criterion (no bench — meaningless under consensus change); `consensus_issue` aborts immediately (was already coordinator-skipped at Phase 2 entry).
  - Budget = `optimizer_attempts` × wall clock `optimizer_budget_minutes`, whichever exhausts first. `codex_exec_timeout_sec` remains the hard kill.
- **Settings:** `optimizer_attempts: Option<u32>` (default 5), `optimizer_budget_minutes: Option<u32>` (default 60).
- **`OptimizerPrompt`:** new fields `stacks_bench_data_dir`, `optimizer_attempts`, `optimizer_budget_minutes`; wired in `session/optimizers.rs`.
- **`sbagent prompt lint`** clean against the new template. 79/79 tests pass.

**Deferred to Layer 1B v2 (or absorbed by Layer 2):**

- The agent reads `chainstate path` / `network` from the coordinator's environment today; the prompt has a TODO marker. Plumb both through OptimizerPrompt explicitly so the agent doesn't need to guess.
- Typed `attempts.jsonl` parser + integration with the event log (Layer 2). For v1 the file is agent-written freeform JSONL with the documented schema; coordinator doesn't parse it back.
- Per-target bench-lock granularity to allow `--parallel-agents > 1` with inner-loop benches. Today's v1 requires `--parallel-agents 1` because parallel optimizers would collide on the shared stacks-bench DB.
- Local baseline propagation to Phase 4 finalize. Today Phase 4 still compares against the global baseline; the local one lives only in `attempts.jsonl` for audit.

**Effect:** Per-target quality should jump materially — the agent gets N attempts (default 5) at finding a working hypothesis instead of one shot. Wall time roughly unchanged per session because the targeted-replay bench (from 1A) keeps each attempt cheap (~30s-2min instead of ~30min for full-range).

**Caveats:**

- Operator-tuned `optimizer.md` overrides in `.sbagent/prompts/` from before today will NOT pick up the inner-loop semantics (don't-replace-if-exists). Operators on the new shape should `sbagent prompt sync --force` to refresh, then re-apply their tunings.

### 2026-05-12 (Layer 1A shipped)

- **Layer 1A landed**. Bundled with the unstable-id cleanup audit findings, so ended up ~1 day instead of ½:
  - New `VerificationReplay` model (`models/common.rs`) — `txids?`/`blocks?` as hex-hash string arrays + `repetitions` + `rationale`. Schema `HEX_HASH_PATTERN` = `^0x[0-9a-fA-F]{64}$` enforced via `#[schemars(inner(regex(...)))]` on both fields. Wired onto both `AnalyzerTarget` (per-analysis) and `MergedTarget` (carried through merge); analyzer can emit, merger picks/unions across contributors.
  - `RepresentativeIds` retyped: `stacks_tx_ids: Vec<i64>` → `stacks_tx_hashes: Vec<String>`; `synthetic_block_ids: Vec<i64>` → `stacks_block_hashes: Vec<String>` (rename + retype since they're no longer DB-local synthetic ids). Schema pattern enforced.
  - `VerificationReplay::validate()` called from `AnalyzerTarget::validate` and `MergedTarget::validate` — guards against the "recipe is present but both `txids` and `blocks` are empty/null" footgun that would otherwise silently fall back to full-range bench.
  - Drilldown queries rewritten: `profiler_trace_tx.sql` takes `:stacks_tx_hash`, resolves via `stacks_tx` dim-join to the indexed FK. `profiler_trace_block.sql` takes `:stacks_block_hash`, resolves via `synthetic_block`+`stacks_block` join. `top_blocks_for_span.sql` + `txs_for_contract.sql` doc comments updated to direct hash-based downstream usage.
  - Triage / analyzer / merge prompts updated for hash-only emission discipline; analyzer prompt has a new "Targeted-replay recipe" section spec'ing the three operating modes (txids-only / blocks-only / both / omit) + per-mode repetition guidance.
  - `bench_experiments.rs` refactored around `BenchPhase`: full-range mode keeps 2 invocations for variance; targeted mode produces 1-2 phases (one per non-empty recipe section, since `--txid` and `--block` conflict on the CLI). Phase suffix lands in the bench `--name` for operator-side disambiguation.
  - New snapshot test (`bench_experiments_uses_verification_replay_when_present`) locks the targeted-replay CLI shape: `--repetitions` + repeated `--txid`/`--block` flags, no `--start-at`, name-suffix discipline.
  - JSON schemas regenerated; `sbagent check` passes drift check.
- **Effect**: Phase 3 wall-time drops from ~30 min/run to a few minutes/run for most targets. Substrate for Layer 1B (optimizer inner loop) — the cheap signal exists now.
- **Caveats / follow-ups noted, not blockers**:
  - The merge phase's union-of-`txids`/`blocks` + max-`repetitions` rules are prompt-only (see [`templates/merge-analyses.md`](https://github.com/cylewitruk/stacks-bench-agent/blob/main/crates/stacks-bench-agent/templates/merge-analyses.md)). `validate_merge_output` enforces coverage / bucket / cross-consensus invariants but does NOT enforce that the merger preserved contributor recipes. Acceptable for an LLM-owned consolidation pass; if drift becomes an issue, add a per-target check that the merged `verification_replay` is a superset of contributors'.
- Next: **Layer 1B** (optimizer inner-loop mode), or per ad-hoc operator priority.

### 2026-05-12 (design decision)

- **Layer 1A design refined before implementation**: `verification_replay.blocks` is **string[] of 0x-prefixed index block hashes**, not the previously sketched `(i64 | string)[]` union. Rationale: integers in this field are ambiguous between stacks height / burn height / synthetic bench id; hashes are globally unique, self-describing, and survive bench DB rebuilds. CLI side (stacks-bench `--block`) still accepts both; we just always pass hashes from artifacts.

### 2026-05-12 (latest)

- **Submodule migration**: `repos/stacks-core` removed from tool repo; added to this operator repo at `feat/stacks-bench` pinned to `6ec953ee94`. Tool's `Settings::base` relaxed from "required at startup" to "required at use" (`Option<PathBuf>` + `Layout::require_base()` helper); ~25 callsites updated. Unblocks `sbagent prompt lint` / `sync` for users who haven't set up a stacks-core checkout yet.
- **Disk-first prompts shipped**: Askama replaced with **MiniJinja** (runtime template engine, Jinja2-compatible syntax, strict-undefined mode, `keep_trailing_newline=true`). Bundled templates seeded into operator's `layout.prompt_overrides_dir` on every `sbagent` startup with `O_CREAT|O_EXCL` (don't-replace-if-exists). Operator's edited templates survive seeding; force-sync available via explicit subcommand.
- **`sbagent prompt {lint,sync}` subcommands added**: lint parses + dry-renders every disk template against a field-complete synthetic context (replaces Askama's compile-time drift check); sync force-rewrites all templates from bundled defaults (requires `--force`). Verified end-to-end on 2026-05-12.
- **Reference docs (`non-targets.md`, `bucket-anchors.md`) consolidated** into the operator's prompts dir alongside renderable templates; agent prompts reference them via `prompts_dir/<name>` instead of `<framework>/prompts/<name>`. Kills the asymmetry between tunable-via-overrides templates and ad-hoc-tunable reference docs.
- 78/78 tests pass. lint clean.
- Next: **Layer 1A** (`verification_replay` schema field + analyzer prompt update + `bench_experiments.rs` branch). Substrate + infrastructure now in place to start.

### 2026-05-12 (earlier)

- **Split decision taken**: yes, option (1) — now. Operator repo created at [`cylewitruk/stacks-bench-agentic-operator`](https://github.com/cylewitruk/stacks-bench-agentic-operator). Naming kept symmetric (`stacks-bench-agent` ↔ `stacks-bench-agentic-operator`). `feature-requests-stacks-bench.md` stays here; `autonomous-roadmap.md` moves to operator repo (this copy becomes a stub).
- Operator-repo skeleton bootstrapped (README, `.sbagent/example.config.toml`, empty `events/`/`sessions/`/`reports/` with `.gitkeep`, `.github/workflows/` placeholder).

### 2026-05-12 (initial)

- New section added: **Open architectural decision — tool vs operator split**. Industry pattern (Renovate / Dependabot / Atlantis / Flux / autoresearch itself) consistently separates tool from operator; current single-repo proposal is fine for a prototype but not the shape for long-lived autonomy.
- Documented: layer allocation if the split is taken (Layer 0 + Layer 1 stay in `stacks-bench-agent`; Layer 2 + Layer 3 + this roadmap doc move to a new operator repo).
- Recommended decision timing: option (1) now, or option (2) after Layer 1 lands. Must be settled before Layer 2A begins so the event log starts in its long-term home.
- Follow-up: user to decide split (yes/no), timing, operator-repo name, and ownership of `feature-requests-stacks-bench.md`.

### 2026-05-11

- Roadmap document created. No layer items implemented yet; Layer 0 already complete via prior work (stacks-bench targeted-replay shipped upstream as `6ec953ee`, submodule bumped + smoke-tested in this session).
- Decision recorded: per-target branches will carry a single `.sbagent/optimizations/<target-id>.md` audit file (not the full artifact tree). Rationale: diff pollution.
- Decision recorded: event log JSONL is the source of truth; SQLite is a disposable cache rebuilt by replay. Rationale: git-mergeable, audit-friendly, format-stable.
