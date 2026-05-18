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

pub mod analyze;
pub mod candidates;
pub mod common;
pub mod optimizer_report;
pub mod summary;
pub mod targets;
