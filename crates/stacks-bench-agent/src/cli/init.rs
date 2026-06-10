//! `sbagent init` — one-shot bootstrap for a fresh operator directory.
//!
//! Bootstrap order:
//!
//! 1. Validate the loaded [`Settings`] has the fields `init` needs
//!    (`layout.prompt_overrides_dir`). If `--push` is set, also
//!    `publish.token_file` (push relies on the bot's PAT to authenticate
//!    against GitHub).
//! 2. Resolve the target dir (`--dir` or cwd). Either the dir is empty or it's
//!    an existing repo with `.git/` already configured; the bootstrap is a
//!    no-op in the second case for the parts that already exist (`init` is
//!    loosely idempotent for re-runs).
//! 3. `git init` if no `.git/` is present.
//! 4. Seed `.sbagent/{prompts,schemas,queries,context}/` via the bundle
//!    `seed_to` helpers. Don't-replace semantics so re-runs preserve operator
//!    tunes. Default layout: `<layout.prompt_overrides_dir>` (typically
//!    `.sbagent/prompts/`) + the sibling subdirs derived from its parent when
//!    the corresponding `layout.*_dir` is unset.
//! 5. Drop a `.gitignore` with the conventional excludes (defensive
//!    `/config.toml`, editor noise).
//! 6. Stage ONLY the init-owned paths (`.gitignore`,
//!    `<layout.prompt_overrides_dir>`, `<layout.schemas_dir>`,
//!    `<layout.queries_dir>`, `<context_dir>`) — never `git add -A` — then
//!    commit "chore: initial operator state" authored as the bot via
//!    [`crate::session::optimizers::optimizer_git_env`]. Stray pre-existing
//!    files in the target dir survive as untracked. Skipped if nothing was
//!    staged (re-run safe).
//! 7. If `--push`: read PAT via
//!    [`crate::session::publish::read_publish_token`], then
//!    `git push -u origin <branch>` using `http.<prefix>.extraheader`
//!    via env-vars for one invocation only. `origin` must start with
//!    the configured `git.auth_url_prefix` (default
//!    `https://github.com/`). The token never lands on disk.
//!
//! Per-session source clones are materialized at session start from
//! `[source]` (see [`crate::source`]); init does not touch source repo
//! state at all. No submodule is added, no `.gitmodules` is written.
//!
//! `init` is filesystem-only. It does NOT create the GitHub repo, the
//! bot fork, or the PAT — those stay manual.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::git::{self, push_with_pat, run_git, run_git_check, run_git_output, validate_auth_url};
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

    /// After the initial commit, push to `origin` using the bot's PAT
    /// (read from `publish.token_file`). Requires that `origin` already
    /// be configured in the target directory (either by `git clone`'ing
    /// the empty bot repo first, or by `git remote add origin <url>`).
    ///
    /// Auth uses an `http.<git.auth_url_prefix>.extraheader` injected
    /// via `GIT_CONFIG_COUNT` env-vars for one invocation only — the
    /// token never lands in `.git/config`, in the remote URL, or in
    /// shell history. The `origin` URL MUST start with the configured
    /// `git.auth_url_prefix` (default `https://github.com/`); SSH or
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
    // bundle seeding.
    let prompts_dir = settings
        .layout
        .prompt_overrides_dir
        .as_deref()
        .context(
            "`layout.prompt_overrides_dir` not set in config; required for init (where bundled \
             prompt defaults get seeded)",
        )?;

    // If --push was set, validate the token NOW. Failing fast is
    // cheaper than bootstrapping and then hitting an auth error
    // mid-flow.
    let token: Option<String> = if args.push {
        let token_file = settings
            .publish
            .token_file_required()
            .context("`sbagent init --push`")?;
        Some(publish::read_publish_token(token_file)?)
    } else {
        None
    };

    // Validation done. Begin filesystem work.
    eprintln!("sbagent init: target dir = {}", target_dir.display());

    // Resolve forge-agnostic auth knobs once; they govern the `--push`
    // validation below plus the `http.<prefix>.extraheader` config key
    // used by the PAT-via-env mechanism.
    let auth_username = settings
        .git
        .effective_auth_username();
    let auth_url_prefix = settings
        .git
        .effective_auth_url_prefix()?;

    // Step 1: `git init` if needed.
    let dot_git = target_dir.join(".git");
    if !dot_git.exists() {
        run_git(&target_dir, &["init", "-b", args.push_branch.as_str()]).context("git init")?;
        eprintln!("  ✓ git init (-b {})", args.push_branch);
    } else {
        eprintln!("  · git init: already initialized");
    }

    // Step 2: seed prompts.
    let prompts_abs = if prompts_dir.is_absolute() {
        prompts_dir.to_path_buf()
    } else {
        target_dir.join(prompts_dir)
    };
    let report = crate::prompts::seed_to(&prompts_abs)
        .with_context(|| format!("seeding bundled prompts into {}", prompts_abs.display()))?;
    eprintln!("  ✓ prompts seeded: {} written, {} kept", report.seeded.len(), report.kept.len(),);

    // Step 2b: seed schemas. Resolution goes through the shared
    // `settings::default_schemas_dir` helper so init's choice here
    // matches Layout's runtime choice byte-for-byte — preventing the
    // "init commits .sbagent/schemas, runtime uses schemas" drift
    // an operator with a bare-filename `layout.prompt_overrides_dir` could
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

    // Step 2c: seed queries (triage + analyzer SQL bundle). Same
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

    // Step 2d: seed context docs (operator-tunable reference material,
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

    // Step 3: drop a `.gitignore` template if absent.
    write_default_gitignore(&target_dir)?;

    // Step 4: commit. Stage ONLY the paths init owns — never `git add
    // -A` which would sweep unrelated pre-existing files into the
    // "chore: initial operator state" commit (e.g. an operator
    // running `sbagent init` inside a non-empty dir or an existing
    // git repo with local edits). The init-owned paths are:
    //   - `.gitignore` (we created it in step 3)
    //   - `<layout.prompt_overrides_dir>` (seeded prompts)
    //   - `<layout.schemas_dir>` (seeded JSON Schemas)
    //   - `<layout.queries_dir>` (seeded triage/analyzer SQL bundle)
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

    // Step 5: optional push.
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
        // Validate `origin` matches the configured `git.auth_url_prefix`
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

/// Write a `.gitignore` template at the target dir if one isn't
/// already there. Covers editor / OS noise plus a defensive ignore on
/// an in-tree `config.toml` (config is loaded from
/// `~/.config/sbagent/config.toml` — any in-tree copy is a stale
/// convenience and shouldn't be committed).
fn write_default_gitignore(target_dir: &Path) -> Result<()> {
    let gitignore = target_dir.join(".gitignore");
    if gitignore.exists() {
        eprintln!("  · .gitignore: already present");
        return Ok(());
    }
    let body = "# Defensive: not auto-loaded (config lives at ~/.config/sbagent/config.toml),\n# \
                but ignore any in-tree `config.toml` so legacy or `-c config.toml`\n# convenience \
                copies don't end up committed.\n/config.toml\n\n# Editor / OS \
                noise.\n.DS_Store\n.idea/\n.vscode/\n*.swp\n";
    std::fs::write(&gitignore, body).with_context(|| format!("writing {}", gitignore.display()))?;
    eprintln!("  ✓ .gitignore (default)");
    Ok(())
}
