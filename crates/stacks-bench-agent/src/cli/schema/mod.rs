//! `sbagent schema ...` — schema-related commands.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;

pub mod export;

/// `sbagent schema ...`.
#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: SchemaCommand,
}

/// Schema subcommands.
#[derive(Debug, Subcommand)]
pub enum SchemaCommand {
    /// Generate JSON Schema files from the typed v2 models. CI guards
    /// against drift by re-running this and `git diff --exit-code`.
    Export(export::SchemaExportArgs),
}

/// Dispatch.
pub async fn run(args: SchemaArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        SchemaCommand::Export(a) => export::run(a, ctx).await,
    }
}
