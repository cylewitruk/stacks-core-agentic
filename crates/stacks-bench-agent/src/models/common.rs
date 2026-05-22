//! Shared types used across the v2 artifact models.
//!
//! Anything that appears in more than one of the four artifact JSON files
//! lives here: enums (lens, bucket, risk, breakage class, delivery mode),
//! the three-axis improvement vector, the hotspot record, and the lens
//! disposition propagated from analysis through merge into summary.

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;

/// Selection lens — the value-axis triage promoted a candidate on, and that
/// the analyzer must dispose of explicitly. Carries through verbatim into
/// merge output and the summary.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SelectionLens {
    /// Wall-time savings under execution-bucket spans.
    TxLatency,
    /// Clarity-budget headroom freed (deterministic Clarity cost units).
    TenureThroughput,
    /// Wall-time savings under commit-bucket spans.
    CommitTime,
}

/// Family kind discriminator on triage candidates and downstream analyses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FamilyKind {
    /// Family identified by representative transaction shapes.
    TxFamily,
    /// Family identified by representative blocks.
    BlockFamily,
    /// Family identified by a specific contract.function.
    ContractFamily,
}

/// Work-bucket classification for a target. Determined by the nearest
/// `Segment: ...` ancestor of the target span in the trace tree.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    /// Target lives under `Segment: Tx Execution` / `Transaction`.
    BlockProcessing,
    /// Target lives under one of the commit-bucket Segment anchors.
    BlockCommit,
}

/// Subjective risk level on a proposed fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Low-risk: localized, mechanical, easily reverted.
    Low,
    /// Medium-risk: moderate scope, well-tested area.
    Medium,
    /// High-risk: cross-cutting, hard-to-verify, or touches subtle invariants.
    High,
}

/// Classification of a consensus-breaking change. Required when
/// `consensus_breaking == true`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BreakageClass {
    /// Recalibrating a Clarity-VM cost weight (e.g. `costs.toml`).
    ClarityCostWeight,
    /// Changing how the Clarity VM evaluates an opcode or function.
    ClarityVmBehavior,
    /// Block production logic (mempool ordering, tx admission, fee logic).
    /// Exercised by stacks-bench.
    MiningFlow,
    /// Block acceptance / header / signature checks. NOT exercised by
    /// stacks-bench — `poc_implementable` MUST be false.
    BlockValidation,
    /// MARF on-disk storage format change.
    MarfLayout,
    /// Other consensus-relevant serialization changes.
    OnChainFormat,
}

/// Derived routing decision computed by the merge phase from
/// `consensus_breaking` + `poc_implementable`. Drives Phase 5 publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    /// Performance fix — full bench, normal PR.
    NormalPr,
    /// Deliberate consensus change shipped as a PoC PR (scoped tests, no
    /// benchmark, draft PR with safety labels).
    ConsensusPocPr,
    /// Consensus change too large for PoC mode — issue-only.
    ConsensusIssue,
}

impl DeliveryMode {
    /// Derive a `DeliveryMode` from `consensus_breaking` + `poc_implementable`
    /// per the merge schema's if/then chain. `poc_implementable` is ignored
    /// (and irrelevant) when `consensus_breaking == false`.
    pub fn derive(consensus_breaking: bool, poc_implementable: Option<bool>) -> Self {
        match (consensus_breaking, poc_implementable) {
            (false, _) => Self::NormalPr,
            (true, Some(true)) => Self::ConsensusPocPr,
            (true, _) => Self::ConsensusIssue,
        }
    }

    /// True iff this delivery mode is benchmark-eligible (only `NormalPr`).
    pub fn bench_eligible(self) -> bool {
        matches!(self, Self::NormalPr)
    }
}

/// Per-axis honest estimate of a fix's percent reduction on each value lens.
/// All three axes are required; use `0` for axes the fix doesn't move.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImprovementVector {
    /// Percent reduction in execution-bucket wall time.
    pub tx_latency: f64,
    /// Percent reduction in the binding Clarity-cost axis.
    pub tenure_throughput: f64,
    /// Percent reduction in commit-bucket wall time.
    pub commit_time: f64,
}

/// Hotspot record carried on every analyzer-emitted target and merged target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hotspot {
    /// Span name as it appears in trace queries.
    pub span: String,
    /// Exclusive wall time at the target span (microseconds).
    pub self_wall_us: i64,
    /// Inclusive wall time at the target span (self plus subtree, µs).
    pub total_wall_us: i64,
    /// Number of times the span was entered in the run.
    pub calls: i64,
    /// `file.rs:LINE` location string.
    pub location: String,
}

/// Status of a lens disposition: either an analyzer committed at least one
/// target on the lens (`Addressed`) or it confirmed the lens signal is real
/// but structurally unfixable (`NotActionable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LensDispositionStatus {
    /// At least one target in the analysis claims non-trivial impact on the
    /// lens.
    Addressed,
    /// Analyzer drilled in, confirmed the signal, found no structural handle.
    /// Requires a code-level reason in the carrying record.
    NotActionable,
}

