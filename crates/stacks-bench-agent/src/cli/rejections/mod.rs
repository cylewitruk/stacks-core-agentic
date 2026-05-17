//! `sbagent rejections` — operate on the cross-session analyzed-
//! rejections ledger.
//!
//! Two distinct caller classes:
//!
//! - **Agents (triage)** call `sbagent rejections probe ...` once per candidate
//!   they're considering promoting. Exit 0 + JSON on hit; exit 0 + empty stdout
//!   on no hit. Hard skip on hit.
//! - **Coordinators** call `sbagent rejections append ...` after each analyzer
//!   subagent that returned `status: rejected` or `accepted +
//!   lens_disposition.status: not_actionable`.
//! - **Operators** use `list` / `show` / `search` / `render` / `remove` /
//!   `trim` / `fingerprint` directly for inspection and housekeeping.
//!
//! All commands route through [`crate::analyzed_rejections`] for
//! file I/O + lock acquisition; this module is purely arg parsing +
//! output formatting.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;

pub mod append;
pub mod fingerprint;
pub mod list;
pub mod probe;
pub mod remove;
pub mod render;
pub mod search;
pub mod show;
pub mod trim;

/// Args for `sbagent rejections`.
#[derive(Debug, Args)]
pub struct RejectionsArgs {
    /// The subcommand to run.
    #[clap(subcommand)]
    pub command: RejectionsCommand,
}

/// `sbagent rejections` subcommands.
#[derive(Debug, Subcommand)]
pub enum RejectionsCommand {
    /// Probe the ledger for entries matching the given fingerprint
    /// inputs. Exit 0 with matching JSON on stdout when found;
    /// exit 0 with empty stdout when no match. The agent's primary
    /// per-candidate check.
    Probe(probe::ProbeArgs),

    /// Append a new rejection record to the ledger. Called by the
    /// coordinator after each analyzer-rejection outcome; also
    /// available to operators for manual entries.
    Append(append::AppendArgs),

    /// List ledger entries with optional filtering. One line per
    /// record by default; `--json` for piping.
    List(list::ListArgs),

    /// Show full record(s) for a given family id or fingerprint.
    Show(show::ShowArgs),

    /// Search ledger entries by structured fields (spans, kind,
    /// contract, reason text).
    Search(search::SearchArgs),

    /// Render the ledger as a human-readable markdown view.
    Render(render::RenderArgs),

    /// Remove entries matching a family id or fingerprint
    /// (operator's escape hatch to re-enable a previously-rejected
    /// family for future triage).
    Remove(remove::RemoveArgs),

    /// Trim accumulated history (keep newest N per family, drop
    /// entries from before a date or commit SHA).
    Trim(trim::TrimArgs),

    /// Compute the canonical fingerprint for given lens/kind/spans
    /// inputs. Operator-debug helper; agents don't need it because
    /// `probe` computes the fingerprint internally.
    Fingerprint(fingerprint::FingerprintArgs),
}

/// Run a `sbagent rejections` subcommand.
pub async fn run(args: RejectionsArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        RejectionsCommand::Probe(a) => probe::run(a, ctx),
        RejectionsCommand::Append(a) => append::run(a, ctx),
        RejectionsCommand::List(a) => list::run(a, ctx),
        RejectionsCommand::Show(a) => show::run(a, ctx),
        RejectionsCommand::Search(a) => search::run(a, ctx),
        RejectionsCommand::Render(a) => render::run(a, ctx),
        RejectionsCommand::Remove(a) => remove::run(a, ctx),
        RejectionsCommand::Trim(a) => trim::run(a, ctx),
        RejectionsCommand::Fingerprint(a) => fingerprint::run(a, ctx),
    }
}
