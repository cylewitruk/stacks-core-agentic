You are preparing a GitHub pull request for an autonomous optimization run on `stacks-core`. There are two PR shapes you may be asked to write — see "Delivery mode" below — and the expected framing differs significantly between them.

# Goal

Write concise, factual PR artifacts for this target:

- `{{ output_dir }}/pr-title.txt` — a single-line PR title
- `{{ output_dir }}/pr-body.md` — a markdown PR body

Do NOT create the PR yourself. Do NOT use GitHub tools. Only write the two files above.

# Delivery mode

Your delivery mode for this target is `{{ delivery_mode }}`. Two PR shapes:

- **`normal_pr`** — a standard performance optimization. The optimizer ran the full nextest suite and the coordinator measured a real improvement above the noise floor. The PR is a regular draft (or non-draft per operator preference) seeking review and merge in the usual way.

- **`consensus_poc_pr`** — a deliberate consensus-breaking change shipped as a PoC. The optimizer ran nextest filtered to `poc_test_scope` ONLY; the full suite is not the acceptance gate and may encode old consensus expectations that the change deliberately invalidates. **No benchmark ran** — the bench harness encodes pre-change consensus rules and would either crash or produce meaningless numbers. The PR is ALWAYS a draft and the publisher applies safety labels (`consensus-change`, `needs-HIP`, `do-not-merge`) to prevent accidental merging. The PR is the entry point for HIP-style discussion of the consensus change.

If `{{ delivery_mode }}` is `consensus_poc_pr`, the framing in your PR body MUST make the consensus nature obvious:

- Title: prefer `consensus(PoC): <specific change summary>` or `perf(consensus PoC): <…>` so the consensus nature shows up at a glance.
- `## Summary`: state explicitly that this is a consensus-breaking PoC and that the change requires HIP-style coordination before merge.
- `## What changed`: same content, no special framing needed.
- `## Benchmark result`: the bench was SKIPPED BY DESIGN. State this explicitly. Do not invent improvement numbers. Cite the analyzer's `expected_improvement` vector from the target JSON if useful, but make clear it's an analyzer estimate, not a measured result.
- `## Validation`: the scoped nextest run is the acceptance gate. Cite the scoped tests that passed (from `{{ output_dir }}/nextest.log`) and the breakage_class. Note explicitly that the full suite was NOT the gate and that some non-scoped tests may encode pre-change consensus expectations the fix invalidates. Do NOT claim full-suite passage.
- Add a final `## Consensus / HIP coordination` section pulling from the target's `consensus_writeup` field — what the rule change is, who pays for it, what HIP discussion would be required.

# Inputs

- Session id: `{{ opt_session_id }}`
- Target id: `{{ target_id }}`
- Output directory: `{{ output_dir }}`
- Worktree directory: `{{ worktree_dir }}`
- Accepted target JSON:

```json
{{ target_json }}
```

- Final benchmark summary for this target:

```json
{{ experiment_json }}
```

- Implementation notes are in `{{ output_dir }}/implementation.md`
- Test output (truncate as needed) lives in `{{ output_dir }}/nextest.log` and `{{ output_dir }}/nextest.stderr.log`. Cite specific numbers from these files in the `Validation` section rather than paraphrasing.
- Build log (for any flag/version-related notes) is at `{{ output_dir }}/cargo-build.log`.

# Requirements

- Be accurate and conservative. Do not claim results that are not present in the inputs.
- Keep the title under 80 characters when possible.
- Title style depends on delivery mode: `perf: <…>` for `normal_pr`, `consensus(PoC): <…>` (or `perf(consensus PoC): <…>`) for `consensus_poc_pr`.
- The PR body MUST include these sections (in this order):
  - `## Summary`
  - `## What changed`
  - `## Benchmark result`
  - `## Validation`
- Plus, when `{{ delivery_mode }}` is `consensus_poc_pr`: a final `## Consensus / HIP coordination` section.
- For `normal_pr`: in `Benchmark result`, include the measured `improvement_pct` from `{{ experiment_json }}` and the run ids from `run_ids`. In `Validation`, summarize tests/verification from `implementation.md` without inventing anything.
- For `consensus_poc_pr`: `{{ experiment_json }}` is `{}` (no benchmark ran). Do NOT invent improvement numbers. State explicitly that the benchmark was skipped by design (the harness encodes pre-change consensus). Cite the analyzer's `expected_improvement` vector from `{{ target_json }}` only as an analyzer estimate.
- Mention risk briefly if it is present in the target JSON.

# Output format

- `pr-title.txt` should contain exactly one plain-text line.
- `pr-body.md` should be valid markdown with the sections above.

Do not edit source code. Do not stage, commit, push, or publish anything.
