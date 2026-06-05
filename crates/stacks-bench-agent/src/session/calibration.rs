//! Phase 1.8: per-target targeted baseline calibration.
//!
//! For each `bench_eligible` target (`delivery_mode == normal_pr`), runs
//! every `BenchInvocation` in the target's `verification_replay` against
//! the strict archived baseline binary from Phase 0a. The resulting
//! per-invocation baseline run-ids feed Phase 3.5's per-invocation
//! candidate ↔ baseline pairing so numerator and denominator are both
//! measured under matching sample sets, repetitions, warmup, and
//! profiler mode.
//!
//! Consensus-mode targets (`consensus_poc_pr` / `consensus_issue`) never
//! reach Phase 1.8. The merge schema enforces that `verification_replay`
//! is present on every `bench_eligible` target, so this phase always has
//! invocations to run.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::models::ValidateModel;
use crate::models::common::{
    BenchInvocation, BenchSamples, DeliveryMode, InvocationRunId, InvocationRunIds, ProfilerMode,
};
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

/// Outputs of [`run`] — one entry per `bench_eligible` target.
#[derive(Debug, Default)]
pub struct Outputs {
    /// Per-target invocation run ids. Keyed by target id.
    pub per_target: std::collections::BTreeMap<String, InvocationRunIds>,
}

