//! Phase 3: build per-target release binaries, copy them out, then run
//! one `stacks-bench bench run` per `verification_replay.invocations[]`
//! entry, serialized under BENCH_LOCK.
//!
//! Per target, the phase has two halves:
//! 1. **Build** — `cargo build --release -p stacks-bench` inside the target's
//!    worktree, copy the produced binary to `optimize/<id>/bin/stacks-bench`,
//!    optional `cargo clean`.
//! 2. **Bench** — for each invocation, invoke the per-target binary with the
//!    args derived from `BenchInvocation` (samples + repetitions + warmup +
//!    profiler), capturing stdout/stderr to `optimize/<id>/<invocation-id>/`.
//!    Wrapped in flock so two targets can't contend for the host. Run ids land
//!    in `optimize/<id>/candidate-run-ids.json` as
//!    [`crate::models::common::InvocationRunIds`].
//!
//! Targets are skipped when:
//! - `bench_eligible == false` (consensus-routing modes — see
//!   [`crate::models::common::DeliveryMode`])
//! - `optimize/<id>/consensus-issue.md` is present (coordinator-written marker;
//!   the optimizer is skipped for `consensus_issue` mode, so no typed report
//!   exists for this branch)
//! - `optimize/<id>/optimizer-report.json` is absent, fails schema / context
//!   validation, or has `outcome=aborted`. Gating on the typed report (not the
//!   rendered companion `abort.md`, which could be stale after a demotion) is
//!   what makes "aborted" authoritative.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::models::common::DeliveryMode;
use crate::models::optimizer_report::OptimizerReport;
use crate::models::targets::{MergedTarget, OptimizationTargets};
use crate::session::bench::{BenchClient, InvokeOptions, extract_run_id};
use crate::session::cargo::{CargoRunner, worktree_release_bin};
use crate::session::{SessionLayout, loader};

/// Inputs to a bench-experiments phase.
pub struct Inputs<'a> {
    /// The session layout.
    pub layout: &'a SessionLayout,
    /// Worktrees root — `<sessions>/<id>/worktrees`. Each target's worktree
    /// is `<worktrees>/<target_id>`. The bash uses `$WORKTREES`.
    pub worktrees_root: &'a Path,
    /// Loaded merged-targets artifact.
    pub targets: &'a OptimizationTargets,
    /// Session-global bench env (`--source` / `--network` / optional
    /// `--shadow-dir-root`). Per-invocation knobs come from each target's
    /// `verification_replay.invocations[]`.
    pub env: BenchEnv<'a>,
    /// BENCH_LOCK path — flock'd around each `bench run` invocation.
    pub bench_lock: &'a Path,
    /// When true, skip the per-worktree `cargo clean` after building.
    pub skip_cargo_clean: bool,
    /// Cargo runner (typically [`crate::session::cargo::StdCargoRunner`]).
    pub cargo: &'a dyn CargoRunner,
    /// Factory: given the per-target binary path, return a `BenchClient`
    /// configured to invoke it. Tests inject a recording fake; production
    /// wraps [`crate::session::bench::StacksBenchCli`]. The `'a` lifetime
    /// on the returned box lets tests pass back trait objects that borrow
    /// from a recorder owned by the test.
    pub bench_for_target: &'a dyn Fn(&Path) -> Box<dyn BenchClient + 'a>,
}

/// Session-global bench env. The per-invocation knobs (`--start-at`,
/// `--count`, `--repetitions`, `--warmup`, `--filter`) are sourced from
/// each target's `verification_replay.invocations[]`, not from here.
#[derive(Debug, Clone, Copy)]
pub struct BenchEnv<'a> {
    /// `--source` arg.
    pub source_dir: &'a Path,
    /// `--network` arg.
    pub network: &'a str,
    /// Optional `--shadow-dir-root` arg. When set, stacks-bench creates
    /// its reflink shadow copy of the source chainstate under this dir
    /// instead of beside the source. Required when the source's parent
    /// isn't writable (e.g. `/Volumes/Extern` from inside the codex
    /// sandbox). Must be on the same filesystem as `source_dir` —
    /// stacks-bench refuses to proceed otherwise.
    pub shadow_dir_root: Option<&'a Path>,
}

/// Per-target outcome.
#[derive(Debug, PartialEq, Eq)]
pub enum TargetOutcome {
    /// Target was successfully built + benchmarked.
    Benched {
        /// Run ids produced by Phase 3, one per invocation in the order
        /// listed on the target's `verification_replay.invocations[]`.
        run_ids: Vec<i64>,
    },
    /// Target was skipped; no work happened.
    Skipped {
        /// Human-readable skip reason.
        reason: String,
    },
}

