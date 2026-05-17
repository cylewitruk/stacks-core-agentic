# Non-targets

These profiler spans are known to be unproductive optimization targets. Triage
must not promote them as candidates; the optimizer must abort if its assigned
target's span name matches an entry below.

Scope: this is a **span-level** exclusion list, NOT a subtree exclusion list.
A span is excluded only if its name matches an entry exactly (or is an obvious
alias for the same span). Spans that live *under* a non-target wrapper —
including descendants, callees, and unrelated work that just happens to share
the same hot path — remain valid candidates if they are individually
actionable. The whole reason `with_abort_callback` is on this list, for
example, is that its self-time IS the Clarity VM execution it wraps; the
actual optimization handles live below it (`lookup_variable`, MARF reads,
serialization, etc.) and should be considered on their own merits.

| Span                  | Reason                                                                              |
| --------------------- | ----------------------------------------------------------------------------------- |
| `with_abort_callback` | Represents Clarity VM execution time, not callback overhead.                        |
| `Segment`             | Benchmark harness wrapper, not node code.                                           |
| `Segment: Setup`      | Benchmark harness setup; runs before chain processing and is not on the hot path.   |
| `fetch_metadata`      | Already has a read-through cache in `RollbackWrapper`.                              |
| `get_contract`        | Already cached with `Rc` in `ClarityDatabase`.                                      |
| `canonicalize_types`  | Already addressed by contract caching.                                              |

Append to this file as additional dead-end spans are discovered. Do not duplicate this list inside the prompt templates.
