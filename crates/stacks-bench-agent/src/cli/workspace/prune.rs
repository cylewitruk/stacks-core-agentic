//! `sbagent workspace prune` — remove stale per-session scratch dirs.
//!
//! See [`crate::session::workspace`] for the underlying decision
//! matrix. This module only handles arg parsing + the production
//! wiring (sessions root, ledger path, real liveness probe).

use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::session::run_pid;
use crate::session::workspace::{
    self, LivenessProbe, PruneInputs, PruneOptions, parse_duration, print_report,
};

/// Args for `sbagent workspace prune`.
#[derive(Debug, Args)]
pub struct PruneArgs {
    /// Only prune sessions older than this duration. Format: `<n><s|m|h|d|w>`,
    /// e.g. `7d`, `48h`, `30m`.
    #[clap(long, value_name = "DURATION")]
    pub older_than: Option<String>,

    /// Only prune sessions present in operator-main `sessions.jsonl`
    /// (terminal/archived). Required-by-default safety filter: without
    /// either `--archived-only` or `--older-than`, every candidate is
    /// kept and the command effectively reports state without removing.
    #[clap(long)]
    pub archived_only: bool,

    /// Print decisions, do not remove anything. Implicit when no
    /// filter is set.
    #[clap(long)]
    pub dry_run: bool,
}

/// Dispatch — resolve durable signals, run the prune, print the report.
pub fn run(args: PruneArgs, ctx: &CliContext) -> Result<()> {
    let sessions_root: PathBuf = ctx
        .layout
        .sessions_root
        .clone();
    let operator_ledger = ctx
        .layout
        .operator_repo_root
        .as_ref()
        .map(|repo| repo.join("sessions.jsonl"));
    let older_than = args
        .older_than
        .as_deref()
        .map(parse_duration)
        .transpose()
        .context("parsing --older-than")?;

    // Implicit dry-run when no filter is set keeps the destructive
    // path explicit, matching the iteration contract.
    let dry_run = args.dry_run || (!args.archived_only && older_than.is_none());

    let options = PruneOptions {
        older_than,
        archived_only: args.archived_only,
        dry_run,
    };
    let liveness: LivenessProbe = run_pid::is_live;
    let report = workspace::prune(&PruneInputs {
        sessions_root: &sessions_root,
        operator_ledger: operator_ledger.as_deref(),
        options,
        now: SystemTime::now(),
        liveness,
    })
    .context("running workspace prune")?;
    print_report(&report, &sessions_root);
    Ok(())
}
