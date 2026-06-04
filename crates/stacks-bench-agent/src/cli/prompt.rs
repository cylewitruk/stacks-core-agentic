//! `sbagent prompt ...` — prompt-template lifecycle commands.
//!
//! Two subcommands:
//!
//! - `lint` — parses every on-disk template with MiniJinja strict mode and
//!   dry-renders each against a field-complete synthetic context. Surfaces
//!   parse errors, unknown variable references, and missing reference docs
//!   before an agent ever consumes the prompt. Exit non-zero on findings.
//! - `sync --force` — overwrites every on-disk template with the bundled
//!   defaults. Destructive (clobbers operator edits); requires `--force`.
//!
//! Both operate on the directory at `settings.layout.prompt_overrides_dir` (or
//! whatever is passed via `--dir`). `sbagent` startup seeds this dir if
//! it's set in config, so lint normally has files to inspect.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::prompts;

/// `sbagent prompt ...`.
#[derive(Debug, Args)]
pub struct PromptArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: PromptCommand,
}

/// `sbagent prompt` subcommands.
#[derive(Debug, Subcommand)]
pub enum PromptCommand {
    /// Validate every on-disk template against its prompt struct.
    Lint(LintArgs),
    /// Force-sync every template from the bundled defaults (destructive —
    /// overwrites operator edits).
    Sync(SyncArgs),
}

/// Args for `sbagent prompt lint`.
#[derive(Debug, Args)]
pub struct LintArgs {
    /// Override the prompts dir to lint. Defaults to
    /// `settings.layout.prompt_overrides_dir`.
    #[clap(long)]
    pub dir: Option<PathBuf>,
}

/// Args for `sbagent prompt sync`.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Override the destination dir. Defaults to
    /// `settings.layout.prompt_overrides_dir`.
    #[clap(long)]
    pub dir: Option<PathBuf>,

    /// Required acknowledgement that this overwrites operator edits.
    #[clap(long)]
    pub force: bool,
}

/// Dispatch for `sbagent prompt ...`.
pub async fn run(args: PromptArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        PromptCommand::Lint(a) => run_lint(a, ctx),
        PromptCommand::Sync(a) => run_sync(a, ctx),
    }
}

fn run_lint(args: LintArgs, ctx: &CliContext) -> Result<()> {
    let dir = match args.dir {
        Some(p) => p,
        None => ctx
            .settings
            .require_prompt_overrides_dir()?
            .to_path_buf(),
    };
    let findings = prompts::lint(&dir).context("running prompt lint")?;
    if findings.is_empty() {
        println!("prompt lint: OK ({})", dir.display());
        Ok(())
    } else {
        eprintln!("prompt lint: {} finding(s) in {}", findings.len(), dir.display());
        for f in &findings {
            eprintln!("  - {}: {}", f.template, f.message);
        }
        anyhow::bail!("prompt lint failed with {} finding(s)", findings.len())
    }
}

fn run_sync(args: SyncArgs, ctx: &CliContext) -> Result<()> {
    if !args.force {
        anyhow::bail!(
            "`sbagent prompt sync` is destructive (overwrites operator edits); pass `--force` to \
             acknowledge",
        );
    }
    let dir = match args.dir {
        Some(p) => p,
        None => ctx
            .settings
            .require_prompt_overrides_dir()?
            .to_path_buf(),
    };
    let written = prompts::sync_force(&dir).context("running prompt sync")?;
    println!("prompt sync: rewrote {} template(s) in {}", written.len(), dir.display());
    for name in &written {
        println!("  - {name}");
    }
    Ok(())
}
