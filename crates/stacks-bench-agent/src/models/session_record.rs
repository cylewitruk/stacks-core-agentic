//! Typed shape of one line in `sessions.jsonl` — the operator repo's
//! append-only ledger of completed bot sessions. Each line is a
//! *terminal session index record* (not a snapshot): emitted once at
//! session-archive time and never edited. Full per-session evidence
//! lives on the corresponding write-once `session/<id>` branch, linked
//! from [`SessionRecord::artifact_branch`] / [`SessionRecord::artifact_url`].
//!
//! See `docs/session-archive.md` for the why and the write-once contract.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;
use crate::models::common::{DeliveryMode, KEBAB_PATTERN, SchemaVersionV3};

/// Discriminator for ledger lines. Single variant today; left open so
/// future event kinds (e.g. post-completion observations from a future
/// `sbagent maintain`) can live in a sibling `maintain.jsonl` with the
/// same enum shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecordKind {
    /// A bot session reached its terminal state (succeeded / failed /
    /// aborted) and produced an archive branch.
    SessionCompleted,
}

/// Terminal status of a session as a whole. Independent of per-target
/// statuses — a session can `succeed` with all targets rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Pipeline ran to completion. Per-target outcomes recorded in
    /// [`SessionRecord::targets`].
    Succeeded,
    /// A phase failed and the session was aborted by the framework.
    /// [`SessionRecord::failure_phase`] + [`SessionRecord::failure_reason`]
    /// are populated.
    Failed,
    /// Operator (or a hard preflight) aborted the session before it
    /// could complete a phase. Distinct from `failed`: not a framework
    /// bug, just an operator-driven stop.
    Aborted,
}

/// Terminal status of a single target inside the session. Wider than
/// [`crate::models::summary::ExperimentStatus`] because the ledger
/// captures targets that never reached benchmarking (i.e. they got
/// aborted at the optimizer or merge stage and never produced a
/// summary row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetStatus {
    /// Bench measured a real improvement (NormalPr) or the PoC PR
    /// landed (ConsensusPocPr) or the issue routing marker was
    /// produced (ConsensusIssue).
    Accepted,
    /// NormalPr: bench did not show improvement above the noise
    /// floor. Not applicable to consensus modes.
    Rejected,
    /// Target was dropped before producing a publishable artifact
    /// (e.g. `optimizer-report.json` declared `outcome: "aborted"`, or
    /// the merge phase pruned it). [`TargetRecord::status_stage`]
    /// names where.
    Aborted,
    /// Pipeline error stopped this target — distinct from aborted in
    /// that the framework (not the agent) decided the target couldn't
    /// proceed.
    Failed,
}

/// Stage where a non-accepted target's status was settled. `None` on
/// accepted targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetStatusStage {
    /// Optimizer agent declared abort (in `optimizer-report.json`).
    Optimizer,
    /// Merge phase pruned the target (e.g. all proposed changes
    /// rejected as not actionable).
    Merge,
    /// Reached bench but `bench-run.json` reported `success=false` or
    /// the candidate did not pass the noise floor.
    Bench,
}

