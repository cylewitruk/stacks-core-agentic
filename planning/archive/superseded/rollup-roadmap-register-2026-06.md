# Roadmap

- **rollup:** `roadmap-register-2026-06`
- **status:** `superseded`
- **archive kind:** historical options register
- **current register:** [planning/iterations/](../../iterations/),
  [planning/backlog.md](../../backlog.md)

> **Historical options register — superseded by the planning system.**
>
> Current selected work lives in [`planning/iterations/`](../../iterations/);
> unscheduled items live in [`planning/backlog.md`](../../backlog.md).
> This file preserves the pre-planning-system register and shipped-item notes.

Options register for future work — design ideas surfaced in planning sessions
and deferred for scope or sequencing reasons. Not a roadmap in the
project-management sense; closer to "future passes we've named but not yet
scoped."

Each item lists **what** it is, **why** it's deferred, and **what triggers
picking it up**.

## Post-bench results-analyzer agent — folded into Pass 1c

Previously framed as a deferred "Pass 4." The independent-review session that
surfaced the methodology gap also surfaced that `improvement_pct` aggregation
without an agent in the loop is fundamentally limited — once Pass 1c lets
the analyzer emit multiple bench invocations per target (each with its own
`expected_signal`), mechanical aggregation across invocations would
re-introduce the same blindness the schema change is fixing. The
results-analyzer is the load-bearing consumer of the new per-invocation
data; without it, the breakdown has no clean way to feed the summary table
/ PR body / SessionRecord.

