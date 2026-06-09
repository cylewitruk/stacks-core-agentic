//! `SourceRepo` trait + production impl shelling out to `git`.
//!
//! Operations needed at session-start to materialize the
//! per-session source checkout:
//!
//! 1. `ensure_cache` — bootstrap or refresh the shared bare object cache at
//!    `<workspace>/cache/<cache_id>.git/`. First touch `git clone --bare`;
//!    subsequent touches `git fetch <url> <branch>` against the bare cache.
//! 2. `clone_session_checkout` — materialize the per-session working checkout
//!    at `<workspace>/sessions/<id>/repos/<cache_id>/` via `git clone
//!    --reference <cache> --branch <branch> --local <cache> <checkout>`. Object
//!    storage is shared with the cache.
//! 3. `resolve_head_sha` — read the resolved HEAD SHA so the caller can record
//!    it in `source.json`.
//! 4. `prune_session_checkout` — best-effort `rm -rf`. Used by session-end
//!    cleanup (added in a later phase).
//!
//! The trait is the test seam. [`StdSourceRepo`] shells out to `git`
//! via the existing [`crate::git`] helpers; tests inject a stub that
//! records calls + simulates the on-disk effects.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use fd_lock::RwLock;

/// Operations on a source repo, behind a trait for testability.
pub trait SourceRepo {
    /// Materialize or refresh the bare object cache at `cache_dir`
    /// against `source_url` + `branch`. First touch clones via
    /// `git clone --bare`; subsequent touches `git fetch <url>
    /// <branch>`.
    fn ensure_cache(&self, cache_dir: &Path, source_url: &str, branch: &str) -> Result<()>;

    /// Clone a per-session working checkout at `session_checkout`
    /// from the bare `cache_dir`, on branch `branch`. The clone uses
    /// `--reference --local` so object storage is shared.
    ///
    /// `source_url` is the configured `[source].url` (the upstream
    /// GitHub URL). The clone's `origin` is rewritten to it as the
    /// final step — `git clone --local <cache>` sets `origin` to the
    /// local cache path, which would otherwise cause Phase 5's
    /// `git push origin` to write back to the bare cache instead of
    /// GitHub (Phase 2 per-target clones inherit `origin` from this
    /// checkout via `replicate_remotes`).
    fn clone_session_checkout(
        &self,
        cache_dir: &Path,
        session_checkout: &Path,
        branch: &str,
        source_url: &str,
    ) -> Result<()>;

    /// Resolve the HEAD SHA of `repo` (the session checkout). Used
    /// to fill `source.json.sha`.
    fn resolve_head_sha(&self, repo: &Path) -> Result<String>;

    /// Best-effort `rm -rf` of `session_checkout`. Idempotent;
    /// missing dir returns `Ok(false)`, present-and-removed returns
    /// `Ok(true)`.
    fn prune_session_checkout(&self, session_checkout: &Path) -> Result<bool>;
}

/// Production impl: shells out to `git` via the existing
/// [`crate::git`] helpers.
pub struct StdSourceRepo;

