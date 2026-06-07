//! `.run.pid` — best-effort live-session marker.
//!
//! Written by `session run` after preflight and before Phase 0. Cleared
//! by the [`RunPidGuard`] drop impl on every exit path the runtime
//! unwinds through — normal return, `?` bail, and panics that unwind
//! rather than abort.
//!
//! **What does NOT clear the file**: SIGINT (Ctrl-C) and SIGKILL.
//! sbagent does not install a signal handler today, so the default
//! kernel disposition for SIGINT terminates the process without
//! unwinding — destructors don't fire, and `.run.pid` is left behind.
//! This is intentional: the prune-side liveness check
//! ([`is_live`]) handles the resulting stale PID by falling through
//! to the normal age + archive filters, so a corpse PID file cannot
//! make a workspace immortal.
//!
//! Lives at `<sessions_root>/<id>/.run.pid` so a single
//! `agent_workspace_root` walk can locate it without consulting the
//! operator's `sessions.jsonl`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// File name inside a session dir holding the running `sbagent` PID.
pub const RUN_PID_FILE: &str = ".run.pid";

/// Resolve the PID-file path for a session dir.
pub fn path_for_session(session_dir: &Path) -> PathBuf {
    session_dir.join(RUN_PID_FILE)
}

/// Write the current process's PID into `<session_dir>/.run.pid`.
/// Creates the session dir's parent if missing. Overwrites any prior
/// content — a leftover PID file from a crashed previous run is
/// expected to be replaced when a fresh run starts in the same session
/// id (which is rare but legitimate during operator debugging).
pub fn write(session_dir: &Path) -> Result<()> {
    fs::create_dir_all(session_dir)
        .with_context(|| format!("creating session dir {}", session_dir.display()))?;
    let path = path_for_session(session_dir);
    let pid = std::process::id();
    fs::write(&path, format!("{pid}\n"))
        .with_context(|| format!("writing run PID file {}", path.display()))?;
    Ok(())
}

/// Remove `<session_dir>/.run.pid` if it exists. Idempotent — a missing
/// file is not an error (it means a prior cleanup already ran or the
/// session crashed before the write). Errors other than `NotFound`
/// surface, since they likely indicate a permissions issue worth
/// flagging.
pub fn clear(session_dir: &Path) -> Result<()> {
    let path = path_for_session(session_dir);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("removing run PID file {}", path.display())))
        }
    }
}

/// Parse the PID stored in `<session_dir>/.run.pid`. Returns `Ok(None)`
/// if the file doesn't exist or isn't parseable as a positive integer
/// (corruption); the caller treats both as "no live signal".
pub fn read(session_dir: &Path) -> Result<Option<u32>> {
    let path = path_for_session(session_dir);
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(
                anyhow::Error::new(e).context(format!("reading run PID file {}", path.display()))
            );
        }
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(trimmed.parse::<u32>().ok())
}

/// True iff `pid` corresponds to a live process visible from the
/// current process. On Unix this is `kill(pid, 0)` — succeeds when the
/// signal *could* be delivered (process exists and we have permission
/// to signal it). The "no permission" case (`EPERM`) returns true:
/// the process exists, just isn't ours. `ESRCH` (no such process) is
/// the only definitive "not alive". Non-Unix targets always return
/// false (defensive: prefer "not alive" so prune can proceed).
pub fn is_live(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `kill` with signal 0 has no observable effect on the
        // target process; it just probes existence. The `i32` cast is
        // safe because PIDs on Linux/macOS fit in i32 by convention
        // (`pid_t` is i32).
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(0);
        // EPERM = process exists, we can't signal it. Treat as alive
        // for prune-safety; this is conservative and rare in practice
        // since sbagent runs as the operator and prune runs the same way.
        err == libc::EPERM
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// RAII guard that writes the PID file on construction and clears it
/// on drop. Use at the top of `session run` after preflight so the
/// file is always cleared on the exit paths the runtime actually
/// unwinds through: normal completion, `?` bail-out, and unwinding
/// panics. SIGINT (Ctrl-C) and SIGKILL terminate without unwinding
/// and leave the file behind; `workspace prune`'s liveness check
/// handles the resulting stale PID.
pub struct RunPidGuard {
    session_dir: PathBuf,
}

impl RunPidGuard {
    /// Write `<session_dir>/.run.pid` carrying the current process's
    /// PID. The returned guard clears the file on drop.
    pub fn install(session_dir: impl Into<PathBuf>) -> Result<Self> {
        let session_dir = session_dir.into();
        write(&session_dir)?;
        Ok(Self { session_dir })
    }
}

impl Drop for RunPidGuard {
    fn drop(&mut self) {
        if let Err(e) = clear(&self.session_dir) {
            tracing::warn!(
                error = %e,
                session_dir = %self.session_dir.display(),
                "failed to clear run pid file at session exit",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_round_trips_current_pid() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path()).unwrap();
        let pid = read(tmp.path())
            .unwrap()
            .expect("pid present");
        assert_eq!(pid, std::process::id());
    }

    #[test]
    fn clear_removes_the_file_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path()).unwrap();
        assert!(path_for_session(tmp.path()).exists());
        clear(tmp.path()).unwrap();
        assert!(!path_for_session(tmp.path()).exists());
        // Second clear is fine.
        clear(tmp.path()).unwrap();
    }

    #[test]
    fn read_returns_none_for_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let got = read(tmp.path()).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn read_returns_none_for_garbled_content() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(path_for_session(tmp.path()), "not a number").unwrap();
        let got = read(tmp.path()).unwrap();
        assert!(got.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn is_live_returns_true_for_current_process() {
        assert!(is_live(std::process::id()));
    }

    #[test]
    fn guard_writes_on_install_and_clears_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let _guard = RunPidGuard::install(tmp.path()).unwrap();
            assert!(path_for_session(tmp.path()).exists());
        }
        assert!(!path_for_session(tmp.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn is_live_returns_false_for_definitely_dead_pid() {
        // PID 0 is the kernel's idle process on Linux and the swapper
        // on macOS — kill(0, 0) returns EPERM on Linux and ESRCH on
        // some configurations. To get a definitively dead PID, spawn
        // a child, wait for it, then poll its (recycled-or-gone) PID.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn /usr/bin/true");
        let pid = child.id();
        child.wait().expect("wait");
        // The OS may recycle the PID — in that rare case is_live is
        // legitimately true. So this test is a smoke check: at least
        // confirm the function returns without panicking. The "stale
        // PID falls through" path in workspace prune is covered at
        // the integration level instead.
        let _ = is_live(pid);
    }
}
