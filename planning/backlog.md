# Backlog

Items here are recoverable but not currently assigned to an iteration.

## Candidate Items

<a id="0025-named-phases"></a>

### Named Phases Over Numbered Phases

- **id:** `0025-named-phases`
- **status:** `backlog`
- **priority:** `low`

**Problem:** Phase labels like `1.8` and `3.5` are precise but hard for new
operators and agents to reason about.

**Scope:** Rename phase references in docs, CLI help, artifacts where useful,
and prompt prose to descriptive names.

**Acceptance:** Current workflow can be understood without knowing phase
numbering history.

**Deferred / non-goals:** Do not change artifact paths unless paired with a
separate migration.

<a id="0027-maintain-ledger"></a>

### Maintain Ledger

- **id:** `0027-maintain-ledger`
- **status:** `backlog`
- **priority:** `low`

**Problem:** Future maintenance observations need an append-only home that does
not mutate write-once session branches.

**Scope:** Add a `maintain.jsonl` sibling to `sessions.jsonl`.

**Acceptance:** Maintenance records can reference a session id without touching
that session's archive branch.

<a id="0028-optimizer-memory"></a>

### Cross-Session Optimizer Memory

- **id:** `0028-optimizer-memory`
- **status:** `backlog`
- **priority:** `low`
- **design:** [design/0028-optimizer-memory.md](design/0028-optimizer-memory.md)

**Problem:** Optimizers start cold and can rediscover fixes or dead ends from
prior sessions.

**Scope:** Surface remembered prior attempts/rejections to triage, analyzer, or
optimizer agents once enough session history exists.

**Acceptance:** A new session can avoid at least one previously rejected or
already-attempted target through durable memory.

## Autonomous Closed-Loop Items

Imported from [assets/autonomous-roadmap.md](../assets/autonomous-roadmap.md).
These are not near-term until live manual sessions prove the core loop.

<a id="0030-event-log-skeleton"></a>

### Event Log Skeleton

- **id:** `0030-event-log-skeleton`
- **status:** `backlog`
- **priority:** `low`
- **design:** [design/0030-event-log-skeleton.md](design/0030-event-log-skeleton.md)

**Problem:** Closed-loop autonomy needs durable event history across sessions.

**Scope:** Add append-only event JSONL plus a disposable SQLite projection.

**Acceptance:** `sbagent history show` can render replayed session/target state.

<a id="0031-triage-merge-dedup-filter"></a>

### Triage / Merge Dedup Filter

- **id:** `0031-triage-merge-dedup-filter`
- **status:** `backlog`
- **priority:** `low`
- **design:** [design/0031-triage-merge-dedup-filter.md](design/0031-triage-merge-dedup-filter.md)

**Problem:** New sessions can re-propose targets already tried, open, merged, or
repeatedly rejected.

**Scope:** Use event-history projection to skip duplicate fix signatures.

**Acceptance:** Merge emits skip events for deduped targets and does not send
them to optimizer.

<a id="0032-per-session-commit-push"></a>

### Per-Session Commit And Push

- **id:** `0032-per-session-commit-push`
- **status:** `backlog`
- **priority:** `low`
- **design:** [design/0032-per-session-commit-push.md](design/0032-per-session-commit-push.md)

**Problem:** Manual sessions need a durable operator-git audit step.

**Scope:** Commit summary artifacts and event logs at session end.

**Acceptance:** A completed session creates a readable operator commit without
committing raw scratch artifacts.

<a id="0033-maintain-command"></a>

### Maintain Command

- **id:** `0033-maintain-command`
- **status:** `backlog`
- **priority:** `low`
- **design:** [design/0033-maintain-command.md](design/0033-maintain-command.md)

**Problem:** Autonomous operation needs PR lifecycle reconciliation.

**Scope:** Add `sbagent maintain` to observe GitHub state and emit maintenance
events.

**Acceptance:** Merged, closed, stale, and failed-open PR states are reflected in
the event log.

<a id="0034-github-actions-wiring"></a>

### GitHub Actions Wiring

- **id:** `0034-github-actions-wiring`
- **status:** `backlog`
- **priority:** `low`
- **design:** [design/0034-github-actions-wiring.md](design/0034-github-actions-wiring.md)

**Problem:** Closed-loop operation needs scheduled session and maintenance runs.

**Scope:** Add cron workflows with concurrency guards and bot identity.

**Acceptance:** Scheduled jobs run without racing each other or recursively
triggering from bot commits.

<a id="0035-autonomy-hygiene"></a>

### Autonomy Hygiene

- **id:** `0035-autonomy-hygiene`
- **status:** `backlog`
- **priority:** `low`
- **design:** [design/0035-autonomy-hygiene.md](design/0035-autonomy-hygiene.md)