/// Run Phase 1.8 for every `bench_eligible` target. Sequential under
/// the bench lock — each invocation owns the lock for its duration.
pub fn run(inputs: &Inputs<'_>) -> Result<Outputs> {
    let mut out = Outputs::default();
    for target in &inputs.targets.targets {
        if !matches!(target.delivery_mode, DeliveryMode::NormalPr) {
            continue;
        }
        let vr = target
            .verification_replay
            .as_ref()
            .with_context(|| {
                format!(
                    "Phase 1.8: bench_eligible target `{}` has no verification_replay; merge \
                     schema should have rejected this",
                    target.id
                )
            })?;
        let mut entries = Vec::with_capacity(vr.invocations.len());
        for inv in &vr.invocations {
            let run_id = invoke_one(inputs, target, inv)?;
            entries.push(InvocationRunId {
                invocation_id: inv.id.clone(),
                run_id,
            });
        }
        let ids = InvocationRunIds { entries };
        ids.validate_model()
            .with_context(|| format!("Phase 1.8: invalid baseline-run-ids for `{}`", target.id))?;
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

/// Invoke one `BenchInvocation` against the strict archived baseline
/// binary. Returns the run id stacks-bench recorded.
fn invoke_one(inputs: &Inputs<'_>, target: &MergedTarget, inv: &BenchInvocation) -> Result<i64> {
    let bench_name = format!("baseline-{}-{}", target.id, inv.id);
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
    let extra = build_invocation_args(inv);

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
    for ea in &extra {
        args.push(ea);
    }

    let bench_run_json = inputs
        .layout
        .verify_baseline_bench_run_json(&target.id, &inv.id);
    let stderr_path = inputs
        .layout
        .verify_baseline_bench_run_stderr(&target.id, &inv.id);
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
        .with_context(|| format!("Phase 1.8 calibration for `{}` / `{}`", target.id, inv.id))?;

    extract_run_id(&bench_run_json)
        .with_context(|| format!("extracting run id from {}", bench_run_json.display()))
}

/// Lower a [`BenchInvocation`] to the CLI arg list appended after the
/// common prefix (`--source`/`--network`/`--name`/`--shadow-dir-root`).
/// Public so [`bench_experiments`](crate::session::bench_experiments) can
/// share the same arg-construction convention with Phase 1.8.
pub(super) fn build_invocation_args(inv: &BenchInvocation) -> Vec<String> {
    let mut extra = Vec::with_capacity(8);
    extra.push("--repetitions".to_owned());
    extra.push(inv.repetitions.to_string());
    extra.push("--warmup".to_owned());
    extra.push(inv.warmup.to_string());
    match &inv.samples {
        BenchSamples::Txids { txids } => {
            for tx in txids {
                extra.push("--txid".to_owned());
                extra.push(strip_hex_prefix(tx));
            }
        }
        BenchSamples::Blocks { blocks } => {
            for b in blocks {
                extra.push("--block".to_owned());
                extra.push(strip_hex_prefix(b));
            }
        }
        BenchSamples::BlockRange { start_at, count } => {
            extra.push("--start-at".to_owned());
            extra.push(start_at.to_string());
            extra.push("--count".to_owned());
            extra.push(count.to_string());
        }
    }
    profiler_flags(inv.profiler)
        .into_iter()
        .for_each(|f| extra.push(f.to_owned()));
    extra
}

/// Profiler-mode → stacks-bench CLI flags. Currently `Rich` (the default)
/// emits no flags; future variants (e.g. lean / no-kv) would add
/// `--bench-spans-only` / `--no-profiler-kv`.
fn profiler_flags(mode: ProfilerMode) -> Vec<&'static str> {
    match mode {
        ProfilerMode::Rich => Vec::new(),
    }
}

/// Helper: assert this target's `verification_replay.invocations[]` is
/// non-empty. Phase 1.8 + Phase 3 both expect non-empty invocations on
/// every `bench_eligible` target; the merge validator enforces this, but
/// callers double-check to surface a clear error if they reach a target
/// in a violating state.
pub(crate) fn require_invocations(target: &MergedTarget) -> Result<&[BenchInvocation]> {
    let Some(vr) = &target.verification_replay else {
        bail!(
            "target `{}`: bench_eligible target must carry verification_replay (Pass 1c invariant)",
            target.id
        );
    };
    if vr.invocations.is_empty() {
        bail!("target `{}`: verification_replay.invocations is empty", target.id);
    }
    Ok(&vr.invocations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::common::{
        BenchSamples, Bucket, DeliveryMode, ExpectedSignal, Hotspot, ImprovementVector,
        ProfilerMode, Risk, SelectionLens, SignalDirection, VerificationReplay,
    };
    use crate::models::targets::{MergedFrom, MergedTarget};

    fn hex64(b: u8) -> String {
        format!("0x{}", std::iter::repeat_n(format!("{:02x}", b), 32).collect::<String>())
    }

    fn invocation(id: &str, samples: BenchSamples) -> BenchInvocation {
        BenchInvocation {
            id: id.to_owned(),
            label: format!("label-{id}"),
            purpose: format!("purpose-{id}"),
            samples,
            warmup: 5,
            repetitions: 10,
            profiler: ProfilerMode::Rich,
            expected_signal: ExpectedSignal {
                axis: SelectionLens::TxLatency,
                direction: SignalDirection::Improves,
                estimate_pct: Some(4.0),
                tolerance_pct: Some(2.0),
            },
        }
    }

    fn target(id: &str, vr: Option<VerificationReplay>) -> MergedTarget {
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
            verification_replay: vr,
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

    #[test]
    fn build_invocation_args_emits_txids() {
        let inv = invocation("cold", BenchSamples::Txids { txids: vec![hex64(0xab)] });
        let args = build_invocation_args(&inv);
        assert!(args.contains(&"--repetitions".to_owned()));
        assert!(args.contains(&"10".to_owned()));
        assert!(args.contains(&"--warmup".to_owned()));
        assert!(args.contains(&"5".to_owned()));
        assert!(args.contains(&"--txid".to_owned()));
        assert!(
            args.iter()
                .any(|a| a.starts_with("ab") && a.len() == 64)
        );
    }

    #[test]
    fn build_invocation_args_emits_blocks() {
        let inv = invocation("warm", BenchSamples::Blocks { blocks: vec![hex64(0xcd)] });
        let args = build_invocation_args(&inv);
        assert!(args.contains(&"--block".to_owned()));
        assert!(
            !args
                .iter()
                .any(|a| a == "--txid")
        );
    }

    #[test]
    fn build_invocation_args_emits_block_range() {
        let inv = invocation("rng", BenchSamples::BlockRange { start_at: 100, count: 50 });
        let args = build_invocation_args(&inv);
        assert!(args.contains(&"--start-at".to_owned()));
        assert!(args.contains(&"100".to_owned()));
        assert!(args.contains(&"--count".to_owned()));
        assert!(args.contains(&"50".to_owned()));
    }

    #[test]
    fn require_invocations_rejects_target_without_vr() {
        let t = target("a", None);
        assert!(require_invocations(&t).is_err());
    }

    #[test]
    fn require_invocations_accepts_target_with_vr() {
        let vr = VerificationReplay {
            rationale: "r".to_owned(),
            invocations: vec![invocation("cold", BenchSamples::Blocks { blocks: vec![hex64(1)] })],
            suspected_spans: None,
        };
        let t = target("a", Some(vr));
        assert_eq!(
            require_invocations(&t)
                .unwrap()
                .len(),
            1
        );
    }
}
