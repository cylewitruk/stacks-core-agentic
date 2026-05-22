//! `sbagent init` — one-shot bootstrap for a fresh operator directory.
//!
//! Bootstrap order:
//!
//! 1. Validate the loaded [`Settings`] has the fields `init` needs (`base`,
//!    `base_repo_url`, `publish_base_branch`, `prompt_overrides_dir`). If
//!    `--push` OR `--seed-from` is set, also `publish_token_file` (both rely on
//!    the bot's PAT to authenticate against GitHub).
//! 2. Resolve the target dir (`--dir` or cwd). Either the dir is empty or it's
//!    an existing repo with `.git/` already configured; the bootstrap is a
//!    no-op in the second case for the parts that already exist (`init` is
//!    loosely idempotent for re-runs).
//! 3. **`--seed-from` (optional)**: bare-clone the configured
//!    `publish_base_branch` from the source URL and push it to
//!    `base_repo_url` via PAT-via-env. Required when the bot fork is
//!    brand-new and doesn't yet carry the substrate branch.
//!    `base_repo_url` must start with the configured
//!    [`Settings::git_auth_url_prefix`] (defaults to
//!    `https://github.com/`); the auth header only matches HTTPS URLs
//!    under that prefix.
//! 4. `git init` if no `.git/` is present.
//! 5. Add `base` as a submodule from `base_repo_url`, checked out at
//!    `publish_base_branch`. Skipped if the submodule entry already exists.
//! 6. Replicate the bot remote on the submodule (best-effort, derived from
//!    `publish_head_owner` / `publish_base_repo`).
//! 7. Seed `.sbagent/prompts/` (templates + reference docs) via
//!    [`crate::prompts::seed_to`] and `.sbagent/schemas/` (JSON Schemas) via
//!    [`crate::schemas::seed_to`]. Both use don't-replace semantics so re-runs
//!    preserve operator tunes. Default layout: `<prompt_overrides_dir>`
//!    (typically `.sbagent/prompts/`) + the sibling `.sbagent/schemas/` derived
//!    from its parent when `schemas_dir` is unset in config.
//! 8. Drop a `.gitignore` with the conventional excludes (defensive
//!    `/config.toml`, `repos/<base>/target/`, mutable session state, editor
//!    noise).
//! 9. Stage ONLY the init-owned paths (`.gitignore`, `.gitmodules`, `<base>`,
//!    `<prompt_overrides_dir>`, `<schemas_dir>`) — never `git add -A` — then
//!    commit "chore: initial operator state" authored as the bot via
//!    [`crate::session::optimizers::optimizer_git_env`]. Stray pre-existing
//!    files in the target dir survive as untracked. Skipped if nothing was
//!    staged (re-run safe).
//! 10. If `--push`: read PAT via
//!     [`crate::session::publish::read_publish_token`], then
//!     `git push -u origin <branch>` using the same env-var override
//!     mechanism for the auth header. `origin` must start with the
//!     configured `git_auth_url_prefix` (default `https://github.com/`).
//!     The token never lands on disk (no `.git/config` mutation, no
//!     remote URL rewrite).
//!
//! `init` is filesystem-only. It does NOT create the GitHub repo, the
//! bot fork, or the PAT — those stay manual.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::git::{
    self, build_auth_header_env, push_with_pat, run_git, run_git_check, run_git_output,
    validate_auth_url,
};
use crate::session::optimizers::optimizer_git_env;
use crate::session::publish;
use crate::settings::Settings;

/// Args for `sbagent init`.
#[derive(Debug, Args, Clone)]
pub struct InitArgs {
    /// Target directory to initialize. Defaults to the current working
    /// directory.
    #[clap(long)]
    pub dir: Option<PathBuf>,

