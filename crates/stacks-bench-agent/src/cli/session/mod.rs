//! `sbagent session ...` — per-session subcommands.
//!
//! Layout: every session command takes a `--session-id` (or
//! `SBAGENT_SESSION_ID`) that resolves to `<data_root>/sessions/<id>/results`.
//! `session run` is the exception — it mints a fresh `YYYYMMDD-HHMMSS` id
//! when none is supplied. Sub-subcommands live in nested modules
//! (e.g. `session baseline run`, `session triage clean`) — see the
//! [`SessionCommand`] variants.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::types::SessionId;

pub mod analysis;
pub mod baseline;
pub mod bench;
pub mod bench_range;
pub mod finalize;
pub mod optimize;
pub mod publish;
pub mod run;
pub mod tail;
pub mod triage;
pub mod validate;

/// Args carried by every `sbagent session ...` invocation.
#[derive(Debug, Args)]
pub struct SessionArgs {
    /// Session id (e.g. `20260507-104400`). Used as a path segment under
    /// `<sessions_root>/`. Optional only for `session run` — that
    /// subcommand mints a fresh id when omitted; every other subcommand
    /// requires an explicit id (or `SBAGENT_SESSION_ID`).
    #[clap(long, env = "SBAGENT_SESSION_ID", global = true)]
    pub session_id: Option<SessionId>,

    /// Subcommand.
    #[clap(subcommand)]
    pub command: SessionCommand,
}

/// `sbagent session <subcommand>` variants.
#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// Validate that a session has produced every required artifact under
    /// the v2 pipeline shape.
    Validate(validate::ValidateSessionArgs),

    /// Emit `summary.json` + `summary.md` for a finished session.
    Finalize(finalize::FinalizeArgs),

    /// Tail session log streams.
    Tail(tail::TailSessionArgs),

    /// Phase 0: baseline.
    Baseline(baseline::BaselineArgs),

    /// Phase 1: triage.
    Triage(triage::TriageArgs),

    /// Phase 1.5 / 1.7: analysis (analyzer fanout + merge).
    Analysis(analysis::AnalysisArgs),

    /// Phase 2 / 3: optimize (optimizer fanout + bench).
    Optimize(optimize::OptimizeArgs),

    /// Phase 3: build per-target release binaries and run benchmarks
    /// serialized under the bench lock.
    Bench(bench::BenchArgs),

    /// Phase 5: generate per-target publish artifacts and push branches /
    /// open PRs / open issues. With `--dry-run`, generate only.
    Publish(publish::PublishArgs),

    /// Run the full pipeline (Phase 0 → Phase 5).
    Run(run::RunSessionArgs),
}

fn require_session_id(id: Option<SessionId>, subcommand: &str) -> Result<SessionId> {
    id.ok_or_else(|| {
        anyhow::anyhow!(
            "session id is required for `sbagent session {subcommand}`; pass --session-id <id> or \
             set SBAGENT_SESSION_ID"
        )
    })
}

/// Dispatch a `sbagent session ...` invocation.
pub async fn run(args: SessionArgs, ctx: &CliContext) -> Result<()> {
    let SessionArgs { session_id, command } = args;
    match command {
        SessionCommand::Validate(a) => {
            validate::run(a, ctx, &require_session_id(session_id, "validate")?).await
        }
        SessionCommand::Finalize(a) => {
            finalize::run(a, ctx, &require_session_id(session_id, "finalize")?).await
        }
        SessionCommand::Tail(a) => {
            tail::run(a, ctx, &require_session_id(session_id, "tail")?).await
        }
        SessionCommand::Baseline(a) => {
            baseline::run(a, ctx, &require_session_id(session_id, "baseline")?).await
        }
        SessionCommand::Triage(a) => {
            triage::run(a, ctx, &require_session_id(session_id, "triage")?).await
        }
        SessionCommand::Analysis(a) => {
            analysis::run(a, ctx, &require_session_id(session_id, "analysis")?).await
        }
        SessionCommand::Optimize(a) => {
            optimize::run(a, ctx, &require_session_id(session_id, "optimize")?).await
        }
        SessionCommand::Bench(a) => {
            bench::run(a, ctx, &require_session_id(session_id, "bench")?).await
        }
        SessionCommand::Publish(a) => {
            publish::run(a, ctx, &require_session_id(session_id, "publish")?).await
        }
        SessionCommand::Run(a) => {
            let id = session_id.unwrap_or_else(SessionId::mint_now);
            run::run(a, ctx, &id).await
        }
    }
}
