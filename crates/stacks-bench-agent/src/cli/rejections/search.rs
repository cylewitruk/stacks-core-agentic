//! `sbagent rejections search` — structured query for operator
//! exploration. Triage uses `probe` (fingerprint-exact); operators
//! use this for "find me everything matching these criteria."

use anyhow::Result;
use clap::Args;

use crate::analyzed_rejections;
use crate::cli::CliContext;

/// Args for `sbagent rejections search`.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Match records whose `suspected_spans` contains any of these
    /// (comma-separated). Logical OR across the list.
    #[clap(long, value_delimiter = ',', num_args = 0..)]
    pub spans: Vec<String>,
    /// Filter by kind.
    #[clap(long)]
    pub kind: Option<String>,
    /// Filter by contract function key
    /// (`<issuer>.<contract>[.function]`).
    #[clap(long)]
    pub contract: Option<String>,
    /// Match records whose `reason` text contains this substring
    /// (case-insensitive).
    #[clap(long)]
    pub reason_grep: Option<String>,
    /// Filter by lens.
    #[clap(long)]
    pub lens: Option<String>,
    /// Max number of results.
    #[clap(long, default_value_t = 50)]
    pub limit: usize,
    /// Output as JSON array instead of human-readable lines.
    #[clap(long)]
    pub json: bool,
}

/// Run the search.
pub fn run(args: SearchArgs, ctx: &CliContext) -> Result<()> {
    let mut records = analyzed_rejections::load_all(&ctx.layout.memory_dir)?;

    if !args.spans.is_empty() {
        let needles: Vec<String> = args
            .spans
            .iter()
            .map(|s| s.to_lowercase())
            .collect();
        records.retain(|r| {
            r.suspected_spans
                .iter()
                .any(|s| {
                    needles
                        .iter()
                        .any(|n| s.to_lowercase().contains(n))
                })
        });
    }
    if let Some(kind) = &args.kind {
        records.retain(|r| r.kind == *kind);
    }
    if let Some(contract) = &args.contract {
        records.retain(|r| {
            r.contract_function
                .as_deref()
                .is_some_and(|c| c.contains(contract))
        });
    }
    if let Some(needle) = &args.reason_grep {
        let lower = needle.to_lowercase();
        records.retain(|r| {
            r.reason
                .to_lowercase()
                .contains(&lower)
        });
    }
    if let Some(lens) = &args.lens {
        records.retain(|r| r.lens == *lens);
    }

    records.truncate(args.limit);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else if records.is_empty() {
        println!("(no matches)");
    } else {
        for r in &records {
            println!(
                "{ts}  {family_id} (lens={lens}, outcome={outcome}, spans=[{spans}], \
                 reason={reason})",
                ts = r.ts,
                family_id = r.family_id,
                lens = r.lens,
                outcome = r.outcome.as_str(),
                spans = r.suspected_spans.join(","),
                reason = r.reason,
            );
        }
    }
    Ok(())
}
