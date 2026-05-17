//! `sbagent rejections show` — full detail view for a family id or
//! fingerprint.

use anyhow::{Result, bail};
use clap::Args;

use crate::analyzed_rejections;
use crate::cli::CliContext;

/// Args for `sbagent rejections show`.
#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Family id to look up (exact match). Mutually exclusive with
    /// `--fingerprint`.
    pub family_id: Option<String>,
    /// Fingerprint to look up (exact match). Mutually exclusive with
    /// the positional family_id arg.
    #[clap(long)]
    pub fingerprint: Option<String>,
    /// Output as JSON array instead of pretty-printed text.
    #[clap(long)]
    pub json: bool,
}

/// Run the show.
pub fn run(args: ShowArgs, ctx: &CliContext) -> Result<()> {
    let records = match (&args.family_id, &args.fingerprint) {
        (Some(family_id), None) => {
            analyzed_rejections::by_family_id(&ctx.layout.memory_dir, family_id)?
        }
        (None, Some(fp)) => analyzed_rejections::by_fingerprint(&ctx.layout.memory_dir, fp)?,
        (Some(_), Some(_)) => {
            bail!("pass either a positional family_id OR --fingerprint, not both")
        }
        (None, None) => bail!("pass either a positional family_id OR --fingerprint"),
    };
    if records.is_empty() {
        eprintln!("no matching records");
        return Ok(());
    }
    if args.json {
        let json = serde_json::to_string_pretty(&records)?;
        println!("{json}");
    } else {
        for r in &records {
            println!("---");
            println!("ts:              {}", r.ts);
            println!("family_id:       {}", r.family_id);
            println!("lens:            {}", r.lens);
            println!("outcome:         {}", r.outcome.as_str());
            println!("kind:            {}", r.kind);
            println!("session_id:      {}", r.session_id);
            println!("fingerprint:     {}", r.fingerprint);
            if let Some(sha) = &r.stacks_core_sha {
                println!("stacks_core_sha: {sha}");
            }
            if !r.suspected_spans.is_empty() {
                println!("suspected_spans: {}", r.suspected_spans.join(", "));
            }
            if let Some(cf) = &r.contract_function {
                println!("contract:        {cf}");
            }
            println!("reason:          {}", r.reason);
            if let Some(p) = &r.evidence_path {
                println!("evidence_path:   {p}");
            }
        }
    }
    Ok(())
}