    /// Seed `publish_base_branch` on the bot fork (`base_repo_url`)
    /// from a different upstream fork BEFORE `git submodule add` runs.
    /// Required on the very first init against a brand-new bot fork
    /// that doesn't yet carry the configured `publish_base_branch` —
    /// without it, `git submodule add -b <branch>` errors because the
    /// branch doesn't exist on the bot fork.
    ///
    /// The flag's value is the URL of the upstream fork that DOES
    /// carry the branch (typically the human operator's fork during
    /// pilot, e.g. `https://github.com/cylewitruk/stacks-core.git`).
    /// init fetches the branch from this URL and pushes it to
    /// `base_repo_url` using the same PAT-via-env mechanism as
    /// `--push`, so the same `publish_token_file` precondition applies.
    ///
    /// Skip this flag on subsequent re-inits or when bootstrapping
    /// against a fork that already has the branch (e.g. upstream-mode
    /// where the bot fork was forked from a repo that already has
    /// the canonical branch).
    #[clap(long)]
    pub seed_from: Option<String>,

    /// After the initial commit, push to `origin` using the bot's PAT
    /// (read from `publish_token_file`). Requires that `origin` already
    /// be configured in the target directory (either by `git clone`'ing
    /// the empty bot repo first, or by `git remote add origin <url>`).
    ///
    /// Auth uses an `http.<git_auth_url_prefix>.extraheader` injected
    /// via `GIT_CONFIG_COUNT` env-vars for one invocation only — the
    /// token never lands in `.git/config`, in the remote URL, or in
    /// shell history. The `origin` URL MUST start with the configured
    /// `git_auth_url_prefix` (default `https://github.com/`); SSH or
    /// other-prefix URLs ignore the injected header and silently fall
    /// back to SSH / prompted auth, so we reject them up-front rather
    /// than producing a confusing "no credentials" error from git.
    #[clap(long)]
    pub push: bool,

    /// Branch name to push when `--push` is set. Defaults to `main`.
    #[clap(long, default_value = "main")]
    pub push_branch: String,
}

