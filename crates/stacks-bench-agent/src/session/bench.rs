//! `BenchClient` trait — abstracts `cargo stacks-bench` invocations so the
//! ported phases can be tested with a fake.
//!
//! The trait has two methods:
//! - [`total_duration_us`] — typed convenience for finalize's bench-show
//!   summary lookup.
//! - [`invoke`] — generic shell-out to `cargo stacks-bench --db DATA --json
//!   <args>` with optional stdout/stderr redirection and BENCH_LOCK
//!   serialization. Used by every other ported phase that needs to drive the
//!   bench binary.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use serde_json::Value;

/// Options for [`BenchClient::invoke`].
#[derive(Debug, Default)]
pub struct InvokeOptions<'a> {
    /// Subcommand args (e.g. `["bench", "run", "--source", "/path", ...]`).
    /// `--db <data>` and `--json` are prepended automatically.
    pub args: &'a [&'a str],
    /// File to receive captured stdout. `None` inherits the parent's stdout.
    pub stdout: Option<&'a Path>,
    /// File to receive captured stderr. `None` inherits the parent's stderr.
    pub stderr: Option<&'a Path>,
    /// When `Some`, take an exclusive flock on this path before invoking
    /// and release after. Mirrors bash `flock $BENCH_LOCK`.
    pub lock: Option<&'a Path>,
}

/// `BenchClient` abstracts the stacks-bench CLI so tests can inject a fake
/// that records calls + writes canned JSON to the requested paths.
pub trait BenchClient: Send + Sync {
    /// Return `total_duration_us` for `run_id`, or `None` if the run has no
    /// summary (e.g. interrupted run, or a run id that doesn't exist).
    fn total_duration_us(&self, run_id: i64) -> Result<Option<i64>>;

    /// Invoke `cargo stacks-bench --db DATA --json <opts.args>`. When
    /// `opts.stdout`/`opts.stderr` are `Some`, captures the stream to that
    /// file (truncating any prior contents). When `opts.lock` is `Some`,
    /// holds an exclusive `flock` on that path for the duration.
    fn invoke(&self, opts: InvokeOptions<'_>) -> Result<()>;
}

/// Default implementation: shell out to `cargo stacks-bench` (or a prebuilt
/// release binary when present).
pub struct StacksBenchCli {
    /// Path to `<stacks-core>/target/release/stacks-bench`. When the file
    /// exists, used directly to skip cargo's lockfile check on every call.
    /// Otherwise falls back to `cargo stacks-bench`.
    pub release_bin: Option<PathBuf>,
    /// Stacks-bench data dir (passed as `--db`).
    pub data_dir: PathBuf,
    /// Working directory for `cargo stacks-bench` fallback.
    pub cargo_cwd: PathBuf,
}

impl BenchClient for StacksBenchCli {
    fn total_duration_us(&self, run_id: i64) -> Result<Option<i64>> {
        self.ensure_data_dir()?;
        let mut cmd = self.build_cmd();
        cmd.arg("--db")
            .arg(&self.data_dir)
            .arg("--json")
            .arg("bench")
            .arg("show")
            .arg("--run-id")
            .arg(run_id.to_string());
        let output = cmd
            .output()
            .with_context(|| format!("invoking stacks-bench bench show --run-id {run_id}"))?;
        if !output.status.success() {
            // Mirror the bash behavior: a missing run silently falls through.
            return Ok(None);
        }
        let envelope: BenchShowEnvelope = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("parsing bench show envelope for run_id={run_id}"))?;
        Ok(envelope
            .data
            .and_then(|d| d.summary)
            .and_then(|s| s.total_duration_us))
    }

    fn invoke(&self, opts: InvokeOptions<'_>) -> Result<()> {
        self.ensure_data_dir()?;
        // Materialize the lock file (the lock guard reserves the rest of the
        // function via fd-lock). Order matters: `lock_holder` must outlive
        // `_lock_guard` so the guard is dropped (releasing) before the file.
        let mut lock_holder = match opts.lock {
            Some(p) => Some(open_lock_file(p)?),
            None => None,
        };
        let _lock_guard = match lock_holder.as_mut() {
            Some(l) => Some(
                l.write()
                    .with_context(|| "acquiring bench-lock".to_string())?,
            ),
            None => None,
        };

        let mut cmd = self.build_cmd();
        cmd.arg("--db")
            .arg(&self.data_dir)
            .arg("--json");
        for arg in opts.args {
            cmd.arg(arg);
        }
        if let Some(p) = opts.stdout {
            cmd.stdout(open_for_write(p)?);
        }
        if let Some(p) = opts.stderr {
            cmd.stderr(open_for_write(p)?);
        }

        let status = cmd
            .status()
            .with_context(|| format!("invoking stacks-bench {}", opts.args.join(" ")))?;
        if !status.success() {
            bail!("stacks-bench {} exited with {status}", opts.args.join(" "));
        }
        Ok(())
    }
}

