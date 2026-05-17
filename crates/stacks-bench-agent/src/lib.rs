//! Library crate for `sbagent` (`stacks-bench-agent`).
//!
//! Houses the typed v2 artifact models, schema export logic, session-layout
//! abstractions, and the per-command business logic. The thin binary in
//! `main.rs` is responsible for argument parsing and process lifecycle only —
//! everything testable lives here.

pub mod analyzed_rejections;
pub mod cli;
pub mod context;
pub mod git;
pub mod harnesses;
pub mod layout;
pub mod models;
pub mod prompts;
pub mod queries;
pub mod schema_export;
pub mod schemas;
pub mod session;
pub mod settings;
pub mod types;
