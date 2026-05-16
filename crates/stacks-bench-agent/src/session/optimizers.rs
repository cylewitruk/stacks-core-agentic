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
use crate::models::common::DeliveryMode;
use crate::models::targets::MergedTarget;
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

        // `git clone --reference <base> --local <base> <checkout>` —
        // `--reference` shares the object store, `--local` says the
        // source is on the local filesystem (enables hardlinks for
        // refs/HEAD/etc.; safe + cheap). `--branch <base_branch>`
        // checks out base_branch's tip in the new clone.
        let status = std::process::Command::new("git")
            .arg("clone")
            .arg("--reference")
            .arg(base)
            .arg("--branch")
            .arg(base_branch)
            .arg("--local")
            .arg(base)
            .arg(checkout)
            .status()
            .with_context(|| format!("git clone {} -> {}", base.display(), checkout.display()))?;
        if !status.success() {
            anyhow::bail!(
                "git clone --reference {} --branch {base_branch} --local {} {} exited {status}",
                base.display(),
                base.display(),
                checkout.display(),
            );
        }

        // Switch to the agent's per-target branch (created from the
        // base_branch tip we just cloned).
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(checkout)
            .arg("switch")
            .arg("-c")
            .arg(branch_name)
            .status()
            .with_context(|| format!("git switch -c {branch_name} in {}", checkout.display()))?;
        if !status.success() {
            anyhow::bail!("git -C {} switch -c {branch_name} exited {status}", checkout.display(),);
        }

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
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(base)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(checkout)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(base)
            .arg("worktree")
            .arg("prune")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
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
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(base)
        .arg("remote")
        .output()
        .with_context(|| format!("listing remotes in {}", base.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "git -C {} remote exited {}; cannot replicate remotes into clone {}",
            base.display(),
            out.status,
            checkout.display(),
        );
    }
    let remote_names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if remote_names.is_empty() {
        // Base has no remotes — nothing to replicate. The clone's
        // `origin` still points at the local base path, which is
        // fine for a local-only repo but Phase 5 publish will fail
        // loudly if the operator configured `publish_remote` against
        // a remote that doesn't exist. That's the right failure
        // mode (clearer than silently pushing to the local base).
        return Ok(());
    }
    let clone_existing: std::collections::BTreeSet<String> = {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(checkout)
            .arg("remote")
            .output()
            .with_context(|| format!("listing remotes in {}", checkout.display()))?;
        if !out.status.success() {
            anyhow::bail!("git -C {} remote exited {}", checkout.display(), out.status,);
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect()
    };
    for name in &remote_names {
        let url_out = std::process::Command::new("git")
            .arg("-C")
            .arg(base)
            .arg("remote")
            .arg("get-url")
            .arg(name)
            .output()
            .with_context(|| format!("getting URL for {name} in {}", base.display()))?;
        if !url_out.status.success() {
            anyhow::bail!(
                "git -C {} remote get-url {name} exited {}",
                base.display(),
                url_out.status,
            );
        }
        let url = String::from_utf8_lossy(&url_out.stdout)
            .trim()
            .to_owned();
        if url.is_empty() {
            anyhow::bail!("remote {name} in {} has empty URL", base.display());
        }
        let subcmd = if clone_existing.contains(name) { "set-url" } else { "add" };
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(checkout)
            .arg("remote")
            .arg(subcmd)
            .arg(name)
            .arg(&url)
            .status()
            .with_context(|| {
                format!("git -C {} remote {subcmd} {name} {url}", checkout.display())
            })?;
        if !status.success() {
            anyhow::bail!(
                "git -C {} remote {subcmd} {name} {url} exited {status}",
                checkout.display(),
            );
        }
    }
    Ok(())
}

/// Tear down the per-target git clone for every experiment in this
/// session whose marker is `abort.md` (or has neither marker —
/// meaning the agent crashed / never finished, equivalent to abort
/// for cleanup purposes).
///
/// Since each per-target checkout is a stand-alone clone (own `.git/`
/// inside its cwd, own refs, own branch), teardown is a single
/// `rm -rf <checkout>` — no `git worktree remove` / `git worktree
/// prune` / `git branch -D` ordering to worry about. The
/// `agent/<session>/<target>` branch lived inside the clone and goes
/// away with it.
///
/// **Kept (`implementation.md`-marked) experiments are NOT touched** —
/// their checkouts are what Phase 5 publish reads + pushes from, and
/// their commits must survive until the PR is filed. Operators may
/// also want to inspect a kept checkout post-session.
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
        // Marker-file gating: implementation.md → keep checkout
        // (publish reads it). abort.md OR no marker → drop it.
        if exp
            .join("implementation.md")
            .exists()
        {
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
}

