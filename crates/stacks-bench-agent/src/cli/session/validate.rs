//! `sbagent session validate` — port of `scripts/validate-session.sh`.
//!
//! The validator logic itself lives in
//! [`crate::session::validate::validate`]; this module is the thin CLI
//! wrapper.

use anyhow::{Result, bail};
use clap::Args;

use crate::cli::CliContext;
use crate::session::{SessionLayout, validate};
use crate::types::SessionId;

/// Args for `sbagent session validate`.
#[derive(Debug, Args)]
pub struct ValidateSessionArgs {}

/// Validate one session. Mirrors the bash script's exit code: 0 on OK,
/// 1 on missing artifacts (with a printed list).
#[allow(unused_variables)]
pub async fn run(
    args: ValidateSessionArgs,
    ctx: &CliContext,
    session_id: &SessionId,
) -> Result<()> {
    let layout = SessionLayout::from_layout(&ctx.layout, session_id.clone());
    let report = validate::validate(&layout)?;

    if !report
        .schema_warnings
        .is_empty()
    {
        println!("SCHEMA WARNINGS:");
        for w in &report.schema_warnings {
            println!("  {w}");
        }
    }

    if report.ok() {
        println!("OK");
        Ok(())
    } else {
        println!("MISSING:");
        for m in &report.missing {
            println!("  {m}");
        }
        bail!("session validation failed: {} missing artifact(s)", report.missing.len())
    }
}
