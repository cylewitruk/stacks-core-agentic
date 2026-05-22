//! Phase 2: optimizer fan-out.
//!
//! Per merged target: create a per-target git clone at
//! `<sessions>/<id>/worktrees/<target_id>` (path name is historical; the
//! checkout is a stand-alone clone, NOT a linked worktree — see
//! [`GitCheckoutManager`]), switched to a fresh
//! `agent/<session>/<target>` branch off the configured base branch.
//! Render the optimizer prompt with the per-target `${TARGET_JSON}`
//! payload, then invoke Codex inside that clone.
//!
//! Three branches per target's `delivery_mode`:
//! - **`consensus_issue`** — write `optimize/<id>/consensus-issue.md` marker;
//!   SKIP the optimizer; clean any stale optimizer markers.
//! - **`consensus_poc_pr`** — render with `POC_TEST_SCOPE_EXPR` joined from
//!   `poc_test_scope`; invoke Codex normally.
//! - **`normal_pr`** — render with `POC_TEST_SCOPE_EXPR=""`; invoke Codex
//!   normally.
//!
//! Checkout management is shelled out to `git clone` / `git switch`.
//! Tests inject a [`GitCheckoutManager`] fake that just creates/removes
//! directories without invoking git.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::harnesses::{AgentHarness, InvokeInputs};
use crate::layout::Layout;
use crate::models::common::{DeliveryMode, SchemaVersionV2};
use crate::models::optimizer_report::{
    AbortedOutcomeTag, AbortedReport, FailedGate, ImplementedReport, OptimizerReport,
};
use crate::models::targets::MergedTarget;
use crate::models::{FromJsonValidated, ToJson, ValidateModel};
use crate::prompts;
use crate::session::{SessionLayout, loader};
use crate::settings::Settings;

/// Manages per-target git checkouts for the optimizer fan-out. Each
/// checkout is a [local clone](https://git-scm.com/docs/git-clone) of
/// the operator's `base` repo with its **own `.git/` directory inside
/// the checkout cwd** — NOT a linked worktree.
///
/// Why clones, not linked worktrees: a linked worktree's `.git` is a
/// pointer file back to `<base>/.git/worktrees/<wt>/` (or
/// `<superproject>/.git/modules/<sub>/worktrees/<wt>/` for submodules).
/// Every `git commit` / `git reset` from inside the worktree writes
/// the index lock + ref updates to that out-of-cwd path. Codex's
/// `workspace-write` sandbox + macOS Seatbelt deny those writes even
/// when the common-dir is in `--add-dir`. A `git clone` keeps all git
/// state inside the cwd, so the agent can commit + reset without
/// crossing the sandbox boundary.
///
/// Disk economy: `git clone --reference <base> --local` shares the
/// base repo's object store via `objects/info/alternates` — only
/// working-tree files + per-clone refs duplicate. The clone's `target/`
/// (cargo build cache) is per-target, same as the old worktree model.
///
/// Branch model: each clone gets a fresh `agent/<session>/<target>`
/// branch created from `base_branch` at clone time. The branch lives
/// inside the clone; tearing down the clone (`rm -rf`) deletes the
/// branch with it. No separate `git branch -D` dance.
///
/// Push model (Phase 5): the clone's `origin` is rewritten from the
/// default `<base>` (a local path) to `<base>`'s own `origin` URL so
/// `git push origin <branch>` from the clone goes directly to the
/// configured GitHub fork — not back to the operator's local
/// checkout.
///
/// Tests inject a fake that just creates directories without invoking
/// git.
pub trait GitCheckoutManager: Send + Sync {
    /// Tear down `checkout` if it exists, then create a fresh git
    /// clone at `checkout` from `base`, switched to a new branch
    /// `branch_name` rooted at `base_branch`'s tip. The clone shares
    /// `base`'s object store via `--reference --local`. Equivalent
    /// shell:
    ///
    /// ```bash
    /// rm -rf "$CHECKOUT"
    /// git clone --reference "$BASE" --branch "$BASE_BRANCH" --local "$BASE" "$CHECKOUT"
    /// git -C "$CHECKOUT" switch -c "$BRANCH_NAME"
    /// git -C "$CHECKOUT" remote set-url origin "$(git -C $BASE remote get-url origin)"
    /// ```
    ///
    /// Idempotent — pre-existing `checkout` dirs (real clone, leftover
    /// worktree, stale junk) are nuked before the clone runs.
    fn recreate_checkout(
        &self,
        base: &Path,
        checkout: &Path,
        branch_name: &str,
        base_branch: &str,
    ) -> Result<()>;

    /// Tear down `checkout` if it exists. Just `rm -rf` — the clone
    /// owns its own git state, no `git worktree remove` /
    /// `worktree prune` / `branch -D` dance. Idempotent: a missing
    /// checkout is a no-op (returns `false`).
    fn remove_checkout(&self, checkout: &Path) -> Result<bool>;
}

/// Default impl: shells out to `git`.
pub struct StdGitCheckoutManager;

impl GitCheckoutManager for StdGitCheckoutManager {
    fn recreate_checkout(
        &self,
        base: &Path,
        checkout: &Path,
        branch_name: &str,
        base_branch: &str,
    ) -> Result<()> {
        // Tear down any pre-existing checkout at this path before
        // re-cloning. Three cases:
        //   1. Real clone (`.git/` is a directory) — `rm -rf` is enough.
        //   2. Linked worktree from the old layout (`.git` is a FILE pointing back to
        //      `<base>/.git/worktrees/<wt>/`) — a plain `rm -rf` removes the working
        //      tree but leaves `<base>/.git/worktrees/<wt>/` registered. The next `git
        //      worktree list` still shows the stale path, and a future `git worktree
        //      add` at the same location could collide. Run `git -C <base> worktree
        //      remove --force`
        //      + `git worktree prune` first to clear git's
        //      bookkeeping, THEN rm -rf as a belt-and-suspenders
        //      cleanup.
        //   3. Stale non-git dir — `rm -rf` is enough.
        cleanup_existing_checkout(base, checkout).with_context(|| {
            format!("cleaning up existing checkout {} before re-cloning", checkout.display())
        })?;
        if let Some(parent) = checkout.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        // Per-target clone: `--reference` shares the object store with
        // the base, `--local` enables hardlinks for refs/HEAD/etc.,
        // `--branch <base_branch>` checks out base_branch's tip.
        crate::git::clone_with_reference(base, base_branch, checkout)?;

        // Switch to the agent's per-target branch (created from the
        // base_branch tip we just cloned).
        crate::git::switch_create_branch(checkout, branch_name)?;

        // Replicate every remote from `base` into the clone. `git clone
        // --local` only creates `origin` pointing at the local base path
        // (not at GitHub). Phase 5 pushes using the operator's
        // configured `publish_remote`, which may be `origin` OR a
        // separate remote (e.g. `fork`). Copying all base remotes wholesale
        // means the clone has whatever push target the operator set
        // up — no special-casing per remote name needed here.
        replicate_remotes(base, checkout).with_context(|| {
            format!("replicating remotes from {} into {}", base.display(), checkout.display())
        })?;
        Ok(())
    }

    fn remove_checkout(&self, checkout: &Path) -> Result<bool> {
        if !checkout.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(checkout)
            .with_context(|| format!("rm -rf {}", checkout.display()))?;
        Ok(true)
    }
}

/// Best-effort cleanup of whatever's at `checkout` before re-cloning.
/// Handles the three migration cases described in
/// [`StdGitCheckoutManager::recreate_checkout`]:
/// real clone, linked-worktree pointer, or stale non-git dir.
///
/// For the linked-worktree case (most subtle), we run
/// `git -C <base> worktree remove --force <checkout>` + `prune`
/// to clear the registry under `<base>/.git/worktrees/<wt>/`
/// before the `rm -rf` — without that, future `git worktree list`
/// would still show the stale path.
fn cleanup_existing_checkout(base: &Path, checkout: &Path) -> Result<()> {
    if !checkout.exists() {
        return Ok(());
    }

    // Detect a linked-worktree pointer: in a linked worktree, `.git`
    // is a *file* containing `gitdir: /path/to/.git/worktrees/<wt>`,
    // not a directory. In a real clone (or stale non-git dir), it's
    // a directory (or missing).
    let dot_git = checkout.join(".git");
    let is_linked_worktree = dot_git.is_file();

    if is_linked_worktree {
        // `git worktree remove --force` + `prune` clears
        // `<base>/.git/worktrees/<wt>/` so a future `git worktree list`
        // doesn't report a stale registration. Best-effort: if git
        // refuses (worktree locked / corrupt), fall through to the
        // `rm -rf` below — `prune` will eventually catch the missing
        // dir on the next `git worktree` invocation.
        crate::git::worktree_remove_force_quiet(base, checkout);
        crate::git::worktree_prune_quiet(base);
    }

    // Belt + suspenders: nuke the directory regardless of which path
    // got us here. For real clones this is the only step needed; for
    // linked worktrees the previous `worktree remove` may or may not
    // have already deleted the dir (depending on git version), so
    // `rm -rf` is idempotent.
    if checkout.exists() {
        std::fs::remove_dir_all(checkout)
            .with_context(|| format!("rm -rf {}", checkout.display()))?;
    }
    Ok(())
}

/// Replicate every remote from `base` into `checkout` (which was just
/// produced by `git clone --local`, so its only remote is `origin`
/// pointing at the local base path).
///
/// For each base remote `<name>`:
///   - if the clone has it (only `origin` will, after a fresh clone): `git -C
///     <checkout> remote set-url <name> <base-url>`
///   - else: `git -C <checkout> remote add <name> <base-url>`
///
/// Hard-errors on rewrite failure — silently leaving the clone's
/// `origin` pointing at the local base would mean Phase 5's
/// `git push origin <branch>` writes back to the operator's local
/// checkout (not GitHub), which is exactly the bug the rewrite was
/// supposed to prevent.
fn replicate_remotes(base: &Path, checkout: &Path) -> Result<()> {
    let remote_names = crate::git::list_remotes(base).with_context(|| {
        format!(
            "listing remotes in {} (cannot replicate into clone {})",
            base.display(),
            checkout.display(),
        )
    })?;
    if remote_names.is_empty() {
        // Base has no remotes — nothing to replicate. The clone's
        // `origin` still points at the local base path, which is
        // fine for a local-only repo but Phase 5 publish will fail
        // loudly if the operator configured `publish_remote` against
        // a remote that doesn't exist. That's the right failure
        // mode (clearer than silently pushing to the local base).
        return Ok(());
    }
    for name in &remote_names {
        let url = crate::git::get_remote_url(base, name)?;
        if url.is_empty() {
            anyhow::bail!("remote {name} in {} has empty URL", base.display());
        }
        crate::git::add_or_set_remote(checkout, name, &url)?;
    }
    Ok(())
}