/// Outputs of an optimizer fan-out.
#[derive(Debug, Default)]
pub struct Outputs {
    /// Total targets considered.
    pub total: usize,
    /// `optimize/<id>/implementation.md` exists post-run.
    pub landed: usize,
    /// `optimize/<id>/abort.md` exists post-run.
    pub aborted: usize,
    /// `optimize/<id>/consensus-issue.md` exists post-run.
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
            target_json: serde_json::to_string(target)
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

    // Tally per-target markers.
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
        } else if dir.join("abort.md").is_file() {
            outputs.aborted += 1;
        } else if dir
            .join("implementation.md")
            .is_file()
        {
            outputs.landed += 1;
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
            non_targets_path: prompts_dir
                .join("non-targets.md")
                .to_string_lossy()
                .into_owned(),
            optimization_targets_schema_path: state
                .framework
                .schemas_dir
                .join("optimization-targets.schema.json")
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
    //   1. Operator's prompts dir — the agent reads non-targets.md (referenced by
    //      absolute path in the rendered prompt).
    //   2. Operator's schemas dir — the agent reads
    //      optimization-targets.schema.json for output validation.
    //   3. Experiment dir — `nextest.log`, `implementation.md` / `abort.md`,
    //      `side-observations.md` all land here.
    //
    // **Not added in b.1**: `stacks_bench_shadow_dir` — the optimizer
    // prompt explicitly forbids running `stacks-bench` inside codex
    // (bench moved to coordinator in pass-b). Keeping it in `--add-dir`
    // would expand the sandbox blast radius (typically `/Volumes/Extern`)
    // for no reason and would also fail outright if the configured
    // shadow dir isn't mounted. Pass-b.2 surfaces shadow-dir-root
    // coordinator-side only.
    let add_dirs: Vec<PathBuf> = vec![
        prompts_dir.to_path_buf(),
        state
            .framework
            .schemas_dir
            .clone(),
        exp_dir.clone(),
    ];

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
    // Strict verification contract before committing — pulled from
    // Codex's b.1 review:
    //   1. `implementation.md` is present AND `abort.md` is absent.
    //   2. `git status --porcelain` reports actual changes (catches "agent wrote
    //      the marker but did nothing").
    //   3. The commit itself succeeds.
    //   4. HEAD advances past `baseline_head` (defense in depth —
    //      `verify_kept_or_demote` re-checks this after we return).
    //
    // Any failure in (1)-(3) demotes to `abort.md` immediately so the
    // rest of the pipeline correctly skips this target.
    coordinator_commit_if_kept(&exp_dir, &worktree, &target.id, &state.settings)?;

    // Post-invoke correctness gate: if `implementation.md` survived
    // the coordinator commit step, verify HEAD has actually advanced.
    // This is now defense-in-depth — `coordinator_commit_if_kept` already
    // demotes on most failure modes — but it catches any path that
    // produced an `implementation.md` without a corresponding HEAD
    // advance (e.g. a future refactor that skips the commit step).
    verify_kept_or_demote(&exp_dir, &worktree, baseline_head.as_deref(), &target.id)?;

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
    settings: &Settings,
) -> Result<()> {
    let impl_md = exp_dir.join("implementation.md");
    let abort_md = exp_dir.join("abort.md");

    // Contract step 1: marker file shape.
    if !impl_md.is_file() {
        // Agent didn't declare kept; nothing for us to commit. If
        // `abort.md` is also missing, `verify_kept_or_demote` won't
        // demote (no marker = NoMarker path), which is the right
        // call — the experiment crashed and downstream phases will
        // treat it as such.
        return Ok(());
    }
    if abort_md.is_file() {
        // Both markers present = agent confusion. Trust `abort.md`
        // (the more conservative signal) and demote.
        demote_kept_to_abort(
            exp_dir,
            target_id,
            "agent wrote both `implementation.md` AND `abort.md`; treating as abort (the \
             conservative signal)",
        )?;
        return Ok(());
    }

    // Contract step 2: tree must actually have changes. `git status
    // --porcelain` outputs one line per modified/untracked path; empty
    // stdout means "clean tree."
    let porcelain = std::process::Command::new("git")
        .arg("-C")
        .arg(checkout)
        .arg("status")
        .arg("--porcelain")
        .output()
        .with_context(|| format!("git status in {}", checkout.display()))?;
    if !porcelain.status.success() {
        demote_kept_to_abort(
            exp_dir,
            target_id,
            &format!(
                "`git status --porcelain` failed in {} (exit {})",
                checkout.display(),
                porcelain.status
            ),
        )?;
        return Ok(());
    }
    let dirty = !porcelain.stdout.is_empty();
    if !dirty {
        demote_kept_to_abort(
            exp_dir,
            target_id,
            "agent wrote `implementation.md` but the worktree has no changes (status --porcelain \
             empty); coordinator has nothing to commit",
        )?;
        return Ok(());
    }

    // Contract step 3: commit, with env-var identity overrides.
    let env = optimizer_git_env(settings);
    let add = std::process::Command::new("git")
        .arg("-C")
        .arg(checkout)
        .arg("add")
        .arg("-A")
        .envs(
            env.iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .status()
        .with_context(|| format!("git add -A in {}", checkout.display()))?;
    if !add.success() {
        demote_kept_to_abort(
            exp_dir,
            target_id,
            &format!("coordinator `git add -A` failed in {} (exit {})", checkout.display(), add),
        )?;
        return Ok(());
    }
    let msg = format!("perf: optimize {target_id}");
    let commit = std::process::Command::new("git")
        .arg("-C")
        .arg(checkout)
        .arg("commit")
        .arg("-m")
        .arg(&msg)
        .envs(
            env.iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .status()
        .with_context(|| format!("git commit in {}", checkout.display()))?;
    if !commit.success() {
        demote_kept_to_abort(
            exp_dir,
            target_id,
            &format!(
                "coordinator `git commit -m {msg:?}` failed in {} (exit {}); check operator's git \
                 config / signing setup",
                checkout.display(),
                commit
            ),
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

/// Rename `implementation.md` → `implementation.md.demoted` and write
/// `abort.md` carrying `reason`. Used by
/// [`coordinator_commit_if_kept`] whenever the verification contract
/// fails. Mirrors [`verify_kept_or_demote`]'s demotion mechanics
/// (same file layout for downstream phases to consume).
fn demote_kept_to_abort(exp_dir: &Path, target_id: &str, reason: &str) -> Result<()> {
    let impl_md = exp_dir.join("implementation.md");
    let demoted = exp_dir.join("implementation.md.demoted");
    let _ = std::fs::rename(&impl_md, &demoted);
    let note = format!(
        "Coordinator demoted `implementation.md` → `abort.md` for `{target_id}`.\n\nReason: \
         {reason}\n\nThe original `implementation.md` is preserved at `implementation.md.demoted` \
         for diagnosis. Inspect `final-message.md` + `events.jsonl` for the agent's own \
         narrative.\n"
    );
    std::fs::write(exp_dir.join("abort.md"), note).with_context(|| {
        format!("writing demotion abort.md for {target_id} at {}", exp_dir.display())
    })?;
    tracing::warn!(
        target = "session.optimizers",
        target_id = %target_id,
        reason = %reason,
        "demoted implementation.md → abort.md (coordinator commit contract failed)",
    );
    Ok(())
}

/// Outcome of [`verify_kept_or_demote`]. Exposed (with `pub(super)` would
/// be tighter, but this module is private to `session` already) so tests
/// can assert which branch ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemotionOutcome {
    /// `implementation.md` wasn't present — nothing to verify.
    NoMarker,
    /// `implementation.md` present + HEAD advanced past baseline. Kept.
    HeadAdvanced,
    /// `implementation.md` present, HEAD == baseline. Demoted to
    /// `abort.md` (original preserved as `implementation.md.demoted`).
    Demoted,
    /// `implementation.md` present, but we couldn't resolve a HEAD SHA
    /// on one side (worktree corrupt / never created / etc.). Marker
    /// kept as-is; downstream phases will fail loudly if the worktree
    /// is unusable.
    Indeterminate,
}

/// Post-invoke correctness gate. If the agent claimed
/// `implementation.md` ("kept attempt(s)"), verify the worktree's HEAD
/// has advanced past `baseline_head` (the worktree's initial HEAD,
/// captured right after creation = base_branch tip at that moment).
/// If HEAD hasn't advanced, no commits were made and there's nothing
/// for Phase 3 to bench or Phase 5 to publish — demote to `abort.md`
/// (preserving the original implementation writeup as
/// `implementation.md.demoted` for diagnosis).
///
/// The marker file alone isn't proof of work: sandbox/signing failures
/// (operator's global `commit.gpgsign=true` with an unreachable token,
/// sandbox deny on `.git/...`, etc.) can block `git commit` while the
/// agent's clippy/nextest gates still pass.
pub fn verify_kept_or_demote(
    exp_dir: &Path,
    worktree: &Path,
    baseline_head: Option<&str>,
    target_id: &str,
) -> Result<DemotionOutcome> {
    let impl_md = exp_dir.join("implementation.md");
    if !impl_md.is_file() {
        return Ok(DemotionOutcome::NoMarker);
    }
    let head_now = git_rev_parse_head(worktree);
    match (baseline_head, head_now.as_deref()) {
        (Some(base), Some(now)) if base == now => {
            let note = format!(
                "Coordinator demoted `implementation.md` → `abort.md` for `{target_id}`.\n\nThe \
                 optimizer wrote `implementation.md`, signalling a kept attempt — but the \
                 worktree's HEAD is still at the base branch tip (`{base}`). No commits were \
                 made, so there is nothing for Phase 3 to bench or Phase 5 to publish. The most \
                 common cause is a `git commit` that failed silently inside the codex sandbox \
                 (signing prompt blocked by missing socket / YubiKey, or a sandbox write deny on \
                 `.git/...`).\n\nInspect `final-message.md` + `events.jsonl` for the agent's own \
                 diagnostics; the original `implementation.md` is preserved in \
                 `implementation.md.demoted` so its content isn't lost.\n"
            );
            let preserved = exp_dir.join("implementation.md.demoted");
            let _ = std::fs::rename(&impl_md, &preserved);
            std::fs::write(exp_dir.join("abort.md"), note).with_context(|| {
                format!("writing demotion abort.md for {target_id} at {}", exp_dir.display())
            })?;
            tracing::warn!(
                target = "session.optimizers",
                target_id = %target_id,
                "demoted implementation.md → abort.md (HEAD did not advance past base branch)"
            );
            Ok(DemotionOutcome::Demoted)
        }
        (Some(_), Some(_)) => Ok(DemotionOutcome::HeadAdvanced),
        _ => {
            tracing::warn!(
                target = "session.optimizers",
                target_id = %target_id,
                "could not verify HEAD advance (rev-parse failed); trusting implementation.md as-is"
            );
            Ok(DemotionOutcome::Indeterminate)
        }
    }
}

/// Run `git -C <worktree> rev-parse HEAD` and return the trimmed SHA.
/// Returns `None` on any failure — caller decides whether to treat that
/// as a soft "trust the marker" path or a hard error. Used by the
/// "did the agent actually commit?" gate at the end of `run_one`.
pub fn git_rev_parse_head(worktree: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout)
        .ok()?
        .trim()
        .to_owned();
    if s.is_empty() { None } else { Some(s) }
}

/// Clear per-target optimizer outputs so a retry doesn't leave stale
/// decision markers behind. Mirrors the bash `rm -f ...` block.
fn clear_optimizer_artifacts(exp_dir: &Path) -> Result<()> {
    for name in [
        "abort.md",
        "implementation.md",
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

    /// Real-git: stage a worktree, write `implementation.md` WITHOUT
    /// committing, and verify the demotion path. HEAD unchanged should
    /// yield `DemotionOutcome::Demoted`, `implementation.md` should
    /// move to `implementation.md.demoted`, and `abort.md` should be
    /// written with a diagnostic note.
    #[test]
    fn verify_kept_or_demote_demotes_when_head_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("base");
        std::fs::create_dir_all(&base).unwrap();

        // git init + one commit (so HEAD has a SHA we can capture).
        run_git(&base, &["init", "-q", "-b", "main"]);
        // Local identity / no signing — otherwise this test would inherit
        // the caller's gpgsign config and need a YubiKey.
        run_git(&base, &["config", "user.email", "t@t"]);
        run_git(&base, &["config", "user.name", "t"]);
        run_git(&base, &["config", "commit.gpgsign", "false"]);
        std::fs::write(base.join("x"), "x").unwrap();
        run_git(&base, &["add", "x"]);
        run_git(&base, &["commit", "-q", "-m", "init"]);

        let baseline_head = git_rev_parse_head(&base).expect("base HEAD");

        // Add a real worktree at a sibling path (so it's not nested
        // inside `base`).
        let worktree = tmp.path().join("wt");
        run_git(&base, &["worktree", "add", "-q", "-b", "agent/t", worktree.to_str().unwrap()]);

        // Simulate the agent writing implementation.md without committing.
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        std::fs::write(exp_dir.join("implementation.md"), "# pretend kept\n").unwrap();

        let outcome =
            verify_kept_or_demote(&exp_dir, &worktree, Some(&baseline_head), "test-target")
                .expect("verify_kept_or_demote");

        assert_eq!(outcome, DemotionOutcome::Demoted);
        assert!(
            !exp_dir
                .join("implementation.md")
                .exists(),
            "implementation.md should have been moved"
        );
        assert!(
            exp_dir
                .join("implementation.md.demoted")
                .is_file(),
            "implementation.md.demoted should preserve the original"
        );
        let abort = std::fs::read_to_string(exp_dir.join("abort.md")).unwrap();
        assert!(abort.contains("test-target"), "abort.md missing target id: {abort}");
        assert!(abort.contains(&baseline_head), "abort.md missing baseline SHA: {abort}");
    }

    /// Conversely: if the agent DID commit, HEAD advances and the
    /// `implementation.md` marker stays in place.
    #[test]
    fn verify_kept_or_demote_preserves_when_head_advanced() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("base");
        std::fs::create_dir_all(&base).unwrap();
        run_git(&base, &["init", "-q", "-b", "main"]);
        run_git(&base, &["config", "user.email", "t@t"]);
        run_git(&base, &["config", "user.name", "t"]);
        run_git(&base, &["config", "commit.gpgsign", "false"]);
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
        std::fs::write(exp_dir.join("implementation.md"), "# kept\n").unwrap();

        let outcome =
            verify_kept_or_demote(&exp_dir, &worktree, Some(&baseline_head), "test-target")
                .expect("verify_kept_or_demote");

        assert_eq!(outcome, DemotionOutcome::HeadAdvanced);
        assert!(
            exp_dir
                .join("implementation.md")
                .is_file(),
            "implementation.md should be preserved"
        );
        assert!(
            !exp_dir
                .join("abort.md")
                .exists()
        );
        assert!(
            !exp_dir
                .join("implementation.md.demoted")
                .exists()
        );
    }

    /// When `implementation.md` is absent (e.g. agent wrote `abort.md`),
    /// the gate is a no-op.
    #[test]
    fn verify_kept_or_demote_noop_when_no_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        // Worktree path doesn't need to exist — the gate short-circuits
        // before touching it.
        let outcome =
            verify_kept_or_demote(&exp_dir, &tmp.path().join("nonexistent"), Some("abcd"), "t")
                .expect("verify_kept_or_demote");
        assert_eq!(outcome, DemotionOutcome::NoMarker);
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} failed: {status}");
    }

    /// Initialize a one-commit real git repo at `dir` with a default
    /// identity + signing disabled (so the test can `git commit` without
    /// inheriting the host operator's signing setup). Returns the
    /// initial HEAD SHA.
    fn init_test_repo_with_initial_commit(dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        run_git(dir, &["init", "-q", "-b", "main"]);
        run_git(dir, &["config", "user.email", "t@t"]);
        run_git(dir, &["config", "user.name", "t"]);
        run_git(dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("x"), "x").unwrap();
        run_git(dir, &["add", "x"]);
        run_git(dir, &["commit", "-q", "-m", "init"]);
        git_rev_parse_head(dir).expect("HEAD")
    }

    /// Layer 1B v2 pass-b.1 happy path: agent edits a file + writes
    /// `implementation.md`. Coordinator's commit step must produce a
    /// real commit advancing HEAD; downstream `verify_kept_or_demote`
    /// then accepts.
    #[test]
    fn coordinator_commit_commits_when_implementation_md_with_dirty_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkout = tmp.path().join("clone");
        let _baseline_head = init_test_repo_with_initial_commit(&checkout);
        // Simulate the agent's edit.
        std::fs::write(checkout.join("x"), "x edited\n").unwrap();
        // Simulate the agent's marker.
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        std::fs::write(exp_dir.join("implementation.md"), "# kept\n").unwrap();

        coordinator_commit_if_kept(&exp_dir, &checkout, "tgt", &Settings::default())
            .expect("coordinator_commit_if_kept");

        // Coordinator created a real commit.
        let head_after = git_rev_parse_head(&checkout).expect("post-commit HEAD");
        assert_ne!(head_after, _baseline_head, "HEAD must advance after coordinator commit");
        // `implementation.md` survives (not demoted).
        assert!(
            exp_dir
                .join("implementation.md")
                .is_file()
        );
        assert!(
            !exp_dir
                .join("abort.md")
                .exists()
        );
        assert!(
            !exp_dir
                .join("implementation.md.demoted")
                .exists()
        );
    }

    /// Contract step 2: `implementation.md` present but the worktree
    /// has NO changes ("agent wrote the marker but did nothing").
    /// Must demote to `abort.md` so downstream phases skip the
    /// target — committing an empty diff would produce a no-op PR.
    #[test]
    fn coordinator_commit_demotes_when_tree_is_clean() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkout = tmp.path().join("clone");
        let baseline_head = init_test_repo_with_initial_commit(&checkout);
        // NO edits — clean tree.
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        std::fs::write(exp_dir.join("implementation.md"), "# kept (lying)\n").unwrap();

        coordinator_commit_if_kept(&exp_dir, &checkout, "tgt", &Settings::default())
            .expect("coordinator_commit_if_kept");

        assert!(
            exp_dir
                .join("abort.md")
                .is_file(),
            "abort.md must be written when tree is clean"
        );
        assert!(
            exp_dir
                .join("implementation.md.demoted")
                .is_file(),
            "original implementation.md preserved as .demoted"
        );
        assert!(
            !exp_dir
                .join("implementation.md")
                .exists(),
            "implementation.md must be moved out of the way after demotion"
        );
        // Demotion reason mentions the empty-tree case.
        let abort_body = std::fs::read_to_string(exp_dir.join("abort.md")).unwrap();
        assert!(
            abort_body.contains("no changes") || abort_body.contains("porcelain empty"),
            "abort.md should explain the demotion cause; got:\n{abort_body}",
        );
        // HEAD must NOT have advanced (no commit happened).
        let head_after = git_rev_parse_head(&checkout).expect("HEAD still resolvable");
        assert_eq!(head_after, baseline_head);
    }

    /// Contract step 1 edge: agent wrote BOTH `implementation.md` AND
    /// `abort.md`. Treat as abort (conservative signal); don't commit.
    #[test]
    fn coordinator_commit_demotes_when_both_markers_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkout = tmp.path().join("clone");
        let baseline_head = init_test_repo_with_initial_commit(&checkout);
        std::fs::write(checkout.join("x"), "x edited\n").unwrap();
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();
        std::fs::write(exp_dir.join("implementation.md"), "# kept\n").unwrap();
        std::fs::write(exp_dir.join("abort.md"), "actually no\n").unwrap();

        coordinator_commit_if_kept(&exp_dir, &checkout, "tgt", &Settings::default())
            .expect("coordinator_commit_if_kept");

        // implementation.md got demoted; abort.md remains (now
        // overwritten by the coordinator's diagnostic).
        assert!(
            exp_dir
                .join("abort.md")
                .is_file()
        );
        assert!(
            exp_dir
                .join("implementation.md.demoted")
                .is_file()
        );
        // No commit.
        let head_after = git_rev_parse_head(&checkout).expect("HEAD");
        assert_eq!(head_after, baseline_head);
    }

    /// No marker → no-op. The agent crashed or wrote nothing;
    /// `verify_kept_or_demote` downstream will handle it.
    #[test]
    fn coordinator_commit_noops_when_no_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkout = tmp.path().join("clone");
        let baseline_head = init_test_repo_with_initial_commit(&checkout);
        std::fs::write(checkout.join("x"), "x edited\n").unwrap();
        // No marker at all.
        let exp_dir = tmp.path().join("exp");
        std::fs::create_dir_all(&exp_dir).unwrap();

        coordinator_commit_if_kept(&exp_dir, &checkout, "tgt", &Settings::default())
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
        let head_after = git_rev_parse_head(&checkout).expect("HEAD");
        assert_eq!(head_after, baseline_head);
    }

    /// Return the trimmed stdout of `git -C <dir> <args...>`, panicking
    /// on non-zero exit. Used by the cleanup/replicate tests to assert
    /// `git worktree list` / `git remote get-url` output.
    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
        assert!(out.status.success(), "git {args:?} failed: {}", out.status);
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .to_owned()
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
        run_git(&base, &["init", "-q", "-b", "main"]);
        run_git(&base, &["config", "user.email", "t@t"]);
        run_git(&base, &["config", "user.name", "t"]);
        run_git(&base, &["config", "commit.gpgsign", "false"]);
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
        run_git(&base, &["init", "-q", "-b", "main"]);
        run_git(&base, &["config", "user.email", "t@t"]);
        run_git(&base, &["config", "user.name", "t"]);
        run_git(&base, &["config", "commit.gpgsign", "false"]);
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
}
