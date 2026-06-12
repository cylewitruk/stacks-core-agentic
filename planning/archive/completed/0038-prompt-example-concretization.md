# Completed: Prompt Example Concretization

- **id:** `0038-prompt-example-concretization`
- **status:** `shipped`
- **completed:** `2026-06-11`
- **iteration:** [v7: Evidence-Backed Verification](v7-evidence-backed-verification.md)

## Problem

Some analyzer prompt examples used schema-invalid placeholders, so the
schema-example lint added in v2 could not cover them without changing prompt
prose deliberately.

## Shipped

v7 replaced placeholder output examples with concrete schema-valid values and
added the lint markers needed for the bundled prompt lint path to validate them.

## Validation

- `sbagent prompt lint` covers the updated analyzer examples.
- v7 schema parity and prompt-lint tests passed during review.