/// Tear down the per-target git clone for every experiment in this
/// session whose typed report says `outcome=aborted`, has no report at
/// all (agent crashed / never finished), or whose report fails to
/// parse / validate (treat as abort for cleanup purposes).
///
/// Since each per-target checkout is a stand-alone clone (own `.git/`
/// inside its cwd, own refs, own branch), teardown is a single
/// `rm -rf <checkout>` — no `git worktree remove` / `git worktree
/// prune` / `git branch -D` ordering to worry about. The
/// `agent/<session>/<target>` branch lived inside the clone and goes
/// away with it.
///
/// **Experiments whose typed report has `outcome=implemented` are NOT
/// touched** — their checkouts are what Phase 5 publish reads + pushes
/// from, and their commits must survive until the PR is filed.
/// Operators may also want to inspect a kept checkout post-session.
///
/// Returns the count of checkouts dropped. Failures on individual
/// entries are logged but don't abort the caller — this runs at
/// session-end and a partial cleanup is better than a crash that
/// leaves the operator's tree in an even messier state.
pub fn prune_aborted_experiments(
    git: &dyn GitCheckoutManager,
    checkouts_root: &Path,
    layout: &SessionLayout,
) -> Result<usize> {
    let experiments_dir = layout.optimize_dir();
    let entries = match std::fs::read_dir(&experiments_dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(
                anyhow::Error::new(e).context(format!("reading {}", experiments_dir.display()))
            );
        }
    };
    let mut dropped = 0usize;
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let exp = entry.path();
        let target_id = entry
            .file_name()
            .to_string_lossy()
            .into_owned();
        // Typed-report gating: `outcome=implemented` → keep checkout
        // (publish reads it). `outcome=aborted` OR no report → drop it.
        // Skip on malformed JSON / validation failures too — those
        // would have been demoted by `coordinator_commit_if_kept` if
        // reachable, but if not, dropping the checkout is the safe
        // conservative call (matches the prior "no marker" path).
        if let Ok(Some(OptimizerReport::Implemented(_))) = read_optimizer_report(&exp) {
            continue;
        }

        let checkout = checkouts_root.join(&target_id);
        match git.remove_checkout(&checkout) {
            Ok(true) => {
                dropped += 1;
                tracing::info!(
                    target = "session.cleanup",
                    checkout = %checkout.display(),
                    "dropped aborted-experiment checkout (clone + branch)"
                );
            }
            Ok(false) => {
                tracing::debug!(
                    target = "session.cleanup",
                    checkout = %checkout.display(),
                    "checkout absent (already removed or never created)"
                );
            }
            Err(e) => {
                tracing::warn!(
                    target = "session.cleanup",
                    checkout = %checkout.display(),
                    error = %e,
                    "checkout remove errored"
                );
            }
        }
    }
    Ok(dropped)
}

/// Build the env-var set passed to the optimizer codex invocation so
/// `git commit` inside the agent's worktree uses the bot identity AND
/// skips the operator's signing setup — without mutating any git
/// config file.
///
/// **Why env vars, not `git config`**: `git config <key> <value>` from
/// inside a linked worktree writes to the SHARED repo config (the
/// common-dir's `.git/config`), not a per-worktree file — confirmed
/// against real git. So a naive "worktree-local" `git config user.name`
/// silently mutates the operator's `repos/stacks-core/` checkout for
/// every subsequent operation, agent OR human. `extensions.worktreeConfig`
/// would scope per worktree but still requires opting the base repo in,
/// which mutates shared state.
///
/// Env vars sidestep both: `GIT_AUTHOR_*` / `GIT_COMMITTER_*` cover the
/// commit-identity path, and `GIT_CONFIG_COUNT` + `GIT_CONFIG_KEY_N` /
/// `GIT_CONFIG_VALUE_N` inject per-process config overrides that git
/// applies on top of every config file (operator's `~/.gitconfig` still
/// supplies proxies/credential-helpers/etc.; only the keys we override
/// are clobbered). Scope is the codex process tree — when codex exits,
/// no state persists.
///
/// **Why we override signing**: the operator's `~/.gitconfig` typically
/// has `commit.gpgsign=true` + an SSH/GPG signing program tied to a
/// hardware token. Inside the codex sandbox the agent can't reach
/// `$SSH_AUTH_SOCK` / GPG sockets / a YubiKey, so `git commit` fails
/// (often with "Operation not permitted" — the failure is in the
/// signing subprocess, not the index.lock the surface error blames).
/// Setting `commit.gpgsign=false` + `tag.gpgsign=false` via env makes
/// agent commits unsigned. When the operator graduates to a bot
/// identity with its own signing key, this can be relaxed.
pub fn optimizer_git_env(settings: &Settings) -> Vec<(String, String)> {
    let name = settings
        .git_author_name
        .as_deref()
        .unwrap_or("stacks-bench-bot")
        .to_owned();
    let email = settings
        .git_author_email
        .as_deref()
        .unwrap_or("stacks-bench-bot@users.noreply.github.com")
        .to_owned();
    let overrides: &[(&str, &str)] = &[
        ("user.name", &name),
        ("user.email", &email),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
    ];
    let mut env: Vec<(String, String)> = vec![
        ("GIT_AUTHOR_NAME".to_owned(), name.clone()),
        ("GIT_AUTHOR_EMAIL".to_owned(), email.clone()),
        ("GIT_COMMITTER_NAME".to_owned(), name.clone()),
        ("GIT_COMMITTER_EMAIL".to_owned(), email.clone()),
        ("GIT_CONFIG_COUNT".to_owned(), overrides.len().to_string()),
    ];
    for (i, (k, v)) in overrides.iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), (*k).to_owned()));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), (*v).to_owned()));
    }
    env
}

/// Inputs to an optimizer fan-out.
pub struct Inputs<H: AgentHarness + 'static, G: GitCheckoutManager + 'static> {
    /// Resolved per-session layout.
    pub layout: SessionLayout,
    /// Resolved framework + data layout.
    pub framework: Layout,
    /// Settings (codex model + reasoning effort + timeout + parallel cap).
    pub settings: Settings,
    /// Concurrency cap. `None` defaults to "one task per target".
    pub parallel: Option<usize>,
    /// Base branch for the per-target worktrees (`feat/stacks-bench` in
    /// the bash script).
    pub base_branch: String,
    /// Agent harness, shared across spawned tasks via `Arc`.
    pub harness: Arc<H>,
    /// Git worktree manager.
    pub git: Arc<G>,
    /// When true, targets that already carry a valid
    /// `optimize/<id>/optimizer-report.json` (parses, validates,
    /// `outcome ∈ {implemented, aborted}`, target/session/delivery
    /// metadata matches) are skipped; only targets with missing,
    /// corrupt, or context-mismatched reports get re-run. Used to
    /// recover a partially-failed optimizer phase without redoing the
    /// targets that already succeeded.
    pub resume: bool,
}

/// Outputs of an optimizer fan-out.
#[derive(Debug, Default)]
pub struct Outputs {
    /// Total targets considered.
    pub total: usize,
    /// `optimize/<id>/optimizer-report.json` has `outcome=implemented`.
    pub landed: usize,
    /// `optimize/<id>/optimizer-report.json` has `outcome=aborted`,
    /// is missing, or failed to parse/validate (all treated as abort
    /// for tally purposes).
    pub aborted: usize,
    /// `optimize/<id>/consensus-issue.md` exists post-run
    /// (coordinator-written marker; optimizer skipped for this mode).
    pub routed_to_issue: usize,
}

/// Run the optimizer fan-out. Mirrors `scripts/run-optimizers.sh`.
pub async fn run<H, G>(inputs: Inputs<H, G>) -> Result<Outputs>
where
    H: AgentHarness + 'static,
    G: GitCheckoutManager + 'static,
{
    let targets = loader::read_optimization_targets(&inputs.layout)
        .context("loading optimization-targets.json")?;
    if targets.targets.is_empty() {
        return Ok(Outputs::default());
    }
    let total = targets.targets.len();
    let requested = inputs
        .parallel
        .unwrap_or(total)
        .max(1);
    // Layer 1B v2 pass-b.1: the agent no longer runs in-loop benches
    // (bench moves to the coordinator in pass-b.2). The parallelism
    // clamp historically guarded against parallel agents racing on the
    // shared `stacks-bench` SQLite db; that race no longer exists
    // since agents don't bench. We keep `--parallel-agents` configurable
    // for forward compatibility but the existing CLI defaults to 1 for
    // normal_pr targets, which we preserve here. Future pass-b.2
    // re-evaluates: coordinator-side bench can use a per-target run
    // ID so parallel agents don't conflict.
    let has_normal_pr = targets
        .targets
        .iter()
        .any(|t| t.delivery_mode == DeliveryMode::NormalPr);
    let parallel = if has_normal_pr && requested > 1 {
        tracing::warn!(
            requested,
            "clamping --parallel-agents to 1 (Layer 1B v2 pass-b.1 default; pass-b.2 reconsiders \
             once coordinator-side bench can demultiplex per-target run ids)"
        );
        1
    } else {
        requested
    };
    let semaphore = Arc::new(Semaphore::new(parallel));

    // Layer 1B v2 pass-b.1: the multi-attempt inner loop has been
    // disabled. Each codex invocation produces exactly one candidate
    // change; the coordinator commits if kept. Multi-attempt
    // orchestration (with coordinator-driven keep/discard between
    // attempts) returns in pass-b.2 along with coordinator-side bench.
    // We keep `optimizer_attempts` on Settings + CLI for surface
    // stability, but warn if an operator set it > 1.
    if let Some(n) = inputs
        .settings
        .optimizer_attempts
        && n > 1
    {
        tracing::warn!(
            requested = n,
            "optimizer_attempts > 1 is ignored in Layer 1B v2 pass-b.1 (effectively clamped to \
             1); coordinator-driven multi-attempt orchestration returns in pass-b.2"
        );
    }

    let worktrees_root = inputs
        .framework
        .session_optimizer_checkouts_dir(&inputs.layout.id);
    std::fs::create_dir_all(&worktrees_root)
        .with_context(|| format!("creating {}", worktrees_root.display()))?;

    let mut set: JoinSet<Result<()>> = JoinSet::new();
    for target in &targets.targets {
        let task = OptimizerTaskInputs {
            target: target.clone(),
            target_json: target
                .to_json()
                .context("serializing single-target slice for the optimizer prompt")?,
            framework: inputs.framework.clone(),
            settings: inputs.settings.clone(),
            session_id: inputs
                .layout
                .id
                .as_str()
                .to_owned(),
            session_results_dir: inputs
                .layout
                .results_dir
                .clone(),
            worktrees_root: worktrees_root.clone(),
            base_branch: inputs.base_branch.clone(),
            sem: semaphore.clone(),
            harness: inputs.harness.clone(),
            git: inputs.git.clone(),
            resume: inputs.resume,
        };
        set.spawn(run_one(task));
    }

    let mut errors: Vec<anyhow::Error> = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(anyhow::Error::new(e).context("optimizer task panicked")),
        }
    }
    if !errors.is_empty() {
        let mut iter = errors.into_iter();
        let first = iter.next().unwrap();
        for e in iter {
            tracing::error!(?e, "additional optimizer task error");
        }
        return Err(first);
    }

    // Tally per-target outcomes. consensus_issue is the only branch
    // still gated on its marker file (coordinator-written, no agent
    // report); everything else reads the typed `optimizer-report.json`.
    let mut outputs = Outputs { total, ..Default::default() };
    for t in &targets.targets {
        let dir = inputs
            .layout
            .experiment_dir(&t.id);
        if dir
            .join("consensus-issue.md")
            .is_file()
        {
            outputs.routed_to_issue += 1;
            continue;
        }
        match read_optimizer_report(&dir) {
            Ok(Some(OptimizerReport::Implemented(_))) => outputs.landed += 1,
            Ok(Some(OptimizerReport::Aborted(_))) => outputs.aborted += 1,
            Ok(None) => {
                // Agent crashed before writing a report; treat as aborted
                // for tallying so the operator sees the failure surface.
                outputs.aborted += 1;
            }
            Err(e) => {
                tracing::warn!(
                    target = "session.optimizers",
                    target_id = %t.id,
                    error = %e,
                    "tally: optimizer-report.json failed to parse/validate; counting as aborted",
                );
                outputs.aborted += 1;
            }
        }
    }
    Ok(outputs)
}

