# v13: Cross-Session Optimizer Memory

Successor to [v12: Cross-Session Dedup Filter](../archive/completed/v12-cross-session-dedup-filter.md).
v12 prevents exact duplicate signatures from reaching optimizer fan-out. v13
uses the same durable history as context so future agents can learn from prior
attempts before they propose, merge, or implement another patch shape.

> **Status:** planned
>
> v13 is intentionally advisory. Deterministic blocking remains owned by v12's
> dedup filter. This iteration surfaces concise memory to agents so they can
> avoid stale patch shapes, explain when a repeat is justified, and reuse
> evidence from prior sessions without adding another hard gate.

## Items

| Item | Role | Status |
| ---- | ---- | ------ |
| `0028-optimizer-memory` | primary | planned |

## Why

The first full smoke proved sbagent can produce, publish, archive, and maintain
real PR lifecycle state. v12 now uses that state to block exact duplicate fix
signatures when a prior artifact is open, merged, or repeatedly unsuccessful.
That is necessary but deliberately late in the flow: it rejects a target after
analysis has already spent time rediscovering it.

The next autonomy step is to make history visible earlier and more usefully:

- analyzers should know when a family already has shipped or failed patch
  shapes;
- merge should know which current targets are historically promising, stale, or
  risky before it chooses optimizer work;
- optimizers should avoid repeating known-bad implementation approaches and
  should prefer variants informed by successful prior patches.

v13 should not create a second decision engine. It should produce a small,
schema-checked memory view that agents can cite and act on, while v12 remains
the source of deterministic "do not optimize this exact signature" behavior.

## Scope

In scope:

- New read-only memory projection over `sessions.jsonl` + `maintain.jsonl`,
  keyed by `family_id` and exact `fix_signature`.
- Projection states for useful agent context:
  - open PR / issue;
  - merged PR;
  - closed-unmerged PR or closed issue;
  - stale PR;
  - repeated unsuccessful attempt count;
  - archived accepted / rejected / failed / aborted result;
  - most recent known PR / issue URL;
  - prior source SHA, using `SessionRecord.source_sha` when available.
- Compact per-session memory artifact for the current candidate/analysis/merge
  set. The artifact lives at `<session>/results/optimizer-memory.json`,
  alongside other session-spanning result artifacts. It should include only
  families/signatures relevant to the current session so prompt cost stays
  bounded.
- Prompt integration for analyzer, merge, and optimizer:
  - analyzer treats memory as context while estimating opportunity and risk;
  - merge uses memory to explain why a historically-risky target should or
    should not proceed;
  - optimizer uses memory to avoid repeating failed patch shapes and to borrow
    concrete implementation lessons from successful attempts.
- Tests proving the projection handles open, merged, stale, closed, and failed
  histories without changing v12's dedup semantics.
- Docs describing memory as advisory context, not a hard block.

Out of scope:

- Changing `sessions.jsonl`, `maintain.jsonl`, or `optimization-targets.json`
  schemas.
- Fuzzy semantic dedup or similarity scoring. v13 is still exact family /
  signature history.
- Unified event log / SQLite projection (`0030-event-log-skeleton`).
- Automatic PR comments, closes, or mutations.
- Changing v12's deterministic dedup policy.
- Analyzer estimate calibration (`0047`).
- Weekly history reports (`0043`).
- Results-analyzer memory context. Verdicts should stay evidence-backed by the
  current session's measurement artifacts unless a future smoke shows
  cross-session verdict context is useful.
- A separate memory append log. Memory writes back only through normal
  `sessions.jsonl` archival and `maintain.jsonl` reconciliation.

## Memory Contract

The memory projection is a current-session advisory input:

- It may say "this family has a merged sibling signature" or "this exact
  signature failed twice."
- It may point at URLs, prior verdicts, caveats, and rejection reasons.
- It should include the prior session's source SHA when available so agents can
  distinguish recent failures from attempts against older code.
- It must not silently remove current targets. Removal remains the merge
  coordinator's v12 dedup responsibility.
