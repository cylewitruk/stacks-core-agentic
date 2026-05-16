# Agent Guidance

## Workspace Commands

Prefer these commands over using `cargo ...` or `rustfmt ...` directly. Only fall-back to custom
usage if necessary.

| Command | Description |
| ------- | ----------- |
| `just build` | Build the workspace |
| `just lint` | Lint the workspace (incl. `rustfmt` check) |
| `just test` | Run workspace tests |