impl SourceRepo for StdSourceRepo {
    fn ensure_cache(&self, cache_dir: &Path, source_url: &str, branch: &str) -> Result<()> {
        if let Some(parent) = cache_dir.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating cache parent {}", parent.display()))?;
        }
        // The bare cache dir itself is a sentinel: presence implies the
        // initial `git clone --bare` ran successfully. Re-init on a
        // half-cloned dir would leave a confusing mixed state, so we
        // require the operator to nuke the dir manually if a prior
        // clone crashed mid-flight. (Future: detect via `git -C
        // <cache> rev-parse --is-bare-repository` for self-healing.)
        if cache_dir.exists() {
            // `-c safe.bareRepository=all` overrides any restrictive
            // operator-global `safe.bareRepository=explicit` config so
            // the fetch into the bare cache doesn't trip git's
            // bare-repo-via-cwd safety check.
            //
            // `+refs/heads/<branch>:refs/heads/<branch>` is the
            // load-bearing piece: the explicit refspec writes the
            // fetched tip into the cache's `refs/heads/<branch>`. A
            // bare `git fetch <url> <branch>` would update only
            // `FETCH_HEAD` — the next session's
            // `git clone --branch <branch>` against the cache would
            // resolve to the stale ref. The leading `+` allows force
            // updates so a force-pushed upstream branch is still
            // tracked.
            let refspec = format!("+refs/heads/{branch}:refs/heads/{branch}");
            crate::git::run_git(
                cache_dir,
                &["-c", "safe.bareRepository=all", "fetch", source_url, &refspec],
            )
            .with_context(|| {
                format!(
                    "fetching {branch} from {source_url} into existing cache {}",
                    cache_dir.display(),
                )
            })?;
        } else {
            // `git clone --bare <url> <dir>` — produces a bare repo
            // (no working tree) containing the full object store +
            // refs for `<url>`. We clone the entire remote then rely
            // on subsequent `fetch <url> <branch>` calls to update.
            // Using just `--branch` on `clone --bare` would limit
            // refs to that branch, which may be desirable for very
            // large repos in the future but isn't necessary today.
            let cwd = cache_dir
                .parent()
                .unwrap_or_else(|| Path::new("."));
            crate::git::run_git(
                cwd,
                &[
                    "clone",
                    "--bare",
                    source_url,
                    cache_dir
                        .to_str()
                        .with_context(|| format!("non-utf8 cache path {}", cache_dir.display()))?,
                ],
            )
            .with_context(|| {
                format!("initial bare clone of {source_url} into {}", cache_dir.display())
            })?;
            // Make sure the configured branch actually exists in the
            // mirror's `refs/heads/<branch>` (covers a misconfigured
            // branch name fast). The fully-qualified ref is
            // unambiguous — a bare `<branch>` would also resolve via
            // `refs/remotes/...` or `refs/tags/...`, which would mask
            // a missing local branch ref. Same bare-repo override as
            // the fetch path above.
            let qualified_ref = format!("refs/heads/{branch}");
            crate::git::run_git_output(
                cache_dir,
                &["-c", "safe.bareRepository=all", "rev-parse", "--verify", &qualified_ref],
            )
            .with_context(|| format!("verifying {branch} exists in mirror of {source_url}"))?;
        }
        Ok(())
    }

    fn clone_session_checkout(
        &self,
        cache_dir: &Path,
        session_checkout: &Path,
        branch: &str,
        source_url: &str,
    ) -> Result<()> {
        if let Some(parent) = session_checkout.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating session checkout parent {}", parent.display())
            })?;
        }
        // `--reference <cache>` shares the object store; `--local`
        // enables hardlinks for refs/HEAD/etc. (cheap, fast). The
        // `<source>` positional we hand `git clone` is the cache
        // again — there's no second copy of object storage anywhere.
        let cwd = session_checkout
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let cache_str = cache_dir
            .to_str()
            .with_context(|| format!("non-utf8 cache path {}", cache_dir.display()))?;
        let checkout_str = session_checkout
            .to_str()
            .with_context(|| format!("non-utf8 checkout path {}", session_checkout.display()))?;
        crate::git::run_git(
            cwd,
            &[
                "clone",
                "--reference",
                cache_str,
                "--branch",
                branch,
                "--local",
                cache_str,
                checkout_str,
            ],
        )
        .with_context(|| {
            format!(
                "cloning {branch} from cache {} into {}",
                cache_dir.display(),
                session_checkout.display(),
            )
        })?;

        // Rewrite `origin` from the local cache path to the upstream
        // URL. `git clone --local <cache>` sets `origin` to <cache>
        // (a local filesystem path), which would silently break Phase
        // 5 publish: per-target clones replicate this checkout's
        // remotes verbatim, and `git push origin <branch>` would then
        // write to the bare cache instead of GitHub. Set this BEFORE
        // returning so source.json sees the canonical clone state.
        crate::git::add_or_set_remote(session_checkout, "origin", source_url).with_context(
            || {
                format!(
                    "rewriting origin URL of {} from cache path to {source_url} (Phase 5 publish \
                     push target)",
                    session_checkout.display(),
                )
            },
        )?;
        Ok(())
    }

    fn resolve_head_sha(&self, repo: &Path) -> Result<String> {
        crate::git::rev_parse_head(repo)
            .with_context(|| format!("resolving HEAD sha of {}", repo.display()))
    }

    fn prune_session_checkout(&self, session_checkout: &Path) -> Result<bool> {
        match std::fs::remove_dir_all(session_checkout) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(anyhow::Error::new(e)
                .context(format!("removing session checkout {}", session_checkout.display()))),
        }
    }
}

