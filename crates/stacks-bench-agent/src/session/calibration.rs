//! Phase 1.8: per-target targeted baseline calibration.
//!
//! For each `normal_pr` target with a non-empty `verification_replay`,
//! runs ONE bench invocation per replay phase (txid + block) against
//! the strict archived baseline binary from Phase 0a. The resulting
//! per-target baseline-run-ids feed Phase 4 finalize's per-target
//! `improvement_pct` comparison so numerator (candidate) and
//! denominator (baseline) are both measured under targeted-replay
//! cache regimes.
//!
//! Targets without `verification_replay` are skipped at Phase 1.8 and
//! keep the legacy P0-vs-full-range comparison until Pass 2 lands the
//! full-range fallback machinery.
//!
//! See [baseline-verification-agent-plan.md](../../../../
//! baseline-verification-agent-plan.md) (Pass 1a, Sub-step C).

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::models::common::DeliveryMode;
use crate::models::targets::{MergedTarget, OptimizationTargets};
use crate::session::SessionLayout;
use crate::session::bench::{BenchClient, InvokeOptions, extract_run_id};
use crate::session::bench_experiments::strip_hex_prefix;

/// Inputs to [`run`].
pub struct Inputs<'a> {
    pub layout: &'a SessionLayout,
    /// Strict archived baseline binary (Phase 0a output).
    pub bench: &'a dyn BenchClient,
    /// Chainstate source path (`--source`).
    pub source_dir: &'a Path,
    /// Stacks network (`--network`).
    pub network: &'a str,
    /// Optional `--shadow-dir-root`.
    pub shadow_dir_root: Option<&'a Path>,
    /// Bench lock path.
    pub bench_lock: &'a Path,
    /// Optimization-targets file already parsed by the caller (the
    /// orchestrator reads it once and forwards it to multiple phases).
    pub targets: &'a OptimizationTargets,
}

/// Outputs of [`run`] — one entry per target that received a
/// calibration. Targets without `verification_replay` are absent.
#[derive(Debug, Default)]
pub struct Outputs {
    /// Per-target structured baseline run ids. Keyed by target id.
    pub per_target: std::collections::BTreeMap<String, BaselineRunIds>,
}

/// Phase-aware baseline run ids for one target. Serialized to
/// `verify/<target>/baseline-run-ids.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BaselineRunIds {
    /// Run ids from the txid-phase calibration(s). Pass 1a writes
    /// at most one; Pass 1b (multi-invocation variance) may write
    /// more.
    pub txid_run_ids: Vec<i64>,
    /// Run ids from the block-phase calibration(s). Same shape
    /// semantics as `txid_run_ids`.
    pub block_run_ids: Vec<i64>,
}

/// Per-replay-phase descriptor for a single Phase 1.8 invocation.
struct CalibrationPhase {
    /// `"txid"` or `"block"`. Used in artifact paths
    /// (`baseline-{phase}-run-K/`) and the bench name suffix.
    phase: &'static str,
    /// Args to append AFTER the common prefix
    /// (`--source/--network/--name/--shadow-dir-root` are added by
    /// `run`). Includes `--repetitions`, `--warmup`, and the
    /// per-replay `--txid`/`--block` repeats.
    extra: Vec<String>,
}

/// Run Phase 1.8 for every `normal_pr` target with a non-empty
/// `verification_replay`. Sequential under the bench lock — each
/// invocation owns the lock for its duration.
pub fn run(inputs: &Inputs<'_>) -> Result<Outputs> {
    let mut out = Outputs::default();
    for target in &inputs.targets.targets {
        if !matches!(target.delivery_mode, DeliveryMode::NormalPr) {
            continue;
        }
        let phases = build_phases(target);
        if phases.is_empty() {
            // No verification_replay OR both phases empty — skip
            // Phase 1.8 for this target. Legacy P0 ↔ candidate
            // full-range comparison continues to apply at Phase 4.
            continue;
        }
        let mut ids = BaselineRunIds::default();
        for phase in &phases {
            let run_id = invoke_phase(inputs, target, phase)?;
            match phase.phase {
                "txid" => ids.txid_run_ids.push(run_id),
                "block" => ids.block_run_ids.push(run_id),
                other => unreachable!("unexpected phase tag {other}"),
            }
        }
        // Persist structured run-ids.json.
        let ids_path = inputs
            .layout
            .verify_baseline_run_ids_json(&target.id);
        if let Some(parent) = ids_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let serialized = serde_json::to_string_pretty(&ids)
            .with_context(|| format!("serializing baseline-run-ids for {}", target.id))?;
        fs::write(&ids_path, format!("{serialized}\n"))
            .with_context(|| format!("writing {}", ids_path.display()))?;
        out.per_target
            .insert(target.id.clone(), ids);
    }
    Ok(out)
}

/// Build the list of phases (txid, block, or both) for a single
/// target. Mirrors the Phase 3 candidate bench's phase structure so
/// candidate ↔ baseline comparison stays phase-by-phase symmetric.
fn build_phases(target: &MergedTarget) -> Vec<CalibrationPhase> {
    let Some(vr) = &target.verification_replay else {
        return Vec::new();
    };
    let mut phases = Vec::with_capacity(2);
    let reps = vr.repetitions.to_string();
    let warmup = vr
        .warmup
        .unwrap_or(10)
        .to_string();
    if let Some(txids) = vr
        .txids
        .as_deref()
        .filter(|v| !v.is_empty())
    {
        let mut extra = Vec::with_capacity(4 + 2 * txids.len());
        extra.push("--repetitions".to_owned());
        extra.push(reps.clone());
        extra.push("--warmup".to_owned());
        extra.push(warmup.clone());
        for tx in txids {
            extra.push("--txid".to_owned());
            extra.push(strip_hex_prefix(tx));
        }
        phases.push(CalibrationPhase { phase: "txid", extra });
    }
    if let Some(blocks) = vr
        .blocks
        .as_deref()
        .filter(|v| !v.is_empty())
    {
        let mut extra = Vec::with_capacity(4 + 2 * blocks.len());
        extra.push("--repetitions".to_owned());
        extra.push(reps);
        extra.push("--warmup".to_owned());
        extra.push(warmup);
        for b in blocks {
            extra.push("--block".to_owned());
            extra.push(strip_hex_prefix(b));
        }
        phases.push(CalibrationPhase { phase: "block", extra });
    }
    phases
}

