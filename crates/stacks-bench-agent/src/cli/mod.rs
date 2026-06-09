//! Top-level CLI argument parsing and dispatch.
//!
//! The CLI is intentionally shallow at this layer: each subcommand module
//! owns both its `clap`-derived arg struct and the `run` function that
//! executes it. The dispatch function in this module just routes.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};

use crate::layout::Layout;
use crate::settings::Settings;

pub mod check;
pub mod init;
pub mod preflight;
pub mod prompt;
pub mod publish;
pub mod rejections;
pub mod schema;
pub mod session;
pub mod source;
pub mod sync;
pub mod workspace;

/// Primary CLI entry point for the stacks-bench-agent, responsible for parsing
/// command-line arguments and initializing the CLI context.
#[derive(Debug, Parser)]
#[command(author, version, about)]
pub struct CliArgs {
    /// Path to the TOML configuration file. When unset, resolution
    /// falls through to `$XDG_CONFIG_HOME/sbagent/config.toml`, then
    /// `~/.config/sbagent/config.toml`. See
    /// [`crate::settings::Settings::load`].
    #[clap(long, short = 'c', global = true)]
    pub config_path: Option<PathBuf>,

    /// The subcommand to run.
    #[clap(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Verify the local environment is ready to run a session: required tools
    /// installed, agent harnesses compatible, and committed JSON Schemas in
    /// sync with the typed models.
    Check(check::CheckArgs),

    /// One-shot bootstrap for a fresh operator directory: seed bundled
    /// prompt / schema / query / context templates, drop a
    /// `.gitignore`, and produce an initial commit authored as the
    /// bot. Optionally push with `--push` (PAT-via-env auth, no
    /// `.git/config` mutation). No source submodule is added — source
    /// repos are materialized per-session from `[source]` under
    /// `<workspace>/sessions/<id>/repos/<cache_id>/`.
    Init(init::InitArgs),

    /// Prompt-template lifecycle: lint disk-loaded templates, force-sync
    /// from bundled defaults.
    Prompt(prompt::PromptArgs),

    /// Publish PRs / issues for a session's accepted artifacts. Stub.
    Publish(publish::PublishArgs),

    /// Operate on the cross-session analyzed-rejections ledger
    /// (probe / append / list / show / search / render / remove /
    /// trim / fingerprint). Agents call `probe`; coordinators call
    /// `append`; operators use the rest for inspection and
    /// housekeeping.
    Rejections(rejections::RejectionsArgs),

    /// Schema-related commands (export typed models to JSON Schema files).
    Schema(schema::SchemaArgs),

    /// Per-session commands (validate, finalize, baseline, triage, ...).
    Session(session::SessionArgs),

    /// Source-clone helpers (read-only). `cache-id` prints the
    /// deterministic id used to name `<workspace>/cache/<id>.git`; the
    /// operator migration recipe in `docs/setup.md` calls it.
    Source(source::SourceArgs),

    /// Refresh the operator's on-disk bundle from the running
    /// binary's embedded defaults. By default rewrites ALL bundles
    /// (`.sbagent/{schemas,queries,prompts,context}/`); pass
    /// `--keep-tunables` to preserve operator-edited prompts +
    /// context docs (still refreshes schemas + queries).
    Sync(sync::SyncArgs),

    /// Workspace hygiene — prune stale per-session scratch.
    Workspace(workspace::WorkspaceArgs),
}

/// Context for the CLI; passed through various CLI commands and subcommands to
/// provide access to shared resources like the working directory.
pub struct CliContext {
    /// Parsed settings (loaded from `config.toml`).
    pub settings: Settings,
    /// Resolved on-disk layout (framework dir + data dir).
    pub layout: Layout,
}

