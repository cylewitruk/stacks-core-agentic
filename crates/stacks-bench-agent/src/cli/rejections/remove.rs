//! `sbagent rejections remove` — operator escape hatch to re-enable
//! a previously-rejected family for the next triage session.

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::analyzed_rejections;
use crate::cli::CliContext;

/// Args for `sbagent rejections remove`.
#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// Family id to remove (drops every record with this exact
    /// family_id). Mutually exclusive with `--fingerprint`.
    pub family_id: Option<String>,
    /// Fingerprint to remove (drops every record with this exact
    /// fingerprint). Mutually exclusive with the positional family_id
    /// arg.
    #[clap(long)]
    pub fingerprint: Option<String>,
    /// Skip the "are you sure?" confirmation prompt.
    #[clap(long)]
    pub yes: bool,
}

/// Run the remove.
pub fn run(args: RemoveArgs, ctx: &CliContext) -> Result<()> {
    // Validate the filter args before touching the lock — bail cheaply
    // for the bad-args path rather than acquiring an exclusive lock
    // only to fail.
    let filter_desc = match (&args.family_id, &args.fingerprint) {
        (Some(family_id), None) => format!("family_id == `{family_id}`"),
        (None, Some(fp)) => format!("fingerprint == `{fp}`"),
        (Some(_), Some(_)) => {
            bail!("pass either a positional family_id OR --fingerprint, not both")
        }
        (None, None) => bail!("pass either a positional family_id OR --fingerprint"),
    };

    // Pre-flight: count what *would* be removed under the current
    // ledger state, so we can show the operator a concrete number and
    // bail with a friendly message when nothing matches. The actual
    // mutation re-runs the filter under the exclusive lock so a
    // concurrent append between this read and the rewrite can't be
    // dropped (TOCTOU-safe via `load_filter_and_rewrite`).
    let records = analyzed_rejections::load_all(&ctx.layout.memory_dir)?;
    let preview_removed = match (&args.family_id, &args.fingerprint) {
        (Some(family_id), None) => records
            .iter()
            .filter(|r| r.family_id == *family_id)
            .count(),
        (None, Some(fp)) => records
            .iter()
            .filter(|r| r.fingerprint == *fp)
            .count(),
        _ => unreachable!("filter validation above guarantees exactly one of (family_id, fp)"),
    };
    if preview_removed == 0 {
        eprintln!("no records matched {filter_desc}; ledger unchanged");
        return Ok(());
    }
    if !args.yes {
        eprintln!("about to remove {preview_removed} record(s) matching {filter_desc}");
        eprintln!("pass `--yes` to skip this prompt; rerun the command to confirm");
        bail!("removal aborted (no --yes)");
    }

    // Atomic load+filter+rewrite. The actual removed count may differ
    // from the preview if an append landed between the preview load
    // and the lock acquisition.
    let family_id = args.family_id.clone();
    let fingerprint = args.fingerprint.clone();
    let removed = analyzed_rejections::load_filter_and_rewrite(&ctx.layout.memory_dir, |recs| {
        Ok(recs
            .into_iter()
            .filter(|r| match (&family_id, &fingerprint) {
                (Some(id), None) => r.family_id != *id,
                (None, Some(fp)) => r.fingerprint != *fp,
                _ => unreachable!(),
            })
            .collect())
    })
    .context("rewriting ledger after removal")?;
    let remaining = analyzed_rejections::load_all(&ctx.layout.memory_dir)?.len();
    eprintln!("removed {removed} record(s); ledger now has {remaining} entries");
    Ok(())
}