struct OptimizerTaskInputs<H: AgentHarness + 'static, G: GitCheckoutManager + 'static> {
    target: MergedTarget,
    target_json: String,
    framework: Layout,
    settings: Settings,
    session_id: String,
    session_results_dir: PathBuf,
    worktrees_root: PathBuf,
    base_branch: String,
    sem: Arc<Semaphore>,
    harness: Arc<H>,
    git: Arc<G>,
    resume: bool,
}

async fn run_one<H, G>(state: OptimizerTaskInputs<H, G>) -> Result<()>
where
    H: AgentHarness + 'static,
    G: GitCheckoutManager + 'static,
{
    let _permit = state
        .sem
        .acquire_owned()
        .await
        .context("acquiring optimizer semaphore permit")?;

    let target = &state.target;
    let exp_dir = state
        .session_results_dir
        .join("optimize")
        .join(&target.id);
    std::fs::create_dir_all(&exp_dir).with_context(|| format!("creating {}", exp_dir.display()))?;

    // consensus_issue branch: skip the optimizer entirely; drop the marker.
    if target.delivery_mode == DeliveryMode::ConsensusIssue {
        clear_optimizer_artifacts(&exp_dir)?;
        std::fs::write(exp_dir.join("consensus-issue.md"), CONSENSUS_ISSUE_MARKER)?;
        return Ok(());
    }

    // Resume mode: skip targets whose typed report already parses,
    // validates, matches this session's context, AND (for implemented
    // outcomes) whose coordinator-provenance sidecar shows the per-
    // target branch was built on top of the session's archived
    // baseline source SHA. Anything else falls through to the normal
    // re-run path below.
    if state.resume {
        let baseline_source_sha = read_baseline_source_sha(&state.session_results_dir)
            .with_context(|| {
                "resume mode requires baseline/bin/manifest.json to resolve; rerun without \
                 --resume or invoke Phase 0a first"
            })?;
        if resume_target_is_complete(&exp_dir, target, &state.session_id, &baseline_source_sha) {
            tracing::info!(
                target = "session.optimizers",
                target_id = %target.id,
                "resume: skipping target with valid typed report + matching provenance",
            );
            return Ok(());
        }
    }

    // Idempotency: tear down worktree + clear stale optimizer artifacts.
    let worktree = state
        .worktrees_root
        .join(&target.id);
    // Session-scope the branch name so two `session run` invocations
    // against the same `base` checkout don't collide on `agent/<id>`
    // when their target ids overlap. Mirrors the Phase 5 push branch
    // shape (`agentic/<session>/<target>`).
    state.git.recreate_checkout(
        state
            .framework
            .require_base()?,
        &worktree,
        &format!("agent/{}/{}", state.session_id, target.id),
        &state.base_branch,
    )?;
    // Snapshot the worktree's initial HEAD now (= base_branch tip at
    // creation time). Used post-invoke to verify the agent actually
    // committed when it claimed `implementation.md` — without this, a
    // sandbox/signing failure that blocked `git commit` silently would
    // produce a false-positive "landed" with an empty PR.
    let baseline_head = git_rev_parse_head(&worktree);
    clear_optimizer_artifacts(&exp_dir)?;
    let _ = std::fs::remove_file(exp_dir.join("consensus-issue.md"));

    // Build POC_TEST_SCOPE_EXPR for consensus_poc_pr targets.
    let poc_test_scope_expr = match target.delivery_mode {
        DeliveryMode::ConsensusPocPr => target
            .poc_test_scope
            .as_ref()
            .map(|s| s.join(" | "))
            .unwrap_or_default(),
        _ => String::new(),
    };

    // NOTE: optimizer.md inlines `target_json` (which carries the id
    // among everything else) rather than passing target_id on its own.
    let prompts_dir = state
        .settings
        .require_prompt_overrides_dir()?;
    let missing = crate::context::required_missing_for_phase(
        &state.framework.context_dir,
        crate::context::Phase::Optimizer,
    )?;
    if !missing.is_empty() {
        let summary = missing
            .iter()
            .map(|(id, p)| format!("  - `{id}` → expected at {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "required context docs missing or empty for the optimizer phase:\n{summary}\n\nRun \
             `sbagent sync` to restore from the binary's bundled defaults.",
        );
    }
    let ctx_paths = crate::context::paths_for_phase(
        &state.framework.context_dir,
        crate::context::Phase::Optimizer,
    )?;
    let rendered = prompts::render(
        "optimizer",
        &prompts::OptimizerPrompt {
            worktree_dir: worktree
                .to_string_lossy()
                .into_owned(),
            output_dir: exp_dir
                .to_string_lossy()
                .into_owned(),
            target_json: state.target_json.clone(),
            non_targets_path: crate::context::ctx_path(&ctx_paths, "non-targets")?,
            domain_context_path: crate::context::ctx_path(&ctx_paths, "stacks-domain-context")?,
            optimization_targets_schema_path: state
                .framework
                .schemas_dir
                .join("optimization-targets.schema.json")
                .to_string_lossy()
                .into_owned(),
            optimizer_report_schema_path: state
                .framework
                .schemas_dir
                .join("optimizer-report.schema.json")
                .to_string_lossy()
                .into_owned(),
            delivery_mode: match target.delivery_mode {
                DeliveryMode::NormalPr => "normal_pr",
                DeliveryMode::ConsensusPocPr => "consensus_poc_pr",
                DeliveryMode::ConsensusIssue => "consensus_issue",
            }
            .to_owned(),
            poc_test_scope_expr,
            stacks_bench_data_dir: state
                .framework
                .stacks_bench_data_dir
                .to_string_lossy()
                .into_owned(),
            bench_source_dir: state
                .settings
                .source_dir
                .as_deref()
                .map(|p| {
                    p.to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default(),
            // Default matches Phase 0 (baseline) and Phase 3 (bench_experiments)
            // — see `cli/session/run.rs` / `cli/session/baseline/run.rs`. The
            // prompt also wraps `--network` in a Jinja conditional so an empty
            // value omits the flag entirely (which is better than `--network ""`).
            bench_network: state
                .settings
                .stacks_bench_network
                .clone()
                .unwrap_or_else(|| "mainnet".to_owned()),
            bench_warmup: state
                .settings
                .stacks_bench_warmup
                .map(|w| w.to_string())
                .unwrap_or_default(),
            bench_filter: state
                .settings
                .stacks_bench_filter
                .clone()
                .unwrap_or_default(),
            bench_shadow_dir_root: state
                .framework
                .stacks_bench_shadow_dir
                .as_deref()
                .map(|p| {
                    p.to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default(),
            optimizer_attempts: state
                .settings
                .optimizer_attempts
                .unwrap_or(5)
                .to_string(),
            optimizer_budget_minutes: state
                .settings
                .optimizer_budget_minutes
                .unwrap_or(60)
                .to_string(),
        },
        prompts_dir,
    )?;
    std::fs::write(exp_dir.join("prompt.md"), &rendered)?;

    let timeout = state
        .settings
        .codex_exec_timeout_sec
        .filter(|n| *n > 0)
        .map(Duration::from_secs);
    let model = state
        .settings
        .codex_model
        .as_deref()
        .unwrap_or("gpt-5.5");
    let reasoning_effort = state
        .settings
        .codex_reasoning_effort
        .as_deref();
    let dangerous = state
        .settings
        .codex_dangerously_bypass_sandbox
        .unwrap_or(false);
    // Codex `--add-dir` paths: every dir the agent might need to write
    // to OUTSIDE its cwd (the per-target clone). The codex sandbox
    // defaults to `workspace-write`, which allows writes inside `--cd`
    // + each `--add-dir`. The clone's own `.git/` lives inside cwd so
    // git operations don't need additional paths (this is exactly why
    // we use clones rather than linked worktrees — see
    // `GitCheckoutManager` docs).
    //
    //   1. Operator's context dir — the agent reads non-targets.md and
    //      stacks-domain-context.md (referenced by absolute path in the rendered
    //      prompt).
    //   2. Operator's schemas dir — the agent reads
    //      optimization-targets.schema.json for output validation.
    //   3. Operator's prompts dir — kept for forward-compat with any operator-tuned
    //      template that references additional files under it.
    //   4. Experiment dir — `nextest.log`, `implementation.md` / `abort.md`,
    //      `side-observations.md` all land here.
    //
    // **Not added in b.1**: `stacks_bench_shadow_dir` — the optimizer
    // prompt explicitly forbids running `stacks-bench` inside codex
    // (bench moved to coordinator in pass-b). Keeping it in `--add-dir`
    // would expand the sandbox blast radius (typically `/Volumes/Extern`)
    // for no reason and would also fail outright if the configured
    // shadow dir isn't mounted. Pass-b.2 surfaces shadow-dir-root
    // coordinator-side only.
    let mut add_dirs: Vec<PathBuf> = vec![
        state
            .framework
            .context_dir
            .clone(),
        state
            .framework
            .schemas_dir
            .clone(),
        prompts_dir.to_path_buf(),
        exp_dir.clone(),
    ];
    add_dirs.extend(
        state
            .settings
            .codex_extra_writable_roots
            .iter()
            .cloned(),
    );

    let git_env = optimizer_git_env(&state.settings);
    let invoke_outputs = state
        .harness
        .invoke(&InvokeInputs {
            rendered_prompt: &rendered,
            cwd: worktree.as_ref(),
            add_dirs: &add_dirs,
            events_jsonl: &exp_dir.join("events.jsonl"),
            stderr_log: &exp_dir.join("stderr.log"),
            last_message: &exp_dir.join("final-message.md"),
            timeout,
            model,
            reasoning_effort,
            // cwd IS a real git worktree, so don't skip the check.
            skip_git_repo_check: false,
            dangerously_bypass_sandbox: dangerous,
            enable_web_search: false,
            extra_env: &git_env,
        })
        .await
        .with_context(|| format!("invoking codex for optimizer {}", target.id))?;

    if let Some(id) = &invoke_outputs.conversation_id {
        std::fs::write(exp_dir.join("conversation-id"), format!("{id}\n"))?;
    }

    // Layer 1B v2 pass-b.1: the agent no longer runs `git commit` —
    // the codex sandbox blocks writes to `.git/` even when the clone's
    // `.git/` is inside its own cwd (macOS Seatbelt special-cases the
    // directory). So when the agent declares "kept" by writing
    // `implementation.md`, the COORDINATOR runs the commit here from
    // outside the sandbox, using the same `optimizer_git_env` env-var
    // overrides (bot identity + signing off).
    //
    // Strict verification contract before committing:
    //   1. `optimizer-report.json` parses + validates AND `outcome=implemented`.
    //   2. `git status --porcelain` reports actual changes (catches "agent reported
    //      implemented but did nothing").
    //   3. The commit itself succeeds.
    //   4. HEAD advances past `baseline_head` (defense in depth —
    //      `verify_kept_or_demote` re-checks this after we return).
    //
    // Any failure in (1)-(3) demotes the typed report from
    // `outcome=implemented` to `outcome=aborted` (preserving the original
    // at `optimizer-report.json.demoted`) so downstream phases correctly
    // skip this target.
    coordinator_commit_if_kept(
        &exp_dir,
        &worktree,
        &target.id,
        &state.session_id,
        target.delivery_mode,
        &state.settings,
    )?;

    // Post-invoke correctness gate: if `outcome=implemented` survived
    // the coordinator commit step, verify HEAD has actually advanced.
    // Defense-in-depth — `coordinator_commit_if_kept` already demotes on
    // most failure modes — but catches any path that produced a kept
    // report without a corresponding HEAD advance.
    verify_kept_or_demote(
        &exp_dir,
        &worktree,
        baseline_head.as_deref(),
        &target.id,
        &state.session_id,
        target.delivery_mode,
    )?;

    Ok(())
}

/// Coordinator-side commit step (Layer 1B v2 pass-b.1). Runs after the
/// agent exits and before [`verify_kept_or_demote`]. Enforces the
/// verification contract from the b.1 review:
///
///   1. `implementation.md` present AND `abort.md` absent.
///   2. `git status --porcelain` shows non-empty output (real changes).
///   3. `git add -A && git commit -m <msg>` succeeds.
///
/// On any contract violation, demotes to `abort.md` (preserving the
/// agent's `implementation.md` writeup as `implementation.md.demoted`
/// for diagnosis). Uses the same env-var identity overrides
/// [`optimizer_git_env`] applied to the codex invocation, so the
/// coordinator's commit is authored as the bot and skips signing.
///
/// **Why this lives in the coordinator, not the agent**: codex's
/// `workspace-write` sandbox on macOS denies writes to `.git/` even
/// when the clone's `.git/` directory is inside the agent's cwd —
/// observed via Seatbelt + the `com.apple.provenance` xattr. The
/// agent CAN edit source files; it CANNOT commit them. So we split
/// responsibilities: agent owns edits + correctness gates (fmt /
/// clippy / nextest); coordinator owns trusted host operations
/// (commit, future: bench).
fn coordinator_commit_if_kept(
    exp_dir: &Path,
    checkout: &Path,
    target_id: &str,
    session_id: &str,
    delivery_mode: DeliveryMode,
    settings: &Settings,
) -> Result<()> {
    // Contract step 1: read + schema-validate the typed optimizer
    // report, then cross-check its context (target_id, session_id,
    // delivery_mode) against what the merged target says. Without the
    // context check, a misbehaving agent could claim a different
    // delivery_mode and bypass mode-specific invariants.
    let report = match read_optimizer_report(exp_dir)? {
        Some(r) => r,
        None => {
            // Agent never wrote a report (sandbox kill, panic, etc.).
            // Mirrors the prior "no marker = no commit" path — downstream
            // phases treat absence as "experiment crashed."
            return Ok(());
        }
    };
    validate_report_context(&report, target_id, session_id, delivery_mode)?;
    let implemented = match report {
        OptimizerReport::Implemented(r) => r,
        OptimizerReport::Aborted(r) => {
            // Agent declared abort. Render the companion abort.md for the
            // operator's audit trail and leave the worktree alone.
            std::fs::write(exp_dir.join("abort.md"), render_abort_md(&r)).with_context(|| {
                format!("writing companion abort.md for {target_id} at {}", exp_dir.display())
            })?;
            return Ok(());
        }
    };

    // Render the companion implementation.md eagerly — even if commit
    // fails below and we demote, the original implementation report is
    // preserved as `optimizer-report.json.demoted` and the abort.md
    // overwrites this implementation.md. This way operators inspecting
    // mid-run see something readable.
    std::fs::write(exp_dir.join("implementation.md"), render_implementation_md(&implemented))
        .with_context(|| {
            format!("writing companion implementation.md for {target_id} at {}", exp_dir.display())
        })?;

    // Contract step 2: tree must actually have changes. Empty
    // porcelain output means "clean tree."
    let porcelain = match crate::git::status_porcelain(checkout) {
        Ok(p) => p,
        Err(e) => {
            demote_implemented_to_aborted(
                exp_dir,
                target_id,
                session_id,
                delivery_mode,
                &format!("{e:#}"),
                FailedGate::EnvironmentalError,
            )?;
            return Ok(());
        }
    };
    if porcelain.is_empty() {
        demote_implemented_to_aborted(
            exp_dir,
            target_id,
            session_id,
            delivery_mode,
            "agent reported `outcome=implemented` but the worktree has no changes (`git status \
             --porcelain` empty); coordinator has nothing to commit",
            FailedGate::EnvironmentalError,
        )?;
        return Ok(());
    }

    // Contract step 3: commit, with env-var identity overrides.
    let env = optimizer_git_env(settings);
    if let Err(e) = crate::git::add_all_with_env(checkout, &env) {
        demote_implemented_to_aborted(
            exp_dir,
            target_id,
            session_id,
            delivery_mode,
            &format!("coordinator `git add -A` failed: {e:#}"),
            FailedGate::EnvironmentalError,
        )?;
        return Ok(());
    }
    let msg = format!("perf: optimize {target_id}");
    if let Err(e) = crate::git::commit_with_message_and_env(checkout, &msg, &env) {
        demote_implemented_to_aborted(
            exp_dir,
            target_id,
            session_id,
            delivery_mode,
            &format!(
                "coordinator `git commit -m {msg:?}` failed: {e:#}; check operator's git config / \
                 signing setup",
            ),
            FailedGate::EnvironmentalError,
        )?;
        return Ok(());
    }

    // Contract step 4: write the coordinator-provenance sidecar
    // (base + head SHA). Failure demotes — a kept-but-unprovenanced
    // experiment can't be verified by the resume gate or audit trail.
    if let Err(e) =
        write_coordinator_provenance(exp_dir, checkout, target_id, session_id, delivery_mode, &msg)
    {
        demote_implemented_to_aborted(
            exp_dir,
            target_id,
            session_id,
            delivery_mode,
            &format!("coordinator could not record provenance after commit: {e:#}"),
            FailedGate::EnvironmentalError,
        )?;
        return Ok(());
    }

    tracing::info!(
        target = "session.optimizers",
        target_id = %target_id,
        checkout = %checkout.display(),
        "coordinator committed agent changes",
    );
    Ok(())
}

/// Write `optimize/<target>/coordinator-provenance.json` capturing
/// the SHAs the coordinator observed in the per-target clone right
/// after the agent's commit landed. See
/// [`crate::models::coordinator_provenance`] for the artifact's role.
///
/// Read order is `rev-parse HEAD` then `rev-parse HEAD^`: HEAD is the
/// agent's new commit; HEAD^ is the base we want to confirm matches
/// the session's archived `baseline/bin/manifest.json.source_sha`.
fn write_coordinator_provenance(
    exp_dir: &Path,
    checkout: &Path,
    target_id: &str,
    session_id: &str,
    delivery_mode: DeliveryMode,
    commit_message: &str,
) -> Result<()> {
    use crate::models::common::SchemaVersionV1;
    use crate::models::coordinator_provenance::CoordinatorProvenance;

    let head_sha = crate::git::rev_parse_head(checkout)
        .with_context(|| format!("post-commit `git rev-parse HEAD` in {}", checkout.display()))?;
    let base_sha = crate::git::rev_parse_head_parent(checkout)
        .with_context(|| format!("post-commit `git rev-parse HEAD^` in {}", checkout.display()))?;
    let provenance = CoordinatorProvenance {
        schema_version: SchemaVersionV1,
        session_id: session_id.to_owned(),
        target_id: target_id.to_owned(),
        delivery_mode,
        base_sha,
        head_sha,
        commit_message: commit_message.to_owned(),
    };
    provenance
        .validate_model()
        .context("coordinator-provenance.json validation")?;
    let json = provenance
        .to_json_pretty()
        .context("serializing coordinator-provenance.json")?;
    let path = exp_dir.join("coordinator-provenance.json");
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Read + validate the typed optimizer report for the target whose
/// `optimize/<target-id>/` dir is `exp_dir`. Operates on a raw path
/// (not a `SessionLayout`) so callers iterating the experiments
/// directory (prune, tally) don't have to look up each target in
/// `optimization-targets.json` just to read the report.
///
/// Does NOT cross-check the report's `target_id`/`session_id`/
/// `delivery_mode` against any expected context — callers that grant
/// privileges based on the report (commit, demote, publish, finalize)
/// MUST additionally call [`validate_report_context`] before acting on
/// it. Pure observers (prune, tally) can skip the context check; the
/// worst an agent can do there is mis-bucket its own outcome.
///
/// Returns `Ok(None)` when the report is absent — agent never wrote it.
/// Resume gate — true iff this target's optimizer outputs are
/// terminal and trustworthy:
///
/// - `optimize/<id>/optimizer-report.json` exists, parses, validates, and its
///   `target_id` / `session_id` / `delivery_mode` match the expected context.
/// - For `outcome=implemented`: `coordinator-provenance.json` ALSO exists,
///   parses, validates, and its `base_sha` matches the session's archived
///   `baseline/bin/manifest.json.source_sha` (passed in via
///   `baseline_source_sha`). Catches the case where the per-target branch was
///   built against a different base than Phase 0a archived — e.g. a mid-session
///   submodule bump that didn't propagate to every clone.
/// - For `outcome=aborted`: no sidecar required; aborted targets produced no
///   commit, so there's nothing to provenance. The agent report's context match
///   is sufficient.
///
/// Any failure mode (missing, corrupt, context mismatch, base SHA
/// mismatch) falls back to "incomplete" so the caller re-runs. All
/// re-run reasons log at WARN so operators can distinguish "agent
/// never ran" from "stale provenance" when investigating skips.
fn resume_target_is_complete(
    exp_dir: &Path,
    target: &MergedTarget,
    session_id: &str,
    baseline_source_sha: &str,
) -> bool {
    let report = match read_optimizer_report(exp_dir) {
        Ok(Some(r)) => r,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(
                target = "session.optimizers",
                target_id = %target.id,
                error = %e,
                "resume: optimizer-report.json failed to parse/validate; re-running",
            );
            return false;
        }
    };
    if let Err(e) = validate_report_context(&report, &target.id, session_id, target.delivery_mode) {
        tracing::warn!(
            target = "session.optimizers",
            target_id = %target.id,
            error = %e,
            "resume: optimizer-report.json context mismatch; re-running",
        );
        return false;
    }
    // Aborted reports don't produce a commit → no provenance sidecar
    // to check. Implemented reports REQUIRE a matching sidecar.
    if matches!(&report, OptimizerReport::Aborted(_)) {
        return true;
    }
    match read_coordinator_provenance(exp_dir) {
        Ok(Some(p)) => {
            if let Err(e) = p.validate_context(session_id, &target.id, target.delivery_mode) {
                tracing::warn!(
                    target = "session.optimizers",
                    target_id = %target.id,
                    error = %e,
                    "resume: coordinator-provenance.json context mismatch (sidecar from a \
                     different target/session?); re-running",
                );
                return false;
            }
            if p.base_sha != baseline_source_sha {
                tracing::warn!(
                    target = "session.optimizers",
                    target_id = %target.id,
                    provenance_base_sha = %p.base_sha,
                    baseline_source_sha = %baseline_source_sha,
                    "resume: coordinator-provenance.json base_sha != archived baseline source_sha; \
                     per-target branch was built against a different base — re-running",
                );
                false
            } else {
                true
            }
        }
        Ok(None) => {
            tracing::warn!(
                target = "session.optimizers",
                target_id = %target.id,
                "resume: optimizer-report.json says outcome=implemented but \
                 coordinator-provenance.json is missing — re-running",
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                target = "session.optimizers",
                target_id = %target.id,
                error = %e,
                "resume: coordinator-provenance.json failed to parse/validate; re-running",
            );
            false
        }
    }
}

/// Read `source_sha` out of the session's archived baseline
/// manifest. Resume gate compares this against each per-target
/// provenance sidecar's `base_sha` to enforce apples-to-apples.
fn read_baseline_source_sha(session_results_dir: &Path) -> Result<String> {
    let path = session_results_dir.join("baseline/bin/manifest.json");
    let manifest = crate::models::baseline_binary_manifest::BaselineBinaryManifest::read(&path)?;
    Ok(manifest.source_sha)
}

/// Read + validate the coordinator-provenance sidecar for the target
/// whose `optimize/<target-id>/` dir is `exp_dir`. Companion to
/// [`read_optimizer_report`] for the audit-trail surface.
///
/// Returns `Ok(None)` when the sidecar is absent (expected for
/// aborted experiments or for sessions that predate the sidecar),
/// `Ok(Some(_))` when it parses + validates, and an error on parse
/// or validation failure — same soft-fallback contract as the
/// optimizer report itself.
fn read_coordinator_provenance(
    exp_dir: &Path,
) -> Result<Option<crate::models::coordinator_provenance::CoordinatorProvenance>> {
    let path = exp_dir.join("coordinator-provenance.json");
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
        }
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let p = crate::models::coordinator_provenance::CoordinatorProvenance::from_json_validated(&raw)
        .with_context(|| format!("parsing/validating {}", path.display()))?;
    Ok(Some(p))
}

fn read_optimizer_report(exp_dir: &Path) -> Result<Option<OptimizerReport>> {
    let path = exp_dir.join("optimizer-report.json");
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("reading {}", path.display())));
        }
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let report = OptimizerReport::from_json_validated(&raw)
        .with_context(|| format!("parsing/validating {}", path.display()))?;
    Ok(Some(report))
}

/// Cross-check the loaded report's `target_id`, `session_id`, and
/// `delivery_mode` against the expected context. Without this check, a
/// misbehaving agent could emit `delivery_mode: consensus_poc_pr` for a
/// `normal_pr` target — bypassing the `clippy_clean: true` invariant
/// since the typed model's `validate()` only enforces `clippy_clean`
/// for `normal_pr`. Mirrors
/// [`crate::session::loader::read_optimizer_report_for_target`] for
/// callers that hold a raw `exp_dir` (and have already loaded the
/// report) instead of a `SessionLayout`.
fn validate_report_context(
    report: &OptimizerReport,
    expected_target_id: &str,
    expected_session_id: &str,
    expected_delivery_mode: DeliveryMode,
) -> Result<()> {
    let (tid, sid, mode) = match report {
        OptimizerReport::Implemented(r) => {
            (r.target_id.as_str(), r.session_id.as_str(), r.delivery_mode)
        }
        OptimizerReport::Aborted(r) => {
            (r.target_id.as_str(), r.session_id.as_str(), r.delivery_mode)
        }
    };
    if tid != expected_target_id {
        anyhow::bail!(
            "optimizer-report.json for {expected_target_id}: report.target_id={tid:?} does not \
             match expected target_id={expected_target_id:?}",
        );
    }
    if sid != expected_session_id {
        anyhow::bail!(
            "optimizer-report.json for {expected_target_id}: report.session_id={sid:?} does not \
             match expected session_id={expected_session_id:?}",
        );
    }
    if mode != expected_delivery_mode {
        anyhow::bail!(
            "optimizer-report.json for {expected_target_id}: report.delivery_mode={mode:?} does \
             not match expected delivery_mode={expected_delivery_mode:?} (an agent claiming a \
             different mode could bypass mode-specific invariants like clippy_clean)",
        );
    }
    Ok(())
}

/// Rewrite the per-target `optimizer-report.json` with an aborted body
/// carrying `reason` + `failed_gate`, preserving the original report at
/// `optimizer-report.json.demoted` for diagnosis. Re-renders companion
/// markdown: removes the now-stale `implementation.md` and writes the
/// abort.md derived from the new typed body.
///
/// Used by [`coordinator_commit_if_kept`] and [`verify_kept_or_demote`]
/// whenever the agent claimed `outcome=implemented` but the post-hoc
/// verification (dirty tree, git commit success, HEAD advance) fails.
fn demote_implemented_to_aborted(
    exp_dir: &Path,
    target_id: &str,
    session_id: &str,
    delivery_mode: DeliveryMode,
    reason: &str,
    failed_gate: FailedGate,
) -> Result<()> {
    let report_path = exp_dir.join("optimizer-report.json");
    let preserved = exp_dir.join("optimizer-report.json.demoted");
    let _ = std::fs::rename(&report_path, &preserved);

    let new_report = AbortedReport {
        schema_version: SchemaVersionV2,
        session_id: session_id.to_owned(),
        target_id: target_id.to_owned(),
        outcome: AbortedOutcomeTag::Aborted,
        delivery_mode,
        reason: format!(
            "Coordinator demoted `outcome=implemented` → `outcome=aborted`: {reason}\n\nOriginal \
             report preserved at `optimizer-report.json.demoted`."
        ),
        failed_gate: Some(failed_gate),
        failing_tests: None,
    };
    let json = new_report
        .to_json_pretty()
        .context("serializing demoted aborted report")?;
    std::fs::write(&report_path, json + "\n").with_context(|| {
        format!("writing demoted optimizer-report.json at {}", report_path.display())
    })?;
    // Stale companion: agent's implementation.md no longer reflects truth.
    let _ = std::fs::remove_file(exp_dir.join("implementation.md"));
    std::fs::write(exp_dir.join("abort.md"), render_abort_md(&new_report)).with_context(|| {
        format!("writing demotion abort.md for {target_id} at {}", exp_dir.display())
    })?;
    tracing::warn!(
        target = "session.optimizers",
        target_id = %target_id,
        reason = %reason,
        "demoted optimizer-report.json (implemented → aborted; coordinator gate failed)",
    );
    Ok(())
}

/// Coordinator-rendered companion view of an `implemented` report.
/// Dense header + fenced JSON dump. The JSON is authoritative; the
/// markdown is operator sugar.
fn render_implementation_md(r: &ImplementedReport) -> String {
    let json = r
        .to_json_pretty()
        .expect("ImplementedReport always serializable as JSON");
    format!(
        "# Implementation report — `{target}`\n\n_Coordinator-rendered companion view of \
         `optimizer-report.json`. The JSON is authoritative; this file regenerates from it on \
         every commit/demote pass._\n\n- **Target**: `{target}`\n- **Delivery mode**: \
         `{mode:?}`\n- **PR title**: {title}\n\n```json\n{json}\n```\n",
        target = r.target_id,
        mode = r.delivery_mode,
        title = r.pr_title,
        json = json,
    )
}

/// Coordinator-rendered companion view of an `aborted` report. Dense
/// header + fenced JSON dump.
fn render_abort_md(r: &AbortedReport) -> String {
    let json = r
        .to_json_pretty()
        .expect("AbortedReport always serializable as JSON");
    let gate = r
        .failed_gate
        .map(|g| format!("`{g:?}`"))
        .unwrap_or_else(|| "—".to_owned());
    format!(
        "# Abort report — `{target}`\n\n_Coordinator-rendered companion view of \
         `optimizer-report.json`. The JSON is authoritative; this file regenerates from it on \
         every commit/demote pass._\n\n- **Target**: `{target}`\n- **Delivery mode**: \
         `{mode:?}`\n- **Failed gate**: {gate}\n\n**Reason**: {reason}\n\n```json\n{json}\n```\n",
        target = r.target_id,
        mode = r.delivery_mode,
        gate = gate,
        reason = r.reason,
        json = json,
    )
}

/// Outcome of [`verify_kept_or_demote`]. Exposed (with `pub(super)` would
/// be tighter, but this module is private to `session` already) so tests
/// can assert which branch ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemotionOutcome {
    /// Optimizer report wasn't present or wasn't `outcome=implemented`
    /// — nothing to verify. Covers both "agent crashed" and "agent
    /// reported aborted" cases.
    NoMarker,
    /// `outcome=implemented` present + HEAD advanced past baseline. Kept.
    HeadAdvanced,
    /// `outcome=implemented` present, HEAD == baseline. Demoted to
    /// `outcome=aborted` (original preserved as
    /// `optimizer-report.json.demoted`).
    Demoted,
    /// `outcome=implemented` present, but we couldn't resolve a HEAD
    /// SHA on one side (worktree corrupt / never created / etc.).
    /// Report kept as-is; downstream phases will fail loudly if the
    /// worktree is unusable.
    Indeterminate,
}

