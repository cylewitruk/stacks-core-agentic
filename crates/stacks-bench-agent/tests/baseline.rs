//! Port verification for `session::baseline::{run, import}`.
//!
//! Uses a recording `BenchClient` fake that captures every invoke call and
//! writes canned JSON envelopes to whatever stdout path the implementation
//! requested. This lets us assert (1) the right stacks-bench subcommands
//! got invoked, in the right order, with the right flags, and (2) the
//! correct artifact files landed in the session results dir.

use std::path::PathBuf;

use parking_lot::Mutex;
use stacks_bench_agent::session::SessionLayout;
use stacks_bench_agent::session::baseline::{self, ImportInputs, RunInputs};
use stacks_bench_agent::session::bench::{BenchClient, InvokeOptions};
use stacks_bench_agent::types::SessionId;

/// Recording fake. Each `invoke` call captures (args, stdout-path) and
/// writes the matching canned JSON to the stdout path so downstream
/// `extract_run_id` reads succeed.
struct RecordingBench {
    /// `args` passed to each invoke, in order.
    calls: Mutex<Vec<Vec<String>>>,
    /// run_id emitted on the next `bench run` / `bench rerun` write
    /// (counter incremented with each call so the rerun gets a distinct id).
    next_run_id: Mutex<i64>,
    /// total_duration_us → run_id table (unused here but matches finalize
    /// tests).
    durations: Mutex<Vec<(i64, i64)>>,
}

impl RecordingBench {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            next_run_id: Mutex::new(500),
            durations: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().clone()
    }
}

impl BenchClient for RecordingBench {
    fn total_duration_us(&self, run_id: i64) -> anyhow::Result<Option<i64>> {
        Ok(self
            .durations
            .lock()
            .iter()
            .find(|(r, _)| *r == run_id)
            .map(|(_, d)| *d))
    }

    fn invoke(&self, opts: InvokeOptions<'_>) -> anyhow::Result<()> {
        let argv: Vec<String> = opts
            .args
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        self.calls
            .lock()
            .push(argv.clone());

        // Write canned JSON depending on subcommand. `bench run` and
        // `bench rerun` write an envelope carrying `data.run_id`. `bench show`
        // and `bench list` write whatever shape the script needs (here we
        // just write the same envelope for simplicity).
        if let Some(stdout_path) = opts.stdout {
            let body = match argv.as_slice() {
                [c1, c2, ..] if c1 == "bench" && (c2 == "run" || c2 == "rerun") => {
                    let mut id = self.next_run_id.lock();
                    *id += 1;
                    let value = *id;
                    serde_json::json!({ "data": { "run_id": value } }).to_string()
                }
                [c1, c2, _flag, run_id_val, ..] if c1 == "bench" && c2 == "show" => {
                    let parsed: i64 = run_id_val
                        .parse()
                        .unwrap_or(0);
                    serde_json::json!({ "data": { "run_id": parsed } }).to_string()
                }
                _ => "{}".to_owned(),
            };
            if let Some(parent) = stdout_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(stdout_path, body)?;
        }
        if let Some(stderr_path) = opts.stderr
            && let Some(parent) = stderr_path.parent()
        {
            std::fs::create_dir_all(parent)?;
            std::fs::write(stderr_path, b"")?;
        }
        Ok(())
    }
}

fn session_layout(tmp: &tempfile::TempDir) -> SessionLayout {
    let id: SessionId = "20260507-104400"
        .to_owned()
        .try_into()
        .unwrap();
    SessionLayout::new(tmp.path(), id)
}

#[test]
fn run_invokes_expected_bench_subcommands_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = session_layout(&tmp);
    let bench = RecordingBench::new();
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");

    let outputs = baseline::run(&RunInputs {
        layout: &layout,
        bench: &bench,
        source_dir: &PathBuf::from("/mnt/chainstate"),
        network: "mainnet",
        start_at: 5_000_000,
        count: 200_000,
        warmup: None,
        filter: None,
        shadow_dir_root: None,
        bench_lock: &bench_lock,
        single_run_noise_floor_pct: 1.0,
    })
    .expect("baseline::run");

    let calls = bench.calls();
    // Pass 1a: Phase 0b runs `bench run` once; the rerun is
    // aliased (no second invocation). Sequence is: run, list, show.
    assert_eq!(calls.len(), 3, "expected 3 invocations: run, list, show (no rerun under Pass 1a)");
    assert_eq!(calls[0][..2], ["bench", "run"]);
    assert!(
        calls[0]
            .iter()
            .any(|s| s == "--source")
    );
    assert!(
        calls[0]
            .iter()
            .any(|s| s == "/mnt/chainstate")
    );
    assert!(
        calls[0]
            .iter()
            .any(|s| s == "--name")
    );
    // Initial run carries the full profiler payload — triage needs it.
    assert!(
        !calls[0]
            .iter()
            .any(|s| s == "--bench-spans-only"),
        "initial baseline run must NOT carry --bench-spans-only (triage reads the full tree)"
    );
    assert_eq!(calls[1][..2], ["bench", "list"]);
    assert_eq!(calls[2][..2], ["bench", "show"]);
    assert!(
        calls[2]
            .iter()
            .any(|s| s == "--profiler-hot")
    );

    // Outputs land in the right files.
    assert_eq!(
        std::fs::read_to_string(layout.baseline_run_id_path())
            .unwrap()
            .trim(),
        outputs
            .baseline_run_id
            .to_string()
    );
    // Pass 1a: rerun id is aliased to run id (single value, both
    // files populated for backward compat).
    assert_eq!(
        std::fs::read_to_string(layout.baseline_rerun_id_path())
            .unwrap()
            .trim(),
        outputs
            .baseline_rerun_id
            .to_string()
    );
    assert_eq!(outputs.baseline_run_id, outputs.baseline_rerun_id);
    assert!(
        layout
            .baseline_bench_run_json()
            .is_file()
    );
    // Rerun JSON is aliased to bench-run JSON content (file copy).
    assert!(
        layout
            .baseline_rerun_json()
            .is_file()
    );
    assert!(
        layout
            .bench_list_json()
            .is_file()
    );
    assert!(
        layout
            .baseline_profiler_hotspots_json()
            .is_file()
    );
    // Noise floor file is written from the configured single-run
    // value.
    assert_eq!(
        std::fs::read_to_string(layout.baseline_noise_floor_path())
            .unwrap()
            .trim(),
        "1",
    );
}

