# v12: Cross-Session Dedup Filter

Successor to [v11: Autonomy Safety + Scheduled Maintain](v11-autonomy-safety-and-maintain-schedule.md).
v10 gave sbagent a `maintain.jsonl` lifecycle ledger; v11 made scheduled
maintain safe. v12 consumes that history so new sessions do not optimize the
same fix signature while a prior PR is still open, already merged, or repeatedly
unsuccessful.

> **Status:** shipped — code-complete, reviewed, validated, and archived.
>
> v12 implements deterministic cross-session dedup at merge time. It does
> not add a unified event log; it uses `sessions.jsonl` plus `maintain.jsonl`
> and records skips as stable `rejected_by_merge` entries.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0031-triage-merge-dedup-filter` | primary | shipped |

## Why

The smoke session proved the full manual loop can publish, archive, and record
maintenance events. Without dedup, a later session can still rediscover and
re-optimize the same `fix_signature`, creating duplicate PRs or spending bench
time on already-failed work. That is the next autonomy blocker: scheduled
sessions need memory of prior outcomes before they fan out optimizer work.

The useful substrate now exists:

- `sessions.jsonl` records archived targets, statuses, PR / issue URLs,
  `family_id`, and canonical target ids.
- `maintain.jsonl` records post-publish lifecycle observations for PRs and
  issues, including open, merged, closed, stale, force-pushed, and
  branch-deleted events.
- `optimization-targets.json` already has `rejected_by_merge`, and the merge
  validator already requires every analyzer target to appear exactly once
  across `targets[].merged_from` and `rejected_by_merge`.

v12 should therefore be small and deterministic: build a read-only projection,
compute blocked analyzer targets before merge, and ensure blocked targets never
reach optimizer fan-out.

## Scope

In scope:

- New dedup projection over `sessions.jsonl` + `maintain.jsonl`, keyed by
  exact `fix_signature`. For archived rows, `TargetRecord.id` is the canonical
  fix signature; v12 treats it as the same key as analyzer
  `target.fix_signature` without a schema migration.
- New `[autonomy].dedup_failure_threshold` setting, default `3`.
- Deterministic pre-merge dedup decisions for analyzer targets:
  - block when a matching PR is open and not known stale;
  - block when a matching PR merged;
  - block when unsuccessful attempts for the signature meet or exceed
    `dedup_failure_threshold`.
- Merge coordinator removes blocked analyzer targets from the LLM merge input,
  then appends deterministic `rejected_by_merge` rows with a stable
  `dedup:` reason before validation and write.
- Merge prompt gets a concise dedup summary for `final-message.md`, but the LLM
  does not decide which targets are deduped.
- Validator checks that all precomputed dedup decisions appear exactly once in
  `rejected_by_merge` and that no non-dedup target uses a `dedup:` reason.
- Operator docs explain the exact-match behavior and the stale/open policy.
- Dedup applies to every delivery mode. Normal PRs, consensus PoC PRs, and
  consensus issues all carry the same target id / family id history, and
  duplicate consensus work is still wasted work.

Out of scope:

- Unified event log / SQLite projection (`0030-event-log-skeleton`).
- Fuzzy or semantic similarity dedup. v12 is exact `fix_signature` only.
- Optimizer memory for previously attempted patches (`0028`).
- PR mutation, auto-close, or auto-comment behavior.
- Changing `sessions.jsonl` or `maintain.jsonl` schemas.
- Changing `optimization-targets.json` schema. Dedup uses existing
  `rejected_by_merge` coverage.

## Policy

The dedup projection classifies a prior signature as blocking when:

- **Open PR:** a target has a `pr_url`, and maintain's latest state for that PR
  is open without a later terminal event or `PrStale`.
- **Merged PR:** maintain records `PrMerged` for the target's PR.
- **Repeated unsuccessful attempts:** count reaches
  `[autonomy].dedup_failure_threshold`. Count:
  - archived targets with status `Rejected`, `Failed`, or `Aborted`;
  - archived targets whose PR later records `PrClosedUnmerged`.

Dedup rejection reasons use a closed prefix set:

- `dedup:open-pr`
- `dedup:open-issue`
- `dedup:merged`
- `dedup:repeated-failure`

Non-blocking history:

- Stale open PRs are not hard-blocked. They should appear in the dedup context
  as prior stale attempts, but they do not prevent a new target if analysis
  still finds the opportunity compelling.
- Closed-unmerged PRs count toward the unsuccessful-attempt threshold but do
  not block by themselves until the threshold is reached.
- Branch-deleted and force-pushed events do not directly block. They refine the
  PR lifecycle projection already maintained by v10.

Repeated-failure counts are lifetime counts in v12. That is intentionally
simple and conservative; a future recency window or manual reset mechanism can
land if operators find that old failed signatures should become eligible after
large upstream changes.

Maintain events are projected by `observed_at`, not by file order. The ledger is
append-only in normal operation, but the projection should remain correct if a
manual repair or multi-run edge case leaves events slightly out of order.

## Phases

### Phase 1: Dedup Projection

**Goal:** Build the deterministic read-only projection that answers "does this
`fix_signature` already have blocking history?"

**Scope:**

- Add `session/dedup.rs` or equivalent module with:
  - `DedupProjection::from_ledgers(sessions, maintain, settings)`;
  - `DedupDecision { family_id, target_index, fix_signature, reason }`;
  - blocking-state helpers for open, merged, and repeated unsuccessful states.
- Reuse existing lossy ledger readers; malformed skipped lines remain warnings
  at CLI/session orchestration boundaries, not silent hard failures inside the
  projection.
- Add `[autonomy].dedup_failure_threshold = 3` to settings and
  `assets/example.config.toml`.
- Treat analyzer `target.fix_signature` as authoritative. Older archived
  target rows use `TargetRecord.id`; in `sessions.jsonl` that field is the
  canonical fix signature for archived sessions.
- Project PR and issue artifacts for all delivery modes, including
  `normal_pr`, `consensus_poc_pr`, and `consensus_issue`.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] A matching open PR with no terminal or stale event returns a blocking
      decision.
- [x] A matching merged PR returns a blocking decision.
- [x] A matching stale open PR does not block, but is retained as prior context.
- [x] Closed-unmerged attempts count toward the unsuccessful threshold.
- [x] Rejected / failed / aborted archived targets count toward the unsuccessful
      threshold.
- [x] Below-threshold unsuccessful history does not block.
- [x] Projection uses exact `fix_signature`; same family with a different
      signature is not blocked.
- [x] Projection sorts maintain events by `observed_at` before deriving latest
      PR / issue state.
- [x] Consensus PoC PR and consensus issue records participate in dedup using
      the same exact-signature key.

**Tests:**

- In-module fixture tests for `DedupProjection` using synthetic
  `SessionRecord` + `MaintEvent` rows.
- A regression fixture for stale open PRs proving stale does not hard-block.

### Phase 2: Merge Input Contract

**Goal:** The merge phase receives only optimizer-eligible targets while still
showing the operator what was deduped and why.

**Scope:**

- Before rendering the merge prompt, compute dedup decisions for every accepted
  analyzer target.
- Remove blocked targets from `accepted_analyses_json` passed to the LLM merge
  prompt.
- Provide a compact `dedup_rejections_json` / markdown summary to the prompt so
  `final-message.md` can report deterministic skips.
- Update `templates/merge-analyses.md`:
  - explain that deterministic dedup skips are precomputed by sbagent;
  - tell the LLM not to invent, drop, or reinterpret `dedup:` rejections;
  - keep ordinary merge rejections limited to shipped / forbidden-scope /
    optimizer-rule reasons.
- Do not change the `optimization-targets.json` schema. The coordinator appends
  dedup rows to existing `rejected_by_merge`.
- `optimization-targets.json` is the authoritative dedup record.
  `final-message.md` is an operator-readable summary and should be tested for
  useful coverage, not treated as a second source of truth.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] A deduped target is absent from the LLM merge input.
- [x] The final written `optimization-targets.json` contains a
      `rejected_by_merge` row with one of the closed reasons:
      `dedup:open-pr`, `dedup:open-issue`, `dedup:merged`, or
      `dedup:repeated-failure`.
- [x] Non-dedup analyzer targets still reach the merge prompt unchanged.
- [x] `final-message.md` reports dedup skip count and reason categories.
- [x] Schema version remains unchanged.

**Tests:**

- Merge fixture test with one blocked analyzer target and one allowed target.
- Prompt contract test pinning the deterministic-dedup language.

### Phase 3: Validator + Optimizer Boundary

**Goal:** Dedup cannot be bypassed by prompt output or a future refactor, and
the existing optimizer boundary stays intact: optimizer fan-out reads
`targets[]`, never `rejected_by_merge`.

**Scope:**

- Extend merge validation with the precomputed dedup decision set:
  - every precomputed decision appears exactly once in `rejected_by_merge`;
  - no target without a precomputed decision may use a `dedup:` reason;
  - every `dedup:` reason is one of the closed categories named above;
  - existing coverage invariant still applies after coordinator-added rows.
- Add a regression test around the existing optimizer fan-out boundary so
  rejected rows cannot accidentally become optimizer inputs later.
- Add diagnostics that name the target pair `(family_id, target_index)` and
  `fix_signature` when the invariant fails.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Validator rejects a missing dedup rejection row.
- [x] Validator rejects an invented `dedup:` rejection row.
- [x] Validator rejects an unknown `dedup:` reason category.
- [x] Validator rejects a deduped target that also appears in `targets[]`.
- [x] Optimizer receives only non-deduped targets in a fixture end-to-end chain.

**Tests:**

- In-module validator tests in `session/merge.rs`.
- Orchestrator-chain fixture proving optimizer target count excludes deduped
  rows.

### Phase 4: Docs + Operator Visibility

**Goal:** Operators can understand why a target was skipped without reading
raw JSON.

**Scope:**

- Update workflow / architecture docs to describe exact-signature dedup at
  merge time.
- Update configuration docs for `[autonomy].dedup_failure_threshold`.
- Ensure `sbagent history show` does not need a new section in v12; dedup is a
  per-session merge decision already visible through archived
  `optimization-targets.json` and `merge/final-message.md`.
- Add a concise note to `assets/autonomous-roadmap.md` that 2B is implemented
  via v10/v11 ledgers rather than the deferred 2A unified event-log plan.

**Status:**

- [x] Core implementation
- [x] Unit/integration tests
- [x] Reviewed
- [x] Validated

**Acceptance & Validation:**

- [x] Docs state that exact `fix_signature` matching is the only v12 dedup
      mechanism.
- [x] Docs state stale open PRs are not hard-blocked.
- [x] Docs state `dedup:` rows live in `rejected_by_merge`.
- [x] Docs state unsuccessful-attempt counts are lifetime counts in v12.
- [x] `assets/example.config.toml` includes the threshold with a short comment.

**Tests:**

- Existing example-config deserialization test covers the new setting.
- Markdown lint / `just lint` covers doc formatting.

## Final Validation

- [x] `just lint --no-sccache`
- [x] `just test --summary --no-sccache`
- [x] Fixture session with a prior open PR skips the matching analyzer target
      and still optimizes unrelated targets.
- [x] Fixture session with a stale prior PR does not hard-block the matching
      analyzer target.
- [x] No schema mirror changes other than config/docs-driven outputs expected
      by the implementation.

## Follow-Ups

- `0028-optimizer-memory` — shipped in v13, using history to help optimizers
  avoid previously-failed patch shapes.
- `0030-event-log-skeleton` — reconsider when another consumer needs a unified
  event stream. v12 deliberately avoids introducing it just for dedup.
- `0050-local-session-cron` — shipped in v14 as systemd templates for local
  scheduled sessions on a dedicated benchmark host.
- Potential future fuzzy dedup item: compare semantically similar proposed
  changes when exact `fix_signature` proves too narrow.
