//! `sbagent workspace ...` — operator-facing workspace hygiene commands.
//!
//! Currently one subcommand: `prune`, which removes stale per-session
//! scratch dirs under `agent_workspace_root/sessions/`. See
//! [`crate::session::workspace`] for the underlying decision logic and
//! [`docs/operations.md`](../../../../docs/operations.md) for operator
//! usage.

pub mod prune;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;

/// `sbagent workspace ...`.
#[derive(Debug, Args)]
pub struct WorkspaceArgs {
    #[clap(subcommand)]
    pub command: WorkspaceCommand,
}

/// `sbagent workspace` subcommands.
#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Remove stale per-session scratch dirs.
    Prune(prune::PruneArgs),
}

/// Dispatch for `sbagent workspace ...`.
pub async fn run(args: WorkspaceArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        WorkspaceCommand::Prune(a) => prune::run(a, ctx),
    }
}