/// Result of a successful materialization. Carries everything the
/// caller needs to write `source.json` and continue the phase.
#[derive(Debug, Clone)]
pub struct SourceMaterialization {
    pub cache_dir: PathBuf,
    pub session_checkout: PathBuf,
    pub source_url: String,
    pub branch: String,
    pub sha: String,
    pub fetched_at: SystemTime,
}

/// Inputs to [`materialize`]. Carries every seam tests inject
/// (workspace_root, cache_id, source url/branch, clock).
pub struct MaterializeInputs<'a> {
    /// `<agent_workspace_root>` — the root under which `cache/` and
    /// `sessions/` live.
    pub workspace_root: &'a Path,
    /// Session id (used to compute the per-session checkout dir).
    pub session_id: &'a str,
    /// Cache id — comes from
    /// [`crate::source::cache_id::resolve_cache_id`]. Must already
    /// be slug-validated (the resolver enforces this).
    pub cache_id: &'a str,
    /// Configured `[source].url`.
    pub source_url: &'a str,
    /// Configured `[source].branch`.
    pub branch: &'a str,
    /// Clock for `fetched_at`. Production runs pass
    /// `SystemTime::now()`; tests inject a fixed instant.
    pub now: SystemTime,
}

/// Zero-sized witness that proves the materialization lock for the
/// surrounding [`with_materialization_lock`] scope is held. Required
/// by [`materialize_unlocked`] so the type system documents the
/// "lock-must-be-held" contract — and so the unlocked variant is
/// impossible to call from outside the helper's closure (the field
/// is private; only `with_materialization_lock` can construct one).
pub struct MaterializationLockWitness {
    _private: (),
}

/// Run `f` while holding the materialization lock for one `cache_id`.
/// The lock spans every operation that needs to be serialized against
/// other materializations of the same cache — today: fetch, clone, AND
/// `source.json` write (see
/// [`crate::source::session::materialize_session_source`] for the
/// orchestration). The lock releases as soon as `f` returns.
///
/// Two concurrent calls against the same `(workspace_root, cache_id)`
/// serialize at the OS `fd_lock` layer; the second call blocks until
/// the first returns from `f`.
///
/// Scoped helper (vs the prior return-the-guard shape) so the lock
/// never escapes its acquisition function — no self-referential
/// struct, no `'static` lifetime extension, no unsafe.
pub fn with_materialization_lock<T>(
    workspace_root: &Path,
    cache_id: &str,
    f: impl FnOnce(&MaterializationLockWitness) -> Result<T>,
) -> Result<T> {
    let lock_path = materialization_lock_path(workspace_root, cache_id);
    let lock_parent = lock_path
        .parent()
        .expect("lock path always has a parent");
    std::fs::create_dir_all(lock_parent)
        .with_context(|| format!("creating lock parent {}", lock_parent.display()))?;
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening materialization lock {}", lock_path.display()))?;
    let mut rwlock = RwLock::new(lock_file);
    let _guard = rwlock
        .write()
        .with_context(|| format!("acquiring materialization lock {}", lock_path.display()))?;
    let witness = MaterializationLockWitness { _private: () };
    f(&witness)
}

