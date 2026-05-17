//! `sbagent rejections render` — operator-facing markdown view of
//! the ledger, grouped by family with history.

use std::fmt::Write as _;

use anyhow::Result;
use clap::Args;

use crate::analyzed_rejections::{self, short_sha};
use crate::cli::CliContext;

/// Args for `sbagent rejections render`.
#[derive(Debug, Args)]
pub struct RenderArgs {
    /// Filter by lens.
    #[clap(long)]
    pub lens: Option<String>,
    /// Filter records to those with `stacks_core_sha` starting with
    /// this value.
    #[clap(long)]
    pub since_sha: Option<String>,
}

/// Run the renderer.
pub fn run(args: RenderArgs, ctx: &CliContext) -> Result<()> {
    let grouped = analyzed_rejections::grouped_by_family(&ctx.layout.memory_dir)?;
    let mut out = String::new();
    let _ = writeln!(out, "# Analyzed rejections ledger");
    let _ = writeln!(out);
    if grouped.is_empty() {
        let _ = writeln!(
            out,
            "_The ledger is empty. Analyzer rejections will accumulate here as sessions run._"
        );
        print!("{out}");
        return Ok(());
    }
    let _ = writeln!(out, "Total families recorded: **{}**.", grouped.len());
    let _ = writeln!(out);
    for (family_id, records) in &grouped {
        // Apply filters per-family using the most-recent entry as the
        // representative (records are already sorted newest-first).
        let representative = &records[0];
        if let Some(lens) = &args.lens
            && representative.lens != *lens
        {
            continue;
        }
        if let Some(sha) = &args.since_sha
            && !representative
                .stacks_core_sha
                .as_deref()
                .is_some_and(|s| s.starts_with(sha))
        {
            continue;
        }
        let _ = writeln!(out, "## `{family_id}`");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "- **lens**: `{}` · **kind**: `{}` · **outcome**: `{}` · **times rejected**: {}",
            representative.lens,
            representative.kind,
            representative
                .outcome
                .as_str(),
            records.len(),
        );
        if !representative
            .suspected_spans
            .is_empty()
        {
            let _ = writeln!(
                out,
                "- **suspected spans**: `{}`",
                representative
                    .suspected_spans
                    .join("`, `"),
            );
        }
        if let Some(cf) = &representative.contract_function {
            let _ = writeln!(out, "- **contract**: `{cf}`");
        }
        let _ = writeln!(out, "- **fingerprint**: `{}`", representative.fingerprint);
        let _ = writeln!(out);
        let _ = writeln!(out, "### Most recent rejection");
        let _ = writeln!(out);
        let _ = writeln!(out, "- session: `{}`", representative.session_id);
        let _ = writeln!(out, "- recorded: `{}`", representative.ts);
        if let Some(sha) = &representative.stacks_core_sha {
            let _ = writeln!(out, "- stacks-core: `{}`", short_sha(sha));
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "> {}", representative.reason);
        let _ = writeln!(out);
        if records.len() > 1 {
            let _ = writeln!(out, "### History ({} more)", records.len() - 1);
            let _ = writeln!(out);
            for r in records.iter().skip(1) {
                let sha_suffix = r
                    .stacks_core_sha
                    .as_deref()
                    .map(|s| format!(" (sha {})", short_sha(s)))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "- {} — session `{}`{sha_suffix}: {}",
                    r.ts, r.session_id, r.reason
                );
            }
            let _ = writeln!(out);
        }
    }
    print!("{out}");
    Ok(())
}