impl StacksBenchCli {
    /// Ensure `data_dir` exists before passing it as `--db` to
    /// `stacks-bench`. If the directory is missing, `stacks-bench`
    /// silently writes its `appdata/stacks-bench.db` somewhere
    /// upstream of the configured path (observed: a leading
    /// `~/.stacks-bench-bot` → DB at `~/appdata/stacks-bench.db`).
    /// Pre-creating the dir prevents that drift. Idempotent — no
    /// error when the dir already exists.
    fn ensure_data_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("creating stacks-bench data dir {}", self.data_dir.display()))
    }

    /// Construct the base `Command`: prebuilt release binary if available,
    /// `cargo stacks-bench` otherwise.
    fn build_cmd(&self) -> Command {
        match &self.release_bin {
            Some(p) if p.is_file() => Command::new(p),
            _ => {
                let mut c = Command::new("cargo");
                c.current_dir(&self.cargo_cwd);
                c.arg("stacks-bench");
                c
            }
        }
    }
}

/// Open `path` for write (truncating). Used to redirect captured
/// stdout/stderr.
fn open_for_write(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("opening {} for write", path.display()))
}

/// Open `path` as a lock file, materializing the parent directory if
/// necessary. The returned [`fd_lock::RwLock`] is held across the bench
/// invocation; calling `.write()` on it acquires an exclusive cross-process
/// lock that releases when the lock and its guard go out of scope.
fn open_lock_file(path: &Path) -> Result<fd_lock::RwLock<File>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("opening lock {}", path.display()))?;
    Ok(fd_lock::RwLock::new(file))
}

/// Envelope shape `bench show --json` writes: `{ data: { summary: { ... } } }`.
/// Other fields are deliberately ignored.
#[derive(Debug, Deserialize)]
struct BenchShowEnvelope {
    #[serde(default)]
    data: Option<BenchShowData>,
}

#[derive(Debug, Deserialize)]
struct BenchShowData {
    #[serde(default)]
    summary: Option<BenchShowSummary>,
    /// Catch-all for forward compatibility.
    #[serde(flatten)]
    #[allow(dead_code)]
    rest: BTreeMapValue,
}

#[derive(Debug, Deserialize)]
struct BenchShowSummary {
    #[serde(default)]
    total_duration_us: Option<i64>,
}

/// Type alias for serde's "ignore other fields" pattern. Using
/// `serde_json::Value` keeps the catch-all inert.
type BTreeMapValue = std::collections::BTreeMap<String, Value>;

/// Read a `data.run_id` from a captured `bench run` / `bench rerun` /
/// `bench show` envelope. Bails with a clear message when the field is
/// absent.
///
/// The bash predecessor used a `SELECT MAX(id) FROM benchmark_run`
/// SQLite fallback, but that's racy — the bench lock is dropped between
/// the bench command exiting and this read, so a concurrent sbagent
/// process can shift `MAX(id)` underneath us. The current
/// `cargo stacks-bench` always emits `data.run_id` in `--json` output;
/// if a future schema change drops it, surface the missing field
/// explicitly rather than guessing the wrong run id.
pub fn extract_run_id(json_path: &Path) -> Result<i64> {
    let raw =
        std::fs::read(json_path).with_context(|| format!("reading {}", json_path.display()))?;
    let parsed: Value =
        serde_json::from_slice(&raw).with_context(|| format!("parsing {}", json_path.display()))?;
    parsed
        .get("data")
        .and_then(|d| d.get("run_id"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "extract_run_id: {} envelope is missing `.data.run_id`; the stacks-bench CLI \
                 contract has changed",
                json_path.display(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ensure_data_dir` creates the configured data dir if it doesn't
    /// exist, including missing parent components. Defends against the
    /// silent-bad-path-fallback we hit in production (config pointed at
    /// `~/.stacks-bench-bot`, dir didn't exist, stacks-bench dropped
    /// the DB at `~/appdata/` instead of erroring or auto-creating).
    #[test]
    fn ensure_data_dir_creates_missing_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp
            .path()
            .join("nested")
            .join("missing")
            .join("stacks-bench-data");
        assert!(!data_dir.exists(), "precondition: dir must not exist");

        let cli = StacksBenchCli {
            release_bin: None,
            data_dir: data_dir.clone(),
            cargo_cwd: tmp.path().to_path_buf(),
        };
        cli.ensure_data_dir()
            .expect("ensure_data_dir must succeed on missing dir");
        assert!(data_dir.is_dir(), "dir must exist after ensure_data_dir");
    }

    /// Idempotent: calling twice on an existing dir is a no-op, not
    /// an error.
    #[test]
    fn ensure_data_dir_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cli = StacksBenchCli {
            release_bin: None,
            data_dir: tmp.path().to_path_buf(),
            cargo_cwd: tmp.path().to_path_buf(),
        };
        cli.ensure_data_dir().unwrap();
        cli.ensure_data_dir().unwrap();
        assert!(tmp.path().is_dir());
    }
}