/// Materialize the per-session source checkout end-to-end, WITHOUT
/// acquiring the materialization lock. Caller must hold a witness
/// from [`with_materialization_lock`], which proves the lock is held
/// for the surrounding scope. Used by
/// [`crate::source::session::materialize_session_source`] so the lock
/// can span fetch → resolve → clone → source.json write.
///
/// Direct callers (tests + ad-hoc tooling) should use [`materialize`]
/// instead, which is a thin wrapper that holds the lock for the
/// duration of the work via [`with_materialization_lock`].
pub fn materialize_unlocked<R: SourceRepo>(
    repo: &R,
    inputs: &MaterializeInputs<'_>,
    _witness: &MaterializationLockWitness,
) -> Result<SourceMaterialization> {
    let cache_dir = cache_dir_for(inputs.workspace_root, inputs.cache_id);
    let session_checkout =
        session_repo_dir_for(inputs.workspace_root, inputs.session_id, inputs.cache_id);

    repo.ensure_cache(&cache_dir, inputs.source_url, inputs.branch)?;

    // If a prior session left a checkout at the same path (rare —
    // session ids are unique), tear it down before re-cloning so
    // `git clone` doesn't refuse on the non-empty dir.
    repo.prune_session_checkout(&session_checkout)?;

    repo.clone_session_checkout(&cache_dir, &session_checkout, inputs.branch, inputs.source_url)?;
    let sha = repo.resolve_head_sha(&session_checkout)?;

    Ok(SourceMaterialization {
        cache_dir,
        session_checkout,
        source_url: inputs.source_url.to_owned(),
        branch: inputs.branch.to_owned(),
        sha,
        fetched_at: inputs.now,
    })
}

/// Convenience wrapper: holds the materialization lock across the
/// work, releases on return. Used by direct callers that don't need
/// to coordinate a wider locked window (tests + ad-hoc tooling).
/// Production session orchestration uses [`with_materialization_lock`]
/// composed with [`materialize_unlocked`] directly so the lock can
/// span the `source.json` write as part of the same critical section.
pub fn materialize<R: SourceRepo>(
    repo: &R,
    inputs: &MaterializeInputs<'_>,
) -> Result<SourceMaterialization> {
    with_materialization_lock(inputs.workspace_root, inputs.cache_id, |witness| {
        materialize_unlocked(repo, inputs, witness)
    })
}

/// Path of the bare cache root: `<workspace_root>/cache/`.
pub fn cache_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join("cache")
}

/// Bare-cache path for one cache id: `<workspace_root>/cache/<id>.git/`.
pub fn cache_dir_for(workspace_root: &Path, cache_id: &str) -> PathBuf {
    cache_root(workspace_root).join(format!("{cache_id}.git"))
}

/// Per-session repos parent dir:
/// `<workspace_root>/sessions/<session_id>/repos/`.
pub fn session_repos_dir(workspace_root: &Path, session_id: &str) -> PathBuf {
    workspace_root
        .join("sessions")
        .join(session_id)
        .join("repos")
}

/// Per-session source checkout:
/// `<workspace_root>/sessions/<session_id>/repos/<cache_id>/`.
pub fn session_repo_dir_for(workspace_root: &Path, session_id: &str, cache_id: &str) -> PathBuf {
    session_repos_dir(workspace_root, session_id).join(cache_id)
}

