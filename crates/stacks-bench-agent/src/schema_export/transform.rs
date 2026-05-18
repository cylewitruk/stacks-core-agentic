//! Post-generation transforms that inject the cross-field invariants
//! schemars cannot express via attributes.
//!
//! These mirror the if/then/else allOf chains the hand-written v2 schemas
//! shipped:
//!
//! - **Candidate**: `kind` discriminator gates the shape of
//!   `representative_ids`.
//! - **AnalyzerTarget** + **MergedTarget**: consensus-routing fields
//!   (`breakage_class`, `poc_implementable`, `poc_test_scope`,
//!   `consensus_writeup`) are required iff `consensus_breaking == true`,
//!   `poc_test_scope` is required iff `poc_implementable == true`, and
//!   `breakage_class == block_validation` forces `poc_implementable: false`.
//! - **MergedTarget**: derived `delivery_mode` and `bench_eligible` are pinned
//!   to specific consts based on `consensus_breaking` + `poc_implementable`.
//! - **LensDispositionEntry**: `status == not_actionable` requires `reason`.
//! - **Experiment**: `status` enum is restricted by `delivery_mode`.
//!
//! Invariant logic is duplicated in each top-level model's `validate()`
//! method (Rust-side enforcement). The injection here keeps the
//! LLM-facing JSON Schema constraint surface in parity with the hand-written
//! v2 schemas.

use serde_json::{Value, json};

/// Walk every `$defs/<TypeName>` in `schema` and apply the matching
/// invariant injection, if any. Top-level schemas are traversed via the
/// `$defs` map schemars emits.
pub fn apply_invariants(schema: &mut Value) {
    let Some(defs) = schema
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for (name, def) in defs.iter_mut() {
        match name.as_str() {
            "Candidate" => inject_candidate(def),
            "AcceptedAnalysis" => inject_accepted_analysis(def),
            "AnalyzerTarget" => inject_analyzer_target(def),
            "MergedTarget" => inject_merged_target(def),
            // Two structurally-distinct types share the same status →
            // reason invariant: `LensDisposition` is the inner struct on
            // accepted analyses; `LensDispositionEntry` is the propagated
            // form in optimization-targets and summary.
            "LensDisposition" | "LensDispositionEntry" => inject_lens_disposition(def),
            "Experiment" => inject_experiment(def),
            "ImplementedReport" => inject_implemented_report(def),
            "AbortedReport" => inject_aborted_report(def),
            _ => {}
        }
    }
}

/// Append an entry to a definition's `allOf` array (creating the array
/// when absent).
fn append_all_of(def: &mut Value, entry: Value) {
    let obj = match def.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    let all_of = obj
        .entry("allOf".to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(arr) = all_of.as_array_mut() {
        arr.push(entry);
    }
}

/// Candidate: `kind` discriminator gates `representative_ids` shape.
/// Mirrors the original schema's three-arm if/then chain.
fn inject_candidate(def: &mut Value) {
    for (kind, required, forbidden) in [
        ("tx_family", &["stacks_tx_hashes"][..], &["stacks_block_hashes", "contract_function"][..]),
        (
            "block_family",
            &["stacks_block_hashes"][..],
            &["stacks_tx_hashes", "contract_function"][..],
        ),
        (
            "contract_family",
            &["contract_function", "stacks_tx_hashes"][..],
            &["stacks_block_hashes"][..],
        ),
    ] {
        let forbidden_anyof: Vec<Value> = forbidden
            .iter()
            .map(|f| json!({ "required": [f] }))
            .collect();
        let then_clause = json!({
            "properties": {
                "representative_ids": {
                    "required": required,
                    "not": { "anyOf": forbidden_anyof }
                }
            }
        });
        append_all_of(
            def,
            json!({
                "if": { "properties": { "kind": { "const": kind } }, "required": ["kind"] },
                "then": then_clause,
            }),
        );
    }
}

/// AnalyzerTarget consensus-routing constraints. Five rules total.
fn inject_analyzer_target(def: &mut Value) {
    inject_consensus_routing(def);
}

/// AcceptedAnalysis cross-field invariants:
/// 1. `lens_disposition.lens` MUST equal `selection_lens` (three if/then arms,
///    one per lens value).
/// 2. `lens_disposition.status == "addressed"` requires non-empty `targets`.
///
/// The serde model also enforces these via `validate()`, but the bash
/// pipeline's first gate is the JSON Schema (e.g. merge-analyses.sh
/// validates analyzer output before consuming it).
fn inject_accepted_analysis(def: &mut Value) {
    for lens in ["tx_latency", "tenure_throughput", "commit_time"] {
        append_all_of(
            def,
            json!({
                "if": {
                    "properties": { "selection_lens": { "const": lens } },
                    "required": ["selection_lens"]
                },
                "then": {
                    "properties": {
                        "lens_disposition": {
                            "properties": { "lens": { "const": lens } },
                            "required": ["lens"]
                        }
                    }
                }
            }),
        );
    }

    append_all_of(
        def,
        json!({
            "if": {
                "properties": {
                    "lens_disposition": {
                        "properties": { "status": { "const": "addressed" } },
                        "required": ["status"]
                    }
                },
                "required": ["lens_disposition"]
            },
            "then": { "properties": { "targets": { "minItems": 1 } } }
        }),
    );
}

/// MergedTarget: same five consensus-routing rules + delivery_mode +
/// bench_eligible derivation.
fn inject_merged_target(def: &mut Value) {
    inject_consensus_routing(def);

    // delivery_mode derivation — three branches per consensus_breaking +
    // poc_implementable combination.
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "consensus_breaking": { "const": false } },
                "required": ["consensus_breaking"]
            },
            "then": { "properties": { "delivery_mode": { "const": "normal_pr" } } }
        }),
    );
    append_all_of(
        def,
        json!({
            "if": {
                "properties": {
                    "consensus_breaking": { "const": true },
                    "poc_implementable": { "const": true }
                },
                "required": ["consensus_breaking", "poc_implementable"]
            },
            "then": { "properties": { "delivery_mode": { "const": "consensus_poc_pr" } } }
        }),
    );
    append_all_of(
        def,
        json!({
            "if": {
                "properties": {
                    "consensus_breaking": { "const": true },
                    "poc_implementable": { "const": false }
                },
                "required": ["consensus_breaking", "poc_implementable"]
            },
            "then": { "properties": { "delivery_mode": { "const": "consensus_issue" } } }
        }),
    );

    // bench_eligible derivation — true iff delivery_mode == normal_pr.
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "delivery_mode": { "const": "normal_pr" } },
                "required": ["delivery_mode"]
            },
            "then": { "properties": { "bench_eligible": { "const": true } } },
            "else": { "properties": { "bench_eligible": { "const": false } } }
        }),
    );
}

