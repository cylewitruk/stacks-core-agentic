# Roadmap

Options register for future work — design ideas surfaced in planning sessions
and deferred for scope or sequencing reasons. Not a roadmap in the
project-management sense; closer to "future passes we've named but not yet
scoped."

Each item lists **what** it is, **why** it's deferred, and **what triggers
picking it up**.

## Post-bench results-analyzer agent (proposed Pass 4)

**What**: Per-target fanout agent that runs after Phase 3 bench and before Phase
5 publish. Reads the calibration + candidate `bench-run.json` files (with rich
profile data, per-block and per-tx breakdowns) plus the bench DB, and emits a
structured assessment of whether the measured `improvement_pct` is real,
meaningful, robust, and free of unexpected side-effects. Output feeds the Phase
5 PR-writer and the operator audit trail.

**Why deferred**: The current plan
([baseline-verification-agent-plan.md](baseline-verification-agent-plan.md))
lands Pass 1a's apples-to-apples comparison and Pass 2's pre-bench verifier.
Together those resolve the methodology integrity gap that's actively blocking
PR-grade numbers — the verifier reasons about whether the targeted-replay
workload is representative, and Pass 1a ensures numerator + denominator are
measured under matched cache regimes. The results-analyzer gap is real but
secondary: `improvement_pct` can still be a defensible PR-body number even
without it, as long as Phase 5's PR-writer surfaces the verifier's context.

Pass 4 sharpens the resulting PR body and catches narrower failure modes; it
doesn't unblock shipping the way Pass 1a + 2 do.

**Trigger to pick up**: After Pass 1a + Pass 2 land and produce real-session
data, audit the resulting `improvement_pct` numbers and PR bodies against the
actual changes. Look for failure modes the mechanical arithmetic obscured:

- Improvement concentrated in one of N replay blocks; the others barely moved.
- Per-call cost ↓ but call count ↑ — net win is smaller than the per-call number
  suggests.
- Wide variance band — the headline number could be noise.
- Shifted hot path — fix didn't speed up the target span; it changed which path
  is hot.
- Specific tx types regressed; specific tx types improved. Net positive but the
  regression deserves disclosure.

If any of these slip past the verifier + PR-writer in real sessions, Pass 4
becomes justified.

**Design sketch (deferred scope)**: same scaffolding as Pass 2's verifier.
Per-target fanout. Advisory pattern: emits
`analyze/<target>/results-analysis.json` with fields like:

- `confidence` — `high` | `medium` | `low`
- `caveats[]` — structured observations
- `recommended_disposition` — `ship` | `ship_with_caveats` |
  `hold` | `reject`
- `pr_body_excerpt` — suggested framing for the PR-writer
- `db_queries[]` — same `{purpose, query_digest, output_path}` shape as the
  verifier

Coordinator applies operator-set thresholds (similar to `verification_floor`) to
translate recommendation into action. Phase 5 PR-writer reads `pr_body_excerpt`
and weaves it into the PR body. Read-only DB access (same pattern as verifier;
via sqlite3 CLI in v1, MCP wrapper in Pass 3).

