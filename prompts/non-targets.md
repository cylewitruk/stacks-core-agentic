# Non-targets

These profiler spans are known to be unproductive optimization targets. The coordinator
must exclude them from the candidate set; the optimizer must abort if its assigned target
overlaps with this list.

| Span                  | Reason                                                       |
| --------------------- | ------------------------------------------------------------ |
| `with_abort_callback` | Represents Clarity VM execution time, not callback overhead. |
| `Segment`             | Benchmark harness, not node code.                            |
| `fetch_metadata`      | Already has a read-through cache in `RollbackWrapper`.       |
| `get_contract`        | Already cached with `Rc` in `ClarityDatabase`.               |
| `canonicalize_types`  | Already addressed by contract caching.                       |

Append to this file as additional dead-end spans are discovered. Do not duplicate this list inside the prompt templates.
