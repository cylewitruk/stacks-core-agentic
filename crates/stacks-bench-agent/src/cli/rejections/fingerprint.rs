//! `sbagent rejections fingerprint` — operator-debug helper. Compute
//! the canonical fingerprint for given inputs without touching the
//! ledger.

use anyhow::Result;
use clap::Args;

use crate::analyzed_rejections::{FingerprintInputs, compute_fingerprint};
use crate::cli::CliContext;

/// Args for `sbagent rejections fingerprint`.
#[derive(Debug, Args)]
pub struct FingerprintArgs {
    /// Lens.
    #[clap(long)]
    pub lens: String,
    /// Family kind.
    #[clap(long)]
    pub kind: String,
    /// Comma-separated suspected spans (any order; canonicalized
    /// internally).
    #[clap(long, value_delimiter = ',', num_args = 0..)]
    pub spans: Vec<String>,
    /// Optional contract function key.
    #[clap(long)]
    pub contract: Option<String>,
}

/// Run the fingerprint computation.
pub fn run(args: FingerprintArgs, _ctx: &CliContext) -> Result<()> {
    let fp = compute_fingerprint(&FingerprintInputs {
        lens: &args.lens,
        kind: &args.kind,
        suspected_spans: &args.spans,
        contract_function: args.contract.as_deref(),
    });
    println!("{fp}");
    Ok(())
}
