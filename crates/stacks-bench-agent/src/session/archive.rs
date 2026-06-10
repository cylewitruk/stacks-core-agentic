//! Session archive flow — promote a completed session from local-only
//! working-tree state to its permanent home in the operator git repo.
//!
//! On success, the operator repo gains two new git objects:
//!
//! 1. **`session/<id>` branch** — a write-once branch holding the full
//!    `sessions/<id>/` evidence bundle. Bypasses main's `.gitignore` via `git
//!    add -f`. Never re-pushed after the initial archive — a re-archive is a
//!    no-op once the branch sha is recorded.
//! 2. **`sessions.jsonl` on main** — one append-only [`SessionRecord`] line.
//!    The terminal index of every completed session. Powers aggregate /
//!    leaderboard / timeline views without traversing archive branches.
//!
//! Idempotent on re-run: if `sessions.jsonl` already carries this
//! session's id, the archive call is a no-op. This makes scheduled CI
//! safe to invoke after every session without coordination.
//!
//! v1 simplifications (documented in `docs/session-archive.md`):
//!
//! - `started_at` is derived from the session id's `YYYYMMDD-HHMMSS` prefix;
//!   precise wall-clock start would need a per-session manifest the
//!   orchestrator writes.
//! - `finished_at` is the latest mtime under `sessions/<id>/`. Good enough for
//!   ordering; not authoritative.
//! - `phase_durations_secs` is empty until phase-timing instrumentation lands
//!   (separate work item).
//!
//! See [`crate::models::session_record`] for the full record shape.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs, io};

use anyhow::{Context as _, Result, anyhow, bail};

use crate::analyzed_rejections::now_utc_iso8601;
use crate::build_info::{SBAGENT_VERSION, sbagent_git_sha};
use crate::git;
use crate::layout::Layout;
use crate::models::ToJson;
use crate::models::common::DeliveryMode;
use crate::models::session_record::{
    SessionRange, SessionRecord, SessionRecordKind, SessionStatus, TargetBench, TargetRecord,
    TargetStatus, TargetStatusStage,
};
use crate::models::summary::{Experiment, ExperimentStatus, Summary};
use crate::session::{SessionLayout, loader};
use crate::settings::Settings;

/// Inputs to [`archive`].
pub struct ArchiveInputs<'a> {
    pub layout: &'a SessionLayout,
    pub framework: &'a Layout,
    pub settings: &'a Settings,
    /// Skip the push step (and the remote-resolution preflight). Local
    /// artifacts — the `session/<id>` branch and the `sessions.jsonl`
    /// commit — are still produced so the operator can inspect them.
    pub dry_run: bool,
}

/// What [`archive`] did, for the CLI to summarize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveOutputs {
    /// `true` when the session was already in `sessions.jsonl` — the
    /// archive call was a no-op.
    pub already_archived: bool,
    /// Name of the archive branch (always `session/<id>`).
    pub branch: String,
    /// Tip sha of the archive branch after this archive run. `None`
    /// when [`already_archived`] is true and we didn't touch the
    /// branch.
    pub branch_sha: Option<String>,
    /// `true` when a new line was appended to `sessions.jsonl` in this
    /// invocation. Equivalent to `!already_archived`.
    pub ledger_appended: bool,
    /// Browsable URL to the archive branch tree on the configured
    /// remote, when one could be derived.
    pub artifact_url: Option<String>,
}

