You are writing issue artifacts for a consensus-breaking `stacks-core` finding.

# Mission

Write exactly:

- `{{ output_dir }}/issue-title.txt`
- `{{ output_dir }}/issue-body.md`

Do not create the issue or use GitHub tools.

# Context

This target has `delivery_mode = "consensus_issue"`. No optimizer ran. There
is no implementation, benchmark result, or test output. The analyzer's
`consensus_writeup` is the primary source.

# Inputs

- Session id: `{{ opt_session_id }}`
- Target id: `{{ target_id }}`
- Target JSON:

```json
{{ target_json }}
```

Use `breakage_class`, `consensus_writeup`, `proposed_change`,
`expected_improvement`, and `evidence`.

# Required Body

Sections, in order: `## Summary`, `## Breakage class`,
`## Proposed change`, `## Expected impact`,
`## HIP / coordination concerns`, `## Why an issue, not a PR`,
`## Reference: target id`.

# Rules

- Be factual and conservative.
- Prefer `consensus: <specific change summary>` under 80 characters.
- Make the consensus nature obvious.
- Do not invent claims beyond target JSON.
- Say when coordination, migration, or safety details are missing rather than
  fabricating them.
- Explain why no PoC PR accompanies this finding, especially
  `block_validation` coverage limits or `poc_implementable: false`.

# Output Format

- `issue-title.txt`: exactly one plain-text line.
- `issue-body.md`: valid markdown.

Do not edit source, stage, commit, push, or publish.
