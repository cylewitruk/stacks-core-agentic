//! `sbagent session analyze-results clean` — drop every Phase 3.5
//! per-target verdict + subagent log so the next `analyze-results run`
//! re-judges from scratch.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::session::SessionLayout;
use crate::session::clean::{self, CleanReport};
use crate::types::SessionId;

/// Args for `sbagent session analyze-results clean`.
#[derive(Debug, Args)]
pub struct ResultsAnalyzerCleanArgs {}

/// Clear `analyze/<target>/` for every target. Idempotent.
pub async fn run(
    _args: ResultsAnalyzerCleanArgs,
    ctx: &CliContext,
    session_id: &SessionId,
) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let mut report = CleanReport::default();

    let analyze_root = layout.analyze_dir();
    let rd = match std::fs::read_dir(&analyze_root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            clean::print_report("analyze-results clean", &report);
            return Ok(());
        }
        Err(e) => {
            return Err(
                anyhow::Error::new(e).context(format!("reading {}", analyze_root.display()))
            );
        }
    };
    for entry in rd.flatten() {
        if !entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        report.merge(clean::remove_one(&entry.path())?);
    }

    clean::print_report("analyze-results clean", &report);
    Ok(())
}