/// Run `sbagent init`. Bootstraps a fresh operator directory.
pub async fn run(args: InitArgs, settings: &Settings) -> Result<()> {
    let target_dir = args
        .dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));
    std::fs::create_dir_all(&target_dir)
        .with_context(|| format!("creating target dir {}", target_dir.display()))?;

    // Validate config has everything init needs. Fail fast: it's
    // cheaper to surface a missing field upfront than partway through
    // a `git submodule add`.
    let base_path = settings
        .base
        .as_deref()
        .context(
            "`base` not set in config; required for init (relative path under the operator dir \
             for the stacks-core submodule)",
        )?;
    let base_repo_url = settings
        .base_repo_url
        .as_deref()
        .context(
            "`base_repo_url` not set in config; required for init (clone URL for the stacks-core \
             submodule)",
        )?;
    let base_branch = settings
        .publish_base_branch
        .as_deref()
        .context(
            "`publish_base_branch` not set in config; required for init (the branch the submodule \
             checks out — same branch publish PRs target)",
        )?;
    let prompts_dir = settings
        .prompt_overrides_dir
        .as_deref()
        .context(
            "`prompt_overrides_dir` not set in config; required for init (where bundled prompt \
             defaults get seeded)",
        )?;

    // If --push or --seed-from was set, validate the token NOW.
    // Both operations rely on the bot's PAT to authenticate against
    // GitHub; failing fast is cheaper than bootstrapping and then
    // hitting an auth error mid-flow.
    let needs_token = args.push || args.seed_from.is_some();
    let token: Option<String> = if needs_token {
        let token_file = settings
            .publish_token_file
            .as_deref()
            .context(
                "`publish_token_file` not set in config; required for `--push` and `--seed-from`",
            )?;
        // Read + validate (existence, non-empty, sensible mode).
        Some(publish::read_publish_token(token_file).context("reading publish_token_file")?)
    } else {
        None
    };

    // Validation done. Begin filesystem work.
    eprintln!("sbagent init: target dir = {}", target_dir.display());

    // Step 0 (optional): seed `<publish_base_branch>` on the bot fork
    // (`base_repo_url`) from `--seed-from`. Required when the bot fork
    // is brand-new and doesn't yet carry the substrate branch. Skipped
    // when --seed-from is absent; in that case `git submodule add -b
    // <branch>` will fail loudly if the branch isn't there, which is
    // the operator's signal to either pass --seed-from or seed
    // manually.
    // Resolve forge-agnostic auth knobs once; they govern both the
    // `--seed-from` validation here and the `--push` validation below,
    // plus the `http.<prefix>.extraheader` config key used by the
    // PAT-via-env mechanism.
    let auth_username = settings.effective_git_auth_username();
    let auth_url_prefix = settings
        .effective_git_auth_url_prefix()
        .context("validating `git_auth_url_prefix` from config")?;
    if let Some(seed_url) = args.seed_from.as_deref() {
        // The seed step PUSHes to `base_repo_url` using the
        // PAT-via-extraheader mechanism, scoped to
        // `http.<auth_url_prefix>.extraheader`. If `base_repo_url`
        // is SSH or doesn't start with that prefix, git won't apply
        // the header and silently falls back to SSH/no auth — which is
        // not the contract --seed-from promises. Strict in all build
        // modes; tests exercise the underlying push mechanism by
        // calling `seed_branch` directly with `file://` URLs.
        validate_auth_url(base_repo_url, &auth_url_prefix, "`--seed-from`'s `base_repo_url`")?;
        let tok = token
            .as_deref()
            .expect("token already validated above when seed-from is set");
        seed_branch_with_auth(
            seed_url,
            base_repo_url,
            base_branch,
            tok,
            auth_username,
            &auth_url_prefix,
        )
        .with_context(|| format!("seeding {base_branch} on {base_repo_url} from {seed_url}"))?;
        eprintln!("  ✓ seeded {base_branch} on bot fork from {seed_url}");
    }

    // Step 1: `git init` if needed.
    let dot_git = target_dir.join(".git");
    if !dot_git.exists() {
        run_git(&target_dir, &["init", "-b", args.push_branch.as_str()]).context("git init")?;
        eprintln!("  ✓ git init (-b {})", args.push_branch);
    } else {
        eprintln!("  · git init: already initialized");
    }

    // Step 2: add the stacks-core submodule at `base`.
    let base_abs =
        if base_path.is_absolute() { base_path.to_path_buf() } else { target_dir.join(base_path) };
    let submodule_exists = base_abs.join(".git").exists()
        || base_abs
            .join("HEAD")
            .is_file();
    let base_rel = base_path
        .to_string_lossy()
        .into_owned();
    if !submodule_exists {
        run_git(
            &target_dir,
            &["submodule", "add", "-b", base_branch, base_repo_url, base_rel.as_str()],
        )
        .with_context(|| {
            format!("git submodule add {base_repo_url} {} (branch {base_branch})", base_rel)
        })?;
        eprintln!("  ✓ submodule add: {base_repo_url} → {base_rel} (-b {base_branch})");
    } else {
        eprintln!("  · submodule {base_rel}: already present");
    }

    // Step 3: add `bot` remote on the submodule, derived from
    // `publish_base_repo`. Best-effort — if the operator is on
    // upstream-mode (`publish_base_repo` already canonical), this
    // still adds a `bot` remote pointing at the bot's fork (which we
    // derive from `publish_head_owner`).
    if let Some(bot_url) = derive_bot_fork_url(settings) {
        let remote_exists = run_git_check(&base_abs, &["remote", "get-url", "bot"]);
        if !remote_exists {
            run_git(&base_abs, &["remote", "add", "bot", &bot_url])
                .with_context(|| format!("git -C {base_rel} remote add bot {bot_url}"))?;
            eprintln!("  ✓ submodule bot remote: {bot_url}");
        } else {
            eprintln!("  · submodule bot remote: already present");
        }
    }

    // Step 4: seed prompts.
    let prompts_abs = if prompts_dir.is_absolute() {
        prompts_dir.to_path_buf()
    } else {
        target_dir.join(prompts_dir)
    };
    let report = crate::prompts::seed_to(&prompts_abs)
        .with_context(|| format!("seeding bundled prompts into {}", prompts_abs.display()))?;
    eprintln!("  ✓ prompts seeded: {} written, {} kept", report.seeded.len(), report.kept.len(),);

    // Step 4b: seed schemas. Resolution goes through the shared
    // `settings::default_schemas_dir` helper so init's choice here
    // matches Layout's runtime choice byte-for-byte — preventing the
    // "init commits .sbagent/schemas, runtime uses schemas" drift
    // an operator with a bare-filename `prompt_overrides_dir` could
    // otherwise hit.
    let schemas_rel = crate::settings::default_schemas_dir(settings)
        .unwrap_or_else(|| std::path::PathBuf::from(".sbagent").join("schemas"));
    let schemas_abs =
        if schemas_rel.is_absolute() { schemas_rel.clone() } else { target_dir.join(&schemas_rel) };
    let schema_report = crate::schemas::seed_to(&schemas_abs)
        .with_context(|| format!("seeding bundled schemas into {}", schemas_abs.display()))?;
    eprintln!(
        "  ✓ schemas seeded: {} written, {} kept",
        schema_report.seeded.len(),
        schema_report.kept.len(),
    );

    // Step 4c: seed queries (triage + analyzer SQL bundle). Same
    // resolution + write semantics as schemas — operator's on-disk
    // mirror MUST byte-match the binary's bundle, refreshed by
    // `sbagent sync`, enforced by `sbagent check`.
    let queries_rel = crate::settings::default_queries_dir(settings)
        .unwrap_or_else(|| std::path::PathBuf::from(".sbagent").join("queries"));
    let queries_abs =
        if queries_rel.is_absolute() { queries_rel.clone() } else { target_dir.join(&queries_rel) };
    let queries_report = crate::queries::seed_to(&queries_abs)
        .with_context(|| format!("seeding bundled queries into {}", queries_abs.display()))?;
    eprintln!(
        "  ✓ queries seeded: {} written, {} kept",
        queries_report.seeded.len(),
        queries_report.kept.len(),
    );

    // Step 4d: seed context docs (operator-tunable reference material,
    // same drift contract as prompts — warn on drift, refreshed by
    // default by `sbagent sync` unless `--keep-tunables` is set).
    let context_rel = crate::settings::default_context_dir(settings)
        .unwrap_or_else(|| std::path::PathBuf::from(".sbagent").join("context"));
    let context_abs =
        if context_rel.is_absolute() { context_rel.clone() } else { target_dir.join(&context_rel) };
    let context_report = crate::context::seed_to(&context_abs)
        .with_context(|| format!("seeding bundled context docs into {}", context_abs.display()))?;
    eprintln!(
        "  ✓ context seeded: {} written, {} kept",
        context_report.seeded.len(),
        context_report.kept.len(),
    );

    // Step 5: drop a `.gitignore` template if absent.
    write_default_gitignore(&target_dir, base_path)?;

    // Step 6: commit. Stage ONLY the paths init owns — never `git add
    // -A` which would sweep unrelated pre-existing files into the
    // "chore: initial operator state" commit (e.g. an operator
    // running `sbagent init` inside a non-empty dir or an existing
    // git repo with local edits). The init-owned paths are:
    //   - `.gitignore` (we created it in step 5)
    //   - `.gitmodules` (git submodule add creates it)
    //   - `<base>` (the stacks-core submodule worktree)
    //   - `<prompt_overrides_dir>` (seeded prompts)
    //   - `<schemas_dir>` (seeded JSON Schemas)
    //   - `<queries_dir>` (seeded triage/analyzer SQL bundle)
    //   - `<context_dir>` (seeded reference docs)
    // We `git add -- <pathspec>` only paths that actually exist (a
    // re-run on an already-seeded dir might have some absent if the
    // operator deleted them in between).
    let env = optimizer_git_env(settings);
    let prompts_rel = prompts_dir
        .to_string_lossy()
        .into_owned();
    let schemas_rel_str = schemas_rel
        .to_string_lossy()
        .into_owned();
    let queries_rel_str = queries_rel
        .to_string_lossy()
        .into_owned();
    let context_rel_str = context_rel
        .to_string_lossy()
        .into_owned();
    let init_paths: &[&str] = &[
        ".gitignore",
        ".gitmodules",
        base_rel.as_str(),
        prompts_rel.as_str(),
        schemas_rel_str.as_str(),
        queries_rel_str.as_str(),
        context_rel_str.as_str(),
    ];
    match git::stage_and_commit(&target_dir, init_paths, "chore: initial operator state", &env)? {
        git::CommitOutcome::Committed => eprintln!("  ✓ initial commit"),
        git::CommitOutcome::NothingToCommit => {
            eprintln!("  · initial commit: nothing to commit (re-run on a clean tree)");
        }
    }

    // Step 7: optional push.
    if args.push {
        let tok = token
            .as_deref()
            .expect("token already validated above when --push is set");
        // Verify `origin` is configured before attempting the push.
        if !run_git_check(&target_dir, &["remote", "get-url", "origin"]) {
            bail!(
                "`--push` requires `origin` configured in {}. Run:\n  git -C {} remote add origin \
                 <bot-operator-repo-url>\nthen re-run with `--push`.",
                target_dir.display(),
                target_dir.display(),
            );
        }
        // Validate `origin` matches the configured `git_auth_url_prefix`
        // (default `https://github.com/`). The PAT-via-extraheader
        // mechanism only kicks in for URLs git recognizes as matching
        // the prefix. SSH remotes (or HTTPS URLs against a different
        // host than the configured prefix) would silently ignore the
        // injected header and fall back to SSH key / prompted auth —
        // which is not the contract `--push` promises. Better to fail
        // up-front with a clear message than produce a confusing "no
        // SSH key" / "credentials prompt" error from git.
        let origin_url = run_git_output(&target_dir, &["remote", "get-url", "origin"])
            .context("reading origin URL")?;
        validate_auth_url(&origin_url, &auth_url_prefix, "`--push`'s `origin`")?;
        push_with_pat(
            &target_dir,
            "origin",
            &args.push_branch,
            tok,
            &env,
            auth_username,
            &auth_url_prefix,
        )
        .context("git push -u origin via PAT header")?;
        eprintln!("  ✓ pushed origin {}", args.push_branch);
    } else {
        eprintln!();
        eprintln!("Done. To push the initial commit to GitHub:");
        eprintln!("  git -C {} remote add origin <bot-operator-url>", target_dir.display());
        eprintln!("  git -C {} push -u origin {}", target_dir.display(), args.push_branch);
        eprintln!("(or re-run `sbagent init --push` after `git remote add origin <url>`.)");
    }

    Ok(())
}

