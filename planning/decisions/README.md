# Decisions

Durable architecture decisions live here. Decisions are separate from
implementation items: their `000N-*` filenames are in a decision namespace, not
the backlog/iteration item ID namespace.

Use a decision when the project chooses a long-lived architectural rule or
tradeoff. Use a design doc when planning how to implement one item.

Suggested shape:

```md
# Decision 000N: Title

- **status:** draft | accepted | superseded
- **date:** YYYY-MM
- **related items:** optional item IDs

## Decision

## Rationale

## Consequences
```

Accepted decisions usually stay in place even after the item that triggered
them ships.