/// Phase 0 baseline must also pass `--shadow-dir-root` when the operator
/// has configured `stacks_bench_shadow_dir` — otherwise a fresh
/// `session baseline run` against a read-only-parent source dir (e.g.
/// `/Volumes/Extern`) fails the same way the optimizer's inner-loop
/// bench would. Mirrors the assertion shape in
/// `tests/bench_experiments.
/// rs::bench_experiments_emits_shadow_dir_root_when_set`.
#[test]
fn baseline_run_emits_shadow_dir_root_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = session_layout(&tmp);
    let bench = RecordingBench::new();
    let bench_lock = tmp
        .path()
        .join("benchmark.lock");
    let shadow_root = tmp.path().join("shadows");

    let _ = baseline::run(&RunInputs {
        layout: &layout,
        bench: &bench,
        source_dir: &PathBuf::from("/mnt/chainstate"),
        network: "mainnet",
        start_at: 5_000_000,
        count: 200_000,
        warmup: None,
        filter: None,
        shadow_dir_root: Some(&shadow_root),
        bench_lock: &bench_lock,
        single_run_noise_floor_pct: 1.0,
    })
    .expect("baseline::run");

    let calls = bench.calls();
    // Pass 1a: only the initial `bench run` is invoked (rerun is
    // aliased, no second bench invocation). Verify that single call
    // carries `--shadow-dir-root` when the operator configured it.
    let shadow_str = shadow_root
        .to_string_lossy()
        .into_owned();
    let call = &calls[0];
    let idx = call
        .iter()
        .position(|a| a == "--shadow-dir-root")
        .unwrap_or_else(|| panic!("--shadow-dir-root missing in initial run: {call:?}"));
    assert_eq!(
        call[idx + 1],
        shadow_str,
        "--shadow-dir-root value mismatch in initial run: {call:?}",
    );
}

#[test]
fn import_with_companion_rerun_does_not_write_fallback_noise_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = session_layout(&tmp);
    let bench = RecordingBench::new();

    let outputs = baseline::import(&ImportInputs {
        layout: &layout,
        bench: &bench,
        run_id: 100,
        rerun_id: 101,
        single_run_noise_floor_pct: 1.5,
        bench_lock: &tmp.path().join("bench.lock"),
    })
    .expect("import");

    assert_eq!(outputs.baseline_run_id, 100);
    assert_eq!(outputs.baseline_rerun_id, 101);
    assert!(!outputs.single_run_fallback);
    let noise_floor = layout
        .results_dir
        .join("baseline-noise-floor-pct");
    assert!(!noise_floor.exists(), "no fallback file when run_id != rerun_id");

    let calls = bench.calls();
    assert_eq!(calls.len(), 4); // show, show, list, show --profiler-hot
    assert_eq!(calls[0][..2], ["bench", "show"]);
    assert!(
        calls[0]
            .iter()
            .any(|s| s == "100")
    );
    assert!(
        calls[1]
            .iter()
            .any(|s| s == "101")
    );
}

#[test]
fn import_single_run_writes_fallback_noise_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = session_layout(&tmp);
    let bench = RecordingBench::new();

    let outputs = baseline::import(&ImportInputs {
        layout: &layout,
        bench: &bench,
        run_id: 100,
        rerun_id: 100,
        single_run_noise_floor_pct: 1.5,
        bench_lock: &tmp.path().join("bench.lock"),
    })
    .expect("import");

    assert!(outputs.single_run_fallback);
    let body = std::fs::read_to_string(layout.baseline_noise_floor_path()).unwrap();
    assert_eq!(body.trim(), "1.5");
}

#[test]
fn import_rejects_run_id_mismatch() {
    // The recording fake writes data.run_id matching the requested arg, so
    // a deliberate mismatch via a custom impl proves the sanity check fires.
    struct LyingBench;
    impl BenchClient for LyingBench {
        fn total_duration_us(&self, _: i64) -> anyhow::Result<Option<i64>> {
            Ok(None)
        }
        fn invoke(&self, opts: InvokeOptions<'_>) -> anyhow::Result<()> {
            if let Some(p) = opts.stdout {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                // Write a different run_id than requested.
                std::fs::write(p, br#"{"data":{"run_id":999}}"#)?;
            }
            if let Some(p) = opts.stderr
                && let Some(parent) = p.parent()
            {
                std::fs::create_dir_all(parent)?;
                std::fs::write(p, b"")?;
            }
            Ok(())
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let layout = session_layout(&tmp);
    let err = baseline::import(&ImportInputs {
        layout: &layout,
        bench: &LyingBench,
        run_id: 100,
        rerun_id: 100,
        single_run_noise_floor_pct: 1.0,
        bench_lock: &tmp.path().join("bench.lock"),
    })
    .expect_err("expected sanity-check failure");

    assert!(
        err.to_string()
            .contains("run_id"),
        "expected mismatch error, got: {err}"
    );
}