- It must be compact enough for prompt inclusion. v13 defaults:
  - last 5 attempts per exact signature;
  - last 3 sibling signatures per family;
  - most recent lifecycle URL per artifact;
  - explicit omitted-row markers when a rendered prompt truncates memory.

Agents should cite memory when it affects their decision. Examples:

- "Prior exact signature was stale, but the code path changed; proceed with a
  different implementation approach."
- "Prior patch failed twice for the same reason; do not spend optimizer budget
  on the same shape."
- "Merged sibling improved commit-time work; this target should explore a
  different span in the same family."

## Phases

### Phase 1: Memory Projection

**Goal:** Build a deterministic read-only projection that summarizes prior
attempts by family and fix signature.

**Scope:**

- Add `session/optimizer_memory.rs` or equivalent module with:
  - `OptimizerMemory::from_ledgers(sessions, maintain)`;
  - per-family and per-signature summaries;
  - helper to select memory relevant to current analyzer targets.
- Reuse existing ledger readers and v10/v12 projection semantics where
  possible.
- Sort maintain events by `observed_at` before deriving lifecycle state.
- Preserve v12 semantics:
  - stale open PRs are historical context, not a hard block;
  - force-push can refresh stale context;
  - exact signatures remain distinct even inside one family.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Projection records open PR / issue context for a matching signature.
- [ ] Projection records merged PR context for a matching signature.
- [ ] Projection records stale PR context without marking it as a hard block.
- [ ] Projection records closed-unmerged and closed-issue attempts.
- [ ] Projection records archived rejected / failed / aborted attempts.
- [ ] Projection groups sibling signatures under the same `family_id`.
- [ ] Projection sorts maintain events by `observed_at`, not file order.
- [ ] Exact-signature rows do not bleed across different signatures in the same
      family.

**Tests:**

- In-module fixture tests using synthetic `SessionRecord` + `MaintEvent` rows.
- Regression fixture for stale / force-pushed lifecycle interactions.

### Phase 2: Compact Memory Artifact

**Goal:** Persist the current session's relevant memory in a small typed
artifact that prompts and future audits can inspect.

**Scope:**

- Add a typed model such as `OptimizerMemoryJson` with schema version 1.
- Write a session-scoped artifact under `results/`, for example
  `results/optimizer-memory.json`.
- Write it once at session start, after source materialization and before the
  first prompt-consuming phase. Later phases read the same file without
  refreshing it.
- Include only memory relevant to current candidate / analyzer / merged target
  families and signatures.
- Use bounded lists:
  - last 5 attempts per signature;
  - last 3 sibling signatures per family;
  - most recent lifecycle URL per artifact.
- Include prior source SHA on attempt rows when the archived session recorded
  it.
- Add schema export and bundled schema mirror.
- Missing memory remains valid for legacy sessions and early dry runs.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Artifact round-trips through the typed model.
- [ ] Artifact validates with `additionalProperties: false`.
- [ ] Artifact is deterministic for the same input ledgers.
- [ ] Empty history writes either an empty-but-valid artifact or omits the file
      with a documented reader fallback.
- [ ] The artifact stays compact in a fixture with many unrelated historical
      sessions.
- [ ] Artifact rows include `source_sha` when available and omit it cleanly for
      legacy rows.

**Tests:**

- Model round-trip tests.
- Schema parity / export tests following the existing schema workflow.
- Fixture test proving unrelated families are not included.

### Phase 3: Analyzer + Merge Prompt Integration

**Goal:** Analyzer and merge agents can reason from prior outcomes before the
optimizer spends work.

**Scope:**

- Pass relevant memory into analyzer prompts.
- Pass relevant memory into merge prompts alongside existing deterministic
  dedup context.
- Render memory with a token budget guard. Analyzer and merge prompt memory
  sections should stay compact and show an omitted-row marker when truncated.
- Update prompt templates:
  - memory is advisory evidence;
  - cite memory when it changes a decision;
  - do not treat memory as a hard rejection unless v12 dedup already did;
  - prefer fresh variants when prior attempts failed for the same reason.