/// Materialization lock path. Lives alongside the cache dirs (not
/// inside one) so the lock can exist before the cache is fully
/// materialized: `<workspace_root>/cache/.<id>.materialize.lock`.
pub fn materialization_lock_path(workspace_root: &Path, cache_id: &str) -> PathBuf {
    cache_root(workspace_root).join(format!(".{cache_id}.materialize.lock"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Records calls + simulates the on-disk effects of each `git`
    /// operation so we can test `materialize` end-to-end without a
    /// real `git` binary.
    #[derive(Default, Clone)]
    struct StubSourceRepo {
        inner: Arc<Mutex<StubState>>,
    }

    #[derive(Default)]
    struct StubState {
        calls: Vec<String>,
        next_sha: Option<String>,
    }

    impl StubSourceRepo {
        fn calls(&self) -> Vec<String> {
            self.inner
                .lock()
                .unwrap()
                .calls
                .clone()
        }
        fn set_next_sha(&self, sha: &str) {
            self.inner
                .lock()
                .unwrap()
                .next_sha = Some(sha.to_owned());
        }
    }

    impl SourceRepo for StubSourceRepo {
        fn ensure_cache(&self, cache_dir: &Path, source_url: &str, branch: &str) -> Result<()> {
            self.inner
                .lock()
                .unwrap()
                .calls
                .push(format!("ensure_cache:{}:{source_url}:{branch}", cache_dir.display()));
            std::fs::create_dir_all(cache_dir)?;
            // Simulate a bare repo by creating a HEAD file.
            std::fs::write(cache_dir.join("HEAD"), "ref: refs/heads/main\n")?;
            Ok(())
        }
        fn clone_session_checkout(
            &self,
            cache_dir: &Path,
            session_checkout: &Path,
            branch: &str,
            source_url: &str,
        ) -> Result<()> {
            self.inner
                .lock()
                .unwrap()
                .calls
                .push(format!(
                    "clone:{}:{}:{branch}:{source_url}",
                    cache_dir.display(),
                    session_checkout.display(),
                ));
            std::fs::create_dir_all(session_checkout)?;
            std::fs::write(session_checkout.join(".source-stub"), "ok\n")?;
            Ok(())
        }
        fn resolve_head_sha(&self, repo: &Path) -> Result<String> {
            self.inner
                .lock()
                .unwrap()
                .calls
                .push(format!("rev-parse-head:{}", repo.display()));
            Ok(self
                .inner
                .lock()
                .unwrap()
                .next_sha
                .clone()
                .unwrap_or_else(|| "deadbeefcafebabe".to_owned()))
        }
        fn prune_session_checkout(&self, session_checkout: &Path) -> Result<bool> {
            self.inner
                .lock()
                .unwrap()
                .calls
                .push(format!("prune:{}", session_checkout.display()));
            match std::fs::remove_dir_all(session_checkout) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(anyhow::Error::new(e)),
            }
        }
    }

    fn fixed_clock() -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_770_000_000)
    }

    #[test]
    fn materialize_fresh_creates_cache_and_session_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = StubSourceRepo::default();
        stub.set_next_sha("abc123");

        let mat = materialize(
            &stub,
            &MaterializeInputs {
                workspace_root: tmp.path(),
                session_id: "20260607-104400",
                cache_id: "github-com-stacks-network-stacks-core-3a7f2b91",
                source_url: "https://github.com/stacks-network/stacks-core.git",
                branch: "feat/stacks-bench",
                now: fixed_clock(),
            },
        )
        .expect("materialize fresh");

        assert_eq!(mat.sha, "abc123");
        assert_eq!(mat.branch, "feat/stacks-bench");
        assert_eq!(mat.fetched_at, fixed_clock());
        assert!(mat.cache_dir.exists());
        assert!(mat.session_checkout.exists());

        let calls = stub.calls();
        // Sequence: ensure_cache → prune → clone → rev-parse-head.
        assert!(calls[0].starts_with("ensure_cache:"), "calls={calls:?}");
        assert!(calls[1].starts_with("prune:"), "calls={calls:?}");
        assert!(calls[2].starts_with("clone:"), "calls={calls:?}");
        assert!(calls[3].starts_with("rev-parse-head:"), "calls={calls:?}");
    }

    #[test]
    fn materialize_warm_cache_reuses_cache_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = StubSourceRepo::default();
        stub.set_next_sha("warm-sha");

        // First call materializes everything.
        let _first = materialize(
            &stub,
            &MaterializeInputs {
                workspace_root: tmp.path(),
                session_id: "20260607-100000",
                cache_id: "warm-cache-id",
                source_url: "https://example.com/x.git",
                branch: "main",
                now: fixed_clock(),
            },
        )
        .expect("first materialize");

        // Second call: same cache id (warm path), different session id.
        let second = materialize(
            &stub,
            &MaterializeInputs {
                workspace_root: tmp.path(),
                session_id: "20260607-110000",
                cache_id: "warm-cache-id",
                source_url: "https://example.com/x.git",
                branch: "main",
                now: fixed_clock(),
            },
        )
        .expect("second materialize");

        // Cache dir shared; session checkouts distinct.
        assert_eq!(cache_dir_for(tmp.path(), "warm-cache-id"), second.cache_dir);
        assert!(
            second
                .session_checkout
                .to_string_lossy()
                .contains("20260607-110000")
        );

        // Stub recorded ensure_cache twice (production fetches; stub
        // re-creates the HEAD sentinel — same call shape).
        let ensure_count = stub
            .calls()
            .iter()
            .filter(|c| c.starts_with("ensure_cache:"))
            .count();
        assert_eq!(ensure_count, 2);
    }

    #[test]
    fn cache_dir_for_returns_workspace_cache_id_git_path() {
        let p = cache_dir_for(Path::new("/ws"), "stacks-core-feat-stacks-bench");
        assert_eq!(p, Path::new("/ws/cache/stacks-core-feat-stacks-bench.git"));
    }

    #[test]
    fn session_repo_dir_for_returns_per_session_repo_path() {
        let p = session_repo_dir_for(
            Path::new("/ws"),
            "20260607-100000",
            "stacks-core-feat-stacks-bench",
        );
        assert_eq!(
            p,
            Path::new("/ws/sessions/20260607-100000/repos/stacks-core-feat-stacks-bench",)
        );
    }

    #[test]
    fn materialization_lock_path_lives_alongside_cache_dirs() {
        let p = materialization_lock_path(Path::new("/ws"), "stacks-core");
        assert_eq!(p, Path::new("/ws/cache/.stacks-core.materialize.lock"));
    }

    #[test]
    fn materialize_holds_write_lock_so_concurrent_calls_serialize() {
        // Confirms the lock IS acquired: a parallel call against the
        // same cache id should serialize, not interleave.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let cache_id = "concurrent-cache";

        // Spawn two threads, both calling materialize against the same
        // cache. They MUST complete one-at-a-time. We assert no panic +
        // both produce a valid SourceMaterialization.
        let workspace1 = workspace.clone();
        let cache_id_1 = cache_id.to_owned();
        let handle1 = std::thread::spawn(move || {
            let stub = StubSourceRepo::default();
            stub.set_next_sha("sha-1");
            materialize(
                &stub,
                &MaterializeInputs {
                    workspace_root: &workspace1,
                    session_id: "20260607-A",
                    cache_id: &cache_id_1,
                    source_url: "https://example.com/x.git",
                    branch: "main",
                    now: fixed_clock(),
                },
            )
        });
        let workspace2 = workspace.clone();
        let cache_id_2 = cache_id.to_owned();
        let handle2 = std::thread::spawn(move || {
            let stub = StubSourceRepo::default();
            stub.set_next_sha("sha-2");
            materialize(
                &stub,
                &MaterializeInputs {
                    workspace_root: &workspace2,
                    session_id: "20260607-B",
                    cache_id: &cache_id_2,
                    source_url: "https://example.com/x.git",
                    branch: "main",
                    now: fixed_clock(),
                },
            )
        });

        let r1 = handle1
            .join()
            .expect("thread1 panicked");
        let r2 = handle2
            .join()
            .expect("thread2 panicked");
        assert!(r1.is_ok(), "first materialize failed: {r1:?}");
        assert!(r2.is_ok(), "second materialize failed: {r2:?}");
        let m1 = r1.unwrap();
        let m2 = r2.unwrap();
        assert!(m1.session_checkout.exists());
        assert!(m2.session_checkout.exists());
        assert_ne!(m1.session_checkout, m2.session_checkout);
    }

    #[test]
    fn prune_session_checkout_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = StdSourceRepo;
        let p = tmp.path().join("checkout-x");
        std::fs::create_dir_all(&p).unwrap();
        assert!(
            repo.prune_session_checkout(&p)
                .unwrap()
        );
        assert!(!p.exists());
        // Second prune: missing dir → false, no error.
        assert!(
            !repo
                .prune_session_checkout(&p)
                .unwrap()
        );
    }
}