/// Run the archive flow. See module docs for the full contract.
pub fn archive(inputs: &ArchiveInputs<'_>) -> Result<ArchiveOutputs> {
    let repo = inputs
        .framework
        .require_operator_repo_root()?
        .to_path_buf();
    let session_id = inputs
        .layout
        .id
        .as_str()
        .to_owned();
    let branch = format!("session/{session_id}");

    // 1. Idempotency: short-circuit if this session is already in the ledger. Cheap
    //    — read happens before any branch manipulation.
    let ledger_path = repo.join("sessions.jsonl");
    if ledger_contains_id(&ledger_path, &session_id)? {
        return Ok(ArchiveOutputs {
            already_archived: true,
            branch,
            branch_sha: None,
            ledger_appended: false,
            artifact_url: derive_artifact_url(&repo, &format!("session/{session_id}")).ok(),
        });
    }

    // 2. Sanity-check the operator repo state. We require a clean working tree on a
    //    non-archive branch — running archive from inside a `session/<id>` branch
    //    (or with uncommitted work) is almost certainly an operator error.
    let starting_branch = git::current_branch(&repo).context(
        "operator repo has detached HEAD; switch to `main` (or your tracking branch) before \
         running archive",
    )?;
    if starting_branch.starts_with("session/") {
        bail!(
            "operator repo is on `{starting_branch}`; archive must be invoked from your tracking \
             branch (typically `main`)"
        );
    }
    ensure_clean_worktree(&repo)?;

    // 3. Build the typed record from session artifacts BEFORE touching any branch.
    //    Failing here leaves the operator repo untouched.
    let record = build_session_record(inputs, &branch)?;

    // 4. Create-or-update the archive branch using a SEPARATE git worktree. The
    //    operator's main worktree never sees `sessions/<id>/` in its tree — that
    //    path is exclusive to the `session/<id>` branch's worktree. This prevents
    //    the "switching branches wipes the bulk" footgun: before this change, `git
    //    checkout main` from `session/<id>` would delete the
    //    tracked-on-source-not-on-dest-but-ignored files.
    let env = bot_identity_env(inputs.settings);
    let archive_wt = archive_worktree_path(inputs.framework, &session_id)?;
    // Resolve push auth ONCE from settings — but ONLY when we
    // actually need it. We need it iff: not dry-run AND the
    // operator repo has a configured remote. A purely local
    // operator repo (no remote) goes through the archive flow
    // without ever pushing, so requiring `publish.token_file` there
    // would be a regression. Both push sites (archive branch + main
    // ledger) read from this single bundle so they can't drift.
    let push_auth = if inputs.dry_run {
        None
    } else if primary_remote(&repo)?.is_some() {
        Some(load_push_auth(inputs)?)
    } else {
        None
    };
    let branch_sha = run_in_archive_worktree(
        &repo,
        &archive_wt,
        &branch,
        &starting_branch,
        &session_id,
        &inputs.layout.results_dir,
        &env,
        push_auth.as_ref(),
    )?;

    // 5. Append the ledger line in the operator's MAIN worktree. The main worktree
    //    is already on `starting_branch`; we never switched off it (the archive ops
    //    happened in `archive_wt`).
    let mut record = record;
    record.artifact_sha = branch_sha.clone();
    // The browsable URL is a pure transform of the remote URL +
    // branch name — same answer whether the push has shipped or
    // not, so write it onto the record BEFORE append_ledger.
    // (Previously we only filled it in for the CLI printout, which
    // meant `sessions.jsonl` carried a null URL while the operator's
    // terminal showed one — confusing audit trail.)
    let artifact_url = derive_artifact_url(&repo, &branch).ok();
    record.artifact_url = artifact_url.clone();

    // Pull-rebase main first if we have a remote to race against —
    // absorbs a peer's prior append before we add our own line. Uses
    // the same PAT auth as the eventual ledger push so a private
    // operator repo can fetch successfully (the unauthenticated
    // `pull_rebase` would fail with "remote requires auth").
    if let Some(auth) = push_auth.as_ref()
        && let Some(remote) = primary_remote(&repo)?
    {
        git::pull_rebase_with_auth(
            &repo,
            &remote,
            &starting_branch,
            &auth.token,
            &[],
            &auth.username,
            &auth.url_prefix,
        )
        .with_context(|| {
            format!("pull --rebase {remote}/{starting_branch} before ledger append")
        })?;
    }

    append_ledger(&ledger_path, &record).context("appending sessions.jsonl")?;
    git::stage_and_commit(
        &repo,
        &["sessions.jsonl"],
        &format!("archive: ledger {session_id}"),
        &env,
    )
    .with_context(|| "committing sessions.jsonl append")?;

    // 6. Push the ledger commit on main. The archive branch was already pushed
    //    inside `run_in_archive_worktree` (or skipped if dry_run); here we only
    //    ship the new commit on the tracking branch, with bounded pull-rebase retry
    //    on race.
    if let Some(auth) = push_auth.as_ref()
        && let Some(remote) = primary_remote(&repo)?
    {
        let remote_url = git::run_git_output(&repo, &["remote", "get-url", &remote])
            .with_context(|| format!("git remote get-url {remote}"))?;
        git::validate_auth_url(&remote_url, &auth.url_prefix, "operator remote")?;

        // Push main with bounded rebase retry on race.
        git::push_or_retry(
            &repo,
            &remote,
            &starting_branch,
            &auth.token,
            &[],
            &auth.username,
            &auth.url_prefix,
            3,
        )
        .with_context(|| format!("pushing ledger commit to {remote}/{starting_branch}"))?;
    }

    Ok(ArchiveOutputs {
        already_archived: false,
        branch,
        branch_sha: Some(branch_sha),
        ledger_appended: true,
        artifact_url,
    })
}

