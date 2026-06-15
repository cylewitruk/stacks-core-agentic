//! Golden fixtures for the v15 cross-session projection migration.
//!
//! These fixtures pin the behavior that existed before dedup and optimizer
//! memory moved behind `HistoryProjectionV1`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use stacks_bench_agent::models::analyze::AnalyzerTarget;
use stacks_bench_agent::models::common::{Bucket, Hotspot, ImprovementVector, Risk};
use stacks_bench_agent::models::maintain_event::MaintEvent;
use stacks_bench_agent::models::session_record::SessionRecord;
use stacks_bench_agent::session::dedup::DedupProjection;
use stacks_bench_agent::session::optimizer_memory::build_for_families;

fn fixture_root() -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/projection")).to_path_buf()
}

#[test]
fn dedup_golden_decisions_match_projection_fixtures() {
    for name in [
        "open-pr-blocks",
        "stale-no-block",
        "force-push-reblocks",
        "merged-blocks",
        "repeated-failures",
        "open-issue-blocks",
    ] {
        let dir = fixture_root().join(name);
        let sessions = read_sessions(&dir.join("sessions.jsonl"));
        let maintain = read_maintain(&dir.join("maintain.jsonl"));
        let projection = DedupProjection::from_ledgers(&sessions, &maintain, 1);
        let decision = projection.decision_for("family-a", 0, &analyzer_target("fix-a"));
        let actual = decision
            .map(|d| format!("{}|{}|{}", d.fix_signature, d.reason, d.detail))
            .unwrap_or_else(|| "none".to_owned());
        let expected = std::fs::read_to_string(
            fixture_root()
                .join("golden/dedup")
                .join(format!("{name}.txt")),
        )
        .unwrap();
        assert_eq!(actual, expected.trim_end(), "fixture {name}");
    }
}

#[test]
fn optimizer_memory_golden_json_matches_projection_fixture() {
    let dir = fixture_root().join("memory-family-context");
    let sessions = read_sessions(&dir.join("sessions.jsonl"));
    let maintain = read_maintain(&dir.join("maintain.jsonl"));
    let families: BTreeSet<String> = ["family-a"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let memory = build_for_families(
        &families,
        &sessions,
        &maintain,
        Some("source-current".to_owned()),
        "2026-06-15T00:00:00Z".to_owned(),
    );
    let actual = serde_json::to_string_pretty(&memory).unwrap();
    let expected =
        std::fs::read_to_string(fixture_root().join("golden/memory/memory-family-context.json"))
            .unwrap();
    assert_eq!(actual, expected.trim_end());
}

fn read_sessions(path: &Path) -> Vec<SessionRecord> {
    let raw = std::fs::read_to_string(path).unwrap();
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| SessionRecord::from_ledger_line(line).unwrap())
        .collect()
}

fn read_maintain(path: &Path) -> Vec<MaintEvent> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| MaintEvent::from_ledger_line(line).unwrap())
        .collect()
}

fn analyzer_target(fix_signature: &str) -> AnalyzerTarget {
    AnalyzerTarget {
        target_span: "span".to_owned(),
        bucket: Bucket::BlockProcessing,
        fix_signature: fix_signature.to_owned(),
        hotspot: Hotspot {
            span: "span".to_owned(),
            self_wall_us: 1,
            total_wall_us: 2,
            calls: 1,
            location: "src/lib.rs:1".to_owned(),
        },
        files: vec!["src/lib.rs".to_owned()],
        evidence: "evidence".to_owned(),
        evidence_queries: vec![],
        proposed_change: "change".to_owned(),
        expected_improvement: ImprovementVector {
            tx_latency: 1.0,
            tenure_throughput: 0.0,
            commit_time: 0.0,
        },
        risk: Risk::Low,
        verification_plan: "verify".to_owned(),
        verification_replay: None,
        consensus_breaking: false,
        breakage_class: None,
        poc_implementable: None,
        poc_test_scope: None,
        consensus_writeup: None,
    }
}
