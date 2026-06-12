You are preparing a GitHub issue for a consensus-breaking finding from an autonomous optimization run on `stacks-core`.

# Goal

Write concise, factual issue artifacts for this consensus-breaking target:

- `{{ output_dir }}/issue-title.txt` — a single-line issue title.
- `{{ output_dir }}/issue-body.md` — a markdown issue body suitable for human review and HIP-style discussion.

Do NOT create the issue yourself. Do NOT use GitHub tools. Only write the two files above.

# Why an issue, not a PR

This target's `delivery_mode` is `consensus_issue`. The analyzer concluded the proposed fix changes consensus rules AND is too large or too coverage-blocked to ship as a PoC PR (typically: `breakage_class == "block_validation"` which the bench harness cannot exercise; OR `poc_implementable: false` because the change requires cross-validator coordination that PoC tests can't capture). No optimizer ran for this target — there is no implementation, no benchmark data, no test output. The shipping artifact is the analyzer's `consensus_writeup`.

The issue you write becomes the entry point for HIP-style discussion. Reviewers must be able to decide whether to start that discussion without re-investigating from scratch.

# Inputs

- Session id: `{{ opt_session_id }}`
- Target id: `{{ target_id }}`
- Output directory: `{{ output_dir }}`
- Accepted target JSON (note: `consensus_breaking == true`, `delivery_mode == "consensus_issue"`):

```json
{{ target_json }}
```

The target carries:

- `breakage_class` — the consensus-rule kind (`clarity_cost_weight`, `clarity_vm_behavior`, `mining_flow`, `block_validation`, `marf_layout`, `on_chain_format`).
- `consensus_writeup` — the analyzer's prose explaining what changes, why, who pays for it, what HIP coordination is implied, and any safety/migration concerns. THIS IS THE PRIMARY SOURCE for the issue body.
- `target_span`, `bucket`, `hotspot`, `files`, `evidence`, `proposed_change`, `expected_improvement` — context the analyzer collected during investigation. Use to support the writeup; don't replace it.

# Requirements

- Be accurate and conservative. Ship the analyzer's writeup; don't invent new claims.
- Title under 80 characters when possible. Prefer `consensus: <specific change summary>` to make the consensus nature visible at a glance.
- The issue body MUST include these sections, in order:
  - `## Summary` — one paragraph. State the proposed consensus rule change and why it's worth proposing, both at the level a reader scanning issues will absorb. Do NOT bury the consensus nature.
  - `## Breakage class` — name the `breakage_class` and explain in one or two sentences what kind of consensus rule it touches.
  - `## Proposed change` — derived from `consensus_writeup` and `proposed_change`. Be specific about what would actually change in the code/protocol.
  - `## Expected impact` — why this is worth doing. Cite `expected_improvement` and (when available) discovery-pass evidence from the analyzer's `evidence` field. Be honest about uncertainty.
  - `## HIP / coordination concerns` — pulled from the analyzer's writeup: who needs to upgrade, what the migration story looks like, any safety considerations. If the writeup names them, list them; if it doesn't, say so explicitly rather than fabricating.
  - `## Why an issue, not a PR` — one paragraph. State whether this is `block_validation` (bench can't exercise) or `poc_implementable: false` for another reason; this helps reviewers understand why no PoC accompanies the proposal.
  - `## Reference: target id` — `{{ target_id }}` so the issue is traceable back to the autonomous run.

# Output format

- `issue-title.txt`: exactly one plain-text line.
- `issue-body.md`: valid markdown with the sections above, in order.

Do not edit source code. Do not stage, commit, push, or publish anything. Do not create the issue or run any GitHub tooling.