/// One ledger line. Append-only; once written, never edited.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    /// Discriminator — currently always `session_completed`.
    pub kind: SessionRecordKind,
    /// Constant: 3. The previous v2 write path hardcoded
    /// `stacks_core_base_sha` to `None` once source provenance moved
    /// to the four `source_*` fields below, so v3 drops the dead
    /// column. Records on disk may also carry `2` (carry
    /// `stacks_core_base_sha`) or `1` (no source fields at all);
    /// [`SessionRecord::from_ledger_line`] is the backwards-compat
    /// reader for all three.
    pub schema_version: SchemaVersionV3,
    /// Session id (e.g. `20260518-190321-nextest-flags-smoke`).
    pub id: String,
    /// Name of the write-once branch holding the full bulk:
    /// `session/<id>`.
    pub artifact_branch: String,
    /// Commit sha at the tip of [`SessionRecord::artifact_branch`] at
    /// archive time. Permanent audit anchor — combined with the
    /// branch being write-once, this lets a reader verify integrity
    /// long after the fact.
    pub artifact_sha: String,
    /// Browsable URL to the archive branch tree on the remote host.
    /// `None` when the operator repo has no GitHub-style remote
    /// configured (purely local archive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_url: Option<String>,
    /// ISO 8601 UTC timestamp of session start.
    pub started_at: String,
    /// ISO 8601 UTC timestamp of session end. For aborted/failed
    /// sessions, the time the framework recorded the terminal state.
    pub finished_at: String,
    /// Session-level outcome.
    pub status: SessionStatus,
    /// Phase that failed, for non-success outcomes.
    /// E.g. `"baseline"`, `"triage"`, `"optimize"`, `"bench"`.
    /// `None` on `succeeded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_phase: Option<String>,
    /// Short stable reason code or one-line explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// `Cargo.toml` version of the sbagent binary that produced this
    /// session.
    pub sbagent_version: String,
    /// HEAD sha of the sbagent workspace at build time. `None` when
    /// the binary was built outside a git checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sbagent_git_sha: Option<String>,
    /// Range parameters passed to the baseline + every bench run in
    /// this session. Captured here so the ledger can answer "did this
    /// session test enough blocks?" without traversing the archive
    /// branch.
    pub range: SessionRange,
    /// Run ids written by the baseline phase (typically a single
    /// `[run_id]`, plus a rerun for noise floor).
    pub baseline_run_ids: Vec<i64>,
    /// Wallclock seconds per phase. Keys: `baseline`, `triage`,
    /// `analysis`, `merge`, `optimize`, `bench`, `finalize`,
    /// `publish`. Values are `f64` so sub-second phases (validate,
    /// schema-emit) round-trip cleanly. Stored in a `BTreeMap` so the
    /// serialized JSON is deterministic.
    pub phase_durations_secs: BTreeMap<String, f64>,
    /// One row per target the session touched. Ordered by target id.
    pub targets: Vec<TargetRecord>,
    /// Source-provenance fields copied from
    /// `<session>/results/source.json` at archive time. Populated
    /// when `source.json` exists in the session bulk; absent on
    /// legacy archives that predate the source.json contract
    /// (notably v1 records, but also any v2/v3 archive whose
    /// session didn't materialize source.json).
    /// [`SessionRecord::from_ledger_line`] promotes records without these
    /// fields to `None` for compatibility. See
    /// [`crate::models::source::SourceJson`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// See [`SessionRecord::source_url`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_branch: Option<String>,
    /// See [`SessionRecord::source_url`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha: Option<String>,
    /// See [`SessionRecord::source_url`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fetched_at: Option<String>,
}

impl SessionRecord {
    /// Read one line of `sessions.jsonl` in a backwards-compatible
    /// way: accept legacy v1 records (no `source_*` fields), v2
    /// records (have `stacks_core_base_sha`), and current v3 records
    /// (drop `stacks_core_base_sha`). All three are promoted in-place
    /// to the v3 in-memory shape — v1 records gain `None` source
    /// fields, v2 records have their `stacks_core_base_sha` field
    /// stripped (the SHA is preserved in the v2 raw JSON on disk;
    /// the typed in-memory shape just doesn't carry it).
    ///
    /// Use this on every `sessions.jsonl` read site to avoid silently
    /// skipping legacy records (the unparseable-line fall-through in
    /// callers like `ledger_contains_id` and `read_archived_ids`).
    pub fn from_ledger_line(line: &str) -> Result<Self> {
        let mut value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("ledger line is not valid JSON: {e}"))?;
        let v = value
            .get("schema_version")
            .and_then(|s| s.as_u64())
            .unwrap_or(1);
        match v {
            1 | 2 => {
                // Bump in-place so the v3 deserializer accepts the
                // shape. v1 records have no `source_*` fields; the
                // v3 struct's `#[serde(default)]` on those Options
                // fills them in as `None`. v2 records carry
                // `stacks_core_base_sha`; strip it here so v3's
                // `deny_unknown_fields` doesn't reject the line.
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("schema_version".to_owned(), serde_json::json!(3));
                    obj.remove("stacks_core_base_sha");
                }
                let rec: Self = serde_json::from_value(value)
                    .map_err(|e| anyhow::anyhow!("v{v} ledger line failed v3 promotion: {e}"))?;
                Ok(rec)
            }
            3 => {
                let rec: Self = serde_json::from_value(value)
                    .map_err(|e| anyhow::anyhow!("v3 ledger line: {e}"))?;
                Ok(rec)
            }
            other => {
                bail!("unsupported sessions.jsonl schema_version {other}; expected 1, 2, or 3");
            }
        }
    }
}

/// Range arguments passed to `stacks-bench bench run` (baseline + every
/// per-target run). Mirrors [`crate::session::bench_experiments::BenchRange`]
/// without the borrow-of-settings concerns.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionRange {
    /// Block index to start at. `None` when every target used
    /// targeted-replay mode (no full-range fallback path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<u64>,
    /// Number of blocks to measure. Same `None` semantics as
    /// [`SessionRange::start_at`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Warmup blocks (or `None` to defer to stacks-bench's default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warmup: Option<u64>,
    /// `--filter` argument, `None` when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Stacks network (`mainnet` / `testnet` / `mocknet`). Resolved
    /// against settings at session start.
    pub network: String,
}