/// Resolve where the archive's temporary git worktree lives. Prefer
/// `<agent_workspace_root>/archive-worktrees/<id>/`; fall back to
/// `<sessions_root>/.archive-worktrees/<id>/` when no workspace is
/// configured. The path MUST be OUTSIDE the operator repo —
/// otherwise switching branches inside the worktree could affect
/// the operator's main worktree (the very bug this refactor exists
/// to fix). When the resolved path lies inside the operator,
/// surface a clear error pointing at `agent_workspace_root` rather
/// than silently nesting.
fn archive_worktree_path(framework: &Layout, session_id: &str) -> Result<PathBuf> {
    let root = framework
        .agent_workspace_root
        .as_deref()
        .map(|w| w.join("archive-worktrees"))
        .unwrap_or_else(|| {
            framework
                .sessions_root
                .join(".archive-worktrees")
        });
    let candidate = root.join(session_id);

    // Refuse the legacy `<operator>/sessions/.archive-worktrees/<id>`
    // path: a git worktree inside the operator's main worktree puts
    // us back in the "branch switch wipes things" hazard zone. Force
    // operators on the legacy layout to set `agent_workspace_root`
    // before archive will work for them.
    if let Some(op) = framework
        .operator_repo_root
        .as_deref()
        && candidate.starts_with(op)
    {
        bail!(
            "archive worktree would land inside the operator repo at {} (operator={}); set \
             `agent_workspace_root` in config to a path outside the operator so the archive's \
             transient git worktree doesn't share working-tree state with the operator's main \
             checkout",
            candidate.display(),
            op.display(),
        );
    }
    Ok(candidate)
}

/// Drive the `session/<id>` branch ops through an isolated git
/// worktree so the operator's primary worktree never has
/// `sessions/<id>/` in its tree. Returns the resulting branch sha.
///
/// On entry: operator's main worktree is on `starting_branch`,
/// clean. On exit: same — the worktree at `archive_wt` has been
/// torn down regardless of success or failure of the inner steps.
#[allow(clippy::too_many_arguments)]
fn run_in_archive_worktree(
    repo: &Path,
    archive_wt: &Path,
    branch: &str,
    starting_branch: &str,
    session_id: &str,
    bulk_results_dir: &Path,
    env: &[(String, String)],
    push_auth: Option<&PushAuth>,
) -> Result<String> {
    // Defensive cleanup: a prior aborted archive run may have left a
    // worktree directory behind. `git worktree remove --force` cleans
    // both the bookkeeping and the on-disk dir; tolerate "not a
    // working tree" errors (untracked dir from a non-archive cause).
    if archive_wt.exists() {
        let _ =
            git::run_git(repo, &["worktree", "remove", "--force", archive_wt.to_str().unwrap()]);
        // If git refused (e.g. it wasn't a worktree), nuke the dir
        // directly. Best-effort.
        if archive_wt.exists() {
            let _ = std::fs::remove_dir_all(archive_wt);
        }
    }
    if let Some(parent) = archive_wt.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating worktree parent {}", parent.display()))?;
    }

    // Create or reuse the archive branch via a fresh worktree.
    if git::branch_exists(repo, branch) {
        git::run_git(repo, &["worktree", "add", archive_wt.to_str().unwrap(), branch])
            .with_context(|| format!("git worktree add (reuse branch {branch})"))?;
    } else {
        git::run_git(
            repo,
            &["worktree", "add", "-b", branch, archive_wt.to_str().unwrap(), starting_branch],
        )
        .with_context(|| format!("git worktree add -b {branch}"))?;
    }

    // Closure so we always tear down the worktree on the way out.
    let outcome = (|| -> Result<String> {
        // Copy bulk into the worktree's `sessions/<id>/results/`. We
        // commit the path `sessions/<id>/` regardless of where the
        // bulk physically lives on the operator's disk; the tree
        // object path is decoupled from the workspace path.
        let dest_session_dir = archive_wt
            .join("sessions")
            .join(session_id);
        let dest_results = dest_session_dir.join("results");
        std::fs::create_dir_all(&dest_results).with_context(|| {
            format!("mkdir -p {} (archive worktree session bulk dest)", dest_results.display())
        })?;
        copy_dir_recursive(bulk_results_dir, &dest_results).with_context(|| {
            format!(
                "copy bulk results from {} into {}",
                bulk_results_dir.display(),
                dest_results.display(),
            )
        })?;

        // Stage + commit. No-op when the branch was already at the
        // exact same tree (a redundant re-archive); a NEW commit
        // when content changed or branch was fresh.
        let commit_msg = format!("archive: {session_id}");
        git::stage_force_and_commit(
            archive_wt,
            &[&format!("sessions/{session_id}/")],
            &commit_msg,
            env,
        )
        .with_context(|| format!("staging sessions/{session_id}/ on {branch}"))?;

        let branch_sha = git::run_git_output(archive_wt, &["rev-parse", "HEAD"])
            .context("rev-parse HEAD in archive worktree")?;

        if let Some(auth) = push_auth
            && let Some(remote) = primary_remote(repo)?
        {
            let remote_url = git::run_git_output(repo, &["remote", "get-url", &remote])
                .with_context(|| format!("git remote get-url {remote}"))?;
            git::validate_auth_url(&remote_url, &auth.url_prefix, "operator remote")?;
            // Archive branch push: write-once, no rebase retry — a
            // race here implies a duplicate session id, which
            // should never happen in normal flow.
            git::push_or_retry(
                archive_wt,
                &remote,
                branch,
                &auth.token,
                &[],
                &auth.username,
                &auth.url_prefix,
                0,
            )
            .with_context(|| format!("pushing archive branch {branch}"))?;
        }

        Ok(branch_sha)
    })();

    // Always tear the worktree down — both git's bookkeeping and the
    // on-disk dir. Errors here are non-fatal; we'd rather surface
    // the original archive result.
    let _ = git::run_git(repo, &["worktree", "remove", "--force", archive_wt.to_str().unwrap()]);
    if archive_wt.exists() {
        let _ = std::fs::remove_dir_all(archive_wt);
    }

    outcome
}

