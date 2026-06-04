//! `sbagent session bench run` — port of `scripts/bench-experiments.sh`.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::bench_experiments::{self, BenchRange, Inputs, TargetOutcome};
use crate::session::cargo::StdCargoRunner;
use crate::session::{SessionLayout, loader};
use crate::types::SessionId;

/// Args for `sbagent session bench`.
#[derive(Debug, Args)]
pub struct BenchRunArgs {
    /// Skip the per-worktree `cargo clean` after building. Equivalent to
    /// bash `SKIP_CARGO_CLEAN=1`.
    #[clap(long)]
    pub skip_cargo_clean: bool,
}

/// Run the bench-experiments phase.
pub async fn run(args: BenchRunArgs, ctx: &CliContext, session_id: &SessionId) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let targets = loader::read_optimization_targets(&layout)?;
    if targets.targets.is_empty() {
        println!("No targets to benchmark; phase is a no-op.");
        return Ok(());
    }

    let source_dir = ctx
        .settings
        .stacks_bench
        .source_dir_required()?;
    let network = ctx
        .settings
        .stacks_bench
        .effective_network();
    // start_at / count are only consumed by the full-range bench
    // fallback (no `verification_replay` on the target). Make the
    // requireds **conditional** on whether any target actually needs
    // that path — sessions where every target has a recipe should
    // load even when these settings are absent.
    let needs_full_range = any_target_needs_full_range(&targets.targets);
    let (start_at, count) = if needs_full_range {
        let s = ctx
            .settings
            .stacks_bench
            .start_at
            .context(
                "settings.stacks_bench.start_at is required (or env $STACKS_BENCH_START_AT) when \
                 at least one target lacks a `verification_replay` recipe",
            )?;
        let c = ctx
            .settings
            .stacks_bench
            .count
            .context(
                "settings.stacks_bench.count is required (or env $STACKS_BENCH_COUNT) when at \
                 least one target lacks a `verification_replay` recipe",
            )?;
        (Some(s), Some(c))
    } else {
        // Every target uses targeted-replay; the full-range fallback
        // is unreachable for this session.
        (None, None)
    };

    let worktrees_root = ctx
        .layout
        .session_optimizer_checkouts_dir(session_id);
    let data_dir = ctx
        .layout
        .stacks_bench_data_dir
        .clone();
    let cargo_cwd = ctx
        .layout
        .require_base()?
        .to_path_buf();
    let bench_for_target =
        move |bin: &std::path::Path| -> Box<dyn crate::session::bench::BenchClient> {
            bench_experiments::stacks_bench_for(bin, &data_dir, &cargo_cwd)
        };
    // Bind the closure to a local so we can pass it as &dyn Fn(...).
    let bench_for_target_ref: &dyn Fn(
        &std::path::Path,
    ) -> Box<dyn crate::session::bench::BenchClient> = &bench_for_target;

    let outcomes = bench_experiments::run(&Inputs {
        layout: &layout,
        worktrees_root: &worktrees_root,
        targets: &targets,
        range: BenchRange {
            source_dir,
            network,
            start_at,
            count,
            warmup: ctx
                .settings
                .stacks_bench
                .warmup,
            filter: ctx
                .settings
                .stacks_bench
                .filter
                .as_deref(),
            shadow_dir_root: ctx
                .layout
                .stacks_bench_shadow_dir
                .as_deref(),
        },
        bench_lock: &ctx.layout.bench_lock,
        skip_cargo_clean: args.skip_cargo_clean,
        cargo: &StdCargoRunner,
        bench_for_target: bench_for_target_ref,
    })?;

    print_summary(&outcomes, &worktrees_root);
    Ok(())
}