impl CliContext {
    /// Construct a context from the parsed CLI args. Loads settings, then
    /// derives the on-disk layout from them. If
    /// `settings.layout.prompt_overrides_dir` is set, seeds bundled
    /// templates into it with don't-replace-if-exists semantics so a fresh
    /// operator gets a working baseline while a tuned operator keeps their
    /// edits. Missing `prompt_overrides_dir` is not an error here —
    /// render-time helpers surface that with a clearer message at the
    /// appropriate phase.
    pub fn from_args(args: &CliArgs) -> Result<Self> {
        let settings = Settings::load(args.config_path.as_deref())?;
        let layout = Layout::from_settings(&settings)?;
        if let Some(dir) = settings
            .layout
            .prompt_overrides_dir
            .as_deref()
        {
            let report = crate::prompts::seed_to(dir)?;
            if !report.seeded.is_empty() {
                tracing::info!(
                    seeded = ?report.seeded,
                    kept = ?report.kept,
                    dir = %dir.display(),
                    "seeded bundled prompt templates into operator dir",
                );
            }
        }
        // Seed the bundled JSON Schemas to the resolved on-disk mirror
        // (`Layout::schemas_dir`) with don't-replace semantics, matching
        // the prompt-seeding behavior. `sbagent check` enforces that
        // operator-on-disk schemas byte-match the bundle; operators who
        // want to refresh stale schemas after an `sbagent` upgrade run
        // `sbagent sync`, which overwrites unconditionally.
        //
        // Exception: `sbagent check` itself MUST observe disk state
        // verbatim, including missing files. Auto-seeding before
        // `check` would mask the `DriftEntry::Missing` branch (a
        // deliberately-deleted schema would be silently restored
        // and `check` would report OK), contradicting the contract.
        // `check` callers expecting auto-heal can run `sbagent sync`
        // first.
        // Legacy context-doc migration is a one-time layout fix; it
        // runs on every command (including `check`) so an operator who
        // upgrades sbagent and runs `sbagent check` first doesn't see a
        // misleading "missing on disk" drift report for files that
        // would be auto-relocated the next time they ran anything else.
        migrate_legacy_context_docs(&settings, &layout)?;
        if !matches!(args.command, Command::Check(_)) {
            let report = crate::schemas::seed_to(&layout.schemas_dir)?;
            if !report.seeded.is_empty() {
                tracing::info!(
                    seeded = ?report.seeded,
                    kept = ?report.kept,
                    dir = %layout.schemas_dir.display(),
                    "seeded bundled JSON Schemas into operator dir",
                );
            }
            // Same skip-on-check rationale applies to queries: `check`
            // must observe disk state verbatim so a deleted SQL file
            // surfaces as `DriftEntry::Missing` rather than getting
            // silently restored before the drift gate sees it.
            let report = crate::queries::seed_to(&layout.queries_dir)?;
            if !report.seeded.is_empty() {
                tracing::info!(
                    seeded = ?report.seeded,
                    kept = ?report.kept,
                    dir = %layout.queries_dir.display(),
                    "seeded bundled SQL queries into operator dir",
                );
            }
            // Context docs are operator-tunable like prompts (warn on
            // drift, not fail), but `seed_to` is still don't-replace so
            // a fresh operator gets a working baseline. Migration
            // already ran above outside this block; by the time we
            // get here, any legacy files have been relocated and
            // seed only fills in what's still missing.
            let report = crate::context::seed_to(&layout.context_dir)?;
            if !report.seeded.is_empty() {
                tracing::info!(
                    seeded = ?report.seeded,
                    kept = ?report.kept,
                    dir = %layout.context_dir.display(),
                    "seeded bundled context docs into operator dir",
                );
            }
        }
        Ok(Self { settings, layout })
    }
}

/// One-time migration of context docs from the legacy `prompts/`
/// location to the new `context/` bundle. For each bundled context
/// file: if `<prompts>/<file>` exists AND `<context>/<file>` doesn't,
/// move the prompts copy to context. Preserves any operator edits via
/// `rename` (atomic on the same fs; falls back to copy+remove). Warns
/// to stderr when files moved so the operator notices the layout
/// change.
///
/// Skipped silently when `prompt_overrides_dir` isn't configured (no
/// legacy location to migrate from). Idempotent.
fn migrate_legacy_context_docs(settings: &Settings, layout: &Layout) -> Result<()> {
    let prompts_dir = match settings
        .layout
        .prompt_overrides_dir
        .as_deref()
    {
        Some(d) => d,
        None => return Ok(()),
    };
    let mut migrated: Vec<String> = Vec::new();
    for name in crate::context::bundled_file_names() {
        let legacy = prompts_dir.join(name);
        let new_path = layout.context_dir.join(name);
        if !legacy.is_file() || new_path.exists() {
            continue;
        }
        std::fs::create_dir_all(&layout.context_dir)
            .with_context(|| format!("creating context dir {}", layout.context_dir.display()))?;
        match std::fs::rename(&legacy, &new_path) {
            Ok(()) => migrated.push(name.to_owned()),
            Err(_) => {
                std::fs::copy(&legacy, &new_path).with_context(|| {
                    format!("copying {} → {}", legacy.display(), new_path.display())
                })?;
                std::fs::remove_file(&legacy)
                    .with_context(|| format!("removing legacy {}", legacy.display()))?;
                migrated.push(name.to_owned());
            }
        }
    }
    if !migrated.is_empty() {
        eprintln!(
            "sbagent: migrated {} legacy context file(s) from {} to {}:",
            migrated.len(),
            prompts_dir.display(),
            layout.context_dir.display(),
        );
        for name in &migrated {
            eprintln!("  - {name}");
        }
        eprintln!(
            "sbagent: reference docs (`non-targets.md`, `bucket-anchors.md`, \
             `stacks-domain-context.md`) now live under the operator's `context/` dir alongside \
             per-doc `.toml` sidecars. Any local edits were preserved.",
        );
    }
    Ok(())
}

/// Parse-then-route the top-level command. Called once from `main`.
pub async fn dispatch(args: CliArgs) -> Result<()> {
    // `init` runs BEFORE building the full `CliContext` — the whole
    // point of init is to create the operator directory (`.git/`,
    // prompts dir, .sbagent/ bundles, etc.). Going through
    // `CliContext::from_args` here would try to materialize a layout
    // against an empty target dir and fail. Init loads Settings
    // directly and does its own setup work.
    if let Command::Init(ref a) = args.command {
        let settings = Settings::load(args.config_path.as_deref())?;
        return init::run(a.clone(), &settings).await;
    }
    let ctx = CliContext::from_args(&args)?;
    match args.command {
        Command::Check(a) => check::run(a, &ctx).await,
        Command::Init(_) => unreachable!("handled before CliContext build"),
        Command::Prompt(a) => prompt::run(a, &ctx).await,
        Command::Publish(a) => publish::run(a, &ctx).await,
        Command::Rejections(a) => rejections::run(a, &ctx).await,
        Command::Schema(a) => schema::run(a, &ctx).await,
        Command::Session(a) => session::run(a, &ctx).await,
        Command::Source(a) => source::run(a, &ctx).await,
        Command::Sync(a) => sync::run(a, &ctx).await,
        Command::Workspace(a) => workspace::run(a, &ctx).await,
    }
}