/// Resolved PAT auth bundle. Computed ONCE per archive invocation
/// from settings and reused for both the archive-branch push (inside
/// the worktree) and the main-branch ledger push. Splitting the
/// resolution previously meant the archive-branch push silently
/// ignored `publish.token_file`, `git.auth_username`, and
/// `git.auth_url_prefix` — meaning custom token paths or non-GitHub
/// forges broke on the archive branch but worked on main.
struct PushAuth {
    token: String,
    username: String,
    url_prefix: String,
}

/// Pull the PAT auth bundle from settings. The token is read from
/// `publish_token_file` (or the default path) at this point; we
/// hold it in memory for the duration of the archive call only,
/// then drop it.
fn load_push_auth(inputs: &ArchiveInputs<'_>) -> Result<PushAuth> {
    let token_file = match &inputs
        .settings
        .publish
        .token_file
    {
        Some(p) => p.clone(),
        None => crate::session::publish::default_publish_token_path(),
    };
    let token = crate::session::publish::read_publish_token(&token_file)
        .context("archive push (set `publish.token_file` to override the default location)")?;
    let username = inputs
        .settings
        .git
        .effective_auth_username()
        .to_owned();
    let url_prefix = inputs
        .settings
        .git
        .effective_auth_url_prefix()?;
    Ok(PushAuth { token, username, url_prefix })
}

/// Recursively copy `src` into `dst`. Directories created as needed;
/// file contents copied. Symlinks are followed (`fs::copy` semantics)
/// — session bulk shouldn't carry any symlinks today, but if a phase
/// ever produces one, this surfaces it as a hard error rather than a
/// silently-skipped file.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("mkdir -p {}", dst.display()))?;
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        let read =
            std::fs::read_dir(&from).with_context(|| format!("reading {}", from.display()))?;
        for entry in read {
            let entry = entry?;
            let meta = entry.metadata()?;
            let dest = to.join(entry.file_name());
            if meta.is_dir() {
                std::fs::create_dir_all(&dest)
                    .with_context(|| format!("mkdir -p {}", dest.display()))?;
                stack.push((entry.path(), dest));
            } else if meta.is_file() {
                std::fs::copy(entry.path(), &dest).with_context(|| {
                    format!("copy {} -> {}", entry.path().display(), dest.display())
                })?;
            } else {
                bail!(
                    "unsupported entry type at {} (symlink or special file); archive flow does \
                     not handle these",
                    entry.path().display()
                );
            }
        }
    }
    Ok(())
}

/// Read an optional artifact: returns `Ok(None)` iff the file is
/// missing, `Ok(Some(t))` on success, and the underlying error
/// otherwise (parse failure, validation failure, permission denied,
/// IO error). Lets archive distinguish "phase hasn't run yet" from
/// "source data is corrupt" — the second must NOT degrade silently
/// into an empty ledger record.
///
/// Uses `fs::metadata` rather than `Path::exists` so transient IO
/// errors (e.g. EACCES on a parent dir) surface as errors instead of
/// being silently treated as "missing" — `Path::exists` collapses
/// every error kind into `false`, which would let a permission-
/// denied source artifact degrade to an empty ledger row.
fn read_optional<T>(path: &Path, load: impl FnOnce() -> Result<T>) -> Result<Option<T>> {
    match std::fs::metadata(path) {
        Ok(_) => load().map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context(format!("stat {}", path.display()))),
    }
}