/// Derive the bot's stacks-core fork URL from `publish_head_owner` (or
/// fall back to `publish_base_repo` if it looks like the bot's own
/// repo). Returns `None` when we can't infer one — caller treats that
/// as "skip the bot remote setup, operator can add it manually."
fn derive_bot_fork_url(settings: &Settings) -> Option<String> {
    // Prefer `publish_head_owner` (explicit "this is the bot"); fall
    // back to `publish_base_repo` for pilot mode where head + base are
    // both the bot's fork.
    settings
        .publish_head_owner
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|owner| format!("https://github.com/{owner}/stacks-core.git"))
        .or_else(|| {
            settings
                .publish_base_repo
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|slug| format!("https://github.com/{slug}.git"))
        })
}

/// Write a `.gitignore` template at the target dir if one isn't
/// already there. Covers the submodule's `target/`, mutable agent
/// scratch state, and editor/OS noise.
fn write_default_gitignore(target_dir: &Path, base_path: &Path) -> Result<()> {
    let gitignore = target_dir.join(".gitignore");
    if gitignore.exists() {
        eprintln!("  · .gitignore: already present");
        return Ok(());
    }
    let base_rel = base_path
        .to_string_lossy()
        .into_owned();
    let body = format!(
        "# Defensive: not auto-loaded (config lives at ~/.config/sbagent/config.toml),\n# but \
         ignore any in-tree `config.toml` so legacy or `-c config.toml`\n# convenience copies \
         don't end up committed.\n/config.toml\n\n# stacks-core build artifacts inside the \
         submodule.\n{base_rel}/target/\n\n# Local-only mutable session state (worktrees from \
         older\n# sbagent versions). Agent scratch state lives under\n# `agent_workspace_root` in \
         /private/tmp/... by default,\n# but old layouts kept it \
         here.\n/sessions/*/worktrees/\n\n# Editor / OS \
         noise.\n.DS_Store\n.idea/\n.vscode/\n*.swp\n"
    );
    std::fs::write(&gitignore, body).with_context(|| format!("writing {}", gitignore.display()))?;
    eprintln!("  ✓ .gitignore (default)");
    Ok(())
}

