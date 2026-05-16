//! `sbagent session bench clean` — clear Phase 3 per-target benchmark
//! artifacts (`run-*` dirs and `run-ids` files) without touching the
//! optimizer-side outputs (implementation/abort markers, subagent logs).
//! Useful for re-benchmarking a target after a flaky run.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::session::SessionLayout;
use crate::session::clean::{self, CleanReport};
use crate::types::SessionId;

/// Args for `sbagent session bench clean`.
#[derive(Debug, Args)]
pub struct BenchCleanArgs {}

/// Clear bench artifacts. Idempotent.
pub async fn run(_args: BenchCleanArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let mut report = CleanReport::default();

    // Walk optimize/<target>/ and remove every per-target bench output
    // (Phase 3 writes candidate-bench-run.json, candidate-rerun.json,
    // run-ids, run-N/ subdirs alongside the optimizer outputs). The
    // optimizer's own artifacts (implementation/abort markers, the
    // subagent log) are intentionally left intact — bench clean
    // exists to re-bench without re-optimizing. Selection happens
    // by file-name prefix (`candidate-`, `run-*`, `run-ids`), not
    // by directory.
    let exp_root = layout.optimize_dir();
    let rd = match std::fs::read_dir(&exp_root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            clean::print_report("bench clean", &report);
            return Ok(());
        }
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", exp_root.display())));
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
        let target_dir = entry.path();
        // Drop run-ids if present.
        report.merge(clean::remove_one(&target_dir.join("run-ids"))?);
        // Drop every run-N subdir.
        if let Ok(srd) = std::fs::read_dir(&target_dir) {
            for sub in srd.flatten() {
                let name = sub.file_name();
                if !sub
                    .file_type()
                    .map(|t| t.is_dir())
                    .unwrap_or(false)
                {
                    continue;
                }
                if name
                    .to_string_lossy()
                    .starts_with("run-")
                {
                    report.merge(clean::remove_one(&sub.path())?);
                }
            }
        }
    }

    clean::print_report("bench clean", &report);
    Ok(())
}