**Estimated scope**: ~15-20 hours (similar to Pass 2's verifier), plus ~1-2
weeks of prompt iteration against real-session data before defaults stabilize.

---

## Other items considered but deferred

These have been mentioned in planning conversations or doc-review passes but not
been worked into the design docs. Captured here so they don't get lost.

### Named phases over numbered phases

Replace "Phase 1.8 / 1.9 / 2 / 3" with descriptive names everywhere (CLI
subcommands, artifact paths, prompts, docs). Codex flagged this in a review; we
agreed the direction is right. Deferred because it's a larger cross-cutting
refactor and the current numbered scheme is workable. Picking it up makes sense
once the phase set stabilizes (i.e. after Pass 2 + the results-analyzer landing
decision).

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

### Analyzer-defined measurement protocol

Today the optimizer-target schema's `verification_replay` carries one bench
shape (`{txids, repetitions, warmup, rationale}`). Phase 1.8 calibration and
Phase 3 candidate bench both run that single shape regardless of what the
target is optimizing. Independent review of session `20260521-051649` flagged
this as the principal methodology concern: a 73.30% reported win on a
node-cache target is internally consistent but can't be separated into "real
cache hits during measurement" vs "warmup smearing the cache effect across
repetitions." The same blind-spot exists for any optimization whose benefit
is workload-shape-dependent (batched writes, lazy-init paths, first-touch
elision).

**Proposed direction**: have the analyzer always emit its recommended
benchmark protocol per target. Replace the one-shape `verification_replay`
with an arbitrary list of analyzer-defined measurement profiles, each
carrying a hypothesis about the expected per-profile delta. The verifier
(Pass 2) then has a concrete contract to check: measured-vs-hypothesized,
per profile — judging SHAPE consistency, not magnitude accuracy.

Sketch:

```json
{
  "txids": [...],
  "blocks": [...],
  "rationale": "...",
  "measurement_profiles": [
    {
      "label": "cold",
      "warmup": 0,
      "repetitions": 5,
      "profiler": "rich",
      "purpose": "isolate first-touch cost; cache benefit should be minimal here",
      "expected_signal": {
        "axis": "tx_latency",
        "direction": "neutral",
        "estimate_pct": 0,
        "tolerance_pct": 2
      }
    },
    {
      "label": "warm",
      "warmup": 10,
      "repetitions": 20,
      "profiler": "rich",
      "purpose": "steady-state cache-hit rate after warmup populates working set",
      "expected_signal": {
        "axis": "tx_latency",
        "direction": "improves",
        "estimate_pct": 4.5,
        "tolerance_pct": 3
      }
    }
  ]
}
```

Key shape choices, with rationale:

- **`expected_signal` is an object, not a percent.** Splits the QUALITATIVE
  hypothesis (`direction: improves | neutral | regresses`) from the
  QUANTITATIVE one (`estimate_pct` + `tolerance_pct`). Some profiles have a
  confident direction but uncertain magnitude ("I know warm should improve;
  could be 5% or 50% depending on working-set hit rate"). Some have the
  inverse ("magnitude small, direction load-bearing"). Numeric fields stay
  optional within the object.
- **`profiler` is a per-profile string, not a flag list.** v1 only accepts
  `"rich"`; lean opt-in later. The invariant: baseline and candidate inside
  a profile must use the same `profiler` value. (See "Flag symmetry between
  baseline and candidate benches" above — this carries that contract
  forward into the analyzer-emitted schema.)
- **Samples (`txids` / `blocks`) stay top-level in v1.** Per-profile sample
  lists are powerful but materially complicate the first implementation;
  defer until a real target needs differentiated samples per profile.
- **`measurement_profiles` is required, `minItems: 1`.** Every accepted
  `normal_pr` target says how it wants to be measured. A single default
  profile is fine, but it must be explicit.

Verifier reasoning against this contract — shape first, magnitude second:

- Cache target hypothesizes `cold: neutral, warm: improves +4.5%`. Measured
  `cold: +1%, warm: +70%` → SHAPE matches (warm-dominant, cold neutral);
  MAGNITUDE on warm is much bigger than estimated. Verifier accepts the
  win, caveats the magnitude in the PR body ("substantially exceeded
  analyzer estimate — possibly working-set-specific to this replay").
- Cache target measures `cold: +35%, warm: +30%` → SHAPE wrong (cold should
  have been neutral). Verifier flags: analyzer's mechanism hypothesis is
  off; either re-attribute the cause or reject.
- Cache target measures `cold: -10%, warm: +70%` → cache regresses
  first-touch latency. Verifier surfaces the tradeoff; operator decides
  whether to ship as default or as opt-in tunable.

Why this is sharper than enumerating cold/warm in the schema: optimization
types we haven't seen yet (sustained-pressure batched writes,
predicate-conditioned MARF aliases, lazy-init for one-time bootstrap paths)
each want their own measurement intents. Hard-coding "cold/warm" as schema
constants constrains the analyzer's reasoning; letting the analyzer emit an
arbitrary list keeps the design open while still gating PR-quality numbers
on the analyzer-stated hypothesis. Cold/warm become canonical examples in
the analyzer prompt, not schema concepts.

**Open design questions to resolve before scoping**:

- Operator cap — `max_measurement_profiles_per_target` setting, default
  ~3. Fail-before-bench on exceed (preserves the analyzer-contract
  honesty) rather than silently truncate.
- Default `profiler` value if the analyzer omits it — probably required +
  enforced via schema, no implicit default.
- Backward-compat — Pass 1c is pre-production; clean-break replacement of
  `verification_replay`'s flat shape is acceptable.

**Why deferred from Pass 1a**: Pass 1a's contract was already heavy enough.
Pass 1a's structural validation is complete; quantitative validation waits
on this redesign.

**Trigger to pick up**: paired with Pass 2's verifier — the verifier
consumes this contract directly. Both can land together as "Pass 1c" or as
part of Pass 2's scope. Whichever land window happens, the measurement-
protocol work blocks quoting any session's numbers externally.

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
picked up, it gets promoted to its own design doc (like
`baseline-verification-agent-plan.md`) and removed from this register.

Items can be removed (rejected / decided against) too. When that happens, note
the decision rationale in a "decided against" section rather than deleting
silently — others' future thinking benefits from knowing what was considered and
why it didn't fit.
