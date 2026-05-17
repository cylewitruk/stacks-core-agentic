//! `sbagent rejections probe` — the agent's per-candidate ledger
//! check. Exit 0 + JSON on match; exit 0 + empty stdout on no match.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::analyzed_rejections::{self, FingerprintInputs};
use crate::cli::CliContext;

/// Args for `sbagent rejections probe`.
#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// Lens this candidate was promoted on
    /// (`tx_latency` / `tenure_throughput` / `commit_time`).
    #[clap(long)]
    pub lens: String,
    /// Family kind (`tx_family` / `block_family` / `contract_family`).
    #[clap(long)]
    pub kind: String,
    /// Comma-separated list of suspected spans. Order doesn't matter
    /// — the fingerprint canonicalizes (sort + dedup).
    #[clap(long, value_delimiter = ',', num_args = 0..)]
    pub spans: Vec<String>,
    /// Optional contract function key (`<issuer>.<contract>[.function]`)
    /// for `contract_family` matches.
    #[clap(long)]
    pub contract: Option<String>,
    /// Override the ledger location. When set, probe reads from
    /// `<memory_dir>/analyzed-rejections.jsonl` instead of
    /// `ctx.layout.memory_dir`. The triage agent passes this
    /// explicitly so the sandboxed `sbagent` child probes the
    /// orchestrator-resolved operator memory dir rather than
    /// whatever the child resolves from its own cwd / defaults.
    #[clap(long)]
    pub memory_dir: Option<PathBuf>,
}

/// Run the probe. Always exits 0; presence of output indicates a hit.
pub fn run(args: ProbeArgs, ctx: &CliContext) -> Result<()> {
    let memory_dir = args
        .memory_dir
        .as_deref()
        .unwrap_or(&ctx.layout.memory_dir);
    let inputs = FingerprintInputs {
        lens: &args.lens,
        kind: &args.kind,
        suspected_spans: &args.spans,
        contract_function: args.contract.as_deref(),
    };
    let hits = analyzed_rejections::probe(memory_dir, &inputs)?;
    if hits.is_empty() {
        return Ok(());
    }
    // Compact JSON array — easy for the agent to consume.
    let json = serde_json::to_string_pretty(&hits)?;
    println!("{json}");
    Ok(())
}
