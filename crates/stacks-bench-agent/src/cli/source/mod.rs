//! `sbagent source ...` — helpers for the per-session source clone
//! subsystem.
//!
//! Subcommands:
//!
//! - `cache-id`: prints the deterministic cache id derived from `[source]`
//!   settings (or echoes a pinned `source.id`). Supports the operator migration
//!   recipe in [docs/setup.md](../../../../docs/setup.md) — the bare-cache
//!   directory is named `<workspace>/cache/<cache_id>.git`.
//! - `seed`: one-shot push of a branch from `<source-url>` to `<dest-url>`.
//!   Bootstraps a brand-new bot fork so the first `session run` can fetch
//!   `[source].branch`. See the [`seed`] module for the auth contract.

pub mod cache_id;
pub mod seed;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;

/// `sbagent source ...`.
#[derive(Debug, Args)]
pub struct SourceArgs {
    #[clap(subcommand)]
    pub command: SourceCommand,
}

/// `sbagent source` subcommands.
#[derive(Debug, Subcommand)]
pub enum SourceCommand {
    /// Print the cache id derived from `[source]` settings.
    CacheId(cache_id::CacheIdArgs),
    /// Bare-clone a branch from a source URL and push it to a
    /// destination URL. Bootstraps a brand-new bot fork before the
    /// first `session run`.
    Seed(seed::SeedArgs),
}

/// Dispatch for `sbagent source ...`.
pub async fn run(args: SourceArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        SourceCommand::CacheId(a) => cache_id::run(a, ctx),
        SourceCommand::Seed(a) => seed::run(a, ctx),
    }
}
