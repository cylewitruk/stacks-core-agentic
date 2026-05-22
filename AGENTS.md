# Agent Guidance

## Project Orientation

This repo builds `sbagent`, a Rust CLI that orchestrates benchmark-driven
agent workflows for `stacks-core`: baseline runs, triage, analysis, merge,
optimization, verification benches, finalize, publish, and session archival.

Key paths:

- `crates/stacks-bench-agent/src/` — CLI, orchestration, models, layout,
  git, and session phase code.
- `crates/stacks-bench-agent/templates/` — agent prompt templates. Keep these
  dense and schema-aligned.
- `context/`, `queries/`, `schemas/` — bundled operator artifacts. Schemas are
  generated from Rust models; avoid hand-editing generated schema drift.
- `docs/` — operator and architecture docs. Start with
  `docs/configuration.md`, `docs/workflow.md`, `docs/operations.md`, and
  `docs/architecture.md`.
- `assets/example.config.toml` — annotated config template for operators.
- `roadmap.md` — active follow-up items and deferred design work.

The operator deployment is separate from this tool repo. Runtime config normally
lives at `~/.config/sbagent/config.toml`; mutable session scratch should live
outside the operator repo under `agent_workspace_root`.

## Workspace Commands

ALWAYS prefer these commands over using `cargo ...` or `rustfmt ...` directly. Only
fall-back to custom tool calls if necessary.

| Command | Description |
| ------- | ----------- |
| `just build` | Build the workspace |
| `just lint` | Lint the workspace (incl. `rustfmt` check) |
| `just test` | Run workspace tests |

`just test` accepts nextest filters and a few agent-friendly output modes:

- `just test <filter>` runs matching tests.
- `just test --summary <filter>` prints only the nextest header and summary.
- `just test --failures <filter>` prints failing tests and captured failure
  output.
- `just test --results <filter>` prints per-test pass/fail statuses without
  captured success output.
- Add `--no-sccache` when the sandbox blocks the configured compiler cache.
  This is supported by `just build`, `just install`, `just lint`,
  `just fix`, and `just test`.

## Coding Style

Write clear, concise, idiomatic and best-practice-driven code for any given
language.

### Rust

### LLM Prompts

When engineering LLM/agent prompts:

- Be mindful of token usage; optimize wording to be clear and concise with high
  signal density.
- Avoid repeating what's already been stated in the document. If you find
  yourself doing this, it may indicate that a structural refactor is necessary
  to preserve density; if so, propose your suggestion(s) to the user.

## Documentation

Follow these rules when writing any documentation, both in code and dedicated
files:

- Avoid long, drawn-out and overexplained documentation; be clear but concise,
  focusing on the important details of the item being documented.
