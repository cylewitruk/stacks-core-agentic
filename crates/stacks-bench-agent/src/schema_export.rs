//! Generate JSON Schema files from the typed v2 models.
//!
//! `sbagent schema export --out <dir>` writes one `<name>.schema.json` per
//! top-level model. `sbagent check` reuses [`generate_all`] for the schema
//! drift gate (compare emitted bytes to whatever's committed under
//! `<framework>/schemas/`).
//!
//! schemars handles the bulk of the JSON Schema; the [`transform`] module
//! injects the cross-field invariants (consensus-routing if/then chains,
//! delivery-mode derivation, etc.) that schemars cannot express via
//! attributes.

pub mod transform;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::Value;

use crate::models::analyze::Analysis;
use crate::models::candidates::Candidates;
use crate::models::coordinator_provenance::CoordinatorProvenance;
use crate::models::optimizer_report::OptimizerReport;
use crate::models::session_record::SessionRecord;
use crate::models::summary::Summary;
use crate::models::targets::OptimizationTargets;

/// Schema entry: `(file_name, root_schema)`. The file name matches the
/// committed JSON Schema layout (`candidates.schema.json` etc.) so the
/// drift gate is a byte-for-byte compare.
pub struct SchemaEntry {
    /// File name (without leading directory).
    pub file_name: &'static str,
    /// Generated root schema as serde_json::Value (canonical for diffing).
    pub schema: Value,
}

/// Generate the v2 schema entries (one per top-level artifact). Order is
/// stable so `sbagent check`'s drift output is deterministic.
pub fn generate_all() -> Result<Vec<SchemaEntry>> {
    Ok(vec![
        entry::<Candidates>("candidates.schema.json")?,
        entry::<Analysis>("analysis.schema.json")?,
        entry::<OptimizationTargets>("optimization-targets.schema.json")?,
        entry::<OptimizerReport>("optimizer-report.schema.json")?,
        entry::<CoordinatorProvenance>("coordinator-provenance.schema.json")?,
        entry::<Summary>("summary.schema.json")?,
        entry::<SessionRecord>("session-record.schema.json")?,
    ])
}

/// Render one model's schema to a `SchemaEntry`. Uses Draft 2020-12 (matching
/// the existing committed schemas + the Python `jsonschema` validator the
/// bash scripts invoke).
fn entry<T: JsonSchema>(file_name: &'static str) -> Result<SchemaEntry> {
    let generator = SchemaGenerator::new(SchemaSettings::draft2020_12());
    let schema = generator.into_root_schema_for::<T>();
    let mut schema = serde_json::to_value(&schema)
        .with_context(|| format!("serializing schema for {file_name}"))?;
    transform::apply_invariants(&mut schema);
    Ok(SchemaEntry { file_name, schema })
}

/// Write every generated schema to `out_dir`, creating it if needed.
/// Returns the list of written file paths.
pub fn write_all(out_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let mut written = Vec::new();
    for SchemaEntry { file_name, schema } in generate_all()? {
        let path = out_dir.join(file_name);
        let body = canonicalize(&schema)?;
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

/// Compare generated schemas to committed ones; return the list of files
/// that differ (empty list = no drift). Used by `sbagent check`.
pub fn drift(committed_dir: &Path) -> Result<Vec<DriftEntry>> {
    let mut out = Vec::new();
    for SchemaEntry { file_name, schema } in generate_all()? {
        let committed_path = committed_dir.join(file_name);
        let generated = canonicalize(&schema)?;
        let committed = match std::fs::read_to_string(&committed_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                out.push(DriftEntry::Missing { file_name });
                continue;
            }
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("reading committed schema {}", committed_path.display())));
            }
        };
        // Normalize both sides through serde_json so trailing-newline /
        // formatting differences in the committed file don't trigger drift.
        let committed_value: Value = serde_json::from_str(&committed)
            .with_context(|| format!("parsing committed schema {}", committed_path.display()))?;
        let committed_norm = canonicalize(&committed_value)?;
        if generated != committed_norm {
            out.push(DriftEntry::Differs { file_name });
        }
    }
    Ok(out)
}

/// One drift finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftEntry {
    /// The committed file is absent.
    Missing {
        /// File name relative to the committed schemas dir.
        file_name: &'static str,
    },
    /// The committed file is present but its contents differ.
    Differs {
        /// File name relative to the committed schemas dir.
        file_name: &'static str,
    },
}

impl DriftEntry {
    /// File name this drift entry refers to.
    pub fn file_name(&self) -> &'static str {
        match self {
            Self::Missing { file_name } | Self::Differs { file_name } => file_name,
        }
    }
}

/// Render a `serde_json::Value` to a canonical, stable-ordered, pretty-printed
/// JSON string with a trailing newline. Object keys are sorted; arrays keep
/// insertion order. This is the on-disk form for both `schema export` and the
/// `check` drift comparison.
fn canonicalize(value: &Value) -> Result<String> {
    let sorted = sort_value(value);
    let mut s = serde_json::to_string_pretty(&sorted)?;
    s.push('\n');
    Ok(s)
}

/// Recursively sort object keys in a JSON value. Arrays keep their order
/// (their order is semantically meaningful in JSON Schema, e.g. `allOf`).
fn sort_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: BTreeMap<String, Value> = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), sort_value(v));
            }
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(sort_value)
                .collect(),
        ),
        other => other.clone(),
    }
}
