//! `sbagent rejections append` — coordinator's write primitive. Also
//! available to operators for manual ledger entries.

use anyhow::{Context as _, Result};
use clap::Args;

use crate::analyzed_rejections::{
    self, FingerprintInputs, Outcome, Record, canonical_contract_key, compute_fingerprint,
    now_utc_iso8601,
};
use crate::cli::CliContext;

/// Args for `sbagent rejections append`.
#[derive(Debug, Args)]
pub struct AppendArgs {
    /// Stable kebab-case family identifier (matches triage's
    /// `candidates[i].id` for the rejected family).
    #[clap(long)]
    pub family_id: String,
    /// Lens the family was promoted on
    /// (`tx_latency` / `tenure_throughput` / `commit_time`).
    #[clap(long)]
    pub lens: String,
    /// Outcome class: `rejected` (signal was wrong) or
    /// `not_actionable` (signal was right but no structural handle).
    #[clap(long)]
    pub outcome: String,
    /// Family kind (`tx_family` / `block_family` / `contract_family`).
    #[clap(long)]
    pub kind: String,
    /// Comma-separated list of suspected spans. Canonicalized
    /// (sorted + deduped) before storage.
    #[clap(long, value_delimiter = ',', num_args = 0..)]
    pub spans: Vec<String>,
    /// Contract function key for `contract_family` rejections
    /// (`<issuer>.<contract>[.function]`). Optional otherwise.
    #[clap(long)]
    pub contract: Option<String>,
    /// Session id that produced the rejection.
    #[clap(long)]
    pub session: String,
    /// One-line code-level rejection reason. Copy from the
    /// analyzer's `analysis.json` `reason` (rejected) or
    /// `lens_disposition.reason` (not_actionable).
    #[clap(long)]
    pub reason: String,
    /// Stacks-core commit SHA the analyzer was looking at.
    /// Coordinator should populate; operator may omit for manual
    /// entries.
    #[clap(long)]
    pub stacks_core_sha: Option<String>,
    /// Optional absolute path to the analyzer's `analysis.json` for
    /// the full evidence chain.
    #[clap(long)]
    pub evidence_path: Option<String>,
}

/// Run the append.
pub fn run(args: AppendArgs, ctx: &CliContext) -> Result<()> {
    let outcome = Outcome::parse(&args.outcome).context("parsing --outcome")?;
    let contract_key = args.contract.clone();
    let fingerprint = compute_fingerprint(&FingerprintInputs {
        lens: &args.lens,
        kind: &args.kind,
        suspected_spans: &args.spans,
        contract_function: contract_key.as_deref(),
    });
    let record = Record {
        ts: now_utc_iso8601(),
        session_id: args.session,
        family_id: args.family_id,
        lens: args.lens,
        outcome,
        kind: args.kind,
        suspected_spans: {
            let mut s = args.spans;
            s.sort();
            s.dedup();
            s
        },
        contract_function: contract_key,
        fingerprint: fingerprint.clone(),
        stacks_core_sha: args.stacks_core_sha,
        reason: args.reason,
        evidence_path: args.evidence_path,
    };
    analyzed_rejections::append(&ctx.layout.memory_dir, &record)?;
    eprintln!(
        "appended rejection: family_id=`{}` lens=`{}` outcome=`{}` fingerprint=`{fingerprint}`",
        record.family_id,
        record.lens,
        record.outcome.as_str(),
    );
    // Suppress unused-import warning on `canonical_contract_key` —
    // it's exported by the module for callers that build the
    // contract key from components (issuer/contract/function); the
    // CLI takes the fully-built key as `--contract` for ergonomics.
    let _ = canonical_contract_key;
    Ok(())
}