- Add contract tests that pin the advisory-memory wording and guard against
  "memory is a gate" language.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Analyzer prompt receives memory for matching family/signature fixtures.
- [ ] Merge prompt receives memory for matching family/signature fixtures.
- [ ] Prompts explicitly say memory is advisory and v12 dedup owns hard blocks.
- [ ] Prompts ask agents to cite memory when it changes a recommendation.
- [ ] Analyzer and merge memory sections are bounded to roughly 2k tokens per
      family context and render an omitted-row marker when truncated.
- [ ] Existing prompt lint remains clean.

**Tests:**

- Prompt render tests with memory present and absent.
- Prompt substring tests for advisory-memory contract.

### Phase 4: Optimizer Prompt Integration

**Goal:** Optimizers can avoid known-bad patch shapes and reuse lessons from
successful prior attempts.

**Scope:**

- Pass target-relevant memory into optimizer prompts.
- Update optimizer guidance:
  - avoid repeating failed implementation shapes;
  - explain when a repeat is justified because upstream code changed or the
    proposed approach differs materially;
  - reuse concrete implementation hints from merged / accepted sibling
    signatures;
  - keep the final patch scoped to the current target.
- Optimizer memory is exact-signature and same-family context. v13 does not ask
  the optimizer to infer fuzzy similarity between unrelated signatures.
- The optimizer prompt may need light prose tightening so the memory section
  does not bloat an already-large prompt.
- Do not mutate optimizer fan-out. The optimizer still receives only
  `targets[]`; rejected/deduped rows remain outside its work queue.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Optimizer prompt receives memory relevant to the target.
- [ ] Optimizer prompt omits unrelated family history.
- [ ] Prompt instructs the agent not to repeat a known failed patch shape.
- [ ] Prompt allows a justified variant when the new approach is materially
      different.
- [ ] Prompt says v13 does not add fuzzy similarity matching.
- [ ] Optimizer fan-out target count is unchanged by memory context.

**Tests:**

- Optimizer prompt render tests with successful and failed prior attempts.
- Orchestrator fixture proving memory does not add or remove optimizer targets.

### Phase 5: Docs + Operator Surface

**Goal:** Operators understand what memory does, where it lives, and how it
differs from dedup.

**Scope:**

- Update workflow / architecture docs with the memory flow.
- Update configuration docs only if v13 adds tunables.
- Update `assets/autonomous-roadmap.md` with a brief archaeology note if the
  implementation materially changes the Layer 2 story.
- Add or update history/show docs only if the memory artifact gets surfaced in
  an existing command.

**Status:**

- [ ] Core implementation
- [ ] Unit/integration tests
- [ ] Reviewed
- [ ] Validated

**Acceptance & Validation:**

- [ ] Docs state memory is advisory context, not a hard gate.
- [ ] Docs state v12 dedup remains responsible for deterministic skips.
- [ ] Docs state where the memory artifact lives and when it is written.
- [ ] Docs explain that exact signatures remain distinct inside a family.

**Tests:**

- Markdown lint / `just lint`.
- Existing example-config test if config changes.

## Final Validation

- [ ] `just lint --no-sccache`
- [ ] `just test --summary --no-sccache`
- [ ] Fixture session with prior failed history surfaces memory to prompts but
      does not remove targets by itself.
- [ ] Fixture session with a prior merged sibling signature surfaces useful
      context to analyzer / merge / optimizer.
- [ ] Fixture session with no relevant history keeps prompts compact and
      produces no spurious memory warnings.

## Follow-Ups

- `0030-event-log-skeleton` — reconsider once memory plus dedup plus maintain
  make two-ledger projection awkward.
- `0047-analyzer-estimate-calibration` — still separate; memory may provide
  examples, but v13 does not recalibrate estimates.
- Potential future memory ranking item: if prompt context grows too large, add
  score-based pruning or a `memory_max_rows_per_family` setting.