**Problem:** Scheduled autonomy needs pause, rate limits, circuit breakers, and
event-version safeguards.

**Scope:** Add fail-closed safety controls before scheduled cron is enabled.

**Acceptance:** Operator-free runs cannot exceed configured PR, cadence, or
failure thresholds.

<a id="0037-triage-anchor-benchmarks"></a>

### Triage Anchor Benchmarks

- **id:** `0037-triage-anchor-benchmarks`
- **status:** `backlog`
- **priority:** `low`
- **design:** [design/0037-triage-anchor-benchmarks.md](design/0037-triage-anchor-benchmarks.md)

**Problem:** If inner-loop signal remains noisy, triage may need to anchor
baseline measurements for every promoted representative.

**Scope:** Let triage run and persist anchor benchmark IDs for downstream reuse.

**Acceptance:** Analyzer, optimizer, and verification can compare against the
same anchor recipe and cache regime.

<a id="0043-history-report"></a>

### History Report (Markdown)

- **id:** `0043-history-report`
- **status:** `backlog`
- **priority:** `low`
- **source:** v6 Phase 5 (deferred); see
  [v6 archive](archive/completed/v6-observability-surface.md#phase-5-stretch-sbagent-history-report).

**Problem:** Operators have `sbagent history list` + `show` (v6) but no
single document they can commit / share / paste into a weekly digest.

**Scope:** Add `sbagent history report [--since <ref>] [--out <path>]`.
Default `--since` is "the most recent ISO week with archived sessions";
default `--out` is stdout. Markdown sections:

- **Summary**: session count, target outcome rollup, total wall-clock
  across the period.
- **Per-session table**: same columns as `history list`, rendered as a
  markdown table.
- **PRs opened**: bulleted list of `pr_url`s grouped by session.
- **Issues opened**: bulleted list of `issue_url`s.

**Promotion trigger:** Best picked up AFTER
[`0033-maintain-command`](#0033-maintain-command) lands. `0033` adds
GitHub-side reconciliation (open / merged / closed / stale PR state) to
the event log. Without it, the report can only render "PR opened" —
which `history list` already does. With it, the report gains the
merged-vs-open dimension that makes a weekly digest meaningfully
richer than the current per-session views.

**Acceptance:** Default invocation produces a markdown document with
the four sections above against a fixture ledger.
`--out reports/<iso-week>.md` writes to disk; stdout stays empty.

<a id="0045-ephemeral-codex-runtime-state"></a>

### Ephemeral Codex Runtime State

- **id:** `0045-ephemeral-codex-runtime-state`
- **status:** `backlog`
- **priority:** `medium`

**Problem:** Nested, Codex-driven smoke reruns can fail when sbagent-launched
Codex subprocesses try to write runtime state under `~/.codex/`, which is
outside the sandbox grant. Granting `~/.codex/` is the wrong fix: that directory
also contains auth/config material and must remain inaccessible to phase agents.

**Scope:** Investigate and implement a way for sbagent-launched Codex
subprocesses to keep writable runtime state under a session-scoped scratch path,
for example `<session>/scratch/codex-state/`, without exposing `~/.codex/` or
Codex auth material to agent-executed shell commands. Confirm whether the Codex
CLI can split mutable runtime state from auth/config; if it cannot, document the
safe fallback for nested supervised smoke runs.

**Acceptance:**

- Inner Codex invocations do not attempt to write `~/.codex/state_*.sqlite`.
- Phase-agent commands cannot read `~/.codex/auth.json`.
- Phase-agent commands cannot print Codex auth secrets from inherited env.
- Nested Codex-driven reruns either work without escalation or fail with a clear
  diagnostic that preserves the secret boundary.

**Deferred / non-goals:** Do not add `~/.codex/` to `codex.extra_writable_roots`
or per-phase sandbox grants. Do not move auth tokens into session scratch unless
the token is proven inaccessible to agent tools and logs.

<a id="0047-analyzer-estimate-calibration"></a>

### Analyzer Estimate Calibration

- **id:** `0047-analyzer-estimate-calibration`
- **status:** `backlog`
- **priority:** `low`
- **source:** v8 planning / smoke session `20260611-172955`.

**Problem:** Analyzer expected-signal magnitude estimates can be badly off even
when the optimized result is a clean, direction-matching win. In the smoke
session, `rollback-wrapper-at-block-read-cache` expected roughly `6%` and
measured `+27.22%`.

**Scope:** Calibrate analyzer guidance for estimating expected improvement
magnitude once there is enough session history to compare estimates against
verification results. Keep this separate from results-analyzer verdict policy.

**Acceptance:** Analyzer output gives more realistic magnitude ranges without
making results-analyzer confidence depend on forecast accuracy.

## Scheduled — see iteration docs

No active iteration currently owns backlog items.
