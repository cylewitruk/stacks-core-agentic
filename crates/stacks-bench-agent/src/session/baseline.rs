//! Phase 0: baseline binary archival + baseline benchmark.
//!
//! Three entry points:
//!
//! - [`archive_baseline_binary`] → Phase 0a (Pass 1a). Build the `stacks-bench`
//!   binary from `repos/stacks-core` HEAD, copy it to
//!   `<session>/results/baseline/bin/stacks-bench`, and write a manifest
//!   carrying source sha + dirty-worktree flag + build metadata. Downstream
//!   baseline / calibration / full-range fallback paths all read from this
//!   archived path via the strict-binary contract. Runs BEFORE Phase 0b
//!   (whether fresh baseline or imported).
//! - [`run`] → Phase 0b. One `stacks-bench bench run` invocation against the
//!   archived binary, then the rerun id is aliased to the run id (no second
//!   `bench` invocation under Pass 1a — see
//!   [baseline-verification-agent-plan.md](../../../../
//!   baseline-verification-agent-plan.md) Sub-step B). The noise floor falls
//!   back to `settings.triage.single_run_noise_floor_pct`. Serialized via
//!   BENCH_LOCK, then captures bench-list + profiler hotspots metadata.
//! - [`import`] → `scripts/import-baseline.sh` — reconstructs the baseline
//!   artifact set from existing run ids in the persistent stacks-bench db.
//!   Phase 0a still runs (Phase 1.8 needs the archived binary regardless of how
//!   Phase 0b was resolved).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::analyzed_rejections::now_utc_iso8601;
use crate::models::ToJson;
use crate::session::SessionLayout;
use crate::session::bench::{BenchClient, InvokeOptions, extract_run_id};
use crate::settings::Settings;

/// Inputs to [`archive_baseline_binary`] (Phase 0a).
pub struct ArchiveBinaryInputs<'a> {
    pub layout: &'a SessionLayout,
    /// Operator's `repos/stacks-core` checkout (where `cargo build`
    /// runs). The submodule HEAD sha is captured into the manifest.
    pub stacks_core_base: &'a Path,
}

/// Outputs of [`archive_baseline_binary`].
#[derive(Debug)]
pub struct ArchiveBinaryOutputs {
    /// Absolute path to the archived binary under
    /// `<session>/results/baseline/bin/stacks-bench`. This is the
    /// path every downstream "use the baseline binary" code path
    /// reads from.
    pub archived_path: PathBuf,
    /// `repos/stacks-core` submodule HEAD sha at archive time.
    pub source_sha: String,
}

