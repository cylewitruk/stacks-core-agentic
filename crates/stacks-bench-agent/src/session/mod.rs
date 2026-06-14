//! Per-session business logic: loading artifacts, validating the artifact
//! tree, and producing the final summary. The CLI commands under
//! `crate::cli::session::{validate,finalize}` are thin wrappers around this
//! module's public API.

pub mod analyzers;
pub mod archive;
pub mod autonomy;
pub mod baseline;
pub mod bench;
pub mod bench_experiments;
pub mod calibration;
pub mod cargo;
pub mod clean;
pub mod db_consistency;
pub mod dedup;
pub mod finalize;
pub mod layout;
pub mod ledger_reader;
pub mod loader;
pub mod maintain;
pub mod maintain_ledger;
pub mod merge;
pub mod optimizers;
pub mod phase_timing;
pub mod preflight;
pub mod publish;
pub mod render;
pub mod results_analyzer;
pub mod run_pid;
pub mod triage;
pub mod triage_queries;
pub mod validate;
pub mod workspace;

pub use layout::SessionLayout;
