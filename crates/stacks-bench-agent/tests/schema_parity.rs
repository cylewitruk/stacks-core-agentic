//! JSON Schema parity test: validate fixtures against the schemars-emitted
//! schemas (proves the LLM-facing constraint surface accepts known-good
//! fixtures), and validate counter-fixtures (mutations of the fixture that
//! should fail) against the same schemas (proves the constraints actually
//! reject malformed inputs).
//!
//! This catches the regression Codex flagged: kebab patterns, length bounds,
//! convergence_count, and the consensus-routing if/then chains all need to
//! be enforced at the JSON Schema level for the bash phases that still
//! validate via the committed `schemas/`.

use std::path::Path;

use jsonschema::Validator;
use serde_json::{Value, json};
use stacks_bench_agent::schema_export::{SchemaEntry, generate_all};

fn fixture(name: &str) -> Value {
    // Map well-known fixture file names through the new phase-namespaced
    // layout so tests can stay terse (`fixture("candidates.json")` etc).
    let rel = match name {
        "candidates.json" => "triage/candidates.json".to_owned(),
        "optimization-targets.json" => "merge/optimization-targets.json".to_owned(),
        other => other.to_owned(),
    };
    let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session")).join(rel);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn schema_for(file_name: &str) -> Validator {
    let entries: Vec<SchemaEntry> = generate_all().expect("generate_all");
    let entry = entries
        .into_iter()
        .find(|e| e.file_name == file_name)
        .unwrap_or_else(|| panic!("no schema named {file_name}"));
    Validator::new(&entry.schema).unwrap_or_else(|e| panic!("compile {file_name}: {e}"))
}

