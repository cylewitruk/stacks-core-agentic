//! Phase 3: build per-target release binaries, copy them out, then run
//! two benchmarks per target serialized under BENCH_LOCK. Port of
//! `scripts/bench-experiments.sh`.
//!
//! Per target, the phase has two halves:
//! 1. **Build** — `cargo build --release -p stacks-bench` inside the target's
//!    worktree, copy the produced binary to `optimize/<id>/bin/stacks-bench`,
//!    optional `cargo clean`. Builds can run in parallel; the bash uses a
//!    sequential loop.
//! 2. **Bench** — for run-1 and run-2, invoke the per-target binary with the
//!    same range args the baseline used, capturing stdout/stderr to
//!    `run-N/bench-run.json`. Wrapped in flock so two targets can't contend for
//!    the host. After each run, the run id is appended to
//!    `optimize/<id>/run-ids` (truncated up-front for idempotency).
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
    /// Bench range args (forwarded to `bench run`).
    pub range: BenchRange<'a>,
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

/// Bench range args.
#[derive(Debug, Clone, Copy)]
pub struct BenchRange<'a> {
    /// `--source` arg.
    pub source_dir: &'a Path,
    /// `--network` arg.
    pub network: &'a str,
    /// `--start-at` arg.
    pub start_at: u64,
    /// `--count` arg.
    pub count: u64,
    /// Optional `--warmup` arg.
    pub warmup: Option<u64>,
    /// Optional `--filter` arg (e.g. `contract-call`).
    pub filter: Option<&'a str>,
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
        /// Run ids appended to `optimize/<id>/run-ids` (one per run, in
        /// order).
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

/// Phase B for one target: two `bench run` invocations under BENCH_LOCK,
/// with run ids appended to `run-ids`.
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

    // Idempotency: truncate run-ids before re-populating so a re-run of
    // this phase doesn't leave stale ids alongside fresh ones.
    let run_ids_path = inputs
        .layout
        .experiment_run_ids_path(&target.id);
    fs::write(&run_ids_path, b"")
        .with_context(|| format!("truncating {}", run_ids_path.display()))?;

    let bench = (inputs.bench_for_target)(&bin_path);
    let source_str = inputs
        .range
        .source_dir
        .to_string_lossy()
        .into_owned();
    let shadow_str = inputs
        .range
        .shadow_dir_root
        .map(|p| {
            p.to_string_lossy()
                .into_owned()
        });

    let phases = build_phases(target, &inputs.range);
    let mut run_ids = Vec::with_capacity(phases.len());
    for (idx, phase) in phases.iter().enumerate() {
        let n = idx + 1;
        let run_dir = exp_dir.join(format!("run-{n}"));
        fs::create_dir_all(&run_dir).with_context(|| format!("creating {}", run_dir.display()))?;
        let bench_name = format!("{}-{}", target.id, phase.bench_name_suffix);
        // Candidate bench: capture only stacks-bench-generated spans and skip
        // profiler key-value records. Finalize reads only `total_duration_us`
        // from these runs; the full profiler tree + k-v records would just
        // bloat the persistent SQLite DB and the post-bench stage/merge.
        let mut args: Vec<&str> = vec![
            "bench",
            "run",
            "--source",
            &source_str,
            "--network",
            inputs.range.network,
            "--name",
            &bench_name,
            "--bench-spans-only",
            "--no-profiler-kv",
        ];
        if let Some(sd) = shadow_str.as_deref() {
            args.push("--shadow-dir-root");
            args.push(sd);
        }
        for a in &phase.extra_args {
            args.push(a);
        }
        let stdout_path = run_dir.join("bench-run.json");
        let stderr_path = run_dir.join("bench-run.stderr.log");
        bench.invoke(InvokeOptions {
            args: &args,
            stdout: Some(&stdout_path),
            stderr: Some(&stderr_path),
            lock: Some(inputs.bench_lock),
        })?;

        let id = extract_run_id(&stdout_path)
            .with_context(|| format!("extracting run id for {} run-{n}", target.id))?;
        run_ids.push(id);

        // Append to run-ids
        let prior = fs::read_to_string(&run_ids_path).unwrap_or_default();
        let mut updated = prior;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&id.to_string());
        updated.push('\n');
        fs::write(&run_ids_path, updated)?;
    }

    Ok(TargetOutcome::Benched { run_ids })
}

/// Strip the `0x` / `0X` prefix from a hex hash. `verification_replay`
/// stores hashes in artifact form (`0x...`) for readability +
/// unambiguity, but stacks-bench's `--txid` / `--block` flags reject
/// the prefix — they want raw 64-hex. This is the single boundary
/// where the prefix gets stripped; the rest of the system keeps the
/// prefixed form.
fn strip_hex_prefix(s: &str) -> String {
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s)
        .to_owned()
}

/// One bench invocation's worth of CLI arg variation. Either a full-range
/// phase (`--start-at`/`--count`/`--filter`) or a targeted-replay phase
/// (`--txid` repeated, or `--block` repeated, with `--repetitions`). The
/// returned vector flattens into `stacks-bench bench run` after the
/// per-invocation common prefix (`--source`/`--network`/`--name`).
struct BenchPhase {
    /// Suffix appended to the bench `--name` arg so operators can tell
    /// per-target phases apart in the stacks-bench UI. Example values:
    /// `"run-1"`, `"txids"`, `"blocks"`.
    bench_name_suffix: String,
    /// Args specific to this phase, in CLI order.
    extra_args: Vec<String>,
}

/// Build the phase list for a single target. Targeted replay (per
/// `target.verification_replay`) produces one phase per non-empty section
/// (`txids` ⇒ one phase, `blocks` ⇒ one phase, both ⇒ two phases — they
/// can't be combined in a single `stacks-bench` invocation because the
/// flags conflict). Absence falls back to two full-range invocations
/// (variance smoothing over the whole block range).
fn build_phases(target: &MergedTarget, range: &BenchRange<'_>) -> Vec<BenchPhase> {
    if let Some(vr) = &target.verification_replay {
        let mut phases = Vec::with_capacity(2);
        let reps = vr.repetitions.to_string();
        // Default warmup = 10 when the recipe omits it. Cold-fork
        // single-tx / single-block replay starts with empty caches; ~10
        // discarded reps lets MARF + SQLite caches settle before
        // measurement.
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
            phases.push(BenchPhase {
                bench_name_suffix: "txids".to_owned(),
                extra_args: extra,
            });
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
            phases.push(BenchPhase {
                bench_name_suffix: "blocks".to_owned(),
                extra_args: extra,
            });
        }
        if !phases.is_empty() {
            return phases;
        }
        // verification_replay was present but empty on both sides; fall
        // through to full-range. (`VerificationReplay::validate` would
        // normally catch this, but defense-in-depth at the bench layer.)
    }

    // Full-range default: two invocations for variance smoothing.
    (1..=2u32)
        .map(|n| {
            let mut extra = Vec::with_capacity(6);
            extra.push("--start-at".to_owned());
            extra.push(range.start_at.to_string());
            extra.push("--count".to_owned());
            extra.push(range.count.to_string());
            if let Some(w) = range.warmup {
                extra.push("--warmup".to_owned());
                extra.push(w.to_string());
            }
            if let Some(f) = range.filter {
                extra.push("--filter".to_owned());
                extra.push(f.to_owned());
            }
            BenchPhase {
                bench_name_suffix: format!("run-{n}"),
                extra_args: extra,
            }
        })
        .collect()
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
    })
}
