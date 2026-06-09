//! `sbagent source ...` — read-only helpers for the per-session source
//! clone subsystem.
//!
//! Currently one subcommand: `cache-id`, which prints the deterministic
//! cache id derived from `[source]` settings (or echoes a pinned
//! `source.id`). Supports the Phase 4 operator migration recipe in
//! [docs/setup.md](../../../../docs/setup.md) — the bare-cache directory
//! is named `<workspace>/cache/<cache_id>.git`, and the recipe's
//! `git clone --bare` step needs that id.

pub mod cache_id;

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
}

/// Dispatch for `sbagent source ...`.
pub async fn run(args: SourceArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        SourceCommand::CacheId(a) => cache_id::run(a, ctx),
    }
}
