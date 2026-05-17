//! `sbagent rejections list` — operator scan of all ledger entries.

use anyhow::Result;
use clap::Args;

use crate::analyzed_rejections;
use crate::cli::CliContext;

/// Args for `sbagent rejections list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by lens.
    #[clap(long)]
    pub lens: Option<String>,
    /// Filter by outcome class (`rejected` or `not_actionable`).
    #[clap(long)]
    pub outcome: Option<String>,
    /// Filter by session id (exact match).
    #[clap(long)]
    pub since_session: Option<String>,
    /// Filter by stacks-core SHA (records whose
    /// `stacks_core_sha` equals or starts with the given value).
    #[clap(long)]
    pub since_sha: Option<String>,
    /// Output as JSON array instead of the human-readable line form.
    #[clap(long)]
    pub json: bool,
}

/// Run the list.
pub fn run(args: ListArgs, ctx: &CliContext) -> Result<()> {
    let mut records = analyzed_rejections::load_all(&ctx.layout.memory_dir)?;
    if let Some(lens) = &args.lens {
        records.retain(|r| r.lens == *lens);
    }
    if let Some(outcome) = &args.outcome {
        records.retain(|r| r.outcome.as_str() == outcome);
    }
    if let Some(session) = &args.since_session {
        records.retain(|r| r.session_id == *session);
    }
    if let Some(sha) = &args.since_sha {
        records.retain(|r| {
            r.stacks_core_sha
                .as_deref()
                .is_some_and(|s| s.starts_with(sha))
        });
    }
    if args.json {
        let json = serde_json::to_string_pretty(&records)?;
        println!("{json}");
    } else if records.is_empty() {
        println!("(no rejections recorded; ledger is empty or fully filtered)");
    } else {
        for r in &records {
            println!(
                "{ts}  {family_id} (lens={lens}, outcome={outcome}, session={session})",
                ts = r.ts,
                family_id = r.family_id,
                lens = r.lens,
                outcome = r.outcome.as_str(),
                session = r.session_id,
            );
        }
    }
    Ok(())
}