fn print_summary(outcomes: &[(String, TargetOutcome)], _worktrees_root: &PathBuf) {
    let mut benched = 0u32;
    let mut skipped = 0u32;
    for (id, outcome) in outcomes {
        match outcome {
            TargetOutcome::Benched { run_ids } => {
                benched += 1;
                println!(
                    "{id}: bench complete; run_ids={}",
                    run_ids
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            TargetOutcome::Skipped { reason } => {
                skipped += 1;
                println!("{id}: skipped ({reason})");
            }
        }
    }
    println!("benched={benched} skipped={skipped}");
}

/// True iff at least one target would fall through to the full-range
/// bench fallback — i.e. has no `verification_replay`, OR has one
/// whose `txids` and `blocks` are both absent or empty. Used by the
/// CLI preflight to decide whether `stacks_bench.start_at` and
/// `stacks_bench.count` are required for this session.
fn any_target_needs_full_range(targets: &[crate::models::targets::MergedTarget]) -> bool {
    targets
        .iter()
        .any(|t| match &t.verification_replay {
            None => true,
            Some(vr) => {
                let txids_empty = vr
                    .txids
                    .as_deref()
                    .is_none_or(<[String]>::is_empty);
                let blocks_empty = vr
                    .blocks
                    .as_deref()
                    .is_none_or(<[String]>::is_empty);
                txids_empty && blocks_empty
            }
        })
}

#[cfg(test)]
mod tests {
    use super::any_target_needs_full_range;
    use crate::models::common::{
        Bucket, DeliveryMode, Hotspot, ImprovementVector, Risk, VerificationReplay,
    };
    use crate::models::targets::{MergedFrom, MergedTarget};

    /// Minimal valid MergedTarget builder; tests override only the
    /// `verification_replay` slot to exercise the conditional.
    fn target(id: &str, replay: Option<VerificationReplay>) -> MergedTarget {
        MergedTarget {
            id: id.to_owned(),
            merged_from: vec![MergedFrom {
                family_id: "f".to_owned(),
                target_index: 0,
            }],
            convergence_count: 1,
            rank: None,
            target_span: "x".to_owned(),
            bucket: Bucket::BlockProcessing,
            hotspot: Hotspot {
                span: "x".to_owned(),
                self_wall_us: 1,
                total_wall_us: 1,
                calls: 1,
                location: "x.rs:1".to_owned(),
            },
            files: vec!["x.rs".to_owned()],
            evidence: "e".to_owned(),
            proposed_change: "p".to_owned(),
            expected_improvement: ImprovementVector {
                tx_latency: 0.0,
                tenure_throughput: 0.0,
                commit_time: 0.0,
            },
            risk: Risk::Low,
            verification_plan: "v".to_owned(),
            verification_replay: replay,
            merge_notes: None,
            contributor_differences: None,
            consensus_breaking: false,
            breakage_class: None,
            poc_implementable: None,
            poc_test_scope: None,
            consensus_writeup: None,
            delivery_mode: DeliveryMode::NormalPr,
            bench_eligible: true,
        }
    }

    fn replay(txids: Option<Vec<String>>, blocks: Option<Vec<String>>) -> VerificationReplay {
        VerificationReplay {
            txids,
            blocks,
            repetitions: 10,
            warmup: None,
            rationale: "r".to_owned(),
        }
    }

    #[test]
    fn target_with_no_replay_needs_full_range() {
        let targets = vec![target("a", None)];
        assert!(any_target_needs_full_range(&targets));
    }

    #[test]
    fn target_with_replay_containing_blocks_does_not_need_full_range() {
        let blocks = vec!["0xa".to_owned()];
        let targets = vec![target("a", Some(replay(None, Some(blocks))))];
        assert!(!any_target_needs_full_range(&targets));
    }

    #[test]
    fn target_with_replay_containing_txids_does_not_need_full_range() {
        let txids = vec!["0x1".to_owned()];
        let targets = vec![target("a", Some(replay(Some(txids), None)))];
        assert!(!any_target_needs_full_range(&targets));
    }

    #[test]
    fn target_with_empty_replay_needs_full_range() {
        // verification_replay present but both txids and blocks are
        // empty / absent — same semantics as no replay at all.
        let targets = vec![target("a", Some(replay(Some(vec![]), Some(vec![]))))];
        assert!(any_target_needs_full_range(&targets));
    }

    #[test]
    fn mixed_slate_needs_full_range_when_any_target_does() {
        let with_replay = target("a", Some(replay(None, Some(vec!["0xa".to_owned()]))));
        let without_replay = target("b", None);
        let targets = vec![with_replay, without_replay];
        assert!(any_target_needs_full_range(&targets));
    }

    #[test]
    fn all_replay_targets_skip_full_range() {
        let a = target("a", Some(replay(None, Some(vec!["0xa".to_owned()]))));
        let b = target("b", Some(replay(Some(vec!["0x1".to_owned()]), None)));
        let targets = vec![a, b];
        assert!(!any_target_needs_full_range(&targets));
    }

    #[test]
    fn empty_target_list_does_not_need_full_range() {
        // No-op session; the CLI's no-targets short-circuit fires
        // before preflight anyway, but defense-in-depth.
        assert!(!any_target_needs_full_range(&[]));
    }
}