/// Post-invoke correctness gate. If the agent emitted
/// `outcome=implemented`, verify the worktree's HEAD has advanced past
/// `baseline_head` (the worktree's initial HEAD, captured right after
/// creation = base_branch tip at that moment). If HEAD hasn't advanced,
/// no commits were made and there's nothing for Phase 3 to bench or
/// Phase 5 to publish — demote to `outcome=aborted` (preserving the
/// original at `optimizer-report.json.demoted`).
///
/// The agent's report alone isn't proof of work: sandbox/signing
/// failures (operator's global `commit.gpgsign=true` with an
/// unreachable token, sandbox deny on `.git/...`, etc.) can block
/// `git commit` while the agent's clippy/nextest gates still pass.
pub fn verify_kept_or_demote(
    exp_dir: &Path,
    worktree: &Path,
    baseline_head: Option<&str>,
    target_id: &str,
    session_id: &str,
    delivery_mode: DeliveryMode,
) -> Result<DemotionOutcome> {
    // Only `outcome=implemented` gates downstream side effects; absence
    // or `outcome=aborted` is the no-op path. Context-check first so an
    // agent claiming a different `delivery_mode` can't sneak past this
    // gate either.
    match read_optimizer_report(exp_dir)? {
        Some(report) => {
            validate_report_context(&report, target_id, session_id, delivery_mode)?;
            if !matches!(report, OptimizerReport::Implemented(_)) {
                return Ok(DemotionOutcome::NoMarker);
            }
        }
        None => return Ok(DemotionOutcome::NoMarker),
    }
    let head_now = git_rev_parse_head(worktree);
    match (baseline_head, head_now.as_deref()) {
        (Some(base), Some(now)) if base == now => {
            let reason = format!(
                "agent reported `outcome=implemented` but the worktree's HEAD is still at the \
                 base branch tip (`{base}`); no commits were made. Most common cause is a `git \
                 commit` that failed silently inside the codex sandbox (signing prompt blocked by \
                 missing socket / YubiKey, or a sandbox write deny on `.git/...`). Inspect \
                 `final-message.md` + `events.jsonl` for the agent's own diagnostics."
            );
            demote_implemented_to_aborted(
                exp_dir,
                target_id,
                session_id,
                delivery_mode,
                &reason,
                FailedGate::EnvironmentalError,
            )?;
            Ok(DemotionOutcome::Demoted)
        }
        (Some(_), Some(_)) => Ok(DemotionOutcome::HeadAdvanced),
        _ => {
            tracing::warn!(
                target = "session.optimizers",
                target_id = %target_id,
                "could not verify HEAD advance (rev-parse failed); trusting \
                 `outcome=implemented` as-is"
            );
            Ok(DemotionOutcome::Indeterminate)
        }
    }
}