/// JSON Schema fragment that forbids the value `null`. Used to narrow
/// nullable property schemas in `then` clauses where the field is
/// conditionally required and must not be `null`.
///
/// Why this is needed: schemars emits `Option<T>` as `anyOf [T, null]`. A
/// bare `required` list ensures the *property* is present, but does NOT
/// reject `"foo": null`. Combining `required: [...]` with a property-level
/// `{ "not": { "type": "null" } }` constraint forces the field to take a
/// concrete (non-null) value.
fn not_null() -> Value {
    json!({ "not": { "type": "null" } })
}

/// Five rules shared between AnalyzerTarget and MergedTarget:
/// 1. `consensus_breaking == true` requires non-null `breakage_class`,
///    `poc_implementable`, `consensus_writeup`.
/// 2. `poc_implementable == true` requires non-null, non-empty
///    `poc_test_scope`.
/// 3. `breakage_class == block_validation` forces `poc_implementable: false`.
/// 4. Non-true `poc_implementable` forbids `poc_test_scope`.
/// 5. `consensus_breaking != true` forbids ALL consensus-only fields.
fn inject_consensus_routing(def: &mut Value) {
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "consensus_breaking": { "const": true } },
                "required": ["consensus_breaking"]
            },
            "then": {
                "required": ["breakage_class", "poc_implementable", "consensus_writeup"],
                "properties": {
                    "breakage_class": not_null(),
                    "poc_implementable": not_null(),
                    "consensus_writeup": not_null()
                }
            }
        }),
    );
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "poc_implementable": { "const": true } },
                "required": ["poc_implementable"]
            },
            "then": {
                "required": ["poc_test_scope"],
                "properties": {
                    "poc_test_scope": {
                        "allOf": [
                            { "not": { "type": "null" } },
                            { "minItems": 1 }
                        ]
                    }
                }
            }
        }),
    );
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "breakage_class": { "const": "block_validation" } },
                "required": ["breakage_class"]
            },
            "then": { "properties": { "poc_implementable": { "const": false } } }
        }),
    );
    append_all_of(
        def,
        json!({
            "if": {
                "not": {
                    "properties": { "poc_implementable": { "const": true } },
                    "required": ["poc_implementable"]
                }
            },
            "then": { "not": { "required": ["poc_test_scope"] } }
        }),
    );
    append_all_of(
        def,
        json!({
            "if": {
                "not": {
                    "properties": { "consensus_breaking": { "const": true } },
                    "required": ["consensus_breaking"]
                }
            },
            "then": {
                "allOf": [
                    { "not": { "required": ["breakage_class"] } },
                    { "not": { "required": ["poc_implementable"] } },
                    { "not": { "required": ["poc_test_scope"] } },
                    { "not": { "required": ["consensus_writeup"] } }
                ]
            }
        }),
    );
}

/// LensDispositionEntry: `status == not_actionable` requires non-null
/// `reason`.
fn inject_lens_disposition(def: &mut Value) {
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "status": { "const": "not_actionable" } },
                "required": ["status"]
            },
            "then": {
                "required": ["reason"],
                "properties": { "reason": not_null() }
            }
        }),
    );
}

