//! `candidates.json` — output of the triage agent.
//!
//! Schema authority lives in this typed model; the committed
//! `schemas/candidates.schema.json` is regenerated from these types via
//! `sbagent schema export`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::{
    Bucket, FamilyKind, HEX_HASH_PATTERN, KEBAB_PATTERN, SchemaVersionV2, SelectionLens,
};

/// Top-level shape of `candidates.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidates {
    /// Constant: 2.
    pub schema_version: SchemaVersionV2,
    /// Session this artifact belongs to.
    pub session_id: String,
    /// Baseline run id (matches `<results>/baseline-run-id`).
    pub baseline_run_id: i64,
    /// Baseline rerun id (matches `<results>/baseline-rerun-id`).
    pub baseline_rerun_id: i64,
    /// Per-host noise floor expressed as a percentage.
    pub noise_floor_pct: f64,
    /// One entry per identified workload-entry family.
    pub candidates: Vec<Candidate>,
}

impl Candidates {
    /// Cross-field validation: every candidate's `representative_ids` shape
    /// matches its declared `kind`. Serde already enforces required fields
    /// + `deny_unknown_fields` per variant.
    pub fn validate(&self) -> Result<(), String> {
        for c in &self.candidates {
            if c.id.is_empty() {
                return Err("candidate id is empty".to_owned());
            }
            c.validate_kind_consistency()?;
        }
        Ok(())
    }
}

/// One workload-entry family.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// Stable kebab-case family id; used as a path segment downstream.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub id: String,
    /// Family kind.
    pub kind: FamilyKind,
    /// Lens triage promoted this family on.
    pub selection_lens: SelectionLens,
    /// Representative workload entry points the analyzer should inspect.
    /// Shape is constrained by [`kind`](Self::kind).
    pub representative_ids: RepresentativeIds,
    /// One-line motivation.
    pub rationale: String,
    /// Optional bucket hint; analyzer commits the authoritative bucket per
    /// emitted target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<Bucket>,
    /// Optional non-binding suspected hot-span hints for the analyzer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspected_spans: Option<Vec<String>>,
    /// Optional aggregate cost signal across the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_materiality: Option<GlobalMateriality>,
}

impl Candidate {
    /// Cross-field check: `representative_ids` shape must match `kind`.
    fn validate_kind_consistency(&self) -> Result<(), String> {
        match (self.kind, &self.representative_ids) {
            (FamilyKind::TxFamily, RepresentativeIds::Tx { .. })
            | (FamilyKind::BlockFamily, RepresentativeIds::Block { .. })
            | (FamilyKind::ContractFamily, RepresentativeIds::Contract { .. }) => Ok(()),
            (kind, ids) => Err(format!(
                "candidate `{}`: kind={:?} does not match representative_ids variant {:?}",
                self.id,
                kind,
                ids.discriminator()
            )),
        }
    }
}

/// Representative workload entry points. Shape varies by family kind; the
/// untagged enum deserializes by required-field discrimination.
///
/// All identifier values are 0x-prefixed 64-hex-char hashes
/// ([`HEX_HASH_PATTERN`](crate::models::common::HEX_HASH_PATTERN)) — the
/// triage agent picks them from query output via the `tx_hash` / dim
/// `stacks_block_hash` columns, never the stacks-bench-DB synthetic
/// integer ids (which are data-dir-local).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum RepresentativeIds {
    /// `contract_family` shape — listed first so that a payload containing
    /// both `contract_function` and `stacks_tx_hashes` is unambiguously
    /// matched here rather than partially against `Tx`.
    Contract {
        /// Issuer / contract / function triple identifying the function.
        contract_function: ContractFunction,
        /// Heaviest representative txs exercising the function.
        #[schemars(length(min = 1, max = 5))]
        #[schemars(inner(regex(pattern = HEX_HASH_PATTERN)))]
        stacks_tx_hashes: Vec<String>,
    },
    /// `tx_family` shape: 1–5 representative transaction hashes.
    Tx {
        /// Heaviest representative txs (hex hashes).
        #[schemars(length(min = 1, max = 5))]
        #[schemars(inner(regex(pattern = HEX_HASH_PATTERN)))]
        stacks_tx_hashes: Vec<String>,
    },
    /// `block_family` shape: 1–5 representative stacks index block hashes.
    Block {
        /// Heaviest representative stacks index block hashes.
        #[schemars(length(min = 1, max = 5))]
        #[schemars(inner(regex(pattern = HEX_HASH_PATTERN)))]
        stacks_block_hashes: Vec<String>,
    },
}

impl RepresentativeIds {
    /// Variant name for diagnostic messages.
    fn discriminator(&self) -> &'static str {
        match self {
            Self::Tx { .. } => "Tx",
            Self::Block { .. } => "Block",
            Self::Contract { .. } => "Contract",
        }
    }
}

/// Issuer / contract / function triple identifying a Clarity contract.function.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContractFunction {
    /// Issuer principal (standard or contract principal).
    pub issuer: String,
    /// Contract name.
    pub contract: String,
    /// Function name.
    pub function: String,
}

/// Aggregate cost signal across the whole run, populated from
/// `span_recurrence`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GlobalMateriality {
    /// Maximum `pct_blocks` across the family's suspected spans.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pct_blocks: Option<f64>,
    /// Sum of `self_wall_ms` across the family's suspected spans.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_wall_ms: Option<f64>,
    /// Optional analyst note (e.g. workload-conditional caveat).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}