/// Per-family lens-disposition entry as it appears in
/// `optimization-targets.json` and `summary.json`. Independent of the
/// targets array — every accepted analysis contributes one entry here, even
/// when it contributes zero targets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LensDispositionEntry {
    /// Identifier of the family this disposition belongs to.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: String,
    /// Lens triage promoted the family on.
    pub lens: SelectionLens,
    /// Disposition status.
    pub status: LensDispositionStatus,
    /// Required when `status == NotActionable`; optional but allowed when
    /// `status == Addressed` (e.g. to note an unintuitive mechanism).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Schema version sentinel: every v2 artifact file carries `schema_version: 2`.
pub const SCHEMA_VERSION_V2: u32 = 2;

/// Schema version sentinel for v1 artifacts. Currently only the
/// `sessions.jsonl` ledger record uses v1 — it's a fresh model added
/// after the v2 in-session contracts settled, so its versioning is
/// independent of the rest.
pub const SCHEMA_VERSION_V1: u32 = 1;

/// Kebab-case identifier regex applied to family ids, target ids, and
/// fix signatures. Matches `^[a-z0-9][a-z0-9-]*$` — same pattern the
/// hand-written v2 schemas enforced.
pub const KEBAB_PATTERN: &str = "^[a-z0-9][a-z0-9-]*$";

/// 0x-prefixed 64-hex-char regex used for stacks transaction hashes and
/// stacks index block hashes. Stable, globally unique, cryptographic —
/// preferred over stacks-bench DB synthetic integer ids (which are
/// data-dir-local and change on re-index) anywhere an identifier lives in
/// a long-lived artifact (`candidates.json`, `analysis.json`,
/// `optimization-targets.json`, events).
pub const HEX_HASH_PATTERN: &str = "^0x[0-9a-fA-F]{64}$";

/// Per-target replay recipe — the analyzer's verification plan rendered as
/// machine-readable inputs for stacks-bench's targeted-replay modes
/// (`--txid`/`--block` with `--repetitions`). Optional: absence keeps the
/// current full-range fallback in `bench_experiments.rs`.
///
/// `txids` and `blocks` are mutually inclusive: an analyzer may emit
/// either, both, or neither. When both are present, the bench coordinator
/// runs separate replay phases (txid mode first, block mode second)
/// against the same per-target binary; the per-phase results land in one
/// `bench-run.json` per target.
///
/// All identifier values are 0x-prefixed 64-hex-char hashes
/// ([`HEX_HASH_PATTERN`]). Heights and synthetic ids are NEVER acceptable
/// here — see `feature-requests-stacks-bench.md` and the autonomous
/// roadmap's Layer 1A design notes for the rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationReplay {
    /// Representative transaction hashes the analyzer wants re-run.
    /// Picked from drilldown queries via the `tx_hash` (a.k.a.
    /// `stacks_tx.tx_hash_hex`) column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 16))]
    #[schemars(inner(regex(pattern = HEX_HASH_PATTERN)))]
    pub txids: Option<Vec<String>>,
    /// Representative stacks index block hashes the analyzer wants
    /// re-run. Picked from drilldown queries via the `stacks_block_hash`
    /// (a.k.a. `stacks_block.block_hash_hex`) column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 16))]
    #[schemars(inner(regex(pattern = HEX_HASH_PATTERN)))]
    pub blocks: Option<Vec<String>>,
    /// Number of measured repetitions per replay target. Mapped to
    /// `--repetitions` on stacks-bench `bench run`. The full bench
    /// substitutes per-target replay budget for full-range wall-time.
    #[schemars(range(min = 1, max = 200))]
    pub repetitions: u32,
    /// Number of warmup repetitions discarded before measurement
    /// begins. Mapped to `--warmup` on stacks-bench `bench run`.
    /// Cold-fork single-tx / single-block replay has empty caches
    /// (MARF node cache, SQLite page cache, allocator state); 5-10
    /// warmup reps lets caches settle into a representative-ish state
    /// before the measured reps run. Defaults to `10` when omitted;
    /// recipes can override (e.g. set lower for very expensive blocks
    /// where each rep is already minutes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0, max = 200))]
    pub warmup: Option<u32>,
    /// One-line explanation of why the analyzer picked txids vs blocks
    /// vs both (e.g. "block-context: seal-path change needs full block
    /// commit"). Surfaced in summaries; non-actionable for the
    /// coordinator.
    pub rationale: String,
}

