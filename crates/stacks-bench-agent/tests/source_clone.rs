//! Integration test for the v3 Phase 1 source-clone primitives.
//!
//! Exercises the production `StdSourceRepo` against a hand-built
//! local bare repo seeded by `git init --bare` + one commit — no
//! external network call required, but the full `git clone --bare`,
//! `git fetch`, and `git clone --reference --local` paths run.

use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use stacks_bench_agent::source::repo::{
    MaterializeInputs, cache_dir_for, materialize, session_repo_dir_for,
};
use stacks_bench_agent::source::{SourceMaterialization, StdSourceRepo};

/// Seed a fully-functional local bare repo at `bare_path` carrying
/// one commit on `branch`. Returns the resolved commit SHA.
fn seed_bare_repo(bare_path: &Path, branch: &str) -> String {
    let parent = bare_path
        .parent()
        .expect("bare path has parent");
    std::fs::create_dir_all(parent).unwrap();

    // Build a working-tree repo in a sibling dir, commit, then clone
    // it bare — simpler than constructing a bare repo by hand.
    let working = parent.join("__seed_working");
    std::fs::create_dir_all(&working).unwrap();
    sh(&working, &["git", "init", "-q", "-b", branch]);
    sh(&working, &["git", "config", "user.email", "seed@example.com"]);
    sh(&working, &["git", "config", "user.name", "Seed"]);
    sh(&working, &["git", "config", "commit.gpgsign", "false"]);
    std::fs::write(working.join("README"), "hello\n").unwrap();
    sh(&working, &["git", "add", "README"]);
    sh(&working, &["git", "commit", "-q", "-m", "seed"]);

    // `git clone --bare` from the working repo into the bare path.
    sh(
        parent,
        &["git", "clone", "--bare", "-q", working.to_str().unwrap(), bare_path.to_str().unwrap()],
    );
    // `-c safe.bareRepository=all` covers operator git configs that
    // set `safe.bareRepository=explicit` globally; without it, this
    // and the production StdSourceRepo fetch would both refuse to
    // operate on the bare repo via cwd discovery.
    let sha_raw = sh_out(bare_path, &["git", "-c", "safe.bareRepository=all", "rev-parse", branch])
        .expect("rev-parse seed bare repo");
    let sha = sha_raw.trim().to_owned();

    std::fs::remove_dir_all(&working).unwrap();
    sha
}

/// Add one new commit to `branch` in the bare repo at `bare_path` and
/// return the resulting tip SHA. Models "upstream moved" between two
/// session materializations.
fn advance_bare_repo(bare_path: &Path, branch: &str) -> String {
    let parent = bare_path
        .parent()
        .expect("bare path has parent");
    let working = parent.join("__advance_working");
    // Clean any leftover working dir from a previous call.
    let _ = std::fs::remove_dir_all(&working);
    std::fs::create_dir_all(&working).unwrap();

    sh(&working, &["git", "clone", "-q", "--branch", branch, bare_path.to_str().unwrap(), "."]);
    sh(&working, &["git", "config", "user.email", "advance@example.com"]);
    sh(&working, &["git", "config", "user.name", "Advance"]);
    sh(&working, &["git", "config", "commit.gpgsign", "false"]);

    // Append a line to README, commit, push back.
    let readme = working.join("README");
    let mut existing = std::fs::read_to_string(&readme).unwrap_or_default();
    existing.push_str("advanced\n");
    std::fs::write(&readme, existing).unwrap();
    sh(&working, &["git", "add", "README"]);
    sh(&working, &["git", "commit", "-q", "-m", "advance upstream"]);
    sh(&working, &["git", "push", "-q", "origin", branch]);

    let sha_raw = sh_out(&working, &["git", "rev-parse", "HEAD"]).expect("rev-parse advance");
    let sha = sha_raw.trim().to_owned();
    std::fs::remove_dir_all(&working).unwrap();
    sha
}

fn sh(dir: &Path, args: &[&str]) {
    let status = Command::new(args[0])
        .args(&args[1..])
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("running {args:?}: {e}"));
    assert!(status.success(), "command {args:?} in {} failed", dir.display());
}

fn sh_out(dir: &Path, args: &[&str]) -> std::io::Result<String> {
    let out = Command::new(args[0])
        .args(&args[1..])
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "{args:?} in {} failed: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr),
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn std_source_repo_materializes_against_a_local_bare_repo_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path();
    // Place the "upstream" bare repo outside the workspace so it
    // mirrors a real remote URL the cache pulls from.
    let upstream = tmp
        .path()
        .join("__upstream_bare.git");
    let upstream_url = upstream
        .to_string_lossy()
        .into_owned();
    let branch = "feat/stacks-bench";
    let upstream_sha = seed_bare_repo(&upstream, branch);

    let SourceMaterialization {
        cache_dir,
        session_checkout,
        sha,
        ..
    } = materialize(
        &StdSourceRepo,
        &MaterializeInputs {
            workspace_root: workspace,
            session_id: "20260607-104400",
            cache_id: "local-test-cache",
            source_url: &upstream_url,
            branch,
            now: SystemTime::UNIX_EPOCH,
        },
    )
    .expect("materialize succeeds against local bare repo");

    // Resolved SHA matches the upstream's tip.
    assert_eq!(sha, upstream_sha, "session checkout HEAD must match upstream branch tip");

    // Cache + session checkout landed at the expected paths.
    assert_eq!(cache_dir_for(workspace, "local-test-cache"), cache_dir);
    assert_eq!(
        session_repo_dir_for(workspace, "20260607-104400", "local-test-cache"),
        session_checkout,
    );

    // Cache is bare (`HEAD` file present at the cache root, no working
    // tree).
    assert!(
        cache_dir
            .join("HEAD")
            .is_file(),
        "cache should be a bare repo"
    );
    assert!(
        !cache_dir
            .join("README")
            .exists(),
        "cache must NOT have a working tree",
    );

    // Session checkout has the working tree.
    assert!(
        session_checkout
            .join("README")
            .is_file(),
        "session checkout should have the working tree",
    );

    // `origin` was rewritten from the local cache path to the
    // configured upstream URL. This is load-bearing for Phase 5
    // publish: per-target clones in Phase 2 replicate the session
    // checkout's remotes verbatim, and Phase 5's
    // `git push origin <branch>` would otherwise write to the bare
    // cache instead of GitHub. Hard-fail here so regressions surface
    // immediately instead of as bot pushes mysteriously landing in
    // <workspace>/cache/.
    let origin_url = sh_out(&session_checkout, &["git", "remote", "get-url", "origin"])
        .expect("git remote get-url origin")
        .trim()
        .to_owned();
    assert_eq!(
        origin_url, upstream_url,
        "session checkout `origin` must match the configured [source].url (not the local cache \
         path)",
    );
}