/// One target row inside a session record.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetRecord {
    /// Canonical kebab-case target id.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub id: String,
    /// Family that fed this target (i.e. the analyzer that proposed
    /// the change).
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: String,
    /// Coarse bucket (e.g. `block_processing`, `tx_latency`). Free
    /// string here — typed as enum elsewhere
    /// ([`crate::models::common::Bucket`]) but writing as string keeps
    /// the ledger forward-compatible with future buckets.
    pub bucket: String,
    /// How this target was delivered (or would have been, on non-
    /// accept).
    pub delivery_mode: DeliveryMode,
    /// Terminal status. See [`TargetStatus`] for the four legal
    /// values; cross-checked against [`TargetRecord::status_stage`]
    /// in [`TargetRecord::validate_model`].
    pub status: TargetStatus,
    /// Stage at which a non-accepted target was settled. `None` iff
    /// `status == accepted`. Distinguishes "optimizer agent gave up"
    /// from "merge pruned" from "bench measured no win".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_stage: Option<TargetStatusStage>,
    /// Short stable enum code (e.g. `no_signal`, `consensus_break`,
    /// `noise_floor`, `merge_conflict`). Free-form today; tighten to
    /// an enum once stable codes settle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    /// Head commit sha of the per-target branch (i.e. the PR head).
    /// `None` on non-accepted targets that never produced a branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// URL of the opened PR. `None` until publish pushes it; remains
    /// `None` for `consensus_issue` mode and for `--dry-run` archive
    /// flows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// URL of the opened issue. `None` outside `consensus_issue`
    /// mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<String>,
    /// Bench measurements, when this target reached the bench phase.
    /// `None` for `consensus_issue` mode (no bench by design),
    /// `consensus_poc_pr` targets that landed on tests alone, and any
    /// target aborted before bench.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bench: Option<TargetBench>,
}

impl ValidateModel for TargetRecord {
    /// Cross-field check: `status_stage` must be `None` iff `status`
    /// is `accepted`. Run before appending the record so a malformed
    /// row never reaches the ledger.
    fn validate_model(&self) -> Result<()> {
        match (self.status, &self.status_stage) {
            (TargetStatus::Accepted, None) => Ok(()),
            (TargetStatus::Accepted, Some(_)) => {
                bail!("target `{}`: status_stage must be None when status=accepted", self.id)
            }
            (_, None) => {
                bail!("target `{}`: status_stage required when status={:?}", self.id, self.status)
            }
            (_, Some(_)) => Ok(()),
        }
    }
}

