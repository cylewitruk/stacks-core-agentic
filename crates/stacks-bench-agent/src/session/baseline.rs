//! Phase 0: baseline benchmark.
//!
//! Two entry points port the bash scripts wholesale:
//! - [`run`] → `scripts/run-baseline.sh` — fresh `bench run` + a second `bench
//!   run` (issued in lieu of `bench rerun` so the second invocation can carry
//!   `--bench-spans-only --no-profiler-kv`; the noise-floor computation reads
//!   only `total_duration_us`, so the full profiler tree on the rerun would be
//!   discarded). Serialized via BENCH_LOCK, then captures bench-list and
//!   profiler hotspots metadata.
//! - [`import`] → `scripts/import-baseline.sh` — reconstructs the same artifact
//!   set from existing run ids in the persistent stacks-bench db.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::session::SessionLayout;
use crate::session::bench::{BenchClient, InvokeOptions, extract_run_id};
use crate::settings::Settings;

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
}

/// Outputs of a successful baseline run.
#[derive(Debug)]
pub struct RunOutputs {
    /// Newly-recorded run id from the fresh `bench run`.
    pub baseline_run_id: i64,
    /// Newly-recorded run id from the companion rerun (a `bench run` against
    /// the same range with `--bench-spans-only --no-profiler-kv`).
    pub baseline_rerun_id: i64,
}

/// Run a fresh baseline benchmark + rerun, then capture metadata. Mirrors
/// `scripts/run-baseline.sh` end-to-end.
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

    // 2. baseline rerun — re-issued as `bench run` (NOT `bench rerun`) so we can
    //    pass `--bench-spans-only --no-profiler-kv`. The rerun's only consumer is
    //    the noise-floor computation, which reads `total_duration_us` only; the
    //    full profiler tree + k-v records captured by the default `bench rerun`
    //    would be discarded. Same range args as the initial run; distinct `--name`
    //    for audit.
    let run_id_str = baseline_run_id.to_string();
    let rerun_name = format!("{bench_name}-rerun");
    let mut rerun_args: Vec<&str> = vec![
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
        &rerun_name,
        "--bench-spans-only",
        "--no-profiler-kv",
    ];
    if let Some(sd) = shadow_str.as_deref() {
        rerun_args.push("--shadow-dir-root");
        rerun_args.push(sd);
    }
    if let Some(w) = warmup.as_deref() {
        rerun_args.push("--warmup");
        rerun_args.push(w);
    }
    if let Some(f) = inputs.filter {
        rerun_args.push("--filter");
        rerun_args.push(f);
    }
    let baseline_rerun_json = inputs
        .layout
        .baseline_rerun_json();
    let baseline_rerun_stderr = inputs
        .layout
        .baseline_rerun_stderr();
    inputs
        .bench
        .invoke(InvokeOptions {
            args: &rerun_args,
            stdout: Some(&baseline_rerun_json),
            stderr: Some(&baseline_rerun_stderr),
            lock: Some(inputs.bench_lock),
        })?;
    let baseline_rerun_id = extract_run_id(&baseline_rerun_json)?;
    fs::write(
        inputs
            .layout
            .baseline_rerun_id_path(),
        format!("{baseline_rerun_id}\n"),
    )?;

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
    /// `single_run_noise_floor_pct` fallback file.
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
