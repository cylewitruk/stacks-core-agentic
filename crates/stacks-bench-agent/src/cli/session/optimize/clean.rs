//! `sbagent session optimize clean` — tear down per-target git
//! checkouts (clones) + clear the experiments dir so optimizers (and
//! bench experiments) re-run from scratch.

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::SessionLayout;
use crate::session::clean::{self, CleanReport};
use crate::session::optimizers::{GitCheckoutManager, StdGitCheckoutManager};
use crate::types::SessionId;

/// Args for `sbagent session optimize clean`.
#[derive(Debug, Args)]
pub struct OptimizeCleanArgs {}

/// Clear experiment dirs and the per-target git checkouts. Idempotent.
pub async fn run(_args: OptimizeCleanArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let git = StdGitCheckoutManager;
    let mut report = CleanReport::default();

    // Tear down each per-target checkout (a stand-alone clone — own
    // `.git/` inside its cwd). Teardown is `rm -rf`; no `git worktree
    // remove` / `worktree prune` dance because the clone has no
    // shared bookkeeping with the base repo.
    let checkouts_root = ctx
        .layout
        .session_optimizer_checkouts_dir(&layout.id);
    if let Ok(rd) = std::fs::read_dir(&checkouts_root) {
        for entry in rd.flatten() {
            if !entry
                .file_type()
                .map(|t| t.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let checkout = entry.path();
            let removed = git
                .remove_checkout(&checkout)
                .with_context(|| format!("removing checkout {}", checkout.display()))?;
            if removed {
                report.removed_dirs += 1;
            }
        }
        // Drop the (now empty) checkouts root itself.
        report.merge(clean::remove_one(&checkouts_root)?);
    } else {
        report.skipped_missing += 1;
    }

    // Drop the experiments tree wholesale (per-target subagent logs,
    // implementation/abort markers, bench run-N dirs).
    report.merge(clean::remove_one(&layout.optimize_dir())?);

    clean::print_report("optimize clean", &report);
    Ok(())
}