/// Phase 0a: build + archive the `stacks-bench` binary that the rest
/// of the session uses as the deterministic baseline reference. Runs
/// BEFORE Phase 0b's first bench invocation. See
/// [`baseline-verification-agent-plan.md`](../../../../
/// baseline-verification-agent-plan.md) (Pass 1a, Sub-step A).
///
/// Steps:
///
/// 1. `git rev-parse HEAD` on `stacks_core_base` → source_sha.
/// 2. `cargo build --release -p stacks-bench` in that checkout.
/// 3. Copy `target/release/stacks-bench` to
///    `<session>/results/baseline/bin/stacks-bench`.
/// 4. Write `<session>/results/baseline/bin/manifest.json` with `{source_sha,
///    cargo_version, build_flags, archived_at}`.
pub fn archive_baseline_binary(inputs: &ArchiveBinaryInputs<'_>) -> Result<ArchiveBinaryOutputs> {
    let base = inputs.stacks_core_base;
    if !base.is_dir() {
        bail!(
            "stacks-core base checkout missing at {} (required for Phase 0a binary archival)",
            base.display(),
        );
    }
    let source_sha = crate::git::rev_parse_head(base)
        .with_context(|| format!("reading HEAD sha of {}", base.display()))?;
    // Detect uncommitted changes BEFORE the build. A dirty
    // worktree means `source_sha` doesn't fully identify the
    // archived binary; we still allow the build (operators may be
    // testing local stacks-bench changes intentionally) but
    // surface it in the manifest so downstream audit can tell.
    let dirty_worktree = crate::git::is_worktree_dirty(base);
    if dirty_worktree {
        eprintln!(
            "Phase 0a: stacks-core checkout at {} has uncommitted changes; archived binary will \
             be marked dirty=true in manifest. source_sha={} alone does NOT identify the built \
             binary in this case.",
            base.display(),
            source_sha,
        );
    }

    // cargo build --release -p stacks-bench (same invocation as
    // session/cargo.rs's CargoRunner; no-op if HEAD is already built).
    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg("stacks-bench")
        .current_dir(base)
        .status()
        .with_context(|| {
            format!("invoking `cargo build --release -p stacks-bench` in {}", base.display())
        })?;
    if !status.success() {
        bail!("`cargo build --release -p stacks-bench` exited {status} in {}", base.display(),);
    }

    let built = base
        .join("target")
        .join("release")
        .join("stacks-bench");
    if !built.is_file() {
        bail!(
            "expected built binary at {} after `cargo build` succeeded — not found",
            built.display(),
        );
    }

    // Copy into session artifacts.
    let archive_dir = inputs
        .layout
        .baseline_bin_dir();
    fs::create_dir_all(&archive_dir)
        .with_context(|| format!("creating {}", archive_dir.display()))?;
    let archived_path = inputs
        .layout
        .baseline_bin_path();
    fs::copy(&built, &archived_path)
        .with_context(|| format!("copying {} → {}", built.display(), archived_path.display()))?;

    // Manifest.
    let cargo_version = Command::new("cargo")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_owned())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let manifest = crate::models::baseline_binary_manifest::BaselineBinaryManifest {
        source_sha: source_sha.clone(),
        dirty: dirty_worktree,
        cargo_version,
        build_flags: vec!["--release".to_owned(), "-p".to_owned(), "stacks-bench".to_owned()],
        archived_at: now_utc_iso8601(),
        archived_path: archived_path.clone(),
    };
    let manifest_path = inputs
        .layout
        .baseline_bin_manifest_path();
    let json = manifest
        .to_json_pretty()
        .context("serializing baseline binary manifest")?;
    fs::write(&manifest_path, format!("{json}\n"))
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    Ok(ArchiveBinaryOutputs { archived_path, source_sha })
}

/// Inputs to a fresh baseline benchmark.
pub struct RunInputs<'a> {
    /// The session layout (must already exist; results dir is created here).
    pub layout: &'a SessionLayout,
    /// `BenchClient` used to invoke `cargo stacks-bench`.
    pub bench: &'a dyn BenchClient,
    /// Source dir (`--source` arg, contains `chainstate/`).
    pub source_dir: &'a std::path::Path,
    /// Network identifier (`--network` arg).
    pub network: &'a str,
    /// Block range start (`--start-at` arg).
    pub start_at: u64,
    /// Block count (`--count` arg).
    pub count: u64,
    /// Optional pre-measurement warmup (`--warmup` arg).
    pub warmup: Option<u64>,
    /// Optional `--filter` arg (e.g. `contract-call`).
    pub filter: Option<&'a str>,
    /// Optional `--shadow-dir-root` arg. Same semantics as
    /// [`crate::session::bench_experiments::BenchRange::shadow_dir_root`]:
    /// override stacks-bench's default shadow location when the source
    /// dir's parent isn't writable from the executing process. Must be
    /// on the same filesystem as `source_dir` — stacks-bench's reflink
    /// guard refuses cross-FS shadows.
    pub shadow_dir_root: Option<&'a std::path::Path>,
    /// BENCH_LOCK path — flock'd around each `bench run` invocation (both
    /// the initial and the rerun).
    pub bench_lock: &'a std::path::Path,
    /// Noise floor percent to record at
    /// `baseline/noise-floor-pct` under the Pass 1a aliased-rerun
    /// contract (Phase 0b skips the empirical rerun and falls back
    /// to this constant). Default sourced from
    /// [`Settings::single_run_noise_floor_pct`].
    pub single_run_noise_floor_pct: f64,
}

