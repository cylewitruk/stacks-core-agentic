# Backlog

Items here are recoverable but not currently assigned to an iteration.

## Candidate Items

<a id="0019-prompt-hardening-live-smoke"></a>

### Prompt Hardening From Live Smoke

- **id:** `0019-prompt-hardening-live-smoke`
- **status:** `backlog`
- **priority:** `medium`

**Problem:** Prompt lint verifies rendering, but not judgment quality. The
results-analyzer prompt in particular needs live proof against real
`bench-run.json` shapes.

**Scope:** Patch only prompt text and prompt-facing examples exposed by the
smoke run.

**Acceptance:** The next smoke produces less operator correction, with no schema
or handoff regressions.

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

<a id="0029-sync-commit-push"></a>

### Sync Commit / Push Convenience

- **id:** `0029-sync-commit-push`
- **status:** `backlog`
- **priority:** `low`

**Problem:** Operator bundle sync and committing pushed operator changes are
still separate manual steps.

**Scope:** Add ergonomic `sbagent sync --commit/--push` behavior if it stays
useful after live sessions.

**Acceptance:** Operator bundle updates can be synced and shipped with one
explicit command.

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
  [iterations/v6-observability-surface.md](iterations/v6-observability-surface.md#phase-5-stretch-sbagent-history-report).

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

## Scheduled — see iteration docs

The following item IDs are owned by an active iteration; full specs
live there, not here.

- `0039-v3-transition-marker-scrub`,
  `0040-session-record-source-sha-cleanup`,
  `0041-migration-recipe-rehearsal`,
  `0042-source-seed-helper` —
  [v4: v3 Polish + Bot-Fork Seed](iterations/v4-v3-polish-and-bot-fork-seed.md).
- `0036-observability-surface` — shipped via
  [v6: Observability Surface](iterations/v6-observability-surface.md).
  Phase 5 (markdown report) deferred to
  [`0043-history-report`](#0043-history-report); pick up after `0033`.
  (Prologue closed `0021-preflight-v2` as superseded by v3's per-session
  source clone — see [archive/superseded/0021-preflight-v2.md](archive/superseded/0021-preflight-v2.md).)
- `0044-evidence-backed-verification` — shipped via
  [v7: Evidence-Backed Verification](iterations/v7-evidence-backed-verification.md).
  Also closed `0038-prompt-example-concretization` as a Phase 2 prompt-lint
  cleanup.
