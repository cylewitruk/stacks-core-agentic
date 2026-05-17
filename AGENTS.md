# Agent Guidance

## Workspace Commands

Prefer these commands over using `cargo ...` or `rustfmt ...` directly. Only
fall-back to custom usage if necessary.

| Command | Description |
| ------- | ----------- |
| `just build` | Build the workspace |
| `just lint` | Lint the workspace (incl. `rustfmt` check) |
| `just test` | Run workspace tests |

## Coding Style

Write clear, concise, idiomatic and best-practice-driven code for any given
language.

### Rust

## Documentation

Follow these rules when writing any documentation, both in code and dedicated
files:

- Avoid long, drawn-out and overexplained documentation; be clear but concise,
  focusing on the important details of the item being documented.