/// Invoke one calibration phase against the strict archived
/// baseline binary. Returns the run id stacks-bench recorded.
///
/// Pass 1a writes one invocation per phase (k=1). Pass 1b's
/// multi-invocation variance work would call this in a loop with
/// incrementing k and aggregate the resulting run ids.
fn invoke_phase(
    inputs: &Inputs<'_>,
    target: &MergedTarget,
    phase: &CalibrationPhase,
) -> Result<i64> {
    let k = 1_usize;
    let bench_name = format!("baseline-{}-{}-run-{k}", target.id, phase.phase);
    // Common prefix matches Phase 0b's baseline run except that this
    // is a targeted-replay phase (no `--start-at`/`--count` at this
    // layer; those args come from `phase.extra` when applicable).
    // Notably: NO `--bench-spans-only` and NO `--no-profiler-kv`.
    // Phase 1.9's verifier needs span + profiler-kv data the
    // candidate's minimal flags strip.
    let source_str = inputs
        .source_dir
        .to_string_lossy()
        .into_owned();
    let shadow_str = inputs
        .shadow_dir_root
        .map(|p| {
            p.to_string_lossy()
                .into_owned()
        });
    let mut args: Vec<&str> = vec![
        "bench",
        "run",
        "--source",
        &source_str,
        "--network",
        inputs.network,
        "--name",
        &bench_name,
    ];
    if let Some(sd) = shadow_str.as_deref() {
        args.push("--shadow-dir-root");
        args.push(sd);
    }
    for ea in &phase.extra {
        args.push(ea);
    }

    let bench_run_json = inputs
        .layout
        .verify_baseline_bench_run_json(&target.id, phase.phase, k);
    let stderr_path = inputs
        .layout
        .verify_baseline_bench_run_stderr(&target.id, phase.phase, k);
    if let Some(parent) = bench_run_json.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    inputs
        .bench
        .invoke(InvokeOptions {
            args: &args,
            stdout: Some(&bench_run_json),
            stderr: Some(&stderr_path),
            lock: Some(inputs.bench_lock),
        })
        .with_context(|| format!("Phase 1.8 calibration for {} ({})", target.id, phase.phase))?;

    extract_run_id(&bench_run_json)
        .with_context(|| format!("extracting run id from {}", bench_run_json.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::common::{
        Bucket, DeliveryMode, Hotspot, ImprovementVector, Risk, VerificationReplay,
    };
    use crate::models::targets::{MergedFrom, MergedTarget};

    fn target_with_replay(id: &str, replay: Option<VerificationReplay>) -> MergedTarget {
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

    fn replay(
        txids: Option<Vec<String>>,
        blocks: Option<Vec<String>>,
        reps: u32,
        warmup: Option<u32>,
    ) -> VerificationReplay {
        VerificationReplay {
            txids,
            blocks,
            repetitions: reps,
            warmup,
            rationale: "r".to_owned(),
        }
    }

    #[test]
    fn build_phases_returns_empty_for_target_without_replay() {
        let t = target_with_replay("a", None);
        assert!(build_phases(&t).is_empty());
    }

    #[test]
    fn build_phases_returns_empty_for_replay_with_empty_arrays() {
        let t = target_with_replay("a", Some(replay(Some(vec![]), Some(vec![]), 10, None)));
        assert!(build_phases(&t).is_empty());
    }

    #[test]
    fn build_phases_emits_txid_only_when_blocks_absent() {
        let t =
            target_with_replay("a", Some(replay(Some(vec!["0xabc".to_owned()]), None, 5, Some(3))));
        let phases = build_phases(&t);
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].phase, "txid");
        // Sanity: args carry stripped txid + repetitions + warmup.
        assert!(
            phases[0]
                .extra
                .contains(&"--repetitions".to_owned())
        );
        assert!(
            phases[0]
                .extra
                .contains(&"5".to_owned())
        );
        assert!(
            phases[0]
                .extra
                .contains(&"--warmup".to_owned())
        );
        assert!(
            phases[0]
                .extra
                .contains(&"3".to_owned())
        );
        assert!(
            phases[0]
                .extra
                .contains(&"--txid".to_owned())
        );
        assert!(
            phases[0]
                .extra
                .contains(&"abc".to_owned())
        );
    }

    #[test]
    fn build_phases_emits_block_only_when_txids_absent() {
        let t =
            target_with_replay("a", Some(replay(None, Some(vec!["0xdef".to_owned()]), 7, None)));
        let phases = build_phases(&t);
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].phase, "block");
        // Default warmup is 10 when omitted.
        assert!(
            phases[0]
                .extra
                .contains(&"10".to_owned())
        );
    }

    #[test]
    fn build_phases_emits_both_when_replay_carries_txids_and_blocks() {
        let t = target_with_replay(
            "a",
            Some(replay(
                Some(vec!["0x111".to_owned()]),
                Some(vec!["0x222".to_owned()]),
                3,
                Some(1),
            )),
        );
        let phases = build_phases(&t);
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].phase, "txid");
        assert_eq!(phases[1].phase, "block");
    }
}