/// Outputs of a successful baseline run.
#[derive(Debug)]
pub struct RunOutputs {
    /// Newly-recorded run id from the fresh `bench run` against
    /// the archived baseline binary.
    pub baseline_run_id: i64,
    /// Under Pass 1a's aliased-rerun contract, equal to
    /// `baseline_run_id` — the `bench rerun` invocation is skipped
    /// and both id files are populated with the same value.
    /// Schema contracts (triage, summary, archive ledger, baseline
    /// import) stay intact. Pass 1b later migrates this to
    /// `Option<i64>` once the consumers are updated.
    pub baseline_rerun_id: i64,
}

/// Phase 0b: run one `stacks-bench bench run` against the archived
/// baseline binary and alias the rerun id to the run id (no second
/// bench invocation — see module-level doc + Sub-step B of the
/// execution plan). Captures bench-list + profiler hotspots
/// metadata. All bench invocations are serialized via BENCH_LOCK.
pub fn run(inputs: &RunInputs<'_>) -> Result<RunOutputs> {
    fs::create_dir_all(&inputs.layout.results_dir).with_context(|| {
        format!(
            "creating {}",
            inputs
                .layout
                .results_dir
                .display()
        )
    })?;

    let session_id = inputs.layout.id.as_str();
    let bench_name = format!("baseline-{session_id}");

    // 1. bench run
    let start_at = inputs.start_at.to_string();
    let count = inputs.count.to_string();
    let warmup = inputs
        .warmup
        .map(|w| w.to_string());
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
    let mut run_args: Vec<&str> = vec![
        "bench",
        "run",
        "--source",
        &source_str,
        "--network",
        inputs.network,
        "--start-at",
        &start_at,
        "--count",
        &count,
        "--name",
        &bench_name,
    ];
    if let Some(sd) = shadow_str.as_deref() {
        run_args.push("--shadow-dir-root");
        run_args.push(sd);
    }
    if let Some(w) = warmup.as_deref() {
        run_args.push("--warmup");
        run_args.push(w);
    }
    if let Some(f) = inputs.filter {
        run_args.push("--filter");
        run_args.push(f);
    }
    let baseline_run_json = inputs
        .layout
        .baseline_bench_run_json();
    let baseline_run_stderr = inputs
        .layout
        .baseline_bench_run_stderr();
    inputs
        .bench
        .invoke(InvokeOptions {
            args: &run_args,
            stdout: Some(&baseline_run_json),
            stderr: Some(&baseline_run_stderr),
            lock: Some(inputs.bench_lock),
        })?;
    let baseline_run_id = extract_run_id(&baseline_run_json)?;
    fs::write(
        inputs
            .layout
            .baseline_run_id_path(),
        format!("{baseline_run_id}\n"),
    )?;

    // 2. Phase 0b under Pass 1a: rerun id aliased to run id. The `bench rerun`
    //    invocation is skipped because per-target targeted calibration (Phase 1.8)
    //    now provides the apples-to-apples noise basis for the dominant comparison
    //    path. Full-range comparisons that DO need an empirical noise floor wait
    //    for Pass 1b (lazy in-phase calibration). Until then, the noise floor for
    //    full-range comparisons falls back to `triage.single_run_noise_floor_pct`,
    //    the existing single-run-fallback path the framework supports.
    //
    //    Both `baseline-run-id` and `baseline-rerun-id` files are
    //    populated with the same value so existing consumers
    //    (triage, summary, archive ledger, baseline import) see no
    //    schema breakage. Pass 1b later migrates the rerun id to
    //    `Option<i64>`.
    let baseline_rerun_id = baseline_run_id;
    fs::write(
        inputs
            .layout
            .baseline_rerun_id_path(),
        format!("{baseline_rerun_id}\n"),
    )?;
    // Alias the rerun JSON to the run JSON so validate.rs's
    // require-non-empty check still passes without us actually
    // re-running. Pass 1b makes `baseline/rerun.json` optional once
    // `baseline_rerun_id` becomes `Option<i64>` and validate is
    // updated.
    fs::copy(
        &baseline_run_json,
        inputs
            .layout
            .baseline_rerun_json(),
    )?;
    let single_run_noise_floor_pct = inputs.single_run_noise_floor_pct;
    eprintln!(
        "baseline rerun aliased to run id {baseline_run_id}; using configured single-run noise \
         floor {single_run_noise_floor_pct}%",
    );
    fs::write(
        inputs
            .layout
            .baseline_noise_floor_path(),
        format!("{single_run_noise_floor_pct}\n"),
    )?;
    let run_id_str = baseline_run_id.to_string();

    // 3. bench list. Reads from the persistent SQLite DB shared with
    // every other `bench run`/`bench rerun`. Take the bench lock so a
    // concurrent sbagent process can't mutate the DB mid-read and
    // produce a partial JSON envelope.
    let list_args: Vec<&str> = vec!["bench", "list", "--all", "--with-args", "--limit", "100"];
    inputs
        .bench
        .invoke(InvokeOptions {
            args: &list_args,
            stdout: Some(
                &inputs
                    .layout
                    .bench_list_json(),
            ),
            stderr: None,
            lock: Some(inputs.bench_lock),
        })?;

    // 4. bench show --profiler-hot 50. Same lock contract as `bench
    // list` above.
    let show_args: Vec<&str> =
        vec!["bench", "show", "--run-id", &run_id_str, "--profiler-hot", "50"];
    inputs
        .bench
        .invoke(InvokeOptions {
            args: &show_args,
            stdout: Some(
                &inputs
                    .layout
                    .baseline_profiler_hotspots_json(),
            ),
            stderr: None,
            lock: Some(inputs.bench_lock),
        })?;

    Ok(RunOutputs {
        baseline_run_id,
        baseline_rerun_id,
    })
}

