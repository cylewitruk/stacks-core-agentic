//! Top-level CLI argument parsing and dispatch.
//!
//! The CLI is intentionally shallow at this layer: each subcommand module
//! owns both its `clap`-derived arg struct and the `run` function that
//! executes it. The dispatch function in this module just routes.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::layout::Layout;
use crate::settings::Settings;

pub mod check;
pub mod init;
pub mod preflight;
pub mod prompt;
pub mod publish;
pub mod schema;
pub mod session;
pub mod sync;

/// Primary CLI entry point for the stacks-bench-agent, responsible for parsing
/// command-line arguments and initializing the CLI context.
#[derive(Debug, Parser)]
#[command(author, version, about)]
pub struct CliArgs {
    /// Path to the TOML configuration file. When unset, defaults to
    /// `./config.toml` if present; otherwise falls back to per-command
    /// defaults.
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

    /// One-shot bootstrap for a fresh operator directory: add the
    /// stacks-core submodule, seed bundled prompt templates, drop a
    /// `.gitignore`, and produce an initial commit authored as the bot.
    /// Optionally push with `--push` (PAT-via-env auth, no
    /// `.git/config` mutation).
    Init(init::InitArgs),

    /// Prompt-template lifecycle: lint disk-loaded templates, force-sync
    /// from bundled defaults.
    Prompt(prompt::PromptArgs),

    /// Publish PRs / issues for a session's accepted artifacts. Stub.
    Publish(publish::PublishArgs),

    /// Schema-related commands (export typed models to JSON Schema files).
    Schema(schema::SchemaArgs),

    /// Per-session commands (validate, finalize, baseline, triage, ...).
    Session(session::SessionArgs),

    /// Refresh the operator's on-disk bundle (`.sbagent/schemas/`
    /// always; `.sbagent/prompts/` with `--force-prompts`) from the
    /// running binary's embedded defaults.
    Sync(sync::SyncArgs),
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
    /// derives the on-disk layout from them. If `settings.prompt_overrides_dir`
    /// is set, seeds bundled templates into it with
    /// don't-replace-if-exists semantics so a fresh operator gets a
    /// working baseline while a tuned operator keeps their edits. Missing
    /// `prompt_overrides_dir` is not an error here — render-time helpers
    /// surface that with a clearer message at the appropriate phase.
    pub fn from_args(args: &CliArgs) -> Result<Self> {
        let settings = Settings::load(args.config_path.as_deref())?;
        let layout = Layout::from_settings(&settings)?;
        if let Some(dir) = settings
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
        }
        Ok(Self { settings, layout })
    }
}

/// Parse-then-route the top-level command. Called once from `main`.
pub async fn dispatch(args: CliArgs) -> Result<()> {
    // `init` runs BEFORE building the full `CliContext` — the whole
    // point of init is to create the operator directory (with its
    // submodule, prompts dir, layout dirs, etc.). Going through
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
        Command::Schema(a) => schema::run(a, &ctx).await,
        Command::Session(a) => session::run(a, &ctx).await,
        Command::Sync(a) => sync::run(a, &ctx).await,
    }
}