#[test]
fn fixtures_pass_emitted_schemas() {
    let cases = [
        ("candidates.schema.json", "candidates.json"),
        ("optimization-targets.schema.json", "optimization-targets.json"),
    ];
    for (schema_name, fixture_name) in cases {
        let validator = schema_for(schema_name);
        let value = fixture(fixture_name);
        let errs: Vec<_> = validator
            .iter_errors(&value)
            .collect();
        assert!(
            errs.is_empty(),
            "{fixture_name} should validate against {schema_name}; errors:\n{}",
            errs.iter()
                .map(|e| format!("  {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn analyses_pass_emitted_schema() {
    let validator = schema_for("analysis.schema.json");
    let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session/analysis"));
    let mut count = 0usize;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry
            .unwrap()
            .path()
            .join("analysis.json");
        if !path.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        let errs: Vec<_> = validator
            .iter_errors(&value)
            .collect();
        assert!(
            errs.is_empty(),
            "{} should validate; errors:\n{}",
            path.display(),
            errs.iter()
                .map(|e| format!("  {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        count += 1;
    }
    assert_eq!(count, 3);
}

#[test]
fn kebab_pattern_rejects_invalid_id() {
    // Mutate candidates.json to have an UPPERCASE id and assert the schema
    // catches it.
    let validator = schema_for("candidates.schema.json");
    let mut value = fixture("candidates.json");
    value["candidates"][0]["id"] = json!("BadCamelCase");
    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "uppercase id should fail kebab regex");
}

#[test]
fn representative_ids_length_bounds_enforced() {
    // Six tx ids on a tx_family — exceeds the maxItems=5 bound.
    let validator = schema_for("candidates.schema.json");
    let mut value = fixture("candidates.json");
    // Find the tx_family-style entry (synthetic; we only have one in the fixture)
    // and append a 6th id.
    if let Some(arr) = value["candidates"]
        .as_array_mut()
        .and_then(|cs| {
            cs.iter_mut()
                .find(|c| c["kind"] == json!("tx_family"))
        })
        .and_then(|c| c["representative_ids"]["stacks_tx_hashes"].as_array_mut())
    {
        for i in 0..10u32 {
            arr.push(json!(format!("0x{:064x}", 1000 + i)));
        }
    }
    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "representative_ids exceeding maxItems=5 should fail");
}

#[test]
fn convergence_count_minimum_enforced() {
    let validator = schema_for("optimization-targets.schema.json");
    let mut value = fixture("optimization-targets.json");
    value["targets"][0]["convergence_count"] = json!(0);
    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "convergence_count=0 should fail range(min=1)");
}

#[test]
fn consensus_breaking_requires_breakage_class() {
    // Mutate a non-consensus target to flip consensus_breaking=true without
    // adding the required fields. The injected if/then chain should reject.
    let validator = schema_for("optimization-targets.schema.json");
    let mut value = fixture("optimization-targets.json");
    let target = &mut value["targets"][0];
    target["consensus_breaking"] = json!(true);
    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(
        !errs.is_empty(),
        "consensus_breaking=true without breakage_class/poc_implementable/consensus_writeup must \
         fail"
    );
}

#[test]
fn block_validation_forces_poc_implementable_false() {
    // Construct a hypothetical analyzer-target with breakage_class=block_validation
    // + poc_implementable=true, which the injected rule must reject.
    let validator = schema_for("analysis.schema.json");
    let value = json!({
        "schema_version": 4,
        "family_id": "x-fam",
        "status": "accepted",
        "selection_lens": "tx_latency",
        "lens_disposition": { "lens": "tx_latency", "status": "addressed" },
        "targets": [{
            "target_span": "x::y",
            "bucket": "block_processing",
            "fix_signature": "x-fix",
            "hotspot": {
                "span": "x::y", "self_wall_us": 1, "total_wall_us": 1,
                "calls": 1, "location": "x.rs:1"
            },
            "files": ["x.rs"],
            "evidence": "e",
            "proposed_change": "p",
            "expected_improvement": { "tx_latency": 0.0, "tenure_throughput": 0.0, "commit_time": 0.0 },
            "risk": "low",
            "verification_plan": "v",
            "consensus_breaking": true,
            "breakage_class": "block_validation",
            "poc_implementable": true,
            "poc_test_scope": ["package(x)::test::y"],
            "consensus_writeup": "writeup"
        }]
    });
    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(
        !errs.is_empty(),
        "block_validation + poc_implementable=true must fail per injected rule"
    );
}

#[test]
fn consensus_required_fields_reject_null() {
    // schemars emits `Option<T>` as `anyOf [T, null]`. The injected `then`
    // clause must reject `null` for conditionally-required fields, not
    // just verify presence.
    let validator = schema_for("optimization-targets.schema.json");
    let mut value = fixture("optimization-targets.json");

    // Find the consensus_poc_pr target (clarity-cost-recalibration) and null
    // out its breakage_class. Required-list passes (key present); type-narrow
    // must reject null.
    let target = value["targets"]
        .as_array_mut()
        .and_then(|arr| {
            arr.iter_mut()
                .find(|t| t["id"] == json!("clarity-cost-recalibration"))
        })
        .expect("fixture has clarity-cost-recalibration target");
    target["breakage_class"] = json!(null);

    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "consensus_breaking=true with breakage_class=null must be rejected");
}

#[test]
fn poc_test_scope_rejects_null() {
    // poc_implementable=true requires non-null, non-empty poc_test_scope.
    let validator = schema_for("optimization-targets.schema.json");
    let mut value = fixture("optimization-targets.json");
    let target = value["targets"]
        .as_array_mut()
        .and_then(|arr| {
            arr.iter_mut()
                .find(|t| t["id"] == json!("clarity-cost-recalibration"))
        })
        .expect("fixture has clarity-cost-recalibration target");
    target["poc_test_scope"] = json!(null);

    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "poc_implementable=true with poc_test_scope=null must be rejected");
}

#[test]
fn lens_disposition_reason_rejects_null() {
    // status=not_actionable requires non-null reason.
    let validator = schema_for("summary.schema.json");
    let value = json!({
        "schema_version": 4,
        "session_id": "x",
        "baseline_run_id": 1,
        "baseline_rerun_id": 2,
        "noise_floor_pct": 0.5,
        "experiments": [],
        "outcome_counts": {
            "normal_pr": { "accepted": 0, "rejected": 0, "aborted": 0 },
            "consensus_poc_pr": { "poc_landed": 0, "aborted": 0 },
            "consensus_issue": { "routed_to_issue": 0, "aborted": 0 }
        },
        "lens_dispositions": [{
            "family_id": "x-fam",
            "lens": "tx_latency",
            "status": "not_actionable",
            "reason": null
        }]
    });
    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "not_actionable with reason=null must be rejected");
}