/// Inputs to importing an existing baseline.
pub struct ImportInputs<'a> {
    /// Session layout (results dir created here).
    pub layout: &'a SessionLayout,
    /// `BenchClient` used to drive `bench show` / `bench list`.
    pub bench: &'a dyn BenchClient,
    /// Run id to import as the baseline.
    pub run_id: i64,
    /// Run id to import as the rerun. When equal to `run_id`, writes the
    /// `triage.single_run_noise_floor_pct` fallback file.
    pub rerun_id: i64,
    /// Conservative noise-floor fallback used when `run_id == rerun_id`.
    /// Defaults to 1.0% (mirrors the bash
    /// `${SINGLE_RUN_NOISE_FLOOR_PCT:-1.0}`).
    pub single_run_noise_floor_pct: f64,
    /// Bench lock path. The import path runs `bench show` / `bench list`
    /// against the same persistent SQLite DB another sbagent process
    /// might be writing to; serialize via this lock to avoid partial
    /// reads.
    pub bench_lock: &'a Path,
}

impl<'a> ImportInputs<'a> {
    /// Construct from a settings record, resolving the noise-floor default.
    pub fn from_settings(
        layout: &'a SessionLayout,
        bench: &'a dyn BenchClient,
        run_id: i64,
        rerun_id: Option<i64>,
        settings: &Settings,
        bench_lock: &'a Path,
    ) -> Self {
        Self {
            layout,
            bench,
            run_id,
            rerun_id: rerun_id.unwrap_or(run_id),
            single_run_noise_floor_pct: settings
                .triage
                .single_run_noise_floor_pct
                .unwrap_or(1.0),
            bench_lock,
        }
    }
}

/// Outputs of a successful import.
#[derive(Debug)]
pub struct ImportOutputs {
    /// Imported baseline run id.
    pub baseline_run_id: i64,
    /// Imported rerun id (== `baseline_run_id` for single-run imports).
    pub baseline_rerun_id: i64,
    /// True iff this was a single-run import (no companion rerun); the
    /// fallback noise-floor file is written in that case.
    pub single_run_fallback: bool,
}