/// Experiment: `status` enum varies by `delivery_mode`. Three branches.
fn inject_experiment(def: &mut Value) {
    for (mode, statuses) in [
        ("normal_pr", &["accepted", "rejected", "aborted"][..]),
        ("consensus_poc_pr", &["poc_landed", "aborted"][..]),
        ("consensus_issue", &["routed_to_issue", "aborted"][..]),
    ] {
        append_all_of(
            def,
            json!({
                "if": {
                    "properties": { "delivery_mode": { "const": mode } },
                    "required": ["delivery_mode"]
                },
                "then": { "properties": { "status": { "enum": statuses } } }
            }),
        );
    }
}

/// ImplementedReport: mirrors the Rust-side [`ImplementedReport::validate`]
/// invariants in JSON Schema. Six rules:
///
/// 1. `parity.consensus_sensitive == true` requires non-empty `evidence` and
///    `tests` arrays whose items are also non-empty (`minLength: 1` per item —
///    best the schema can do; Rust validator does the trim-aware check).
/// 2. `parity.unproven_risk` must be `null` (Phase 2 territory).
/// 3. `test_summary.failed` must be `0`.
/// 4. Free-text fields (`implementation_summary`, `pr_title`,
///    `test_summary.log_path`) must be `minLength: 1`.
/// 5. `test_summary.duration_secs` must be `>= 0`.
/// 6. `clippy_clean` matrix: `normal_pr` requires `true`, `consensus_poc_pr`
///    unconstrained, `consensus_issue` impossible (the variant never appears on
///    implemented reports).
fn inject_implemented_report(def: &mut Value) {
    // Rule 1: consensus_sensitive=true → parity.evidence + tests must
    // be non-empty AND each item must contain at least one
    // non-whitespace char (mirrors the Rust trim-aware check via
    // `pattern: "\\S"`; `minLength: 1` alone would let `"   "` pass).
    append_all_of(
        def,
        json!({
            "if": {
                "properties": {
                    "parity": {
                        "properties": { "consensus_sensitive": { "const": true } },
                        "required": ["consensus_sensitive"]
                    }
                },
                "required": ["parity"]
            },
            "then": {
                "properties": {
                    "parity": {
                        "properties": {
                            "evidence": {
                                "minItems": 1,
                                "items": { "minLength": 1, "pattern": "\\S" }
                            },
                            "tests": {
                                "minItems": 1,
                                "items": { "minLength": 1, "pattern": "\\S" }
                            }
                        }
                    }
                }
            }
        }),
    );
    // Rule 2: parity.unproven_risk must be null on implemented
    append_all_of(
        def,
        json!({
            "properties": {
                "parity": {
                    "properties": {
                        "unproven_risk": { "type": "null" }
                    }
                }
            }
        }),
    );
    // Rule 3: test_summary.failed == 0
    append_all_of(
        def,
        json!({
            "properties": {
                "test_summary": {
                    "properties": { "failed": { "const": 0 } }
                }
            }
        }),
    );
    // Rule 4: free-text fields non-blank (trim-aware via `pattern: "\\S"`
    // — `minLength: 1` alone would let `"   "` slip through).
    append_all_of(
        def,
        json!({
            "properties": {
                "implementation_summary": { "minLength": 1, "pattern": "\\S" },
                "pr_title": { "minLength": 1, "pattern": "\\S" },
                "test_summary": {
                    "properties": { "log_path": { "minLength": 1, "pattern": "\\S" } }
                }
            }
        }),
    );
    // Rule 5: test_summary.duration_secs >= 0 (finite-ness is intrinsic to JSON)
    append_all_of(
        def,
        json!({
            "properties": {
                "test_summary": {
                    "properties": { "duration_secs": { "minimum": 0 } }
                }
            }
        }),
    );
    // Rule 6: clippy_clean by delivery_mode
    // - normal_pr → must be true
    // - consensus_poc_pr → unconstrained (any value or omission)
    // - consensus_issue → impossible on implemented; force-fail by forbidding the
    //   const value entirely
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "delivery_mode": { "const": "normal_pr" } },
                "required": ["delivery_mode"]
            },
            "then": {
                "required": ["clippy_clean"],
                "properties": { "clippy_clean": { "const": true } }
            }
        }),
    );
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "delivery_mode": { "const": "consensus_issue" } },
                "required": ["delivery_mode"]
            },
            "then": false
        }),
    );
}

/// AbortedReport: mirrors the Rust-side [`AbortedReport::validate`]
/// invariants. Two rules:
///
/// 1. `reason` must be `minLength: 1` (Rust validator does the trim-aware
///    check).
/// 2. `failed_gate == "nextest"` requires non-empty `failing_tests` with each
///    item also non-empty.
fn inject_aborted_report(def: &mut Value) {
    // Rule 1: reason non-blank (trim-aware via `pattern: "\\S"`).
    append_all_of(
        def,
        json!({
            "properties": {
                "reason": { "minLength": 1, "pattern": "\\S" }
            }
        }),
    );
    // Rule 2: failed_gate=nextest → non-empty failing_tests, each
    // entry trim-aware non-blank.
    append_all_of(
        def,
        json!({
            "if": {
                "properties": { "failed_gate": { "const": "nextest" } },
                "required": ["failed_gate"]
            },
            "then": {
                "required": ["failing_tests"],
                "properties": {
                    "failing_tests": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "minLength": 1, "pattern": "\\S" }
                    }
                }
            }
        }),
    );
}
