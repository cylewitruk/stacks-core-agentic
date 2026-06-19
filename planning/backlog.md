# Backlog

Items here are recoverable but not currently assigned to an iteration.

## Scheduled Items

No items currently scheduled. (Last shipped: `0043-history-report` in
[v17: History Report](archive/completed/v17-history-report.md).)

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

## Autonomous Closed-Loop Items

Imported from [assets/autonomous-roadmap.md](../assets/autonomous-roadmap.md).
These are not near-term until live manual sessions prove the core loop.

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

<a id="0052-session-artifact-layout-and-retention"></a>

### Session Artifact Layout And Retention

- **id:** `0052-session-artifact-layout-and-retention`
- **status:** `backlog`
- **priority:** `medium`
- **source:** Post-smoke archive review. Operator archive branches currently
  carry large duplicated session artifacts, including multiple
  `stacks-bench` binaries at roughly 19 MB each.

**Problem:** The session artifact tree grew phase-by-phase and now mixes
session-global artifacts, target-local artifacts, agent transcripts, publish
sidecars, and large local build outputs in paths that do not always reflect
ownership or retention value. Archive pushes can become unnecessarily large,
and target artifacts such as optimizer binaries are often redundant with the
bot-fork branch and commit SHA that already preserve the optimized source.

**Scope:**

- Inventory every artifact written under a session directory:
  - producer phase / agent;
  - consumer phases;
  - approximate size;
  - whether it is durable audit data, reproducible metadata, local scratch, or
    publish/debug-only output.
- Define a retention policy for large artifacts:
  - prefer source commit SHA, build metadata, and checksums over archiving
    optimizer binaries when the bot fork branch is the durable artifact;
  - keep any truly session-global binary/build artifacts at most once per
    session unless a stronger reproducibility need is documented;
  - exclude local scratch/build outputs from archive branches by default.
- Propose a v2 layout that separates session-global and per-target ownership,
  for example:
  - `sessions/<id>/<session-phase>/...` for session-global phases;
  - `sessions/<id>/targets/<target>/prepare/...`;
  - `sessions/<id>/targets/<target>/optimize/...`;
  - `sessions/<id>/targets/<target>/verify/...`;
  - `sessions/<id>/targets/<target>/results-analysis/...`;
  - `sessions/<id>/targets/<target>/publish/...`.
- Preserve read compatibility for existing archived sessions, including the
  smoke session `20260611-172955`.
- Add tests or archive guards that prevent accidentally archiving large local
  binaries or duplicated phase outputs without an explicit allowlist.

**Acceptance:**

- A documented artifact inventory classifies all current session outputs by
  producer, consumer, size class, and retention class.
- Archive output no longer includes optimizer binaries when a bot-fork branch
  and target `head_sha` are available.
- Any remaining archived binary artifacts have a documented durability reason.
- New layout rules make it clear which agent owns each per-target artifact.
- Existing session readers continue to handle pre-layout-v2 archives.
- Tests fail if a session archive would include unallowlisted large binaries or
  duplicate known build artifacts.