/// Run the bench-experiments phase. Per the bash, build is sequential over
/// targets and bench is sequential under BENCH_LOCK.
pub fn run(inputs: &Inputs<'_>) -> Result<Vec<(String, TargetOutcome)>> {
    let mut outcomes: Vec<(String, TargetOutcome)> =
        Vec::with_capacity(inputs.targets.targets.len());

    // Phase A: build + copy.
    for target in &inputs.targets.targets {
        let outcome = build_one(inputs, target)?;
        outcomes.push((target.id.clone(), outcome));
    }

    // Phase B: bench (only the targets that survived Phase A).
    for (idx, target) in inputs
        .targets
        .targets
        .iter()
        .enumerate()
    {
        // If a target was already skipped in Phase A, leave it alone.
        if let Some((_, TargetOutcome::Skipped { .. })) = outcomes.get(idx) {
            continue;
        }
        let bench_outcome = bench_one(inputs, target)?;
        outcomes[idx].1 = bench_outcome;
    }

    Ok(outcomes)
}

/// Phase A for one target: cargo build, copy binary, optional cargo clean.
fn build_one(inputs: &Inputs<'_>, target: &MergedTarget) -> Result<TargetOutcome> {
    if let Some(reason) = static_skip_reason(inputs, target)? {
        println!("skip {}: {}", target.id, reason);
        return Ok(TargetOutcome::Skipped { reason });
    }

    let worktree = inputs
        .worktrees_root
        .join(&target.id);
    let exp_dir = inputs
        .layout
        .experiment_dir(&target.id);
    fs::create_dir_all(&exp_dir).with_context(|| format!("creating {}", exp_dir.display()))?;

    let build_log = exp_dir.join("cargo-build.log");
    let build_err = exp_dir.join("cargo-build.stderr.log");
    inputs
        .cargo
        .build_release(&worktree, &build_log, &build_err)
        .with_context(|| format!("building {}", target.id))?;

    let bin_dir = exp_dir.join("bin");
    fs::create_dir_all(&bin_dir).with_context(|| format!("creating {}", bin_dir.display()))?;
    let copied_bin = bin_dir.join("stacks-bench");
    let produced_bin = worktree_release_bin(&worktree);
    fs::copy(&produced_bin, &copied_bin).with_context(|| {
        format!("copying {} → {}", produced_bin.display(), copied_bin.display())
    })?;
    set_executable(&copied_bin)?;

    if !inputs.skip_cargo_clean {
        let clean_log = exp_dir.join("cargo-clean.log");
        let clean_err = exp_dir.join("cargo-clean.stderr.log");
        inputs
            .cargo
            .clean(&worktree, &clean_log, &clean_err)?;
    }

    Ok(TargetOutcome::Benched { run_ids: Vec::new() })
}

/// Phase B for one target: one `bench run` invocation per
/// `verification_replay.invocations[]` entry, sequentially under
/// BENCH_LOCK. Run ids land in
/// `optimize/<target>/candidate-run-ids.json` as
/// [`InvocationRunIds`](crate::models::common::InvocationRunIds).
fn bench_one(inputs: &Inputs<'_>, target: &MergedTarget) -> Result<TargetOutcome> {
    let exp_dir = inputs
        .layout
        .experiment_dir(&target.id);
    let bin_path = exp_dir
        .join("bin")
        .join("stacks-bench");
    if !bin_path.is_file() {
        let reason = format!("no binary at {}", bin_path.display());
        eprintln!("no binary for {}, skipping", target.id);
        return Ok(TargetOutcome::Skipped { reason });
    }

    let invocations = crate::session::calibration::require_invocations(target)?;

    let bench = (inputs.bench_for_target)(&bin_path);
    let source_str = inputs
        .env
        .source_dir
        .to_string_lossy()
        .into_owned();
    let shadow_str = inputs
        .env
        .shadow_dir_root
        .map(|p| {
            p.to_string_lossy()
                .into_owned()
        });

    let mut run_ids = Vec::with_capacity(invocations.len());
    let mut entries = Vec::with_capacity(invocations.len());
    for inv in invocations {
        let run_dir = inputs
            .layout
            .experiment_candidate_invocation_dir(&target.id, &inv.id);
        fs::create_dir_all(&run_dir).with_context(|| format!("creating {}", run_dir.display()))?;
        let bench_name = format!("candidate-{}-{}", target.id, inv.id);
        // Verification bench: same profiler-flag set as the Phase 1.8 target
        // calibration baseline this results-analyzer will compare it to.
        // Pass 1c invariant — flag symmetry within a comparison.
        // Asymmetric profiling (e.g. lean verification bench vs rich calibration
        // baseline) lets profile overhead bias the comparison; see
        // [calibration.rs](super::calibration) for the matching calibration baseline
        // arg construction.
        let extra = crate::session::calibration::build_invocation_args(inv);
        let mut args: Vec<&str> = vec![
            "bench",
            "run",
            "--source",
            &source_str,
            "--network",
            inputs.env.network,
            "--name",
            &bench_name,
        ];
        if let Some(sd) = shadow_str.as_deref() {
            args.push("--shadow-dir-root");
            args.push(sd);
        }
        for a in &extra {
            args.push(a);
        }
        let stdout_path = inputs
            .layout
            .experiment_candidate_bench_run_json(&target.id, &inv.id);
        let stderr_path = inputs
            .layout
            .experiment_candidate_bench_run_stderr(&target.id, &inv.id);
        bench.invoke(InvokeOptions {
            args: &args,
            stdout: Some(&stdout_path),
            stderr: Some(&stderr_path),
            lock: Some(inputs.bench_lock),
        })?;

        let id = extract_run_id(&stdout_path)
            .with_context(|| format!("extracting run id for `{}` / `{}`", target.id, inv.id))?;
        run_ids.push(id);
        entries.push(crate::models::common::InvocationRunId {
            invocation_id: inv.id.clone(),
            run_id: id,
        });
    }

    let ids = crate::models::common::InvocationRunIds { entries };
    use crate::models::ValidateModel as _;
    ids.validate_model()
        .with_context(|| format!("invalid candidate-run-ids for `{}`", target.id))?;
    let ids_path = inputs
        .layout
        .experiment_candidate_run_ids_json(&target.id);
    let serialized = serde_json::to_string_pretty(&ids)
        .with_context(|| format!("serializing candidate-run-ids for {}", target.id))?;
    fs::write(&ids_path, format!("{serialized}\n"))
        .with_context(|| format!("writing {}", ids_path.display()))?;

    Ok(TargetOutcome::Benched { run_ids })
}

