You are writing PR artifacts for an autonomous `stacks-core` optimization run.

# Mission

Write exactly:

- `{{ output_dir }}/pr-title.txt`
- `{{ output_dir }}/pr-body.md`

Do not create the PR or use GitHub tools.

# Inputs

- Session id: `{{ opt_session_id }}`
- Target id: `{{ target_id }}`
- Delivery mode: `{{ delivery_mode }}`
- Target JSON:

```json
{{ target_json }}
```

- Experiment JSON:

```json
{{ experiment_json }}
```

- Implementation notes: `{{ output_dir }}/implementation.md`
- Test logs: `{{ output_dir }}/nextest.log`,
  `{{ output_dir }}/nextest.stderr.log`
- Build log: `{{ output_dir }}/cargo-build.log`

# Delivery Modes

`normal_pr`:

- standard performance PR;
- title style: `perf: <specific optimization>`;
- include measured `improvement_pct` and run ids from `experiment_json`.

`consensus_poc_pr`:

- consensus-breaking draft PoC;
- title style: `consensus(PoC): <specific change>` or
  `perf(consensus PoC): <specific change>`;
- no benchmark ran; say it was skipped by design;
- cite scoped tests only, not full-suite passage. Some non-scoped tests may
  encode pre-change consensus expectations that the PoC deliberately
  invalidates;
- cite `expected_improvement` only as an analyzer estimate, not as a measured
  result;
- include `## Consensus / HIP coordination` from `consensus_writeup`.

# Required Body

Sections, in order: `## Summary`, `## What changed`,
`## Benchmark result`, `## Validation`, plus
`## Consensus / HIP coordination` only for `consensus_poc_pr`.

# Rules

- Be factual and conservative; title under 80 characters when possible.
- Do not invent benchmark or test results; mention risk when present.
- For consensus PoCs, make the consensus nature obvious in the summary and
  describe required HIP-style coordination.

# Output

- `pr-title.txt`: exactly one plain-text line.
- `pr-body.md`: valid markdown.

Do not edit source, stage, commit, push, or publish.
