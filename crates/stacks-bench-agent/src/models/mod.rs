//! Typed v2 artifact models.
//!
//! Each top-level artifact lives in its own submodule:
//! - [`candidates`] — `candidates.json` (triage output)
//! - [`analyze`] — `analysis/<family-id>/analysis.json` (one per analyzer)
//! - [`targets`] — `optimization-targets.json` (merge output)
//! - [`optimizer_report`] — `optimize/<target-id>/optimizer-report.json` (one
//!   per optimizer agent; replaces the previous marker-file contract)
//! - [`summary`] — `summary.json` (finalize output)
//!
//! Shared types (selection lens, bucket, delivery mode, improvement vector,
//! hotspot, lens disposition, schema-version sentinel) live in [`common`].

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub mod analyze;
pub mod baseline_binary_manifest;
pub mod candidates;
pub mod common;
pub mod coordinator_provenance;
pub mod optimizer_report;
pub mod results_analysis;
pub mod session_record;
pub mod summary;
pub mod targets;

pub trait ToJson {
    /// Serialize the model to a compact JSON string. Suitable as one
    /// JSONL record; framing (newlines, append semantics) stays the
    /// caller's concern.
    fn to_json(&self) -> Result<String>;
    /// Serialize the model to a pretty-printed JSON string.
    fn to_json_pretty(&self) -> Result<String>;
}

impl<T> ToJson for T
where
    T: Serialize,
{
    fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).context("error serializing model to JSON")
    }

    fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("error serializing model to pretty JSON")
    }
}

pub trait FromJson: DeserializeOwned + Sized {
    /// Deserialize the model from a JSON string without any additional
    /// validation.
    fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("error deserializing JSON into model")
    }
}

impl<T> FromJson for T where T: DeserializeOwned {}

pub trait ValidateModel {
    /// Validate the model's internal consistency and invariants.
    fn validate_model(&self) -> Result<()>;
}

pub trait FromJsonValidated: FromJson + ValidateModel {
    /// Deserialize the model from a JSON string and validate it.
    fn from_json_validated(json: &str) -> Result<Self> {
        let model = Self::from_json(json)?;
        model.validate_model()?;
        Ok(model)
    }
}

impl<T> FromJsonValidated for T where T: FromJson + ValidateModel {}
