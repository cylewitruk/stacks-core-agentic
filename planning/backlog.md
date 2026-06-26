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

<a id="0053-operator-performance-dashboard"></a>

### Operator Performance Dashboard

- **id:** `0053-operator-performance-dashboard`
- **status:** `backlog`
- **priority:** `medium`
- **depends on:** `0052-session-artifact-layout-and-retention`
- **source:** Post-v17 report review. `sbagent history report` provides a
  recent activity digest, but the operator repo still lacks a cumulative
  public status surface for the autonomous loop.

**Problem:** A visitor to the operator repo should be able to understand the
loop's cumulative performance from the README without running local commands:
what the loop has found, how often analyzer hypotheses survive verification,
how many PRs exist and where they stand, and whether the loop is healthy. The
current history report is useful for recent activity, but it is not a durable
repo-level dashboard and does not expose machine-readable metrics for future
self-improvement agents.

**Scope:**

- Define a versioned metrics model, for example `OperatorMetricsV1`, that can
  be rendered to both Markdown and `reports/status/metrics.json`. Generate
  `schemas/operator-metrics.v1.json` from the Rust model through the existing
  schema export pipeline; do not hand-author schema mirrors.
- Compute cumulative metrics from durable operator data:
  - `sessions.jsonl` via `HistoryProjectionV1` for session outcomes, target
    outcomes, wall-clock, PR / issue URLs, and measured improvements;
  - `maintain.jsonl` via `HistoryProjectionV1` for PR / issue lifecycle state;
  - selected per-session archive artifacts through a separate archive-artifact
    reader for:
    - analyzer outputs: estimates, impact classifications, risk, bucket, and
      fix signatures;
    - merge / optimization-targets outputs: merge survivors, merge rejections,
      and dedup decisions;
    - results-analysis outputs: verdicts, confidence,
      `matches_expected_signal`, measured-vs-expected deltas, and caveats.
- Keep ledger access on the v15/v16 projection substrate. Only archive-branch
  artifact reads may use a separate reader, and that reader should target the
  post-`0052` stable artifact layout.
- Render a compact README status block between stable markers such as
  `<!-- sbagent:status:start -->` and `<!-- sbagent:status:end -->`.
- Render deeper drill-down artifacts such as:
  - `reports/status/latest.md`;
  - `reports/status/metrics.json` for future self-improvement agents.
- Add a mutating publish/update command, tentatively
  `sbagent status publish`, that updates the README block and report artifacts,
  then commits and pushes only when content changed. Reuse the existing
  publisher PAT / git commit machinery used by `sync`, `archive`, and
  `maintain`.
- Keep dashboard publishing as a standalone command. Automation should call it
  from a separate operator timer/workflow after `sbagent maintain`, rather than
  having `maintain` shell out to it internally.

**Candidate metrics:**

Each metric should document whether it is ledger-derived, archive-derived, or
mixed, so reviewers can identify heavyweight archive reads at a glance.

- Discovery (archive / mixed): unique targets, unique fix signatures, unique
  families, buckets touched, and high / medium / low impact classifications.
- Analyzer calibration (archive): accepted / mixed / rejected rates, confidence
  distribution, `matches_expected_signal` rate, estimate error, and within /
  under / over expected-range buckets.
- Optimization outcomes (ledger / archive): targets optimized, verification
  wins, aborted targets, average measured improvement, and mixed-verdict rate.
- PR lifecycle (ledger): total PRs created, open / merged / closed counts,
  stale PRs, and time-open summaries where data is available.
- Loop health (ledger): total sessions, succeeded / failed / aborted counts,
  zero-accepted sessions, safety pauses, and phase wall-clock trends.
- Dedup / memory (archive / mixed): targets skipped due open PR, merged prior
  fix, repeated failure, and families with prior-attempt context. Dedup-decision
  metrics depend on either exposing v12 dedup rows through
  `HistoryProjectionV1` or reading the post-`0052` archive artifacts that carry
  `optimization-targets.json`.

**Acceptance:**

- README has a generated, marker-bounded status block that can be refreshed
  without disturbing hand-written content.
- `reports/status/latest.md` provides a cumulative human-readable dashboard.
- `reports/status/metrics.json` provides the same core metrics in a typed,
  schema-versioned machine-readable form.
- `schemas/operator-metrics.v1.json` is generated from the Rust model via the
  existing schema pipeline.
- The command fails with a clear diagnostic when README markers are absent; it
  must not silently append a block at an arbitrary location.
- The command is idempotent: if generated content is unchanged, no commit or
  push occurs. Idempotency is checked by rendering the README block and report
  artifacts in memory, byte-comparing them with current on-disk content, and
  pushing only on diff.
- Metrics document which fields are ledger-derived and which require archived
  session artifacts.
- Ledger reads go through `HistoryProjectionV1`; archive-branch artifact reads
  are isolated behind a separate reader.
- The implementation does not rely on the current pre-cleanup artifact layout
  in ways that would conflict with `0052`.

**Deferred / non-goals:** Do not implement before the session artifact layout
and retention policy is settled. Do not make README the source of truth; it is
only a rendered summary of ledger and archive-derived metrics. Do not ship
dated dashboard snapshots in v1; `sbagent history report --out` already covers
operator-selected weekly snapshots.