/// Reconstruct baseline artifacts from existing run ids. Mirrors
/// `scripts/import-baseline.sh`.
pub fn import(inputs: &ImportInputs<'_>) -> Result<ImportOutputs> {
    fs::create_dir_all(&inputs.layout.results_dir).with_context(|| {
        format!(
            "creating {}",
            inputs
                .layout
                .results_dir
                .display()
        )
    })?;

    let run_id_str = inputs.run_id.to_string();
    let rerun_id_str = inputs.rerun_id.to_string();

    // Reconstruct baseline-bench-run.json from `bench show`. NB the show
    // envelope has slightly different non-essential fields than `bench run`
    // does, but the fields downstream readers consume (.data.run_id and
    // .data.summary.total_duration_us) are present in both. Lock the
    // SQLite DB across these reads so a concurrent writer can't shift
    // state between them.
    let show_run_args: Vec<&str> = vec!["bench", "show", "--run-id", &run_id_str];
    inputs
        .bench
        .invoke(InvokeOptions {
            args: &show_run_args,
            stdout: Some(
                &inputs
                    .layout
                    .baseline_bench_run_json(),
            ),
            stderr: Some(
                &inputs
                    .layout
                    .baseline_bench_run_stderr(),
            ),
            lock: Some(inputs.bench_lock),
        })?;

    let show_rerun_args: Vec<&str> = vec!["bench", "show", "--run-id", &rerun_id_str];
    inputs
        .bench
        .invoke(InvokeOptions {
            args: &show_rerun_args,
            stdout: Some(
                &inputs
                    .layout
                    .baseline_rerun_json(),
            ),
            stderr: Some(
                &inputs
                    .layout
                    .baseline_rerun_stderr(),
            ),
            lock: Some(inputs.bench_lock),
        })?;

    fs::write(
        inputs
            .layout
            .baseline_run_id_path(),
        format!("{}\n", inputs.run_id),
    )?;
    fs::write(
        inputs
            .layout
            .baseline_rerun_id_path(),
        format!("{}\n", inputs.rerun_id),
    )?;

    let list_args: Vec<&str> = vec!["bench", "list", "--all", "--with-args", "--limit", "100"];
    inputs
        .bench
        .invoke(InvokeOptions {
            args: &list_args,
            stdout: Some(
                &inputs
                    .layout
                    .bench_list_json(),
            ),
            stderr: None,
            lock: Some(inputs.bench_lock),
        })?;

    let show_hot_args: Vec<&str> =
        vec!["bench", "show", "--run-id", &run_id_str, "--profiler-hot", "50"];
    inputs
        .bench
        .invoke(InvokeOptions {
            args: &show_hot_args,
            stdout: Some(
                &inputs
                    .layout
                    .baseline_profiler_hotspots_json(),
            ),
            stderr: None,
            lock: Some(inputs.bench_lock),
        })?;

    // Sanity check: the shown envelope's run_id must equal the requested id.
    let raw = fs::read(
        inputs
            .layout
            .baseline_bench_run_json(),
    )
    .context("re-reading baseline-bench-run.json for sanity check")?;
    let parsed: Value = serde_json::from_slice(&raw).context("parsing baseline-bench-run.json")?;
    let actual = parsed
        .get("data")
        .and_then(|d| d.get("run_id"))
        .and_then(|v| v.as_i64());
    if actual != Some(inputs.run_id) {
        bail!(
            "import-baseline: requested run_id {} but `bench show` returned {:?}; check that the \
             run id exists in the stacks-bench db",
            inputs.run_id,
            actual
        );
    }

    let single_run_fallback = inputs.run_id == inputs.rerun_id;
    let noise_floor_path = inputs
        .layout
        .baseline_noise_floor_path();
    if single_run_fallback {
        fs::write(&noise_floor_path, format!("{}\n", inputs.single_run_noise_floor_pct))?;
    } else if noise_floor_path.exists() {
        // Clear any stale fallback from a prior single-run import.
        let _ = fs::remove_file(&noise_floor_path);
    }

    Ok(ImportOutputs {
        baseline_run_id: inputs.run_id,
        baseline_rerun_id: inputs.rerun_id,
        single_run_fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `archive_baseline_binary` fails fast when the stacks-core
    /// base path doesn't exist (rather than running `cargo build` in
    /// a non-existent directory).
    #[test]
    fn archive_fails_fast_on_missing_base_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = SessionLayout::new(
            tmp.path(),
            "20260520-100000-test"
                .to_owned()
                .try_into()
                .unwrap(),
        );
        let bogus_base = tmp.path().join("nope");
        let err = archive_baseline_binary(&ArchiveBinaryInputs {
            layout: &layout,
            stacks_core_base: &bogus_base,
        })
        .expect_err("missing base must surface clearly");
        let msg = format!("{err:#}");
        assert!(msg.contains("stacks-core base checkout missing"), "got: {msg}");
    }
}
