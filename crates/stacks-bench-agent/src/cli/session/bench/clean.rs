//! `sbagent session bench clean` — clear Phase 3 per-target benchmark
//! artifacts AND the paired Phase 1.8 target-calibration-baseline outputs under
//! `verify/<target>/`. The two phases share one invocation-id set per
//! target (the Phase 3 verification bench is the apples-to-apples partner
//! of the Phase 1.8 target calibration baseline for the same invocation), so
//! cleaning both sides together matches how operators rerun: a fresh `bench
//! run` drops both. Optimizer-side outputs (implementation/abort markers,
//! subagent logs, `optimizer-report.json`) are NOT touched — those are
//! `optimize clean`'s domain.

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
    let report = clean_with_layout(&layout)?;
    clean::print_report("bench clean", &report);
    Ok(())
}

/// Pure clean for a given session layout. Exposed for tests + future
/// composition; `run` is a thin wrapper.
///
/// Phase 3 per-target outputs:
///  - `optimize/<target>/candidate-run-ids.json`
///  - `optimize/<target>/<invocation-id>/bench-run.json`
///  - `optimize/<target>/<invocation-id>/bench-run.stderr.log`
///
/// Phase 1.8 per-target outputs:
///  - `verify/<target>/baseline-run-ids.json`
///  - `verify/<target>/<invocation-id>/bench-run.json`
///  - `verify/<target>/<invocation-id>/bench-run.stderr.log`
///
/// The invocation set is sourced from the merged target's
/// `verification_replay.invocations[].id` when the file is present —
/// deleting by that list keeps clean from racing future sibling
/// artifacts and stays correct when an operator hand-edits an id.
///
/// Targets file states:
///  - **Missing** (`analysis clean` already wiped merge/, fresh session, etc.)
///    → skip the per-target loop and fall through to the wholesale
///    `verify_dir()` sweep so the operator can still fully reset Phase 1.8
///    without phase ordering mattering.
///  - **Present but corrupt / fails validation** → error propagates; no
///    half-clean. Operator sees the parser or validator complaint naming the
///    file, fixes it, reruns.
pub fn clean_with_layout(layout: &SessionLayout) -> Result<CleanReport> {
    let mut report = CleanReport::default();

    // Distinguish "no targets file on disk" (legitimate fallback — e.g.
    // `analysis clean` already wiped merge/) from "targets file is
    // corrupt or fails validation" (propagate — silently skipping the
    // Phase 3 candidate cleanup while reporting success would leave
    // `optimize/<target>/` orphans behind).
    let targets_path = layout.optimization_targets_json();
    if targets_path.exists() {
        let targets = loader::read_optimization_targets(layout)?;
        for target in &targets.targets {
            // Phase 3 candidate side under optimize/<target>/.
            let exp = layout.experiment_dir(&target.id);
            report.merge(clean::remove_one(&exp.join("candidate-run-ids.json"))?);
            if let Some(vr) = &target.verification_replay {
                for inv in &vr.invocations {
                    let inv_dir = layout.experiment_candidate_invocation_dir(&target.id, &inv.id);
                    report.merge(clean::remove_one(&inv_dir)?);
                }
            }

            // Phase 1.8 target-calibration-baseline side under verify/<target>/.
            // The whole per-target dir is owned by Phase 1.8, so dropping
            // it wholesale handles `baseline-run-ids.json` and every
            // `<invocation-id>/` subdir in one call.
            let verify_target = layout.verify_target_dir(&target.id);
            report.merge(clean::remove_one(&verify_target)?);
        }
    }

    // Final wholesale sweep of verify/. Idempotent against the per-target
    // loop above (already-removed children → skipped_missing on the parent
    // when nothing's left). Also covers the no-targets-loaded path so
    // `bench clean` can run after `analysis clean` and still reset the
    // Phase 1.8 tree.
    report.merge(clean::remove_one(&layout.verify_dir())?);

    Ok(report)
}
