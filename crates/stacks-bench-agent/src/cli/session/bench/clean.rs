//! `sbagent session bench clean` — clear Phase 3 per-target benchmark
//! artifacts (`candidate-run-ids.json` and the per-invocation
//! `<invocation-id>/` subdirs) without touching the optimizer-side
//! outputs (implementation / abort markers, subagent logs). Useful for
//! re-benchmarking after a flaky run.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::session::clean::{self, CleanReport};
use crate::session::{SessionLayout, loader};
use crate::types::SessionId;

/// Args for `sbagent session bench clean`.
#[derive(Debug, Args)]
pub struct BenchCleanArgs {}

/// Clear bench artifacts. Idempotent.
pub async fn run(_args: BenchCleanArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let mut report = CleanReport::default();

    // The Phase 3 outputs for one target are:
    //  - `optimize/<target>/candidate-run-ids.json`
    //  - `optimize/<target>/<invocation-id>/bench-run.json`
    //  - `optimize/<target>/<invocation-id>/bench-run.stderr.log`
    // The invocation set is sourced from the merged target's
    // `verification_replay.invocations[].id` — deleting by that list
    // (rather than by a name pattern) keeps clean from racing future
    // sibling artifacts and lets it stay correct even when the operator
    // hand-edits an invocation id.
    let targets = match loader::read_optimization_targets(&layout) {
        Ok(t) => t,
        Err(_) => {
            clean::print_report("bench clean", &report);
            return Ok(());
        }
    };
    for target in &targets.targets {
        let exp = layout.experiment_dir(&target.id);
        report.merge(clean::remove_one(&exp.join("candidate-run-ids.json"))?);
        if let Some(vr) = &target.verification_replay {
            for inv in &vr.invocations {
                let inv_dir = layout.experiment_candidate_invocation_dir(&target.id, &inv.id);
                report.merge(clean::remove_one(&inv_dir)?);
            }
        }
    }

    clean::print_report("bench clean", &report);
    Ok(())
}
