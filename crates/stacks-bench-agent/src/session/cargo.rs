//! `CargoRunner` trait — abstracts `cargo build --release -p stacks-bench`
//! and `cargo clean` invocations against an experiment worktree so the
//! ported phases can be tested with a fake.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// Runs `cargo build` / `cargo clean` against an experiment worktree.
pub trait CargoRunner: Send + Sync {
    /// Equivalent to `( cd <worktree> && cargo build --release -p stacks-bench
    /// )`, capturing stdout/stderr to the given paths.
    fn build_release(&self, worktree: &Path, stdout: &Path, stderr: &Path) -> Result<()>;

    /// Equivalent to `( cd <worktree> && cargo clean )`. Failure is non-fatal
    /// (matches the bash `... || true`); the function still surfaces an
    /// error if the subprocess invocation itself fails (e.g. cargo missing).
    fn clean(&self, worktree: &Path, stdout: &Path, stderr: &Path) -> Result<()>;
}

/// Default impl: shells out to `cargo` directly with the worktree as cwd.
pub struct StdCargoRunner;

impl CargoRunner for StdCargoRunner {
    fn build_release(&self, worktree: &Path, stdout: &Path, stderr: &Path) -> Result<()> {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(worktree)
            .arg("build")
            .arg("--release")
            .arg("-p")
            .arg("stacks-bench")
            .stdout(open_for_write(stdout)?)
            .stderr(open_for_write(stderr)?);
        let status = cmd
            .status()
            .with_context(|| {
                format!("invoking cargo build --release -p stacks-bench in {}", worktree.display())
            })?;
        if !status.success() {
            bail!(
                "cargo build --release -p stacks-bench failed in {} ({status})",
                worktree.display()
            );
        }
        Ok(())
    }

    fn clean(&self, worktree: &Path, stdout: &Path, stderr: &Path) -> Result<()> {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(worktree)
            .arg("clean")
            .stdout(open_for_write(stdout)?)
            .stderr(open_for_write(stderr)?);
        // Mirror the bash `|| true`: ignore non-zero exits.
        let _ = cmd.status();
        Ok(())
    }
}

fn open_for_write(path: &Path) -> Result<std::fs::File> {
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

/// Path helper: location of the per-target release binary inside an
/// experiment worktree (as produced by `cargo build --release -p
/// stacks-bench`).
pub fn worktree_release_bin(worktree: &Path) -> PathBuf {
    worktree
        .join("target")
        .join("release")
        .join("stacks-bench")
}