/// Per-target bench fields. Materialized only when bench actually
/// ran for the target.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetBench {
    /// Baseline run ids used for the comparison (the session's
    /// baseline + rerun typically).
    pub baseline_run_ids: Vec<i64>,
    /// Run ids produced by this target's bench invocation.
    pub candidate_run_ids: Vec<i64>,
    /// Sum of `total_duration_us` over the baseline runs.
    pub baseline_total_us: i64,
    /// Sum of `total_duration_us` over the candidate runs.
    pub candidate_total_us: i64,
    /// Signed percent improvement (positive = faster). Same metric
    /// summary.md reports.
    pub improvement_pct: f64,
    /// Whether [`TargetBench::improvement_pct`] cleared the session's
    /// noise floor.
    pub passes_noise_floor: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_target(
        id: &str,
        status: TargetStatus,
        stage: Option<TargetStatusStage>,
    ) -> TargetRecord {
        TargetRecord {
            id: id.to_owned(),
            family_id: "fam-1".to_owned(),
            bucket: "tx_latency".to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            status,
            status_stage: stage,
            reason_code: None,
            head_sha: None,
            pr_url: None,
            issue_url: None,
            bench: None,
        }
    }

    #[test]
    fn accepted_target_rejects_status_stage() {
        let t = make_target("a", TargetStatus::Accepted, Some(TargetStatusStage::Bench));
        assert!(t.validate_model().is_err());
    }

    #[test]
    fn non_accepted_target_requires_status_stage() {
        let t = make_target("a", TargetStatus::Rejected, None);
        assert!(t.validate_model().is_err());
    }

    #[test]
    fn valid_target_combinations_pass() {
        assert!(
            make_target("a", TargetStatus::Accepted, None)
                .validate_model()
                .is_ok()
        );
        assert!(
            make_target("a", TargetStatus::Rejected, Some(TargetStatusStage::Bench))
                .validate_model()
                .is_ok()
        );
        assert!(
            make_target("a", TargetStatus::Aborted, Some(TargetStatusStage::Optimizer))
                .validate_model()
                .is_ok()
        );
        assert!(
            make_target("a", TargetStatus::Failed, Some(TargetStatusStage::Merge))
                .validate_model()
                .is_ok()
        );
    }

    /// Records must round-trip through JSON without losing fields.
    /// schemars/serde will reject unknown fields on read, so a missed
    /// `#[serde(skip_serializing_if = "Option::is_none")]` would show
    /// up as a `null` literal here and break the round trip on a
    /// reader that ever evolves the schema.
    #[test]
    fn record_round_trips_through_json() {
        let r = SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: SchemaVersionV3,
            id: "20260518-190321-test".to_owned(),
            artifact_branch: "session/20260518-190321-test".to_owned(),
            artifact_sha: "deadbeef".repeat(5),
            artifact_url: Some(
                "https://github.com/x/y/tree/session/20260518-190321-test".to_owned(),
            ),
            started_at: "2026-05-18T19:03:21Z".to_owned(),
            finished_at: "2026-05-18T19:11:42Z".to_owned(),
            status: SessionStatus::Succeeded,
            failure_phase: None,
            failure_reason: None,
            sbagent_version: "0.1.0".to_owned(),
            sbagent_git_sha: Some("abcdef".repeat(7)),
            range: SessionRange {
                start_at: Some(5_003_180),
                count: Some(100),
                warmup: Some(50),
                filter: None,
                network: "mainnet".to_owned(),
            },
            baseline_run_ids: vec![1, 2],
            phase_durations_secs: BTreeMap::from([
                ("baseline".to_owned(), 295.6),
                ("optimize".to_owned(), 1200.0),
            ]),
            targets: vec![make_target("ex-target", TargetStatus::Accepted, None)],
            source_url: None,
            source_branch: None,
            source_sha: None,
            source_fetched_at: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: SessionRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, r.id);
        assert_eq!(back.artifact_sha, r.artifact_sha);
        assert_eq!(back.targets.len(), 1);
        assert_eq!(back.targets[0].id, "ex-target");
        // None fields round-trip as absent, not null.
        assert!(!s.contains("\"failure_phase\""));
        assert!(!s.contains("\"failure_reason\""));
        assert!(!s.contains("\"source_url\""));
    }

    /// Backwards-compat reader accepts every supported schema
    /// version (v1 legacy / v2 / v3 current). This case covers v1
    /// (no source fields); v2 and v3 have their own tests below.
    #[test]
    fn from_ledger_line_accepts_legacy_v1_records() {
        // Hand-rolled v1 line (no source_* fields, schema_version=1).
        let v1 = r#"{
            "kind":"session_completed","schema_version":1,
            "id":"20260518-190321-legacy",
            "artifact_branch":"session/20260518-190321-legacy",
            "artifact_sha":"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            "started_at":"2026-05-18T19:03:21Z",
            "finished_at":"2026-05-18T19:11:42Z",
            "status":"succeeded",
            "sbagent_version":"0.1.0",
            "range":{"network":"mainnet"},
            "baseline_run_ids":[],
            "phase_durations_secs":{},
            "targets":[]
        }"#;
        let rec = SessionRecord::from_ledger_line(v1).expect("v1 line should promote to v2");
        assert_eq!(rec.id, "20260518-190321-legacy");
        // v1 records get all source_* fields as None.
        assert!(rec.source_url.is_none());
        assert!(rec.source_branch.is_none());
        assert!(rec.source_sha.is_none());
        assert!(
            rec.source_fetched_at
                .is_none()
        );
        // schema_version is now V3 in-memory (the on-disk record stays
        // v1; `from_ledger_line` doesn't mutate the file).
        assert_eq!(rec.schema_version.get(), 3);
    }

    /// v2 records carry `stacks_core_base_sha`; `from_ledger_line`
    /// strips it during the v2 → v3 promotion so the v3 typed shape
    /// (with `deny_unknown_fields`) accepts the line. The legacy
    /// SHA stays in the on-disk record's raw JSON for archive
    /// readers that want it.
    #[test]
    fn from_ledger_line_accepts_v2_records_and_strips_legacy_stacks_core_base_sha() {
        let v2 = r#"{
            "kind":"session_completed","schema_version":2,
            "id":"20260607-104400",
            "artifact_branch":"session/20260607-104400",
            "artifact_sha":"abcabcabcabcabcabcabcabcabcabcabcabcabca",
            "started_at":"2026-06-07T10:44:00Z",
            "finished_at":"2026-06-07T11:00:00Z",
            "status":"succeeded",
            "sbagent_version":"0.2.0",
            "stacks_core_base_sha":"f4cab0a011e273e1f1f9b5249afee2fba6123da5",
            "range":{"network":"mainnet"},
            "baseline_run_ids":[42],
            "phase_durations_secs":{"baseline":300.0},
            "targets":[],
            "source_url":"https://github.com/stacks-network/stacks-core.git",
            "source_branch":"feat/stacks-bench",
            "source_sha":"0ad33704c259da4102b5f195617760003ac89c18",
            "source_fetched_at":"2026-06-07T10:43:59Z"
        }"#;
        let rec = SessionRecord::from_ledger_line(v2).expect("v2 line should promote to v3");
        assert_eq!(rec.id, "20260607-104400");
        assert_eq!(rec.schema_version.get(), 3);
        assert_eq!(
            rec.source_url.as_deref(),
            Some("https://github.com/stacks-network/stacks-core.git"),
        );
        assert_eq!(rec.source_branch.as_deref(), Some("feat/stacks-bench"));
        assert_eq!(rec.source_sha.as_deref(), Some("0ad33704c259da4102b5f195617760003ac89c18"),);
    }

    #[test]
    fn from_ledger_line_accepts_native_v3_records() {
        let v3 = r#"{
            "kind":"session_completed","schema_version":3,
            "id":"20260609-120000",
            "artifact_branch":"session/20260609-120000",
            "artifact_sha":"cafebabecafebabecafebabecafebabecafebabe",
            "started_at":"2026-06-09T12:00:00Z",
            "finished_at":"2026-06-09T12:15:00Z",
            "status":"succeeded",
            "sbagent_version":"0.3.0",
            "range":{"network":"mainnet"},
            "baseline_run_ids":[100],
            "phase_durations_secs":{"baseline":280.0},
            "targets":[],
            "source_url":"https://github.com/cylewitruk/stacks-core.git",
            "source_branch":"feat/stacks-bench",
            "source_sha":"1234567890abcdef1234567890abcdef12345678",
            "source_fetched_at":"2026-06-09T11:59:59Z"
        }"#;
        let rec = SessionRecord::from_ledger_line(v3).expect("v3 line should parse");
        assert_eq!(rec.id, "20260609-120000");
        assert_eq!(rec.schema_version.get(), 3);
        assert_eq!(rec.source_branch.as_deref(), Some("feat/stacks-bench"));
    }

    /// A v3 record carrying `stacks_core_base_sha` is rejected — the
    /// field was deliberately removed and `deny_unknown_fields`
    /// catches that anyone writing v3 with the legacy field present
    /// is confused about which schema they're on.
    #[test]
    fn from_ledger_line_rejects_v3_with_legacy_stacks_core_base_sha() {
        let bad = r#"{
            "kind":"session_completed","schema_version":3,
            "id":"x","artifact_branch":"x","artifact_sha":"x",
            "started_at":"x","finished_at":"x","status":"succeeded",
            "sbagent_version":"x","range":{"network":"mainnet"},
            "baseline_run_ids":[],"phase_durations_secs":{},"targets":[],
            "stacks_core_base_sha":"f4cab0a011"
        }"#;
        let err = SessionRecord::from_ledger_line(bad).unwrap_err();
        assert!(
            format!("{err:#}").contains("v3 ledger line"),
            "expected v3-deserializer rejection: {err:#}",
        );
    }

    #[test]
    fn from_ledger_line_rejects_unsupported_schema_version() {
        let v99 = r#"{
            "kind":"session_completed","schema_version":99,
            "id":"x","artifact_branch":"x","artifact_sha":"x",
            "started_at":"x","finished_at":"x","status":"succeeded",
            "sbagent_version":"x","range":{"network":"mainnet"},
            "baseline_run_ids":[],"phase_durations_secs":{},"targets":[]
        }"#;
        let err = SessionRecord::from_ledger_line(v99).unwrap_err();
        assert!(
            format!("{err:#}").contains("unsupported sessions.jsonl schema_version 99"),
            "{err:#}",
        );
    }

    #[test]
    fn from_ledger_line_rejects_garbage() {
        let err = SessionRecord::from_ledger_line("not json").unwrap_err();
        assert!(format!("{err:#}").contains("not valid JSON"), "{err:#}");
    }
}