/// Build a minimally-valid accepted analysis as a `serde_json::Value` for
/// counter-fixture mutation in the AcceptedAnalysis tests below. Mirrors
/// the fixture analysis shape but skips the consensus-routing fields
/// (target is non-consensus).
fn accepted_analysis_template() -> Value {
    json!({
        "schema_version": 4,
        "family_id": "x-fam",
        "status": "accepted",
        "selection_lens": "tx_latency",
        "lens_disposition": { "lens": "tx_latency", "status": "addressed" },
        "targets": [{
            "target_span": "x::y",
            "bucket": "block_processing",
            "fix_signature": "x-fix",
            "hotspot": {
                "span": "x::y", "self_wall_us": 1, "total_wall_us": 1,
                "calls": 1, "location": "x.rs:1"
            },
            "files": ["x.rs"],
            "evidence": "e",
            "evidence_queries": [{
                "purpose": "prove span movement",
                "sql_path": "queries/span_run_drift.sql",
                "params": { "run_id": "1", "span_name": "x::y" },
                "output_path": "analysis/x-fam/queries/span-run-drift.csv",
                "key_observation": "baseline p95 self_wall_us = 1000",
                "supports_invocations": ["warm-steady"]
            }],
            "proposed_change": "p",
            "expected_improvement": {
                "tx_latency": 1.0, "tenure_throughput": 0.0, "commit_time": 0.0
            },
            "risk": "low",
            "verification_plan": "v",
            "verification_replay": {
                "rationale": "schema-parity baseline template",
                "invocations": [{
                    "id": "warm-steady",
                    "label": "warm",
                    "purpose": "smoke",
                    "samples": {
                        "kind": "blocks",
                        "blocks": ["0xaa00000000000000000000000000000000000000000000000000000000000000"]
                    },
                    "warmup": 10,
                    "repetitions": 20,
                    "profiler": "rich",
                    "expected_signal": {
                        "axis": "tx_latency",
                        "direction": "improves",
                        "estimate_pct": 4.0,
                        "tolerance_pct": 2.0
                    }
                }]
            },
            "consensus_breaking": false
        }]
    })
}

#[test]
fn accepted_analysis_template_validates() {
    // Sanity check: the template itself is schema-valid. Any mutation test
    // below depends on this being the baseline.
    let validator = schema_for("analysis.schema.json");
    let value = accepted_analysis_template();
    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(
        errs.is_empty(),
        "accepted_analysis_template should be valid; errors:\n{}",
        errs.iter()
            .map(|e| format!("  {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn lens_disposition_lens_must_equal_selection_lens() {
    // selection_lens=tx_latency but lens_disposition.lens=commit_time must
    // be rejected by the injected per-lens if/then chain.
    let validator = schema_for("analysis.schema.json");
    let mut value = accepted_analysis_template();
    value["lens_disposition"]["lens"] = json!("commit_time");

    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(
        !errs.is_empty(),
        "selection_lens=tx_latency with lens_disposition.lens=commit_time must fail"
    );
}

#[test]
fn addressed_disposition_requires_non_empty_targets() {
    // status=addressed with targets=[] must be rejected.
    let validator = schema_for("analysis.schema.json");
    let mut value = accepted_analysis_template();
    value["targets"] = json!([]);

    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "lens_disposition.status=addressed with empty targets[] must fail");
}

#[test]
fn lens_disposition_inner_not_actionable_requires_reason() {
    // Inner LensDisposition: status=not_actionable without reason must fail.
    let validator = schema_for("analysis.schema.json");
    let mut value = accepted_analysis_template();
    value["lens_disposition"] = json!({ "lens": "tx_latency", "status": "not_actionable" });
    value["targets"] = json!([]);

    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "inner LensDisposition: not_actionable without reason must fail");
}

#[test]
fn lens_disposition_inner_reason_rejects_null() {
    // Inner LensDisposition: status=not_actionable with reason=null must fail.
    let validator = schema_for("analysis.schema.json");
    let mut value = accepted_analysis_template();
    value["lens_disposition"] = json!({
        "lens": "tx_latency",
        "status": "not_actionable",
        "reason": null
    });
    value["targets"] = json!([]);

    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(!errs.is_empty(), "inner LensDisposition: not_actionable with reason=null must fail");
}

#[test]
fn experiment_status_constrained_by_delivery_mode() {
    // PocLanded is invalid for normal_pr per the injected enum-narrowing.
    let validator = schema_for("summary.schema.json");
    let value = json!({
        "schema_version": 4,
        "session_id": "x",
        "baseline_run_id": 1,
        "baseline_rerun_id": 2,
        "noise_floor_pct": 0.5,
        "experiments": [{
            "target_id": "x",
            "delivery_mode": "normal_pr",
            "status": "poc_landed"
        }],
        "outcome_counts": {
            "normal_pr": { "accepted": 0, "rejected": 0, "aborted": 0 },
            "consensus_poc_pr": { "poc_landed": 0, "aborted": 0 },
            "consensus_issue": { "routed_to_issue": 0, "aborted": 0 }
        },
        "lens_dispositions": []
    });
    let errs: Vec<_> = validator
        .iter_errors(&value)
        .collect();
    assert!(
        !errs.is_empty(),
        "normal_pr + poc_landed must fail status-by-delivery_mode constraint"
    );
}