/// True iff `sessions.jsonl` already has a `session_completed` record
/// matching `id`. Missing file ⇒ false. Malformed lines are skipped
/// (we tolerate the legacy/hand-edited cases) but a malformed file is
/// not in itself an error here.
fn ledger_contains_id(path: &Path, id: &str) -> Result<bool> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", path.display()))),
    };
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // `from_ledger_line` accepts both legacy v1 (no source_*
        // fields) and current v2 records, so an archive idempotency
        // check on a long-running operator-main `sessions.jsonl`
        // doesn't silently skip every legacy line.
        let Ok(rec) = SessionRecord::from_ledger_line(line) else {
            continue;
        };
        if rec.id == id {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Append one record to `sessions.jsonl`, creating the file if needed.
/// Writes one JSON line followed by `\n`. Caller is responsible for
/// staging + committing.
fn append_ledger(path: &Path, record: &SessionRecord) -> Result<()> {
    use std::io::Write as _;
    let line = record
        .to_json()
        .context("serializing SessionRecord")?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {} for append", path.display()))?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// Build the [`SessionRecord`] from on-disk artifacts. Tolerates
/// missing optional inputs (e.g. summary.json doesn't exist when the
/// session failed at triage) but errors when required state is gone.
fn build_session_record(inputs: &ArchiveInputs<'_>, branch: &str) -> Result<SessionRecord> {
    let session_id = inputs.layout.id.clone();
    let session_id_str = session_id.as_str().to_owned();

    // Optional summary (only present after finalize). A `NotFound`
    // error is fine — failed pre-finalize sessions legitimately won't
    // have it. Any OTHER error (malformed JSON, validation failure,
    // permission denied) must abort the archive: minting a permanent
    // `succeeded` ledger row from corrupt source data would poison the
    // ledger forever, and `succeeded` would be a lie if the summary is
    // unreadable.
    let summary =
        read_optional(&inputs.layout.summary_json(), || loader::read_summary(inputs.layout))
            .context("loading summary.json (treated as required if it exists)")?;
    let targets_doc = read_optional(
        &inputs
            .layout
            .optimization_targets_json(),
        || loader::read_optimization_targets(inputs.layout),
    )?;

    let (status, failure_phase, failure_reason) = derive_session_status(inputs.layout);
    let started_at = derive_started_at_from_id(&session_id_str);
    let finished_at =
        derive_finished_at(&inputs.layout.results_dir).unwrap_or_else(now_utc_iso8601);

    let baseline_run_ids = read_baseline_run_ids(inputs.layout);
    let range = SessionRange {
        start_at: inputs
            .settings
            .stacks_bench
            .start_at,
        count: inputs
            .settings
            .stacks_bench
            .count,
        warmup: inputs
            .settings
            .stacks_bench
            .warmup,
        filter: inputs
            .settings
            .stacks_bench
            .filter
            .clone(),
        network: inputs
            .settings
            .stacks_bench
            .effective_network()
            .to_owned(),
    };

    // Pre-v3 the operator submodule HEAD was the canonical "source
    // SHA" recorded here. Post-cutover the per-session `source.json`
    // (populated below) carries the same information under
    // `source_sha`; this field stays `None` on new sessions and is
    // retained on `SessionRecord` only for read-compatibility with
    // archived pre-cutover entries.
    let stacks_core_base_sha = None;

    let targets = build_target_records(inputs.layout, summary.as_ref(), targets_doc.as_ref())
        .unwrap_or_default();

    // Populate source-provenance fields from
    // `<session>/results/source.json` when it exists. Legacy
    // sessions have no source.json — leave fields `None` so those
    // archives continue to flow through.
    let source_path = inputs.layout.source_json();
    let (source_url, source_branch, source_sha, source_fetched_at) = if source_path.exists() {
        let s = crate::models::source::SourceJson::read(&source_path)
            .with_context(|| format!("loading source.json at {}", source_path.display()))?;
        (Some(s.url), Some(s.branch), Some(s.sha), Some(s.fetched_at))
    } else {
        (None, None, None, None)
    };

    Ok(SessionRecord {
        kind: SessionRecordKind::SessionCompleted,
        schema_version: crate::models::common::SchemaVersionV2,
        id: session_id_str,
        artifact_branch: branch.to_owned(),
        artifact_sha: String::new(), // filled in after the commit lands
        artifact_url: None,          // filled in by caller after push
        started_at,
        finished_at,
        status,
        failure_phase,
        failure_reason,
        sbagent_version: SBAGENT_VERSION.to_owned(),
        sbagent_git_sha: sbagent_git_sha().map(str::to_owned),
        stacks_core_base_sha,
        range,
        baseline_run_ids,
        phase_durations_secs: BTreeMap::new(),
        targets,
        source_url,
        source_branch,
        source_sha,
        source_fetched_at,
    })
}

/// Derive a coarse session-level status by inspecting on-disk state.
/// Today we only return `succeeded` (when summary exists) or `failed`
/// (when it doesn't but optimization-targets does) or `aborted`
/// (everything else). A future per-session manifest with an explicit
/// terminal state would replace this heuristic.
fn derive_session_status(
    layout: &SessionLayout,
) -> (SessionStatus, Option<String>, Option<String>) {
    if layout.summary_json().exists() {
        return (SessionStatus::Succeeded, None, None);
    }
    if layout
        .optimization_targets_json()
        .exists()
    {
        return (
            SessionStatus::Failed,
            Some("post-merge".to_owned()),
            Some("summary.json absent — finalize did not run to completion".to_owned()),
        );
    }
    (
        SessionStatus::Aborted,
        Some("pre-merge".to_owned()),
        Some("session did not produce optimization-targets.json".to_owned()),
    )
}

/// Map the leading 15 chars of `YYYYMMDD-HHMMSS[-suffix]` into
/// `YYYY-MM-DDTHH:MM:SSZ`. Falls back to `now_utc_iso8601()` when the
/// id doesn't fit the expected shape (operator-supplied non-conventional
/// id).
fn derive_started_at_from_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().take(15).collect();
    if chars.len() != 15
        || !chars[0..8]
            .iter()
            .all(char::is_ascii_digit)
        || chars[8] != '-'
        || !chars[9..15]
            .iter()
            .all(char::is_ascii_digit)
    {
        return now_utc_iso8601();
    }
    let y: String = chars[0..4].iter().collect();
    let mo: String = chars[4..6].iter().collect();
    let d: String = chars[6..8].iter().collect();
    let h: String = chars[9..11].iter().collect();
    let mi: String = chars[11..13].iter().collect();
    let s: String = chars[13..15].iter().collect();
    format!("{y}-{mo}-{d}T{h}:{mi}:{s}Z")
}

/// Latest mtime under `results_dir`, formatted as ISO 8601 UTC. v1
/// heuristic for `finished_at` — see module docs.
fn derive_finished_at(results_dir: &Path) -> Option<String> {
    let latest = latest_mtime(results_dir).ok()??;
    let secs = latest
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    // Reuse the same civil-time encoding the analyzed_rejections
    // ledger uses, via a roundabout: we can't reach
    // `civil_from_days` (private), but we can pin SystemTime → now()
    // shape by formatting through a small helper.
    Some(format_unix_secs_iso8601(secs))
}

fn format_unix_secs_iso8601(secs: u64) -> String {
    let secs_per_day: u64 = 86_400;
    let days = secs / secs_per_day;
    let s_of_day = secs % secs_per_day;
    let hour = s_of_day / 3600;
    let minute = (s_of_day % 3600) / 60;
    let second = s_of_day % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant proleptic-Gregorian algorithm — same form as
/// `types.rs::civil_from_days` and
/// `analyzed_rejections::civil_from_days`. Kept local to avoid
/// touching their visibility for what is essentially a one-call dep.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

fn latest_mtime(dir: &Path) -> Result<Option<SystemTime>> {
    let mut latest: Option<SystemTime> = None;
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let read = match fs::read_dir(&d) {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", d.display()))),
        };
        for entry in read {
            let entry = entry?;
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if let Ok(mtime) = meta.modified() {
                latest = Some(latest.map_or(mtime, |l| std::cmp::max(l, mtime)));
            }
        }
    }
    Ok(latest)
}

fn read_baseline_run_ids(layout: &SessionLayout) -> Vec<i64> {
    let mut out = Vec::new();
    if let Ok(id) = loader::read_run_id_file(&layout.baseline_run_id_path()) {
        out.push(id);
    }
    if let Ok(id) = loader::read_run_id_file(&layout.baseline_rerun_id_path()) {
        out.push(id);
    }
    out
}

/// One row per merged target. Joins `optimization-targets.json` (the
/// roster of targets the session committed to) with `summary.json`
/// (per-target outcomes, only present when finalize ran).
fn build_target_records(
    layout: &SessionLayout,
    summary: Option<&Summary>,
    targets_doc: Option<&crate::models::targets::OptimizationTargets>,
) -> Result<Vec<TargetRecord>> {
    let Some(targets_doc) = targets_doc else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for target in &targets_doc.targets {
        let exp = summary.and_then(|s| {
            s.experiments
                .iter()
                .find(|e| e.target_id == target.id)
        });
        let (status, status_stage, reason_code) = derive_target_status(exp);
        let bench = build_target_bench(layout, target, exp, summary);
        let family_id = target
            .merged_from
            .first()
            .map_or_else(|| "unknown".to_owned(), |mf| mf.family_id.clone());
        rows.push(TargetRecord {
            id: target.id.clone(),
            family_id,
            bucket: bucket_str(target.bucket),
            delivery_mode: target.delivery_mode,
            status,
            status_stage,
            reason_code,
            // Pulled from `summary.json` Experiment row, which finalize
            // populates from `optimize/<target>/coordinator-provenance.json`
            // (Pass 1c provenance sidecar). `None` for any target whose
            // optimizer never committed (aborted before commit, or
            // session predates the sidecar). `pr_url` / `issue_url`
            // still wait on publish-feedback integration.
            head_sha: exp.and_then(|e| e.head_sha.clone()),
            pr_url: None,
            issue_url: None,
            bench,
        });
    }
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(rows)
}

fn derive_target_status(
    exp: Option<&Experiment>,
) -> (TargetStatus, Option<TargetStatusStage>, Option<String>) {
    let Some(e) = exp else {
        return (
            TargetStatus::Aborted,
            Some(TargetStatusStage::Optimizer),
            Some("no summary row".to_owned()),
        );
    };
    match e.status {
        ExperimentStatus::Accepted
        | ExperimentStatus::PocLanded
        | ExperimentStatus::RoutedToIssue => (TargetStatus::Accepted, None, e.reason.clone()),
        ExperimentStatus::Rejected => (
            TargetStatus::Rejected,
            Some(TargetStatusStage::Bench),
            e.reason
                .clone()
                .or_else(|| Some("noise_floor".to_owned())),
        ),
        ExperimentStatus::Aborted => {
            (TargetStatus::Aborted, Some(TargetStatusStage::Optimizer), e.reason.clone())
        }
    }
}

fn build_target_bench(
    _layout: &SessionLayout,
    target: &crate::models::targets::MergedTarget,
    exp: Option<&Experiment>,
    summary: Option<&Summary>,
) -> Option<TargetBench> {
    let exp = exp?;
    // Consensus targets never bench.
    if !matches!(target.delivery_mode, DeliveryMode::NormalPr) {
        return None;
    }
    let candidate_run_ids = exp
        .run_ids
        .clone()
        .unwrap_or_default();
    if candidate_run_ids.is_empty() {
        return None;
    }
    // Pass 1a: prefer the per-target baseline run ids finalize
    // ACTUALLY used (carried in `Experiment.baseline_run_ids` when
    // Phase 1.8 calibration produced them). Fall back to the
    // session-level baseline ids only when finalize fell back too
    // (target had no `verification_replay`). Without this branch
    // the ledger would claim finalize used P0 run/rerun while
    // `improvement_pct` was actually computed from targeted
    // calibration — misleading to any future reader.
    let baseline_run_ids = exp
        .baseline_run_ids
        .clone()
        .or_else(|| summary.map(|s| vec![s.baseline_run_id, s.baseline_rerun_id]))
        .unwrap_or_default();
    let noise_floor = summary
        .map(|s| s.noise_floor_pct)
        .unwrap_or(0.0);
    let improvement = exp
        .improvement_pct
        .unwrap_or(0.0);
    let passes = improvement >= noise_floor;
    // The wallclock totals are NOT in the summary row — they live in
    // per-run `bench-run.json`. v1 leaves them zero; aggregating would
    // require parsing every `run-<n>/bench-run.json` and isn't
    // load-bearing for the ledger's primary use case (leaderboard /
    // timeline). Future work item.
    Some(TargetBench {
        baseline_run_ids,
        candidate_run_ids,
        baseline_total_us: 0,
        candidate_total_us: 0,
        improvement_pct: improvement,
        passes_noise_floor: passes,
    })
}

/// Stringify a [`crate::models::common::Bucket`] using the same
/// `snake_case` form schemars uses on the wire, so the ledger
/// matches the same identifier across artifacts.
fn bucket_str(bucket: crate::models::common::Bucket) -> String {
    use crate::models::common::Bucket;
    match bucket {
        Bucket::BlockProcessing => "block_processing".to_owned(),
        Bucket::BlockCommit => "block_commit".to_owned(),
    }
}

fn ensure_clean_worktree(repo: &Path) -> Result<()> {
    let porcelain = git::run_git_output(repo, &["status", "--porcelain"])
        .context("git status --porcelain in operator repo")?;
    if porcelain
        .lines()
        .any(|l| !l.is_empty())
    {
        bail!(
            "operator repo has uncommitted changes — commit or stash before archive:\n{porcelain}"
        );
    }
    Ok(())
}

/// Pick the remote to push to. v1 contract: a single "origin" remote.
/// Operators with split read/write remotes can override via env or set
/// their `origin` to the write target. Returns `None` when no remote
/// is configured (purely local operator repo — archive proceeds
/// without push).
fn primary_remote(repo: &Path) -> Result<Option<String>> {
    let raw =
        git::run_git_output(repo, &["remote"]).context("git remote (listing operator remotes)")?;
    let names: Vec<&str> = raw
        .lines()
        .filter(|l| !l.is_empty())
        .collect();
    if names.is_empty() {
        return Ok(None);
    }
    if names.contains(&"origin") {
        return Ok(Some("origin".to_owned()));
    }
    Ok(Some(names[0].to_owned()))
}

/// Identity env-var pairs for `git commit` so the archive commits land
/// as the bot, not the local user. Mirrors what the optimizer phase
/// does (`optimizer_git_env`), kept inline here to avoid pulling that
/// publish-specific helper.
fn bot_identity_env(settings: &Settings) -> Vec<(String, String)> {
    let name = settings
        .git
        .effective_author_name()
        .to_owned();
    let email = settings
        .git
        .effective_author_email()
        .to_owned();
    vec![
        ("GIT_AUTHOR_NAME".into(), name.clone()),
        ("GIT_AUTHOR_EMAIL".into(), email.clone()),
        ("GIT_COMMITTER_NAME".into(), name),
        ("GIT_COMMITTER_EMAIL".into(), email),
        // Disable signing per the operator-repo policy. Local git
        // config may have `commit.gpgsign=true`; this env var override
        // bypasses it for the archive commit without mutating the
        // operator's config.
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "commit.gpgsign".into()),
        ("GIT_CONFIG_VALUE_0".into(), "false".into()),
    ]
}

