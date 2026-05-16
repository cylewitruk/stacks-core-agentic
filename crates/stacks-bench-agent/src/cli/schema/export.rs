//! `sbagent schema export` — write the typed v2 models out as JSON Schema
//! files.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::schema_export;

/// Args for `sbagent schema export`.
#[derive(Debug, Args)]
pub struct SchemaExportArgs {
    /// Output directory. Defaults to `<framework_root>/schemas/`.
    #[clap(long)]
    pub out: Option<PathBuf>,
}

/// Generate JSON Schema files from the typed models.
pub async fn run(args: SchemaExportArgs, ctx: &CliContext) -> Result<()> {
    // Default `--out` is the committed `<framework>/schemas/` so a
    // tool developer running `sbagent schema export` writes back to
    // the source tree's bundle. Operators don't run this command —
    // they consume schemas via the embedded bundle.
    let out_dir = match args.out {
        Some(p) => p,
        None => ctx
            .layout
            .require_framework_dir()?
            .schemas_dir(),
    };
    let written = schema_export::write_all(&out_dir)?;
    for path in &written {
        tracing::info!(path = %path.display(), "wrote schema");
    }
    println!("wrote {} schema(s) to {}", written.len(), out_dir.display());
    Ok(())
}
