You are preparing a GitHub pull request for an accepted autonomous optimization to `stacks-core`.

# Goal

Write concise, factual PR artifacts for this accepted optimization target:

- `${OUTPUT_DIR}/pr-title.txt` — a single-line PR title
- `${OUTPUT_DIR}/pr-body.md` — a markdown PR body

Do NOT create the PR yourself. Do NOT use GitHub tools. Only write the two files above.

# Inputs

- Session id: `${OPT_SESSION_ID}`
- Target id: `${TARGET_ID}`
- Output directory: `${OUTPUT_DIR}`
- Worktree directory: `${WORKTREE_DIR}`
- Accepted target JSON:

```json
${TARGET_JSON}
```

- Final benchmark summary for this target:

```json
${EXPERIMENT_JSON}
```

- Implementation notes are in `${OUTPUT_DIR}/implementation.md`
- Test output (truncate as needed) lives in `${OUTPUT_DIR}/nextest.log` and `${OUTPUT_DIR}/nextest.stderr.log`. Cite specific numbers from these files in the `Validation` section rather than paraphrasing.
- Build log (for any flag/version-related notes) is at `${OUTPUT_DIR}/cargo-build.log`.

# Requirements

- Be accurate and conservative. Do not claim results that are not present in the inputs.
- Keep the title under 80 characters when possible.
- Prefer a title like `perf: <specific optimization summary>`.
- The PR body should include these sections:
  - `## Summary`
  - `## What changed`
  - `## Benchmark result`
  - `## Validation`
- In `Benchmark result`, include the measured `improvement_pct` when present and the run ids from `run_ids`.
- In `Validation`, summarize tests/verification from `implementation.md` without inventing anything.
- Mention risk briefly if it is present in the target JSON.

# Output format

- `pr-title.txt` should contain exactly one plain-text line.
- `pr-body.md` should be valid markdown with the sections above.

Do not edit source code. Do not stage, commit, push, or publish anything.