#[test]
fn std_source_repo_warm_cache_path_serves_a_second_session_id() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path();
    let upstream = tmp
        .path()
        .join("__upstream_bare.git");
    let upstream_url = upstream
        .to_string_lossy()
        .into_owned();
    let branch = "main";
    let upstream_sha = seed_bare_repo(&upstream, branch);

    // First session: fresh cache.
    let first = materialize(
        &StdSourceRepo,
        &MaterializeInputs {
            workspace_root: workspace,
            session_id: "20260607-100000",
            cache_id: "warm",
            source_url: &upstream_url,
            branch,
            now: SystemTime::UNIX_EPOCH,
        },
    )
    .expect("first materialize");

    // Second session: same cache, different session id. Should
    // reuse the cache dir (warm path) without re-cloning.
    let second = materialize(
        &StdSourceRepo,
        &MaterializeInputs {
            workspace_root: workspace,
            session_id: "20260607-110000",
            cache_id: "warm",
            source_url: &upstream_url,
            branch,
            now: SystemTime::UNIX_EPOCH,
        },
    )
    .expect("second materialize");

    assert_eq!(first.cache_dir, second.cache_dir);
    assert_ne!(first.session_checkout, second.session_checkout);
    assert_eq!(first.sha, upstream_sha);
    assert_eq!(second.sha, upstream_sha);

    // Both checkouts independently exist.
    assert!(
        first
            .session_checkout
            .join("README")
            .is_file()
    );
    assert!(
        second
            .session_checkout
            .join("README")
            .is_file()
    );
}

/// Regression test for the v3 Phase 1 warm-cache ref-update bug
/// Codex caught: `git fetch <url> <branch>` updates `FETCH_HEAD` but
/// NOT `refs/heads/<branch>` in the bare cache, so a second session
/// would keep cloning the stale tip even after upstream advanced.
/// The fix is the explicit refspec `+refs/heads/<branch>:refs/heads/<branch>`.
#[test]
fn std_source_repo_warm_cache_picks_up_advanced_upstream_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path();
    let upstream = tmp
        .path()
        .join("__upstream_bare.git");
    let upstream_url = upstream
        .to_string_lossy()
        .into_owned();
    let branch = "main";
    let initial_sha = seed_bare_repo(&upstream, branch);

    // First session: cache materializes at the initial SHA.
    let first = materialize(
        &StdSourceRepo,
        &MaterializeInputs {
            workspace_root: workspace,
            session_id: "20260607-100000",
            cache_id: "advance-test",
            source_url: &upstream_url,
            branch,
            now: SystemTime::UNIX_EPOCH,
        },
    )
    .expect("first materialize");
    assert_eq!(first.sha, initial_sha, "first materialize must see the initial SHA");

    // Upstream advances one commit.
    let advanced_sha = advance_bare_repo(&upstream, branch);
    assert_ne!(initial_sha, advanced_sha, "advance_bare_repo should produce a new SHA",);

    // Second session: cache is warm — the fetch must pull the new
    // tip into refs/heads/<branch> so the clone resolves to it.
    let second = materialize(
        &StdSourceRepo,
        &MaterializeInputs {
            workspace_root: workspace,
            session_id: "20260607-110000",
            cache_id: "advance-test",
            source_url: &upstream_url,
            branch,
            now: SystemTime::UNIX_EPOCH,
        },
    )
    .expect("second materialize");

    assert_eq!(
        second.sha, advanced_sha,
        "warm-cache materialize must pick up the advanced upstream SHA, not the stale one",
    );
    assert_eq!(first.cache_dir, second.cache_dir, "cache dir must be shared");
}

#[test]
fn std_source_repo_fails_loud_when_branch_is_missing_from_upstream() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path();
    let upstream = tmp
        .path()
        .join("__upstream_bare.git");
    let upstream_url = upstream
        .to_string_lossy()
        .into_owned();
    let _ = seed_bare_repo(&upstream, "main");

    // Configured branch doesn't exist in the upstream.
    let result = materialize(
        &StdSourceRepo,
        &MaterializeInputs {
            workspace_root: workspace,
            session_id: "20260607-104400",
            cache_id: "missing-branch",
            source_url: &upstream_url,
            branch: "does-not-exist",
            now: SystemTime::UNIX_EPOCH,
        },
    );
    assert!(result.is_err(), "missing branch should produce an Err");
}