Full design now lives in [§Pass 1c — analyzer-defined invocations + post-bench
results-analyzer](#pass-1c--analyzer-defined-invocations--post-bench-results-analyzer).

---

## Other items considered but deferred

These have been mentioned in planning conversations or doc-review passes but not
been worked into the design docs. Captured here so they don't get lost.

### Named phases over numbered phases

Replace "Phase 1.8 / 1.9 / 2 / 3" with descriptive names everywhere (CLI
subcommands, artifact paths, prompts, docs). Codex flagged this in a review; we
agreed the direction is right. Deferred because it's a larger cross-cutting
refactor and the current numbered scheme is workable. Picking it up makes sense
once the phase set stabilizes (i.e. after Pass 1c lands Phase 3.5 and Pass 2
lands Phase 1.9).

### Phase-timing instrumentation

`SessionRecord.phase_durations_secs` is empty today by v1 design (see
[docs/session-archive.md](docs/session-archive.md)). Populating it requires
phase-level start/stop hooks in the orchestrator and a durable per-phase log.
Useful for auditing where pipeline time actually goes; not blocking on anything.
Pick up when phase timings become operationally interesting.

### Per-target audit fields in archive ledger (partial)

`SessionRecord.targets[].head_sha` shipped 2026-05-21 (see Completed ledger).
`targets[].pr_url` and bench wallclock totals (`baseline_total_us`,
`candidate_total_us`) are still `None` / `0` by v1 design — they require
publish-feedback integration that doesn't exist yet. Pick up alongside
Pass 3's PR body templating, since both touch the publish ↔ ledger seam.

### `sbagent sync --commit / --push` flags

Convenience flags for the operator-update workflow (when sbagent binary changes,
what does an operator need to do to bring their operator dir in sync, and how
much of that can be one-shot?). Discussed in passing during the publish wiring
work; not blocking.

### `maintain.jsonl` ledger

Post-completion observations from a future `sbagent maintain` (Layer 3B per the
session-branch-archive design memory) need a home that doesn't violate the
write-once contract on `session/<id>` branches. The shape is "sibling to
`sessions.jsonl` on main, append-only, references the session id." Deferred
until Layer 3B is actually built.

### Cross-session optimizer memory

Today each session starts cold — the optimizer doesn't know what fixes have been
tried before, what's been rejected, what's been deferred. The
`memory/analyzed-rejections.jsonl` artifact exists on operator disk but isn't
yet tracked in git or surfaced to agents. Useful for preventing the bot from
re-proposing the same fixes in subsequent sessions. Pick up after the bot has
run enough sessions for "what we've tried before" to be worth remembering.

### Pre-flight invariants & operator-sync drift detection (partial)

v1 + two follow-up slices shipped (see Completed ledger below). v2 scope
remaining:

- **Branch-ref divergence detection** — local `feat/stacks-bench` ref vs
  detached-HEAD position in the submodule. Caught us once when a HEAD bump
  didn't update the branch ref and per-target clones picked up the stale
  one. Lower priority because the per-session ephemeral source clone
  redesign eliminates this drift class entirely.
- **Network-fetch variant of submodule reachability** — today's check is
  local-only; doesn't catch "operator forgot to fetch + bump." Pair with
  the network-permission story (PAT, proxy, etc).

Trigger to pick up: after the next pilot session — incrementally, one
drift mode per slice.

### Pass 1c — analyzer-defined invocations + post-bench results-analyzer

> **Status (2026-06-05): Pass 1c complete.** Schema rewrite,
> per-invocation paths, conditional-required gate, ID-set parity /
> canonicalization, operator `analyzer.max_invocations_per_target`
> cap, the Phase 3.5 results-analyzer agent + finalize cutover, the
> Phase 5 publish gate on verdict content (`rejected` is skipped),
> and the `results_analysis.confidence_floor` operator knob are all
> landed. `Experiment.improvement_pct` + `Experiment.status` are
> sourced verbatim from each target's
> `analyze/<target>/results-analysis.json`; verdicts whose confidence
> falls below the operator's floor (default `medium`) hold for
> operator review rather than auto-PR. Pass 1c is closed.

Today the optimizer-target schema's `verification_replay` carries one bench
shape (`{txids, blocks, repetitions, warmup, rationale}`). Phase 1.8
calibration and Phase 3 candidate bench both run that single shape
regardless of what the target is optimizing. Independent review of session
`20260521-051649` flagged this as the principal methodology concern: a
73.30% reported win on a node-cache target is internally consistent but
can't be separated into "real cache hits during measurement" vs "warmup
smearing the cache effect across repetitions." The same blind-spot exists
for any optimization whose benefit is workload-shape-dependent (batched
writes, lazy-init paths, first-touch elision).

**Direction**: the analyzer is best positioned to decide HOW the target
should be measured — it's the agent that read the profiler data, identified
the hotspot, and reasoned about which workload shape exercises the
mechanism. Pass 1c lets the analyzer emit one or more concrete stacks-bench
invocations per target (samples + warmup + repetitions + expected signal),
and adds a **post-bench results-analyzer agent** that synthesizes the
per-invocation measurements into a structured verdict downstream phases
consume.

This folds in the previously-deferred "Post-bench results-analyzer agent"
(formerly proposed as Pass 4) — without the synthesizer, the per-invocation
breakdown has no clean consumer for the summary table / PR body /
SessionRecord.

#### Schema rewrite

Replace `verification_replay`'s flat shape with:

```json
{
  "rationale": "MARF cache target: first-touch shouldn't change; warm reads should land the win.",
  "invocations": [
    {
      "label": "cold first-touch",
      "purpose": "isolate cold-path cost; cache benefit should be minimal here",
      "samples": { "kind": "txids", "txids": ["0xabc..."] },
      "warmup": 0,
      "repetitions": 5,
      "profiler": "rich",
      "expected_signal": {
        "axis": "tx_latency",
        "direction": "neutral",
        "tolerance_pct": 3
      }
    },
    {
      "label": "warmed steady-state",
      "purpose": "steady-state cache-hit rate after warmup populates working set",
      "samples": { "kind": "txids", "txids": ["0xdef...", "0x123...", "0x456..."] },
      "warmup": 10,
      "repetitions": 5,
      "profiler": "rich",
      "expected_signal": {
        "axis": "tx_latency",
        "direction": "improves",
        "estimate_pct": 4.5,
        "tolerance_pct": 3
      }
    },
    {
      "label": "block-context cross-check",
      "purpose": "corroborate the win shows up in block-context replay",
      "samples": { "kind": "blocks", "blocks": ["0xaaa...", "0xbbb..."] },
      "warmup": 5,
      "repetitions": 5,
      "profiler": "rich",
      "expected_signal": {
        "axis": "tx_latency",
        "direction": "improves",
        "tolerance_pct": 5
      }
    }
  ],
  "suspected_spans": ["MARF.read_node_hash", "MARF.get_block_at_height"]
}
```

Key shape choices, with rationale:

- **Each invocation owns its samples + run-params + hypothesis.** Stacks-bench
  treats txids and blocks as mutually-exclusive CLI inputs anyway, so any
  target that wants to exercise both shapes already needs >1 invocation. The
  array generalizes: each entry is one self-contained `stacks-bench bench
  run` call. Cache-regime variation (cold/warm) is a special case where two
  invocations share samples but differ in `(warmup, repetitions)` — it
  falls out of the schema rather than being baked into it.
- **`samples` is a tagged enum.** Variants today: `txids`, `blocks`,
  `block_range { start_at, count }`. `#[serde(tag = "kind")]` for sharp
  validation errors and human-readable JSON. Future variants (span-filtered
  replay, etc.) drop in cleanly when stacks-bench grows the support.
- **`profiler` is a per-invocation string, not a flag list.** v1 only
  accepts `"rich"`; lean opt-in later. Baseline and candidate runs for the
  same `label` MUST use the same `profiler` value — that's the
  [flag-symmetry](#flag-symmetry-between-baseline-and-candidate-benches--shipped-2026-05-21)
  contract carried forward per-invocation. Putting it on the invocation
  (vs. inferring from CLI flags) means the schema enforces the
  symmetry without phase-level branching.
- **`expected_signal` per invocation.** Direction (`improves | neutral |
  regresses`) is load-bearing for the results-analyzer; magnitude
  (`estimate_pct` + `tolerance_pct`) is advisory. Splitting them lets the
  analyzer commit to qualitative claims it's confident about ("warm should
  improve") without forcing it to invent magnitudes it isn't.
- **`suspected_spans` is optional and cross-invocation.** Hints for the
  results-analyzer about which profiler spans to focus on when reading
  per-invocation data. Not a stacks-bench flag — just an analyzer-to-analyzer
  channel.
- **`invocations` is required, `minItems: 1`.** Every accepted `normal_pr`
  target says how it wants to be measured. A single invocation is fine but
  must be explicit. Operator cap `analyzer.max_invocations_per_target`
  defaults to 3; fail-before-bench on exceed (preserves analyzer-contract
  honesty rather than silent truncation). Sits under `[analyzer]` because
  it's a cap on the analyzer's emitted output, parallel to the existing
  `analyzer.concurrency_cap`.

#### Phase 1.8 + Phase 3 retooling

Phase 1.8 iterates `invocations[]`, runs one `stacks-bench bench run` per
entry against the strict archived binary, writes
`verify/<target>/baseline-<label-slug>-run-K/bench-run.json`. Phase 3
mirrors the same `invocations[]` on the candidate side with identical
`(samples, warmup, repetitions, profiler)` per label — that's the
[flag-symmetry](#flag-symmetry-between-baseline-and-candidate-benches--shipped-2026-05-21)
contract, applied per-invocation.

`baseline_run_ids.json` moves from `{txid_run_ids, block_run_ids}` to
`[{label, run_id}]` (preserves analyzer-chosen order; one entry per
invocation).

#### Post-bench results-analyzer agent (new phase 3.5)

Per-target fanout, runs after Phase 3 candidate bench, before Phase 4
finalize. Inputs:

- `optimization-targets.json` entry for the target (carries the
  invocations[] hypothesis).
- `optimizer-report.json` (the optimization agent's claims + the diff it
  produced — already includes `parity` / `implementation_summary` /
  `pr_title`).
- Per-invocation baseline + candidate `bench-run.json` (with rich profile
  data the agent reads to reason about WHY a measurement came out a
  certain way).
- Bench DB (read-only access via `sqlite3 -readonly` in v1; MCP wrapper in
  Pass 3).
- Full repo context (to read the diff and the hotspot code).

Output: `analyze/<target>/results-analysis.json`

```json
{
  "schema_version": 1,
  "target_id": "marf-historical-read-node-cache",
  "verdict": "accepted",
  "confidence": "high",
  "headline_improvement_pct": 4.7,
  "headline_rationale": "Warm steady-state invocation matched the analyzer's hypothesis (direction + within tolerance); cold first-touch neutral confirms the mechanism is a measurement-phase cache hit, not warmup smearing.",
  "per_invocation": [
    {
      "label": "cold first-touch",
      "baseline_run_id": 142,
      "candidate_run_id": 143,
      "measured_pct": 0.8,
      "matches_expected_signal": true,
      "observations": ["within neutral tolerance"]
    },
    {
      "label": "warmed steady-state",
      "baseline_run_id": 144,
      "candidate_run_id": 145,
      "measured_pct": 4.7,
      "matches_expected_signal": true,
      "observations": ["within ±3% estimate band; warmup-vs-measurement separation clean"]
    }
  ],
  "caveats": ["block-context cross-check showed +2.1% with high variance; consider rerun if borderline"],
  "pr_body_summary": "MARF historical read benefits from a per-block node cache. Warm steady-state improves +4.7% (analyzer estimated 4.5±3%); cold first-touch unchanged at +0.8% confirms the gain comes from cache hits, not warmup smear.",
  "db_queries": [
    { "purpose": "verify-block-contracts-replay", "query_digest": "sha256:...", "rows_returned": 12, "output_path": "analyze/<target>/queries/replay-block-contracts.csv" }
  ]
}
```

The `verdict | confidence` lattice replaces today's "finalize compares
means" mechanical path. Landed behavior:

- **`verdict: accepted | mixed`** — `Experiment.status = Accepted`;
  `pr_body_summary` is the canonical Result-section prose (Phase 5
  pastes verbatim). For `mixed`, `Experiment.reason` carries the
  caveats so the summary table flags them.
- **`verdict: rejected`** — `Experiment.status = Rejected` with
  `reason = headline_rationale`. Phase 5 skips PR-writer before it
  runs.
- **Per-invocation breakdown** — surfaces in `summary.md` via
  `render::render_verdict_block` (per-invocation table linked to each
  side's `bench-run.json`). No new `Experiment` fields; the verdict
  file itself is the authoritative store.

Operator threshold via the `[results_analysis]` stanza:

```toml
[results_analysis]
confidence_floor = "medium"   # high | medium | low — default medium
```

Verdicts whose `confidence` falls below the floor are skipped by
`decide_publish` with an explicit `confidence=<x> below floor=<y>;
hold for operator review` reason. The stanza will hold future tuning
knobs (DB query caps, prompt-version pinning).

#### Summary / PR-body sourcing

Phase 4 finalize stops mechanically computing `improvement_pct`. Instead:

- `Experiment.improvement_pct` ← `results-analysis.json:headline_improvement_pct`.
- `Experiment.status` ← derived from `results-analysis.json:verdict`.
- `Experiment.reason` ← carries the `headline_rationale` (Rejected) or
  joined `caveats[]` (Mixed); Accepted leaves it unset.
- `summary.md` rendering includes a per-target verdict block with the
  per-invocation table + caveats list, loaded via the same canonical
  loader Phase 5 publish uses.
- Phase 5 PR-writer reads `pr_body_summary` verbatim into the PR
  body's Result section; below-floor or rejected verdicts skip
  PR-writer entirely.

#### Backwards compat

Clean break. Pre-Pass-1c sessions are wiring-smoke-tests only (Pass 1a
shipped at the wiring level but explicitly NOT quantitatively validated for
PR-grade numbers); no migration code needed. Test fixtures + the operator's
bench DB get wiped or stamped as "pre-1c" at the cutover.

#### Open design questions (resolved in implementation)

- **Per-invocation pairing in finalize.** Resolved: by `invocation_id`,
  with a canonicalization pass that reorders both run-id files to
  `verification_replay.invocations[]` order before any math or
  rendering. The pairing key is the stable id (validator-enforced
  format); index ordering is the canonical sequence downstream
  consumers see.
- **Failure modes when the results-analyzer can't reach a verdict.**
  Resolved: missing / invalid / wrong-context verdict file →
  `Experiment.status = Aborted` with `reason` naming the missing file
  (`results-analyzer did not produce a verdict — …`). The rest of the
  session ships unaffected; one bad agent run doesn't block the
  pipeline.
- **DB query budget.** Deferred. The results-analyzer is currently
  prompt-only-bounded (the template caps language around DB usage,
  not a runtime gate). A hard cap on query count + total rows per
  target is a Pass 2 follow-up.

#### Estimated scope

| Item | Est |
| ---- | --- |
| Schema rewrite (`VerificationReplay` → invocations + `expected_signal` + samples enum) | ~3-4 h |
| Phase 1.8 + Phase 3 retooling (iterate invocations[], per-label artifacts, flag symmetry) | ~4-6 h |
| Results-analyzer agent (prompt + output schema + harness wiring + tests) | ~12-15 h |
| Prompt iteration against real-session data | ~1-2 weeks calendar |
| Summary / SessionRecord / PR-body migration | ~3-4 h |
| Clean-break of pre-1c data + test fixtures | ~2 h |
| **Total** | **~25-30 h + prompt-iteration calendar** |

Worth shipping as one Pass because the pieces are tightly coupled (schema
↔ invocations ↔ results-analyzer ↔ summary sourcing). Alternative
split: ship schema + plumbing first (~7-10 h, lands as a wiring smoke
test), then results-analyzer + downstream sourcing as a follow-up
(~17-20 h). Smaller reviews vs. one big slice — call this Pass 1c-α
followed by Pass 1c-β if the bundle feels too large to land in one go.

#### Sequence after Pass 1c

| Pass | What lands |
| ---- | ---- |
| **1c** (this) | Schema + Phase 1.8/3 retooling + results-analyzer + summary/ledger sourcing |
| **2** | Pre-bench verifier (Phase 1.9) + coordinator + full-range fallback + budget gate |
| **1b** | `baseline_rerun_id → Option<i64>` + lazy empirical noise floor (co-located with Pass 2's full-range path) |
| **3** | MCP wrapper for DB access, PR-body templating polish, methodology docs |

Pass 1c is the load-bearing pass for "promote Pass 1a from caveated to
clean-shipped." Pass 2 (pre-bench verifier) becomes a compute-saving
optimization once Pass 1c's post-bench analyzer is reliable — Pass 2 catches
bad-fit targets earlier; Pass 1c catches them honestly.

**Trigger to pick up**: now. Pass 1a's structural validation is complete;
Pass 1c is what gates external PR-quality numbers.

### Per-session ephemeral source clone (replace operator submodule)

Today `repos/stacks-core` is a git submodule pinned on the operator's `main`
branch, set up by `sbagent init` and bumped manually between sessions. This
shared-submodule model is the root cause of 3 of the 5 drift modes catalogued
above (SHA staleness, branch-ref divergence after detached-HEAD bump, plus the
cross-session interference where bumping for one session affects whatever else
is running). It also makes session provenance implicit: knowing a session id
doesn't tell you which source SHA was used unless you correlate it against
operator git history.

**Proposed redesign**: drop the `repos/<base>` submodule from the operator
repo. Each session-run materializes its own source tree under
`<agent_workspace_root>/sessions/<id>/repos/<base>/`, cloned from a shared
per-operator bare object cache at
`<agent_workspace_root>/cache/<base>.git/`. Source provenance lives in
per-session artifacts (`source.json` + the equivalent fields in `summary.json`
/ `SessionRecord`), not in `.gitmodules`.

Sketch:

```text
<operator_dir>/                          # operator repo (committable)
  sessions/<id>/results/...              # as today
  sessions/<id>/source.json              # NEW: { url, branch, sha, fetched_at }
  # no repos/<base> submodule on main

<agent_workspace_root>/
  cache/
    <base>.git/                          # NEW: shared bare cache, fetched at
                                          # session-start
  sessions/<id>/
    repos/<base>/                        # NEW: session source tree, cloned
                                          # from cache (--reference --local)
    optimizers/<target>/                 # per-target clones, --reference the
                                          # session source tree
```

**Drift modes eliminated:**

- **Submodule SHA staleness** — each session fetches the configured
  `<base_repo_url>` and pins to the resolved tip of `<publish_base_branch>` at
  session start. No "did the operator remember to bump?" question.
- **Detached-HEAD vs local branch ref** — the session clone is brand new each
  time; no operator-edited local branch refs to drift from origin.
- **Cross-session interference** — two sessions can run against different
  source SHAs in parallel without one stomping on the other.

**Trade-offs:**

- **Disk** — naively each session would re-download ~1-2 GB. The per-operator
  bare cache + `git clone --reference --local` keeps this to ~tens of MB per
  session (just the working tree, not the object store). Acceptable.
- **`sbagent init` becomes simpler** — no more `git submodule add`,
  `.gitmodules` management, or `--seed-from` (which exists today only because
  fresh bot forks don't have the substrate branch). The bare cache's first
  fetch handles the bootstrap.
- **Provenance becomes explicit** — `summary.json` gains `source: { url,
  branch, sha, fetched_at }`. The PR-writer references the session SHA
  directly. Archived sessions become reproducible from session id alone.
- **Migration** — existing operator dirs have a `repos/<base>` submodule
  committed to main. Migration path: `sbagent migrate` removes the submodule
  and records the previous pin into each archived session's `source.json`.
- **Auth surface unchanged** — same PAT-via-env mechanism for the cache's
  fetch as for today's submodule update. `validate_auth_url` still gates.

**Open questions:**

- Should `source.json` live in operator-repo `sessions/<id>/` (committable,
  archivable on `session/<id>` branch) or in `<agent_workspace_root>` (not
  committable but freer of operator-repo concerns)? Probably operator-repo
  — provenance belongs on the durable artifact, not the scratch volume.
- How does this interact with `sbagent session baseline import`? Today import
  reuses a prior session's `bench-list.json` and `baseline_run_id`. Under the
  redesign, the imported session has its own `source.json` — the importing
  session should be required to use a matching SHA (or explicitly accept a
  mismatch with an audit-trail note).
- Does the agent_workspace_root cache become a sync point for multi-operator
  deployments (shared object store, sessions per operator)? Probably out of
  scope for v1; pin to single-operator-per-cache.

**Trigger to pick up**: after the preflight-hardening above lands and we have
one or two more sessions' worth of operator-experience data. The redesign is
larger than the preflight checks but solves the underlying problem more
durably. Sequence: preflight first (cheap, catches the symptoms today),
ephemeral-source second (eliminates the root causes).

### Per-target workspace cleanup phases

Two related hygiene gaps in `agent_workspace_root`:

- **Per-target clone retention** — within one session, per-target clones
  (~14 GB each) accumulate because `recreate_checkout` cleanup only fires at
  session-end, not between targets. With parallel-clamp=1 and N targets, peak
  footprint is N × 14 GB. The first session that hit a full disk crashed
  mid-Phase-2 with a confusing git-clone-failed error.
- **Old-session workspace aging** — workspaces under
  `<agent_workspace_root>/optimizers/<session_id>/` persist indefinitely after
  the session ends. Pre-Pass-1a test sessions had ~25 GB of forgotten cargo
  build artifacts sitting on the operator's machine.

Fixes:

1. Drop the prior target's clone (or just `cargo clean` its `target/` dir)
   before cloning the next target in the optimizer fan-out. Cuts peak
   footprint from N × 14 GB to ~14 GB + N × source-tree-size.
2. On `sbagent session run` start, prune
   `<agent_workspace_root>/optimizers/<id>/` for any `<id>` whose
   corresponding session is archived or older than a configurable TTL
   (default ~14 days).
3. Pre-flight disk check: refuse to start `session run` if
   `agent_workspace_root` has < (target_count × 20 GB) free; surface the
   prune command in the error.

Trigger to pick up: paired with the pre-flight hardening above — same
operator-experience surface.

---

## Completed

Brief ledger of items that started in this register and have shipped. Newest
first. Entries here record motivation + what landed; design discussion has
moved into the code + the commit history.

### DB ↔ artifact run-id consistency check — shipped 2026-05-21

- **Motivation**: when the bench DB gets wiped or `stacks_bench_data_dir`
  is misconfigured, every run-id reference under `sessions/<id>/` becomes
  dangling. Finalize's improvement_pct math, the archive ledger, and the
  Phase 5 PR-writer all silently emit poisoned audit data. This hit us
  mid-session — recovery required hand-written re-bench scripting.
- **Shipped**: [`session/db_consistency.rs`](crates/stacks-bench-agent/src/session/db_consistency.rs)
  with `collect_dangling_run_ids(layout, targets, bench)`. Hooked into
  `sbagent session finalize run` (warns before baking refs into
  `summary.json`) and `sbagent session archive` (warns before the
  write-once `session/<id>` branch + ledger append). Advisory only —
  some dangling refs are expected by design (session-level baseline
  when every normal_pr target has per-target ids); callers decide how
  to react. Closes one of the preflight v2 sub-items.

### `head_sha` propagation into SessionRecord archive ledger — shipped 2026-05-21

- **Motivation**: Pass 1c coordinator-provenance sidecar puts `head_sha`
  on each `summary.json.experiments[]` row, but the archive ledger
  hard-coded `targets[].head_sha = None` and lost it. Reviewers had no
  archived provenance for what code was actually benched.
- **Shipped**: [`session/archive.rs`](crates/stacks-bench-agent/src/session/archive.rs)
  now reads each Experiment's `head_sha` from `summary.json` and writes
  it to the matching `SessionRecord.targets[]` row. Regression test
  pins the flow ([`tests/archive.rs::archive_propagates_head_sha_into_target_record`](crates/stacks-bench-agent/tests/archive.rs)).
  Closes the `head_sha` half of "Per-target audit fields in archive
  ledger"; `pr_url` + bench wallclock totals still wait on
  publish-feedback wiring.

### `sync` refreshes prompts by default — shipped 2026-05-21

- **Motivation**: the conservative "don't clobber operator edits without
  `--force-tunables`" default allowed `optimizer.md` to drift past the
  orchestrator's typed-report contract — caused a 6.5h session where
  4 implemented experiments were marked aborted because the stale
  prompt told agents to write the wrong artifact. Preflight v1 catches
  the symptom; this slice removes the cause.
- **Shipped**: [`cli/sync.rs`](crates/stacks-bench-agent/src/cli/sync.rs)
  default flipped — `sbagent sync` now refreshes ALL bundles (schemas,
  queries, prompts, context). New `--keep-tunables` flag opts out for
  operators who consciously maintain edits. The legacy
  `--force-tunables` / `--force-prompts` flags accepted as deprecated
  no-op aliases for one release with a deprecation notice. Existing
  test suite updated: presence-asserts flipped to absence-asserts,
  three tests renamed to match the new semantics. Closes the most
  load-bearing preflight v2 sub-item.

### Session-start preflight (v1) — shipped 2026-05-21

- **Motivation**: drift modes between operator state and orchestrator
  assumptions had repeatedly wasted hours mid-session — stale prompts
  breaking the typed-report contract, submodule SHA staleness, installed-
  binary lag.
- **Shipped**: [`session/preflight.rs`](crates/stacks-bench-agent/src/session/preflight.rs)
  with three checks (installed-binary mtime, load-bearing prompt drift on
  `optimizer.md`, submodule HEAD reachable from local
  `origin/<publish_base_branch>`). Wired into `session run`, `session
  optimize run`, and `sbagent check`. `--skip-preflight` opt-out on the
  session subcommands. `Fail` severities abort before any heavy phase
  touches disk.
- **Deferred follow-ups**: 4 additional checks tracked in the v2 entry
  above (branch-ref divergence, DB↔artifact consistency, network-fetch
  submodule check, `sync` prompt-refresh default flip).

### Flag symmetry between baseline and candidate benches — shipped 2026-05-21

- **Motivation**: Phase 1.8 baseline ran with rich profiler flags; Phase 3
  candidate ran lean. Asymmetric profiler overhead biases `improvement_pct`
  on profile-heavy workloads — independent review of session
  `20260521-051649` flagged this as the principal magnitude-inflator on
  the 73% reported cache win.
- **Shipped**: dropped `--bench-spans-only` and `--no-profiler-kv` from
  candidate-bench argv in [`session/bench_experiments.rs`](crates/stacks-bench-agent/src/session/bench_experiments.rs).
  v1 default is "rich on both sides". Existing tests flipped from
  presence-asserts to absence-asserts; invariant carries forward into the
  future per-profile flag set ("same flags within a profile" is what's
  load-bearing).

### Coordinator-provenance sidecar (base + head SHA) — shipped 2026-05-21

- **Motivation**: session `20260521-051649` had its consensus_poc target
  rebased onto a different base SHA than the other 4 targets; resume
  gate + finalize didn't notice because the schema recorded no SHAs.
  Apples-to-apples invariant silently broken.
- **Shipped**: [`models/coordinator_provenance.rs`](crates/stacks-bench-agent/src/models/coordinator_provenance.rs)
  plus sidecar [`optimize/<target>/coordinator-provenance.json`](crates/stacks-bench-agent/src/session/optimizers.rs)
  written by `coordinator_commit_if_kept` post-commit. Resume gate
  verifies sidecar's `base_sha` matches the session's archived
  `baseline/bin/manifest.json.source_sha` AND that `session_id` /
  `target_id` / `delivery_mode` context cross-matches. Finalize
  propagates `base_sha` + `head_sha` into each [`Experiment`](crates/stacks-bench-agent/src/models/summary.rs)
  row of `summary.json`. Schema added to `BUNDLED_SCHEMAS` +
  `schema_export::generate_all`; on-disk + bundled `coordinator-provenance.schema.json` regenerated.

---

## How this file is used

Items here are deliberately under-specified. The point is to capture an idea
well enough that it's recoverable later, not to pre-design it. When an item gets
picked up, it gets promoted to its own numbered design doc and removed from
this register.

Items can be removed (rejected / decided against) too. When that happens, note
the decision rationale in a "decided against" section rather than deleting
silently — others' future thinking benefits from knowing what was considered and
why it didn't fit.
