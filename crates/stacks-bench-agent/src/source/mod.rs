//! Per-session ephemeral source clone — bare cache + working
//! checkout primitives, behind a trait-injected seam.
//!
//! Materializes a source-repo checkout under
//! `<agent_workspace_root>/sessions/<id>/repos/<cache_id>/`, backed
//! by a shared bare cache at
//! `<agent_workspace_root>/cache/<cache_id>.git/`. See
//! [Decision 0003](../../planning/decisions/0003-ephemeral-source-clone.md)
//! for the rationale.

pub mod cache_id;
pub mod repo;
pub mod session;

pub use cache_id::{derive_cache_id, resolve_cache_id};
pub use repo::{SourceMaterialization, SourceRepo, StdSourceRepo};
pub use session::{ResolvedSource, materialize_session_source, read_session_source};
