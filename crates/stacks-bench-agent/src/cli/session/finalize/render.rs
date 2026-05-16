//! `sbagent session finalize render` — re-render `summary.md` + `targets.md`
//! from the already-on-disk `summary.json` + `optimization-targets.json` +
//! `analysis/*/analysis.json`.
//!
//! This subcommand does NOT touch the stacks-bench database; it is purely
//! a markdown regeneration pass. Useful when:
//!
//! - the renderer changes and you want fresh `*.md` for a finished session
//!   without redoing the per-target bench-show queries;
//! - a concurrent benchmark is holding the stacks-bench SQLite db.
//!
//! Compare against [`super::run`], which recomputes the entire summary
//! (and hits the DB for every baseline + per-target run id).
//!
//! If `summary.json` is missing, this errors out and points the user at
//! `finalize run`.
//!
//! # Behavior
//!
//! 1. Load `summary.json` — authoritative for per-target dispositions and
//!    `improvement_pct`.
//! 2. Load `optimization-targets.json` — full target catalog for `targets.md`.
//! 3. Load `analysis/*/analysis.json` — for the coverage matrix and cross-links
//!    in `targets.md`.
//! 4. Render and write `summary.md` + `targets.md` in-place.

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::{SessionLayout, loader, render};
use crate::types::SessionId;

/// Args for `sbagent session finalize render`.
#[derive(Debug, Args)]
pub struct FinalizeRenderArgs {}

/// Re-render `summary.md` + `targets.md` from the on-disk JSON artifacts.
#[allow(unused_variables)]
pub async fn run(args: FinalizeRenderArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());

    let summary = loader::read_summary(&layout).with_context(|| {
        format!(
            "loading {} — run `sbagent session finalize run` first",
            layout
                .summary_json()
                .display()
        )
    })?;
    let targets =
        loader::read_optimization_targets(&layout).context("loading optimization-targets.json")?;
    let analyses =
        loader::read_all_analyses(&layout).context("loading analysis/*/analysis.json")?;

    let notes = render::load_experiment_notes(&layout, &targets);
    let summary_md = render::render_summary_md(&summary, &targets, &analyses, &notes);
    let targets_md = render::render_targets_md(&targets, &analyses);

    std::fs::write(layout.summary_md(), summary_md)
        .with_context(|| format!("writing {}", layout.summary_md().display()))?;
    std::fs::write(layout.targets_md(), targets_md)
        .with_context(|| format!("writing {}", layout.targets_md().display()))?;

    println!("re-rendered {} and {}", layout.summary_md().display(), layout.targets_md().display(),);
    Ok(())
}