/// Best-effort `git -C <worktree> rev-parse HEAD`. Returns `None` on
/// any failure — caller decides whether to treat that as a soft
/// "trust the marker" path or a hard error. Used by the "did the
/// agent actually commit?" gate at the end of `run_one`. Thin
/// re-export of [`crate::git::rev_parse_head_optional`] under the
/// historical name.
pub fn git_rev_parse_head(worktree: &Path) -> Option<String> {
    crate::git::rev_parse_head_optional(worktree)
}

/// Clear per-target optimizer outputs so a retry doesn't leave stale
/// decision markers behind. Mirrors the bash `rm -f ...` block.
fn clear_optimizer_artifacts(exp_dir: &Path) -> Result<()> {
    for name in [
        // Typed contract + its rendered companions.
        "optimizer-report.json",
        "optimizer-report.json.demoted",
        "abort.md",
        "implementation.md",
        // Coordinator-owned provenance sidecar (written post-commit).
        "coordinator-provenance.json",
        // Agent-written side artifacts.
        "side-observations.md",
        "nextest.log",
        "nextest.stderr.log",
        "events.jsonl",
        "stderr.log",
        "final-message.md",
        "conversation-id",
        "prompt.md",
    ] {
        let _ = std::fs::remove_file(exp_dir.join(name));
    }
    Ok(())
}

