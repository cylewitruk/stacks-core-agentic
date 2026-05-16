# Autonomous roadmap

The roadmap for the closed-loop autonomous system moved to the operator repo on 2026-05-12.

**Source of truth:** [`stacks-bench-agentic-operator/autonomous-roadmap.md`](https://github.com/cylewitruk/stacks-bench-agentic-operator/blob/main/autonomous-roadmap.md)

## What stays in this repo (tool side)

Items relevant to `stacks-bench-agent`:

- **Layer 0** — substrate (stacks-bench targeted-replay). ✅ Done 2026-05-11.
- **Tool/operator split** — submodule moved to operator repo, `Settings::base` made lazy via `Layout::require_base()`. ✅ Done 2026-05-12.
- **Disk-first prompts** — Askama → MiniJinja swap; bundled templates seeded into operator's `prompt_overrides_dir` on startup with don't-replace semantics; reference docs (`non-targets.md`, `bucket-anchors.md`) seed alongside templates. ✅ Done 2026-05-12.
- **`sbagent prompt lint` / `sbagent prompt sync --force`** — runtime validation replaces Askama's compile-time drift check. ✅ Done 2026-05-12.
- **Layer 1A** — `verification_replay` schema field + analyzer prompt + `bench_experiments.rs` branch + unstable-id cleanup (hash-only `representative_ids`, drilldown queries take hash params with dim-join resolution). ✅ Done 2026-05-12.
- **Layer 1B** — optimizer inner-loop mode (autoresearch-style commit-or-reset). 🚧 v1 shipped 2026-05-12 (prompt + Settings + plumbing); deferred items in operator-side roadmap (explicit chainstate/network plumbing, typed attempts.jsonl parser into Layer 2, parallel-agents > 1, local-baseline propagation to Phase 4).

## What lives in the operator repo

All operational concerns: event log, projection cache, dedup filter, `sbagent maintain`, GitHub Actions, hygiene (pause file, rate limits, signed commits, idempotency keys), observability, the `sessions/` archive.

## Why split

Industry pattern: tool ≠ operational state. See the operator repo's roadmap, section "Architectural decision — tool vs operator split", for the full rationale and decision log.

## Maintenance contract reminder

When implementing any of the tool-side items above, also update the operator repo's `autonomous-roadmap.md`:

1. Status field on each item touched.
2. Append a Change log entry recording what was completed, deviations, and follow-ups.

This stub does not get a Change log; the operator repo's roadmap is the only one to maintain.