impl ValidateModel for VerificationReplay {
    /// Cross-field validation: at least one of `txids` / `blocks` must
    /// be present and non-empty when this recipe is emitted at all (the
    /// recipe is itself optional in carrying structs; absence is the
    /// "fall back to full-range bench" signal); `repetitions` must be in
    /// `1..=200`; `warmup` (when set) must be in `0..=200`. The bounds
    /// mirror the JSON-Schema `range` annotations but are enforced
    /// Rust-side too because the loader doesn't run JSON Schema
    /// validation on load — without these checks a hand-staged recipe
    /// with `repetitions: 99999` would flow to stacks-bench unchecked.
    fn validate_model(&self) -> Result<()> {
        let any_txid = self
            .txids
            .as_ref()
            .is_some_and(|v| !v.is_empty());
        let any_block = self
            .blocks
            .as_ref()
            .is_some_and(|v| !v.is_empty());
        if !any_txid && !any_block {
            bail!(
                "verification_replay: at least one of `txids` / `blocks` must be non-empty (omit \
                 the field entirely to fall back to full-range bench)"
            );
        }
        if !(1..=200).contains(&self.repetitions) {
            bail!(
                "verification_replay.repetitions = {} is out of range [1, 200]",
                self.repetitions
            );
        }
        if let Some(w) = self.warmup
            && w > 200
        {
            bail!("verification_replay.warmup = {w} is out of range [0, 200]");
        }
        Ok(())
    }
}

/// Phantom type that serializes / deserializes as the integer literal `2`
/// and emits `{"const": 2}` in the JSON Schema. Carrying this on each
/// top-level artifact struct forces the wire to declare its version
/// explicitly; an artifact written with the wrong literal fails to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersionV2;

impl SchemaVersionV2 {
    /// Return the underlying integer constant (always `2`).
    pub const fn get(self) -> u32 {
        SCHEMA_VERSION_V2
    }
}

impl Default for SchemaVersionV2 {
    fn default() -> Self {
        Self
    }
}

impl Serialize for SchemaVersionV2 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(SCHEMA_VERSION_V2)
    }
}

impl<'de> Deserialize<'de> for SchemaVersionV2 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = u32::deserialize(d)?;
        if v == SCHEMA_VERSION_V2 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "expected schema_version={SCHEMA_VERSION_V2}, got {v}"
            )))
        }
    }
}

impl JsonSchema for SchemaVersionV2 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("SchemaVersionV2")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "const": SCHEMA_VERSION_V2
        })
    }
}

/// V1 counterpart to [`SchemaVersionV2`]. Serializes as the integer
/// literal `1`; emits `{"const": 1}` in JSON Schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersionV1;

impl SchemaVersionV1 {
    /// Return the underlying integer constant (always `1`).
    pub const fn get(self) -> u32 {
        SCHEMA_VERSION_V1
    }
}

impl Default for SchemaVersionV1 {
    fn default() -> Self {
        Self
    }
}

impl Serialize for SchemaVersionV1 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(SCHEMA_VERSION_V1)
    }
}

impl<'de> Deserialize<'de> for SchemaVersionV1 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = u32::deserialize(d)?;
        if v == SCHEMA_VERSION_V1 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "expected schema_version={SCHEMA_VERSION_V1}, got {v}"
            )))
        }
    }
}

impl JsonSchema for SchemaVersionV1 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("SchemaVersionV1")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "const": SCHEMA_VERSION_V1
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vr_template() -> VerificationReplay {
        VerificationReplay {
            txids: Some(vec!["0x".to_owned() + &"a".repeat(64)]),
            blocks: None,
            repetitions: 10,
            warmup: Some(10),
            rationale: "test".into(),
        }
    }

    #[test]
    fn verification_replay_valid_baseline() {
        vr_template()
            .validate_model()
            .expect("baseline recipe is valid");
    }

    #[test]
    fn verification_replay_rejects_both_empty() {
        let vr = VerificationReplay {
            txids: None,
            blocks: None,
            ..vr_template()
        };
        let e = vr
            .validate_model()
            .expect_err("must reject");
        assert!(
            e.to_string()
                .contains("at least one of `txids` / `blocks`"),
            "{e}"
        );
    }

    #[test]
    fn verification_replay_rejects_repetitions_zero() {
        let vr = VerificationReplay {
            repetitions: 0,
            ..vr_template()
        };
        let e = vr
            .validate_model()
            .expect_err("must reject");
        assert!(
            e.to_string()
                .contains("repetitions"),
            "{e}"
        );
    }

    #[test]
    fn verification_replay_rejects_repetitions_over_200() {
        let vr = VerificationReplay {
            repetitions: 201,
            ..vr_template()
        };
        let e = vr
            .validate_model()
            .expect_err("must reject");
        assert!(
            e.to_string()
                .contains("repetitions"),
            "{e}"
        );
    }

    #[test]
    fn verification_replay_rejects_warmup_over_200() {
        let vr = VerificationReplay {
            warmup: Some(201),
            ..vr_template()
        };
        let e = vr
            .validate_model()
            .expect_err("must reject");
        assert!(
            e.to_string()
                .contains("warmup"),
            "{e}"
        );
    }

    #[test]
    fn verification_replay_accepts_warmup_zero() {
        let vr = VerificationReplay {
            warmup: Some(0),
            ..vr_template()
        };
        vr.validate_model()
            .expect("warmup = 0 is valid");
    }
}
