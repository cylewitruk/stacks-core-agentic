//! Round-trip every fixture artifact through the typed v2 models.
//!
//! Asserts:
//! - the JSON parses cleanly into the typed shape (serde + schemars
//!   `deny_unknown_fields` catches stray fields);
//! - cross-field validators on each top-level model accept the fixture;
//! - re-serializing produces a structurally identical JSON value (the
//!   `serde_json::Value`-level compare ignores whitespace).

use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;
use serde_json::Value;
use stacks_bench_agent::models::ValidateModel;
use stacks_bench_agent::models::analyze::Analysis;
use stacks_bench_agent::models::candidates::Candidates;
use stacks_bench_agent::models::targets::OptimizationTargets;

/// Path to the fixture session results dir.
fn fixture_session() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session"))
}

/// Round-trip helper: parse `path` as `T`, re-serialize, and assert the
/// reparsed JSON equals the originally-parsed JSON.
fn roundtrip<T>(path: &Path) -> T
where
    T: for<'de> serde::Deserialize<'de> + serde::Serialize,
{
    let raw = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let original: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let typed: T = serde_json::from_value(original.clone())
        .unwrap_or_else(|e| panic!("typed parse {}: {e}", path.display()));
    let reparsed: Value = serde_json::to_value(&typed)
        .unwrap_or_else(|e| panic!("reserialize {}: {e}", path.display()));
    assert_eq!(original, reparsed, "round-trip mismatch for {}", path.display());
    typed
}

#[test]
fn candidates_round_trip() {
    let path = fixture_session().join("triage/candidates.json");
    let candidates: Candidates = roundtrip(&path);
    candidates
        .validate_model()
        .expect("candidates validate");
    assert_eq!(candidates.candidates.len(), 3);
}

#[test]
fn optimization_targets_round_trip() {
    let path = fixture_session().join("merge/optimization-targets.json");
    let targets: OptimizationTargets = roundtrip(&path);
    targets
        .validate_model()
        .expect("optimization-targets validate");
    assert_eq!(targets.targets.len(), 3);
    // Coverage on the merged_from references should match the fixture.
    let merged_count: usize = targets
        .targets
        .iter()
        .map(|t| t.merged_from.len())
        .sum();
    assert_eq!(merged_count, 4, "two contributors converged on cache target + two singletons");
}

#[test]
fn analyses_round_trip() {
    let dir = fixture_session().join("analysis");
    let mut count = 0usize;
    for entry in fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry
            .path()
            .join("analysis.json");
        if !path.is_file() {
            continue;
        }
        let analysis: Analysis = roundtrip(&path);
        analysis
            .validate_model()
            .unwrap_or_else(|e| panic!("validate {}: {e}", path.display()));
        count += 1;
    }
    assert_eq!(count, 3);
}

#[test]
fn analyzer_targets_validate_consensus_routing() {
    use stacks_bench_agent::models::common::BreakageClass;
    // Sanity: load throughput-only-family analysis, which has a
    // consensus_breaking + poc_implementable target.
    let path = fixture_session().join("analysis/throughput-only-family/analysis.json");
    let analysis: Analysis = roundtrip(&path);
    let Analysis::Accepted(a) = analysis else { panic!("expected accepted") };
    let t = &a.targets[0];
    assert!(t.consensus_breaking);
    assert_eq!(t.breakage_class, Some(BreakageClass::ClarityCostWeight));
    assert_eq!(t.poc_implementable, Some(true));
    assert!(
        t.poc_test_scope
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    );
}