/// Strip the `0x` / `0X` prefix from a hex hash. `verification_replay`
/// stores hashes in artifact form (`0x...`) for readability +
/// unambiguity, but stacks-bench's `--txid` / `--block` flags reject
/// the prefix — they want raw 64-hex. This is the single boundary
/// where the prefix gets stripped; the rest of the system keeps the
/// prefixed form.
///
/// Public so `session::calibration` (Phase 1.8) can share the same
/// stripping convention as Phase 3 verification bench.
pub fn strip_hex_prefix(s: &str) -> String {
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s)
        .to_owned()
}

/// Skip predicate identical to the bash's two-pass check. Returns
/// `Some(reason)` when the target should be skipped, `None` otherwise.
fn static_skip_reason(inputs: &Inputs<'_>, target: &MergedTarget) -> Result<Option<String>> {
    if !target.bench_eligible {
        return Ok(Some(format!(
            "not bench_eligible (delivery_mode={})",
            match target.delivery_mode {
                DeliveryMode::NormalPr => "normal_pr",
                DeliveryMode::ConsensusPocPr => "consensus_poc_pr",
                DeliveryMode::ConsensusIssue => "consensus_issue",
            }
        )));
    }
    let exp_dir = inputs
        .layout
        .experiment_dir(&target.id);
    if exp_dir
        .join("consensus-issue.md")
        .exists()
    {
        return Ok(Some("consensus-issue (no optimizer ran)".to_owned()));
    }
    // Gate on the typed optimizer report (authoritative), not the
    // companion `abort.md` (could be stale after a demotion that
    // rewrote the JSON but left an old markdown copy around). Missing
    // report or `outcome=aborted` both block bench; a validation/
    // context error blocks too — benching against an unparseable
    // report would consume bench-lock for nothing.
    match loader::read_optimizer_report_for_target(inputs.layout, &target.id, target.delivery_mode)
    {
        Ok(Some(OptimizerReport::Implemented(_))) => {}
        Ok(Some(OptimizerReport::Aborted(_))) => {
            return Ok(Some("aborted (optimizer report outcome=aborted)".to_owned()));
        }
        Ok(None) => {
            return Ok(Some("aborted (no optimizer-report.json on disk)".to_owned()));
        }
        Err(e) => {
            return Ok(Some(format!("aborted (optimizer-report error: {e:#})")));
        }
    }
    Ok(None)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut perms = fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    perms.set_mode(perms.mode() | 0o111);
    fs::set_permissions(path, perms).with_context(|| format!("chmod +x {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    // No-op on non-unix; cargo build outputs are already executable.
    Ok(())
}

/// Convenience wrapper for the production bench-factory closure: builds a
/// `StacksBenchCli` whose `release_bin` is the per-target copied binary.
pub fn stacks_bench_for(binary: &Path, data_dir: &Path, cargo_cwd: &Path) -> Box<dyn BenchClient> {
    use crate::session::bench::StacksBenchCli;
    Box::new(StacksBenchCli {
        release_bin: Some(PathBuf::from(binary)),
        data_dir: data_dir.to_path_buf(),
        cargo_cwd: cargo_cwd.to_path_buf(),
        strict: false,
    })
}
