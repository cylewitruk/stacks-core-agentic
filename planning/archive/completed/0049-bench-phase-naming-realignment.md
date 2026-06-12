# 0049: Bench Phase Naming Realignment

- **id:** `0049-bench-phase-naming-realignment`
- **status:** `shipped`
- **completed:** `2026-06-12`
- **iteration:** [v9: Bench Phase Vocabulary Realignment](v9-bench-phase-vocabulary-realignment.md)

## Problem

Operator-facing prose overloaded "baseline" for three distinct benchmark stages:

- The Phase 0 session-wide range scan that feeds triage.
- The Phase 1.8 per-target unchanged-source runs paired with candidate
  invocations for Phase 3.5 comparisons.
- The Phase 3 optimized-candidate runs (often colloquially called "verify"
  in the artifact tree, which itself adds confusion).

`TargetBench.baseline_run_ids`, `SessionRecord.baseline_run_ids`,
`baseline_dir()`, the artifact path `<session>/results/baseline/`, the SQL
parameter `:baseline_run_id`, and prompt prose all used "baseline" with
different meanings at different scopes. v8's prompt-hardening pass exposed
the cost: PR bodies and `summary.md` mixed the senses, and reviewers had to
infer which "baseline" was being cited.

## Shipped

Tier 1 prose-only realignment to a three-term vocabulary:

- **Discovery pass** — Phase 0 session-wide range scan that feeds triage.
- **Target calibration baseline** — Phase 1.8 per-target unchanged-source
  run paired with a candidate invocation in Phase 3.5.
- **Verification bench** — Phase 3 per-target optimized-candidate run for
  the same invocation.

Touched surfaces:

- **6 prompt templates:** `analyzer.md`, `results-analyzer.md`,
  `optimizer.md`, `merge-analyses.md`, `pr-writer.md`, `triage.md`. Heaviest
  rewrites in analyzer / results-analyzer / pr-writer where the
  calibration↔verification pair is load-bearing for verdicts.
- **7 docs files:** `architecture.md`, `workflow.md`, `operations.md`,
  `configuration.md`, `setup.md`, `git-topology.md`, `session-archive.md`.
- **Query catalog README** + bundled mirror in `.sbagent/queries/README.md`.
- **Rendered prose:** `summary.md` / `targets.md` labels and prose.
- **Explanatory comments:** `session/baseline.rs` gained a vocabulary note
  at the module top explaining that the legacy `baseline` symbol name is
  retained but operator-facing prose calls Phase 0 the discovery pass.
  Selected doc comments in `session/layout.rs` updated similarly.

Tier 1 boundary discipline:

- Where new prose references legacy template variables (e.g.
  `{{ baseline_run_id }}` in `triage.md` and `merge-analyses.md`), explicit
  `(legacy field name)` annotations sit next to the variable so an agent
  reading the prompt knows the symbol name doesn't match the new
  vocabulary.
- Artifact paths (`<session>/results/baseline/`,
  `<session>/results/verify/`), CLI subcommand names
  (`sbagent session baseline run`), SQL parameter names
  (`:baseline_run_id` in `compare_*.sql`), and Rust symbol names
  (`baseline_dir()`, `archive_baseline_binary`, `TargetBench.baseline_run_ids`,
  `SessionRecord.baseline_run_ids`) are explicitly NOT renamed.
- Schema field doc comments are unchanged because schemars regenerates
  JSON Schema `description` fields from those — editing them would force a
  schema bump even though no field/type changed.

## Validation

- `just lint --no-sccache` clean.
- `just test --results -p stacks-bench-agent prompt` → 23/23 (the v8
  contract tests still pass against the relabeled prose; substring
  assertions chosen for v8 were robust to the rename).
- `git diff --stat schemas/` empty (Tier 1 guardrail held).
- Operator-disk mirrors `.sbagent/prompts/*.md` and
  `.sbagent/queries/README.md` byte-identical to bundled sources.

## Follow-Ups

- **Tier 2 (deferred):** schema field renames + Rust symbol renames. Would
  require a `sessions.jsonl` schema bump (v3 → v4) with `from_ledger_line`
  backward-compat extension. File when there's a forcing function.
- **Tier 3 (probably never):** CLI subcommand renames, artifact path
  renames, SQL parameter renames. Tolerable indefinitely as legacy
  symbol names with the vocabulary note as the disambiguating breadcrumb.
- Next live smoke validates that PR bodies + `summary.md` + `history show`
  output speak with one vocabulary; that's the natural successor
  checkpoint alongside v8's smoke comparison bullet.
