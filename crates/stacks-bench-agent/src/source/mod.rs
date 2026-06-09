//! Per-session ephemeral source clone — bare cache + working
//! checkout primitives, behind a trait-injected seam.
//!
//! This module is the v3 Phase 1 layer: it defines the API for
//! materializing a source-repo checkout under
//! `<agent_workspace_root>/sessions/<id>/repos/<cache_id>/`,
//! backed by a shared bare cache at
//! `<agent_workspace_root>/cache/<cache_id>.git/`. No existing
//! callsite consumes this yet — v3 Phase 3 is the consumer cutover.
//!
//! See [Decision 0003](../../planning/decisions/0003-ephemeral-source-clone.md)
//! for the rationale and
//! [v3 iteration](../../planning/iterations/v3-ephemeral-source-clone.md)
//! for the phased plan.

pub mod cache_id;
pub mod repo;
pub mod session;

pub use cache_id::{derive_cache_id, resolve_cache_id};
pub use repo::{SourceMaterialization, SourceRepo, StdSourceRepo};
pub use session::{ResolvedSource, materialize_session_source, read_session_source};
