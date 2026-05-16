//! Per-session business logic: loading artifacts, validating the artifact
//! tree, and producing the final summary. The CLI commands under
//! `crate::cli::session::{validate,finalize}` are thin wrappers around this
//! module's public API.

pub mod analyzers;
pub mod baseline;
pub mod bench;
pub mod bench_experiments;
pub mod cargo;
pub mod clean;
pub mod finalize;
pub mod layout;
pub mod loader;
pub mod merge;
pub mod optimizers;
pub mod publish;
pub mod render;
pub mod triage;
pub mod triage_queries;
pub mod validate;

pub use layout::SessionLayout;