const CONSENSUS_ISSUE_MARKER: &str =
    "# Consensus issue: optimizer skipped\n\ndelivery_mode = consensus_issue\n\nThis target \
     proposes a consensus-breaking change that the analyzer determined is not\nPoC-implementable \
     (poc_implementable = false). The optimizer phase is intentionally\nskipped: the analyzer's \
     consensus_writeup is the shipping artifact.\n\nDownstream phases:\n- `sbagent session bench` \
     skips this target (bench_eligible=false).\n- `sbagent publish` routes this target to the \
     issue writer.\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FromJson;

    /// `optimizer_git_env` should emit `GIT_AUTHOR_*` / `GIT_COMMITTER_*`
    /// for the bot identity, plus `GIT_CONFIG_COUNT` +
    /// `GIT_CONFIG_KEY_N`/`GIT_CONFIG_VALUE_N` overrides for
    /// `user.name`, `user.email`, `commit.gpgsign=false`, and
    /// `tag.gpgsign=false`. All four config keys must be present so the
    /// operator's global `commit.gpgsign=true` (with a YubiKey-bound
    /// signer) gets shadowed for every `git commit` the agent runs.
    #[test]
    fn optimizer_git_env_shape_with_configured_identity() {
        let settings = Settings {
            git_author_name: Some("bot-name".into()),
            git_author_email: Some("bot@example.test".into()),
            ..Settings::default()
        };
        let env = optimizer_git_env(&settings);
        let map: std::collections::BTreeMap<&str, &str> = env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // Identity env vars cover commit identity directly.
        assert_eq!(
            map.get("GIT_AUTHOR_NAME")
                .copied(),
            Some("bot-name")
        );
        assert_eq!(
            map.get("GIT_AUTHOR_EMAIL")
                .copied(),
            Some("bot@example.test")
        );
        assert_eq!(
            map.get("GIT_COMMITTER_NAME")
                .copied(),
            Some("bot-name")
        );
        assert_eq!(
            map.get("GIT_COMMITTER_EMAIL")
                .copied(),
            Some("bot@example.test")
        );

        // GIT_CONFIG_COUNT must match the number of override pairs.
        assert_eq!(
            map.get("GIT_CONFIG_COUNT")
                .copied(),
            Some("4")
        );

        // Collect the (key, value) overrides indexed by GIT_CONFIG_KEY_N.
        let mut overrides: std::collections::BTreeMap<String, String> = Default::default();
        for i in 0..4 {
            let k = map
                .get(format!("GIT_CONFIG_KEY_{i}").as_str())
                .copied()
                .unwrap_or_else(|| panic!("missing GIT_CONFIG_KEY_{i}"));
            let v = map
                .get(format!("GIT_CONFIG_VALUE_{i}").as_str())
                .copied()
                .unwrap_or_else(|| panic!("missing GIT_CONFIG_VALUE_{i}"));
            overrides.insert(k.to_owned(), v.to_owned());
        }
        assert_eq!(
            overrides
                .get("user.name")
                .map(String::as_str),
            Some("bot-name")
        );
        assert_eq!(
            overrides
                .get("user.email")
                .map(String::as_str),
            Some("bot@example.test")
        );
        assert_eq!(
            overrides
                .get("commit.gpgsign")
                .map(String::as_str),
            Some("false")
        );
        assert_eq!(
            overrides
                .get("tag.gpgsign")
                .map(String::as_str),
            Some("false")
        );
    }

    /// When `git_author_name` / `git_author_email` are unset, the helper
    /// falls back to `stacks-bench-bot` +
    /// `stacks-bench-bot@users.noreply.github.com`. The signing-disable
    /// overrides apply unconditionally.
    #[test]
    fn optimizer_git_env_defaults_when_unset() {
        let env = optimizer_git_env(&Settings::default());
        let map: std::collections::BTreeMap<&str, &str> = env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            map.get("GIT_AUTHOR_NAME")
                .copied(),
            Some("stacks-bench-bot")
        );
        assert_eq!(
            map.get("GIT_AUTHOR_EMAIL")
                .copied(),
            Some("stacks-bench-bot@users.noreply.github.com")
        );
        // Signing overrides still emitted regardless of identity configuration.
        let signing_off = (0..4).any(|i| {
            map.get(format!("GIT_CONFIG_KEY_{i}").as_str())
                .copied()
                == Some("commit.gpgsign")
                && map
                    .get(format!("GIT_CONFIG_VALUE_{i}").as_str())
                    .copied()
                    == Some("false")
        });
        assert!(signing_off, "commit.gpgsign=false override missing in {env:#?}");
    }

    /// Write a minimal valid `optimizer-report.json` with
    /// `outcome=implemented` for `target_id` in `exp_dir`. Used by every
    /// coordinator/demote test to stage the agent's typed output.
    fn write_implemented_report(exp_dir: &Path, target_id: &str) {
        use crate::models::optimizer_report::{
            ImplementedOutcomeTag, ImplementedReport, ParityReport, TestFramework, TestSummary,
        };
        let report = OptimizerReport::Implemented(ImplementedReport {
            schema_version: SchemaVersionV2,
            session_id: "20260517-000000".to_owned(),
            target_id: target_id.to_owned(),
            outcome: ImplementedOutcomeTag::Implemented,
            delivery_mode: DeliveryMode::NormalPr,
            implementation_summary: "test implementation".to_owned(),
            deviation_from_proposed_change: None,
            dependency_changes: None,
            test_summary: TestSummary {
                framework: TestFramework::Nextest,
                passed: 1,
                failed: 0,
                duration_secs: 1.0,
                log_path: "nextest.log".to_owned(),
            },
            clippy_clean: Some(true),
            pr_title: "perf: test".to_owned(),
            parity: ParityReport {
                consensus_sensitive: false,
                evidence: vec![],
                tests: vec![],
                unproven_risk: None,
            },
            hard_fork_followup: None,
        });
        let json = report
            .to_json_pretty()
            .unwrap();
        std::fs::write(exp_dir.join("optimizer-report.json"), json).unwrap();
    }

    /// Real-git: stage a worktree, write an `outcome=implemented`
    /// report WITHOUT committing, and verify the demotion path. HEAD
    /// unchanged should yield `DemotionOutcome::Demoted`, the original
    /// report should be preserved at `optimizer-report.json.demoted`,
    /// and the live report should be rewritten with `outcome=aborted`.
    #[test]
    fn verify_kept_or_demote_demotes_when_head_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("base");
        std::fs::create_dir_all(&base).unwrap();

        // git init + one commit (so HEAD has a SHA we can capture).
        crate::git::init_test_repo(&base).unwrap();
        std::fs::write(base.join("x"), "x").unwrap();
        run_git(&base, &["add", "x"]);
        run_git(&base, &["commit", "-q", "-m", "init"]);

        let baseline_head = git_rev_parse_head(&base).expect("base HEAD");

        // Add a real worktree at a sibling path (so it's not nested
        // inside `base`).
        let worktree = tmp.path().join("wt");
        run_git(&base, &["worktree", "add", "-q", "-b", "agent/t", worktree.to_str().unwrap()]);

        // Simulate the agent writing `outcome=implemented` without committing.
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        write_implemented_report(&exp_dir, "test-target");

        let outcome = verify_kept_or_demote(
            &exp_dir,
            &worktree,
            Some(&baseline_head),
            "test-target",
            "20260517-000000",
            DeliveryMode::NormalPr,
        )
        .expect("verify_kept_or_demote");

        assert_eq!(outcome, DemotionOutcome::Demoted);
        // Demoted report: original preserved, live rewritten as aborted.
        assert!(
            exp_dir
                .join("optimizer-report.json.demoted")
                .is_file(),
            "original implemented report should be preserved at .demoted"
        );
        let live = std::fs::read_to_string(exp_dir.join("optimizer-report.json")).unwrap();
        let parsed = OptimizerReport::from_json(&live).unwrap();
        assert!(matches!(parsed, OptimizerReport::Aborted(_)), "live report must be aborted");
        // Companion abort.md rendered; stale implementation.md removed.
        let abort = std::fs::read_to_string(exp_dir.join("abort.md")).unwrap();
        assert!(abort.contains("test-target"), "abort.md missing target id: {abort}");
        assert!(abort.contains(&baseline_head), "abort.md missing baseline SHA: {abort}");
        assert!(
            !exp_dir
                .join("implementation.md")
                .exists(),
            "stale implementation.md should have been removed during demotion"
        );
    }

    /// Conversely: if the agent DID commit, HEAD advances and the
    /// `implementation.md` marker stays in place.
    #[test]
    fn verify_kept_or_demote_preserves_when_head_advanced() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("base");
        std::fs::create_dir_all(&base).unwrap();
        crate::git::init_test_repo(&base).unwrap();
        std::fs::write(base.join("x"), "x").unwrap();
        run_git(&base, &["add", "x"]);
        run_git(&base, &["commit", "-q", "-m", "init"]);

        let baseline_head = git_rev_parse_head(&base).expect("base HEAD");

        let worktree = tmp.path().join("wt");
        run_git(&base, &["worktree", "add", "-q", "-b", "agent/t", worktree.to_str().unwrap()]);

        // Simulate the agent committing something. Worktree inherits
        // base's user.name/email/commit.gpgsign=false via the shared
        // config, so this commit succeeds without signing.
        std::fs::write(worktree.join("y"), "y").unwrap();
        run_git(&worktree, &["add", "y"]);
        run_git(&worktree, &["commit", "-q", "-m", "attempt 1"]);

        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        write_implemented_report(&exp_dir, "test-target");

        let outcome = verify_kept_or_demote(
            &exp_dir,
            &worktree,
            Some(&baseline_head),
            "test-target",
            "20260517-000000",
            DeliveryMode::NormalPr,
        )
        .expect("verify_kept_or_demote");

        assert_eq!(outcome, DemotionOutcome::HeadAdvanced);
        // Report still implemented; no demoted copy, no abort.md.
        let live = std::fs::read_to_string(exp_dir.join("optimizer-report.json")).unwrap();
        let parsed = OptimizerReport::from_json(&live).unwrap();
        assert!(matches!(parsed, OptimizerReport::Implemented(_)));
        assert!(
            !exp_dir
                .join("optimizer-report.json.demoted")
                .exists()
        );
        assert!(
            !exp_dir
                .join("abort.md")
                .exists()
        );
    }

    /// When the optimizer report is absent (e.g. agent crashed), the
    /// gate is a no-op.
    #[test]
    fn verify_kept_or_demote_noop_when_no_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        // Worktree path doesn't need to exist — the gate short-circuits
        // before touching it.
        let outcome = verify_kept_or_demote(
            &exp_dir,
            &tmp.path().join("nonexistent"),
            Some("abcd"),
            "t",
            "20260517-000000",
            DeliveryMode::NormalPr,
        )
        .expect("verify_kept_or_demote");
        assert_eq!(outcome, DemotionOutcome::NoMarker);
    }

    fn run_git(dir: &Path, args: &[&str]) {
        crate::git::run_git(dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e:#}"));
    }

    /// Initialize a one-commit real git repo at `dir` with a default
    /// identity + signing disabled (so the test can `git commit` without
    /// inheriting the host operator's signing setup). Returns the
    /// initial HEAD SHA.
    fn init_test_repo_with_initial_commit(dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        crate::git::init_test_repo(dir).unwrap();
        std::fs::write(dir.join("x"), "x").unwrap();
        run_git(dir, &["add", "x"]);
        run_git(dir, &["commit", "-q", "-m", "init"]);
        git_rev_parse_head(dir).expect("HEAD")
    }

    /// Happy path: agent edits a file + writes `outcome=implemented`.
    /// Coordinator's commit step must produce a real commit advancing
    /// HEAD; downstream `verify_kept_or_demote` then accepts.
    #[test]
    fn coordinator_commit_commits_when_implemented_with_dirty_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkout = tmp.path().join("clone");
        let baseline_head = init_test_repo_with_initial_commit(&checkout);
        // Simulate the agent's edit.
        std::fs::write(checkout.join("x"), "x edited\n").unwrap();
        // Simulate the agent's typed report.
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        write_implemented_report(&exp_dir, "tgt");

        coordinator_commit_if_kept(
            &exp_dir,
            &checkout,
            "tgt",
            "20260517-000000",
            DeliveryMode::NormalPr,
            &Settings::default(),
        )
        .expect("coordinator_commit_if_kept");

        // Coordinator created a real commit.
        let head_after = git_rev_parse_head(&checkout).expect("post-commit HEAD");
        assert_ne!(head_after, baseline_head, "HEAD must advance after coordinator commit");
        // Typed report still implemented; companion implementation.md
        // rendered; no demoted copy, no abort.md.
        let live = std::fs::read_to_string(exp_dir.join("optimizer-report.json")).unwrap();
        let parsed = OptimizerReport::from_json(&live).unwrap();
        assert!(matches!(parsed, OptimizerReport::Implemented(_)));
        assert!(
            exp_dir
                .join("implementation.md")
                .is_file(),
            "companion implementation.md should be rendered post-commit"
        );
        assert!(
            !exp_dir
                .join("abort.md")
                .exists()
        );
        assert!(
            !exp_dir
                .join("optimizer-report.json.demoted")
                .exists()
        );
        // Coordinator-provenance sidecar landed with the correct SHAs.
        let provenance_raw = std::fs::read_to_string(exp_dir.join("coordinator-provenance.json"))
            .expect("coordinator-provenance.json must be written post-commit");
        let provenance =
            crate::models::coordinator_provenance::CoordinatorProvenance::from_json_validated(
                &provenance_raw,
            )
            .expect("provenance parses + validates");
        assert_eq!(provenance.session_id, "20260517-000000");
        assert_eq!(provenance.target_id, "tgt");
        assert_eq!(provenance.delivery_mode, DeliveryMode::NormalPr);
        assert_eq!(
            provenance.base_sha, baseline_head,
            "base_sha must be the pre-coordinator-commit HEAD (the agent's parent)"
        );
        assert_eq!(
            provenance.head_sha, head_after,
            "head_sha must be the coordinator's post-commit HEAD"
        );
        assert_eq!(provenance.commit_message, "perf: optimize tgt");
    }

    /// `outcome=implemented` but the worktree has NO changes ("agent
    /// reported implemented but did nothing"). Must demote so downstream
    /// phases skip the target — committing an empty diff would produce
    /// a no-op PR.
    #[test]
    fn coordinator_commit_demotes_when_tree_is_clean() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkout = tmp.path().join("clone");
        let baseline_head = init_test_repo_with_initial_commit(&checkout);
        // NO edits — clean tree.
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        write_implemented_report(&exp_dir, "tgt");

        coordinator_commit_if_kept(
            &exp_dir,
            &checkout,
            "tgt",
            "20260517-000000",
            DeliveryMode::NormalPr,
            &Settings::default(),
        )
        .expect("coordinator_commit_if_kept");

        // Demoted: live report rewritten as aborted; original preserved.
        let live = std::fs::read_to_string(exp_dir.join("optimizer-report.json")).unwrap();
        let parsed = OptimizerReport::from_json(&live).unwrap();
        let aborted = match parsed {
            OptimizerReport::Aborted(r) => r,
            other => panic!("live report must be aborted; got {other:?}"),
        };
        assert!(
            aborted
                .reason
                .contains("no changes"),
            "demoted reason should mention the empty-tree cause; got: {}",
            aborted.reason
        );
        assert!(
            exp_dir
                .join("optimizer-report.json.demoted")
                .is_file(),
            "original implemented report preserved as .demoted"
        );
        // Companion abort.md rendered; stale implementation.md removed.
        assert!(
            exp_dir
                .join("abort.md")
                .is_file()
        );
        assert!(
            !exp_dir
                .join("implementation.md")
                .exists(),
            "stale implementation.md must be removed after demotion"
        );
        // HEAD must NOT have advanced (no commit happened).
        let head_after = git_rev_parse_head(&checkout).expect("HEAD still resolvable");
        assert_eq!(head_after, baseline_head);
    }

    /// No optimizer-report.json → no-op. The agent crashed or wrote
    /// nothing; `verify_kept_or_demote` downstream will handle it.
    #[test]
    fn coordinator_commit_noops_when_no_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkout = tmp.path().join("clone");
        let baseline_head = init_test_repo_with_initial_commit(&checkout);
        std::fs::write(checkout.join("x"), "x edited\n").unwrap();
        // No report at all.
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();

        coordinator_commit_if_kept(
            &exp_dir,
            &checkout,
            "tgt",
            "20260517-000000",
            DeliveryMode::NormalPr,
            &Settings::default(),
        )
        .expect("coordinator_commit_if_kept");

        // Nothing changed.
        assert!(
            !exp_dir
                .join("abort.md")
                .exists()
        );
        assert!(
            !exp_dir
                .join("implementation.md")
                .exists()
        );
        assert!(
            !exp_dir
                .join("optimizer-report.json")
                .exists()
        );
        let head_after = git_rev_parse_head(&checkout).expect("HEAD");
        assert_eq!(head_after, baseline_head);
    }

    /// Agent emits `outcome=aborted`: coordinator renders the companion
    /// abort.md and does NOT commit. Replaces the "both markers
    /// present" demotion test (structurally impossible under typed
    /// contract — agent only writes one report).
    #[test]
    fn coordinator_commit_renders_companion_when_aborted() {
        use crate::models::optimizer_report::{AbortedOutcomeTag, AbortedReport, FailedGate};
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkout = tmp.path().join("clone");
        let baseline_head = init_test_repo_with_initial_commit(&checkout);
        // Agent edited but then chose to abort (clippy failed mid-loop).
        std::fs::write(checkout.join("x"), "x edited\n").unwrap();
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        let report = OptimizerReport::Aborted(AbortedReport {
            schema_version: SchemaVersionV2,
            session_id: "20260517-000000".to_owned(),
            target_id: "tgt".to_owned(),
            outcome: AbortedOutcomeTag::Aborted,
            delivery_mode: DeliveryMode::NormalPr,
            reason: "clippy failed".to_owned(),
            failed_gate: Some(FailedGate::Clippy),
            failing_tests: None,
        });
        std::fs::write(
            exp_dir.join("optimizer-report.json"),
            report
                .to_json_pretty()
                .unwrap(),
        )
        .unwrap();

        coordinator_commit_if_kept(
            &exp_dir,
            &checkout,
            "tgt",
            "20260517-000000",
            DeliveryMode::NormalPr,
            &Settings::default(),
        )
        .expect("coordinator_commit_if_kept");

        // Companion abort.md rendered; no commit.
        assert!(
            exp_dir
                .join("abort.md")
                .is_file()
        );
        let abort_body = std::fs::read_to_string(exp_dir.join("abort.md")).unwrap();
        assert!(
            abort_body.contains("clippy failed"),
            "abort.md should surface the agent's reason; got:\n{abort_body}"
        );
        let head_after = git_rev_parse_head(&checkout).expect("HEAD");
        assert_eq!(head_after, baseline_head, "no commit on aborted outcome");
    }

    /// Return the trimmed stdout of `git -C <dir> <args...>`, panicking
    /// on non-zero exit. Used by the cleanup/replicate tests to assert
    /// `git worktree list` / `git remote get-url` output.
    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        crate::git::run_git_output(dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e:#}"))
    }

    /// Migration regression: when a path is a linked worktree pointer
    /// (the old layout), `recreate_checkout` must clear
    /// `<base>/.git/worktrees/<wt>/` registry too — not just `rm -rf`
    /// the working tree. Otherwise `git worktree list` still reports
    /// the stale path.
    #[test]
    fn recreate_checkout_clears_linked_worktree_metadata() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("base");
        std::fs::create_dir_all(&base).unwrap();
        crate::git::init_test_repo(&base).unwrap();
        std::fs::write(base.join("x"), "x").unwrap();
        run_git(&base, &["add", "x"]);
        run_git(&base, &["commit", "-q", "-m", "init"]);

        // Stage the old layout: a linked worktree at <wt> registered in
        // <base>/.git/worktrees/.
        let wt = tmp.path().join("wt");
        run_git(&base, &["worktree", "add", "-q", "-b", "agent/old", wt.to_str().unwrap(), "main"]);
        assert!(
            wt.join(".git").is_file(),
            "linked worktree's .git should be a file pointer, got {:?}",
            std::fs::metadata(wt.join(".git")).ok()
        );
        let before = git_stdout(&base, &["worktree", "list"]);
        assert!(
            before.contains(wt.to_str().unwrap()),
            "pre-condition: base should see the linked worktree:\n{before}",
        );

        let git = StdGitCheckoutManager;
        git.recreate_checkout(&base, &wt, "agent/new", "main")
            .expect("recreate_checkout must succeed against a linked-worktree path");

        // Post: clone has its own .git directory (NOT a pointer file).
        assert!(
            wt.join(".git").is_dir(),
            "after recreate_checkout the new clone's .git should be a directory, got {:?}",
            std::fs::metadata(wt.join(".git")).ok()
        );

        // Post: base's worktree list MUST NOT mention the old path.
        let after = git_stdout(&base, &["worktree", "list"]);
        let stale_lines: Vec<&str> = after
            .lines()
            .filter(|l| l.contains(wt.to_str().unwrap()))
            .collect();
        assert!(
            stale_lines.is_empty(),
            "base.git/worktrees/ still registers the old path after migration:\n{after}",
        );
    }

    /// Remote-replication regression: `git clone --local` only creates
    /// an `origin` pointing at the local base path. `recreate_checkout`
    /// must replicate EVERY remote from base (not just `origin`), with
    /// the correct URLs — otherwise Phase 5 publish via a non-origin
    /// `publish_remote` (e.g. `fork`) fails because the remote isn't
    /// in the clone.
    #[test]
    fn recreate_checkout_replicates_all_base_remotes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("base");
        std::fs::create_dir_all(&base).unwrap();
        crate::git::init_test_repo(&base).unwrap();
        std::fs::write(base.join("x"), "x").unwrap();
        run_git(&base, &["add", "x"]);
        run_git(&base, &["commit", "-q", "-m", "init"]);

        // Configure base with TWO remotes: origin + fork (the latter
        // simulates an operator's `publish_remote = "fork"` setup).
        run_git(&base, &["remote", "add", "origin", "https://github.com/upstream/stacks-core.git"]);
        run_git(&base, &["remote", "add", "fork", "https://github.com/bot/stacks-core.git"]);

        let wt = tmp.path().join("clone");
        let git = StdGitCheckoutManager;
        git.recreate_checkout(&base, &wt, "agent/t", "main")
            .expect("recreate_checkout");

        // Both remotes present in the clone, with the URLs from base
        // (NOT the local base path that `--local` defaulted origin to).
        let origin_url = git_stdout(&wt, &["remote", "get-url", "origin"]);
        assert_eq!(
            origin_url, "https://github.com/upstream/stacks-core.git",
            "clone's origin must be base's origin URL, not the local base path",
        );
        let fork_url = git_stdout(&wt, &["remote", "get-url", "fork"]);
        assert_eq!(
            fork_url, "https://github.com/bot/stacks-core.git",
            "non-origin remote `fork` must be replicated into the clone",
        );

        // Sanity: the per-target branch landed.
        let head_branch = git_stdout(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]);
        assert_eq!(head_branch, "agent/t");
    }

    // ── --resume tests ────────────────────────────────────────────
    //
    // `resume_target_is_complete` is the gate. It composes
    // `read_optimizer_report` + `validate_report_context`, so we cover
    // the failure modes (no report, corrupt JSON, schema-invalid,
    // context mismatch) plus the happy paths (implemented + aborted
    // both count as complete).

    fn minimal_resume_target(id: &str, delivery_mode: DeliveryMode) -> MergedTarget {
        use crate::models::common::{Bucket, Hotspot, ImprovementVector, Risk};
        use crate::models::targets::MergedFrom;

        let consensus_breaking = !matches!(delivery_mode, DeliveryMode::NormalPr);
        let poc_implementable = match delivery_mode {
            DeliveryMode::NormalPr => None,
            DeliveryMode::ConsensusPocPr => Some(true),
            DeliveryMode::ConsensusIssue => Some(false),
        };
        MergedTarget {
            id: id.to_owned(),
            merged_from: vec![MergedFrom {
                family_id: "x-fam".to_owned(),
                target_index: 0,
            }],
            convergence_count: 1,
            rank: None,
            target_span: "x::y".to_owned(),
            bucket: Bucket::BlockProcessing,
            hotspot: Hotspot {
                span: "x::y".to_owned(),
                self_wall_us: 1,
                total_wall_us: 1,
                calls: 1,
                location: "x.rs:1".to_owned(),
            },
            files: vec!["x.rs".to_owned()],
            evidence: "e".to_owned(),
            proposed_change: "p".to_owned(),
            expected_improvement: ImprovementVector {
                tx_latency: 1.0,
                tenure_throughput: 0.0,
                commit_time: 0.0,
            },
            risk: Risk::Low,
            verification_plan: "v".to_owned(),
            verification_replay: None,
            merge_notes: None,
            contributor_differences: None,
            consensus_breaking,
            breakage_class: None,
            poc_implementable,
            poc_test_scope: None,
            consensus_writeup: None,
            delivery_mode,
            bench_eligible: matches!(delivery_mode, DeliveryMode::NormalPr),
        }
    }

    /// Test SHA constants — full-width hex to satisfy validate_sha. The
    /// values are arbitrary but must not be equal (base_sha != head_sha).
    const TEST_BASE_SHA: &str = "0ad33704c259da4102b5f195617760003ac89c18";
    const TEST_HEAD_SHA: &str = "f994e6ef03002fb7b1acdc1b5018da40e73b105b";
    const TEST_WRONG_BASE_SHA: &str = "b2ea69397c89f7ef8c61a7dcb289d55a421564e4";

    /// Stage a coordinator-provenance sidecar that pairs with
    /// [`write_implemented_report`] (matching session_id /
    /// delivery_mode). Used by resume tests for implemented outcomes,
    /// which require both the report AND the sidecar to skip.
    fn write_provenance_sidecar(exp_dir: &Path, target_id: &str, base_sha: &str) {
        use crate::models::common::SchemaVersionV1;
        use crate::models::coordinator_provenance::CoordinatorProvenance;
        let p = CoordinatorProvenance {
            schema_version: SchemaVersionV1,
            session_id: "20260517-000000".to_owned(),
            target_id: target_id.to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            base_sha: base_sha.to_owned(),
            head_sha: TEST_HEAD_SHA.to_owned(),
            commit_message: format!("perf: optimize {target_id}"),
        };
        std::fs::write(exp_dir.join("coordinator-provenance.json"), p.to_json_pretty().unwrap())
            .unwrap();
    }

    /// No report on disk → not complete; falls through to the
    /// normal re-run path.
    #[test]
    fn resume_target_is_complete_returns_false_when_no_report() {
        let tmp = tempfile::tempdir().unwrap();
        let target = minimal_resume_target("missing-report", DeliveryMode::NormalPr);
        assert!(!resume_target_is_complete(tmp.path(), &target, "20260521-000000", TEST_BASE_SHA,));
    }

    /// Implemented report + matching provenance sidecar (base_sha
    /// matches the session's archived baseline source_sha) → complete;
    /// skip. The load-bearing happy path.
    #[test]
    fn resume_target_is_complete_returns_true_on_matching_implemented_report() {
        let tmp = tempfile::tempdir().unwrap();
        write_implemented_report(tmp.path(), "test-target");
        write_provenance_sidecar(tmp.path(), "test-target", TEST_BASE_SHA);
        let target = minimal_resume_target("test-target", DeliveryMode::NormalPr);
        assert!(resume_target_is_complete(tmp.path(), &target, "20260517-000000", TEST_BASE_SHA,));
    }

    /// Aborted report whose context matches → complete; skip. Aborted
    /// experiments produce no commit and have no provenance sidecar;
    /// the gate must NOT require one for them.
    #[test]
    fn resume_target_is_complete_returns_true_on_matching_aborted_report() {
        use crate::models::optimizer_report::{AbortedOutcomeTag, AbortedReport, FailedGate};
        let tmp = tempfile::tempdir().unwrap();
        let report = OptimizerReport::Aborted(AbortedReport {
            schema_version: SchemaVersionV2,
            session_id: "20260517-000000".to_owned(),
            target_id: "test-target".to_owned(),
            outcome: AbortedOutcomeTag::Aborted,
            delivery_mode: DeliveryMode::NormalPr,
            reason: "test reason".to_owned(),
            failed_gate: Some(FailedGate::Clippy),
            failing_tests: None,
        });
        std::fs::write(
            tmp.path()
                .join("optimizer-report.json"),
            report
                .to_json_pretty()
                .unwrap(),
        )
        .unwrap();
        let target = minimal_resume_target("test-target", DeliveryMode::NormalPr);
        assert!(resume_target_is_complete(tmp.path(), &target, "20260517-000000", TEST_BASE_SHA,));
    }

    /// Report's session_id doesn't match the live session → not
    /// complete; re-run with a warning. Catches the "report left over
    /// from a previous session" case.
    #[test]
    fn resume_target_is_complete_returns_false_on_session_id_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        write_implemented_report(tmp.path(), "test-target");
        write_provenance_sidecar(tmp.path(), "test-target", TEST_BASE_SHA);
        let target = minimal_resume_target("test-target", DeliveryMode::NormalPr);
        assert!(!resume_target_is_complete(tmp.path(), &target, "20991231-235959", TEST_BASE_SHA,));
    }

    /// Report's target_id doesn't match the expected one → not
    /// complete; re-run with a warning. Catches a misrouted report.
    #[test]
    fn resume_target_is_complete_returns_false_on_target_id_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        write_implemented_report(tmp.path(), "report-says-A");
        write_provenance_sidecar(tmp.path(), "report-says-A", TEST_BASE_SHA);
        let target = minimal_resume_target("expected-is-B", DeliveryMode::NormalPr);
        assert!(!resume_target_is_complete(tmp.path(), &target, "20260517-000000", TEST_BASE_SHA,));
    }

    /// Corrupt JSON on disk → not complete; re-run with a warning.
    /// The warning lets operators distinguish "agent crashed" from
    /// "stale or tampered report" when investigating skips.
    #[test]
    fn resume_target_is_complete_returns_false_on_corrupt_json() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path()
                .join("optimizer-report.json"),
            "{ not valid json",
        )
        .unwrap();
        let target = minimal_resume_target("test-target", DeliveryMode::NormalPr);
        assert!(!resume_target_is_complete(tmp.path(), &target, "20260517-000000", TEST_BASE_SHA,));
    }

    /// Implemented report present but provenance sidecar missing → not
    /// complete; re-run. Closes the historical hole where a
    /// partially-recovered session could skip a target without
    /// verifying the on-disk commit was built against the archived
    /// baseline source.
    #[test]
    fn resume_target_is_complete_returns_false_when_implemented_lacks_provenance() {
        let tmp = tempfile::tempdir().unwrap();
        write_implemented_report(tmp.path(), "test-target");
        // No provenance sidecar.
        let target = minimal_resume_target("test-target", DeliveryMode::NormalPr);
        assert!(!resume_target_is_complete(tmp.path(), &target, "20260517-000000", TEST_BASE_SHA,));
    }

    /// Provenance sidecar present but `base_sha` doesn't match the
    /// session's archived baseline source_sha → not complete; re-run.
    /// This is the load-bearing apples-to-apples invariant: if the
    /// per-target branch was rebased onto / built against a different
    /// base than Phase 0a archived, the comparison isn't honest.
    #[test]
    fn resume_target_is_complete_returns_false_on_base_sha_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        write_implemented_report(tmp.path(), "test-target");
        write_provenance_sidecar(tmp.path(), "test-target", TEST_WRONG_BASE_SHA);
        let target = minimal_resume_target("test-target", DeliveryMode::NormalPr);
        // Gate is given TEST_BASE_SHA as the session's source; sidecar
        // recorded TEST_WRONG_BASE_SHA → mismatch → re-run.
        assert!(!resume_target_is_complete(tmp.path(), &target, "20260517-000000", TEST_BASE_SHA,));
    }

    /// Provenance sidecar has matching `base_sha`, but its `target_id`
    /// belongs to a different target (e.g. an operator copy-paste
    /// during recovery). Resume gate's context check must catch this
    /// — without it the wrong target's `head_sha` would skip the
    /// re-run and the audit chain would silently lie.
    #[test]
    fn resume_target_is_complete_returns_false_on_provenance_context_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        write_implemented_report(tmp.path(), "expected-target");
        // Sidecar says the commit was made for a DIFFERENT target even
        // though the report + dir name align with `expected-target`.
        write_provenance_sidecar(tmp.path(), "some-other-target", TEST_BASE_SHA);
        let target = minimal_resume_target("expected-target", DeliveryMode::NormalPr);
        assert!(!resume_target_is_complete(tmp.path(), &target, "20260517-000000", TEST_BASE_SHA,));
    }
}
