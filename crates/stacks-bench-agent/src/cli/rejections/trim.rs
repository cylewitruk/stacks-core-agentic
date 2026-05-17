//! `sbagent rejections trim` — housekeeping. Drop history entries
//! older than a date, before a sha, or beyond a per-family count cap.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::analyzed_rejections::{self, Record};
use crate::cli::CliContext;

/// Args for `sbagent rejections trim`.
#[derive(Debug, Args)]
pub struct TrimArgs {
    /// Drop records whose `ts` is before this date (YYYY-MM-DD).
    #[clap(long)]
    pub before: Option<String>,
    /// Drop records whose `stacks_core_sha` starts with this value
    /// (the assumption being "this sha is now ancient; rejections
    /// recorded against it deserve re-testing"). Records without a
    /// recorded sha are NOT trimmed by this filter.
    #[clap(long)]
    pub before_sha: Option<String>,
    /// Keep only the N most recent records per family_id.
    #[clap(long)]
    pub keep_last: Option<usize>,
    /// Skip the "are you sure?" confirmation prompt.
    #[clap(long)]
    pub yes: bool,
}

/// Run the trim.
pub fn run(args: TrimArgs, ctx: &CliContext) -> Result<()> {
    if args.before.is_none() && args.before_sha.is_none() && args.keep_last.is_none() {
        bail!("trim requires at least one of `--before`, `--before-sha`, `--keep-last`");
    }

    // Pre-flight count so the operator confirmation can show a real
    // number. The actual mutation re-runs the filter under the
    // exclusive lock so any concurrent append between the preview
    // and the rewrite is preserved (TOCTOU-safe via
    // `load_filter_and_rewrite`).
    let preview = analyzed_rejections::load_all(&ctx.layout.memory_dir)?;
    let preview_dropped = preview.len() - apply_filters(preview.clone(), &args).len();
    if preview_dropped == 0 {
        eprintln!("trim filter matched 0 records; ledger unchanged");
        return Ok(());
    }
    if !args.yes {
        eprintln!("about to drop {preview_dropped} record(s) from the ledger");
        eprintln!("pass `--yes` to skip this prompt; rerun the command to confirm");
        bail!("trim aborted (no --yes)");
    }

    let dropped = analyzed_rejections::load_filter_and_rewrite(&ctx.layout.memory_dir, |recs| {
        Ok(apply_filters(recs, &args))
    })
    .context("rewriting ledger after trim")?;
    let remaining = analyzed_rejections::load_all(&ctx.layout.memory_dir)?.len();
    eprintln!("dropped {dropped} record(s); ledger now has {remaining} entries");
    Ok(())
}

/// Apply the trim filters (`--before`, `--before-sha`, `--keep-last`)
/// in order. Pure function so the preview pass and the in-lock pass
/// agree on what would be dropped.
fn apply_filters(mut records: Vec<Record>, args: &TrimArgs) -> Vec<Record> {
    if let Some(before) = &args.before {
        records.retain(|r| {
            // ts format `2026-05-17T12:34:56Z`; lexical comparison
            // of the YYYY-MM-DD prefix is monotonic.
            let date_part = r.ts.get(..10).unwrap_or("");
            date_part >= before.as_str()
        });
    }
    if let Some(sha_prefix) = &args.before_sha {
        records.retain(|r| {
            r.stacks_core_sha
                .as_deref()
                .is_some_and(|s| !s.starts_with(sha_prefix))
        });
    }
    if let Some(n) = args.keep_last {
        records = keep_last_per_family(records, n);
    }
    records
}

/// Per family_id, keep only the newest `n` records (sorted by `ts`
/// descending). Stable: relative order of kept records preserved.
fn keep_last_per_family(records: Vec<Record>, n: usize) -> Vec<Record> {
    let mut by_family: BTreeMap<String, Vec<Record>> = BTreeMap::new();
    for r in records {
        by_family
            .entry(r.family_id.clone())
            .or_default()
            .push(r);
    }
    let mut out: Vec<Record> = Vec::new();
    for (_, mut group) in by_family {
        group.sort_by(|a, b| b.ts.cmp(&a.ts));
        group.truncate(n);
        out.extend(group);
    }
    // Re-sort by ts desc overall so the final ledger has a sensible
    // newest-first ordering.
    out.sort_by(|a, b| b.ts.cmp(&a.ts));
    out
}