/// Convenience wrapper used by tests + ad-hoc callers: applies the
/// default `x-access-token` + `https://github.com/` auth knobs. Real
/// operator runs go through [`seed_branch_with_auth`] via [`run`] so
/// the `git_auth_username` / `git_auth_url_prefix` settings flow
/// through end-to-end.
pub fn seed_branch(source_url: &str, dest_url: &str, branch: &str, token: &str) -> Result<()> {
    seed_branch_with_auth(
        source_url,
        dest_url,
        branch,
        token,
        crate::settings::DEFAULT_GIT_AUTH_USERNAME,
        crate::settings::DEFAULT_GIT_AUTH_URL_PREFIX,
    )
}

/// Seed `branch` on the bot fork (`dest_url`) from `source_url`.
///
/// Used by `sbagent init --seed-from <source_url>` when the bot's
/// stacks-core fork is brand-new and doesn't yet carry the configured
/// `publish_base_branch`. Without this step, `git submodule add -b
/// <branch>` against the bot fork would fail.
///
/// Mechanism:
///
///   1. `git clone --bare --branch <branch> <source_url>` into a tempdir.
///      Source is typically the human operator's public fork, so no auth is
///      needed for the read.
///   2. `git -C <tmpdir> push <dest_url> <branch>:refs/heads/<branch>` with the
///      PAT injected via the same `http.extraheader` env-var mechanism used by [`push_with_pat`].
///      Token never lands in `.git/config`, argv, or shell history. Auth header
///      is suppressed when `dest_url` is not `https://...` — `file://` and SSH
///      dests get the bare-clone push with no env injection, so tests that use
///      local URLs don't need a real PAT.
///   3. Tempdir is dropped on exit (RAII via `tempfile::TempDir`).
///
/// Re-runs are safe: if the bot fork already carries the same branch
/// at the same SHA, the push is a fast-forward no-op. If the bot fork
/// has diverged, the push fails (non-fast-forward); operator can pass
/// `--seed-from` with a different source or seed manually.
pub fn seed_branch_with_auth(
    source_url: &str,
    dest_url: &str,
    branch: &str,
    token: &str,
    auth_username: &str,
    auth_url_prefix: &str,
) -> Result<()> {
    let tmp = tempfile::tempdir().context("creating tempdir for seed bare clone")?;
    let bare = tmp.path().join("seed.git");
    crate::git::clone_bare_branch(source_url, branch, &bare)?;

    // Build env for the dest push. The auth header is only meaningful
    // for HTTPS dests — git ignores `http.<prefix>.extraheader` for
    // file:// and SSH URLs, but we skip the env injection entirely so
    // tests against `file://` URLs don't need a real PAT and the env
    // shape stays minimal in non-HTTPS code paths.
    let env = build_auth_header_env(dest_url, token, auth_username, auth_url_prefix);
    crate::git::push_url_refspec(&bare, dest_url, &format!("{branch}:refs/heads/{branch}"), &env)
        .with_context(|| {
            "push to bot fork failed; either the PAT lacks Contents:write on that repo, or the bot \
             fork has diverged and a non-fast-forward push was rejected"
        })
}