/// Build a browsable URL to the archive branch's tree view on the
/// remote, when the remote is a recognized GitHub HTTPS URL.
/// `https://github.com/owner/repo.git` →
/// `https://github.com/owner/repo/tree/<branch>`. Errors on anything
/// else (SSH, other forges, etc.) so the caller surfaces `None` for
/// unsupported shapes rather than fabricating an invalid URL.
fn derive_artifact_url(repo: &Path, branch: &str) -> Result<String> {
    let url = git::run_git_output(repo, &["remote", "get-url", "origin"])
        .context("git remote get-url origin")?;
    let stripped = url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| anyhow!("remote {url:?} is not a recognized https://github.com/ URL"))?;
    let stripped = stripped
        .strip_suffix(".git")
        .unwrap_or(stripped);
    Ok(format!("https://github.com/{stripped}/tree/{branch}"))
}

/// Pretty-format an [`ArchiveOutputs`] for CLI output. Single-line per
/// fact so the operator can grep against it.
pub fn print_outputs(out: &ArchiveOutputs) {
    if out.already_archived {
        println!("archive: session already in ledger (branch={}); no-op", out.branch);
        if let Some(url) = &out.artifact_url {
            println!("archive: artifact_url={url}");
        }
        return;
    }
    println!("archive: branch={}", out.branch);
    if let Some(sha) = &out.branch_sha {
        println!("archive: branch_sha={sha}");
    }
    println!("archive: ledger_appended={}", out.ledger_appended);
    if let Some(url) = &out.artifact_url {
        println!("archive: artifact_url={url}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn started_at_parses_canonical_id() {
        assert_eq!(derive_started_at_from_id("20260518-190321"), "2026-05-18T19:03:21Z");
    }

    #[test]
    fn started_at_parses_suffixed_id() {
        assert_eq!(
            derive_started_at_from_id("20260518-190321-nextest-flags-smoke"),
            "2026-05-18T19:03:21Z"
        );
    }

    #[test]
    fn started_at_falls_back_on_unconventional_id() {
        let out = derive_started_at_from_id("custom-session-id");
        // Should look like an ISO 8601 timestamp; verify shape only
        // (the exact value is "now").
        assert!(out.ends_with('Z'));
        assert!(out.contains('T'));
    }

    #[test]
    fn ledger_contains_id_returns_false_for_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp
            .path()
            .join("sessions.jsonl");
        assert!(!ledger_contains_id(&missing, "any").unwrap());
    }

    #[test]
    fn ledger_contains_id_skips_malformed_lines_and_finds_match() {
        use std::io::Write as _;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("sessions.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "this is not json").unwrap();
        writeln!(f, "{{}}").unwrap();
        let rec = SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: crate::models::common::SchemaVersionV2,
            id: "20260518-190321".to_owned(),
            artifact_branch: "session/20260518-190321".to_owned(),
            artifact_sha: "abc".to_owned(),
            artifact_url: None,
            started_at: "2026-05-18T19:03:21Z".to_owned(),
            finished_at: "2026-05-18T19:11:42Z".to_owned(),
            status: SessionStatus::Succeeded,
            failure_phase: None,
            failure_reason: None,
            sbagent_version: "0.1.0".to_owned(),
            sbagent_git_sha: None,
            stacks_core_base_sha: None,
            range: SessionRange {
                start_at: None,
                count: None,
                warmup: None,
                filter: None,
                network: "mainnet".to_owned(),
            },
            baseline_run_ids: vec![],
            phase_durations_secs: BTreeMap::new(),
            targets: vec![],
            source_url: None,
            source_branch: None,
            source_sha: None,
            source_fetched_at: None,
        };
        let line = rec.to_json().unwrap();
        writeln!(f, "{line}").unwrap();
        drop(f);

        assert!(ledger_contains_id(&path, "20260518-190321").unwrap());
        assert!(!ledger_contains_id(&path, "missing-id").unwrap());
    }

    #[test]
    fn derive_artifact_url_translates_https_github_remote() {
        // Skip when git isn't available — but this test only needs
        // the URL transform, not a real git repo. We exercise the
        // transform directly via a helper-style local test.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        crate::git::run_git(repo, &["init", "-q", "-b", "main"]).unwrap();
        crate::git::run_git(
            repo,
            &["remote", "add", "origin", "https://github.com/owner/repo.git"],
        )
        .unwrap();
        let url = derive_artifact_url(repo, "session/20260518-190321").unwrap();
        assert_eq!(url, "https://github.com/owner/repo/tree/session/20260518-190321");
    }

    #[test]
    fn derive_artifact_url_rejects_ssh_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        crate::git::run_git(repo, &["init", "-q", "-b", "main"]).unwrap();
        crate::git::run_git(repo, &["remote", "add", "origin", "git@github.com:owner/repo.git"])
            .unwrap();
        assert!(derive_artifact_url(repo, "session/x").is_err());
    }
}
