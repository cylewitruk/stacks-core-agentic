//! `sbagent maintain` — reconcile archived PR/issue artifacts against
//! GitHub lifecycle state and append maintenance events.

use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::cli::CliContext;
use crate::git;
use crate::models::maintain_event::MaintEventKind;
use crate::session::ledger_reader::{LedgerReadReport, read_all as read_sessions};
use crate::session::maintain::{MaintainReconciler, ReconcileOutcome, needs_github_queries};
use crate::session::maintain_ledger::{append_event, read_all as read_maintain};
use crate::session::optimizers::optimizer_git_env;
use crate::session::publish::{self, StdGhClient};

/// `sbagent maintain` args.
#[derive(Debug, Args)]
pub struct MaintainArgs {
    /// Only reconcile sessions started on or after this ISO date
    /// (`YYYY-MM-DD`). Week syntax can be added if operators need it.
    #[clap(long)]
    pub since: Option<String>,

    /// Compute and print events, but do not append, commit, or push.
    #[clap(long)]
    pub dry_run: bool,

    /// Maximum number of artifacts to query this invocation.
    #[clap(long, default_value_t = 50)]
    pub limit: usize,
}

/// Run `sbagent maintain`.
pub async fn run(args: MaintainArgs, ctx: &CliContext) -> Result<()> {
    let operator = ctx
        .layout
        .require_operator_repo_root()?;
    let sessions_path = operator.join("sessions.jsonl");
    let maintain_path = operator.join("maintain.jsonl");

    let LedgerReadReport { mut records, skipped } = read_sessions(&sessions_path)?;
    for s in &skipped {
        eprintln!(
            "sbagent maintain: skipping malformed sessions.jsonl line {}: {}",
            s.line_number, s.error,
        );
    }
    if let Some(since) = args.since.as_deref() {
        records.retain(|r| started_at_date(r) >= since);
    }

    let maintain = read_maintain(&maintain_path)?;
    for s in &maintain.skipped {
        eprintln!(
            "sbagent maintain: skipping malformed maintain.jsonl line {}: {}",
            s.line_number, s.error,
        );
    }

    if !needs_github_queries(&records, &maintain)? {
        let mut out = std::io::stdout().lock();
        render_outcome(&mut out, &ReconcileOutcome::default(), args.dry_run)?;
        return Ok(());
    }

    let token = publish::read_publish_token(
        ctx.settings
            .publish
            .token_file_required()
            .context("`sbagent maintain`")?,
    )?;
    let gh = StdGhClient::from_token(&token)?;
    let reconciler = MaintainReconciler {
        gh: &gh,
        settings: &ctx.settings.maintain,
        now: SystemTime::now(),
    };
    let outcome = reconciler
        .reconcile(&records, &maintain, args.limit)
        .await?;

    let mut out = std::io::stdout().lock();
    render_outcome(&mut out, &outcome, args.dry_run)?;

    if args.dry_run || outcome.new_events.is_empty() {
        return Ok(());
    }

    for event in &outcome.new_events {
        append_event(&maintain_path, event)?;
    }
    commit_and_push(operator, &ctx.settings)?;
    Ok(())
}

fn started_at_date(record: &crate::models::session_record::SessionRecord) -> &str {
    let s = record.started_at.as_str();
    if s.len() >= 10 { &s[..10] } else { s }
}

fn render_outcome<W: Write>(out: &mut W, outcome: &ReconcileOutcome, dry_run: bool) -> Result<()> {
    if outcome.new_events.is_empty() && outcome.deferred.is_empty() {
        writeln!(out, "no maintenance events; lifecycle state unchanged")?;
        return Ok(());
    }
    let committed = if dry_run { "no" } else { "yes" };
    let rows: Vec<[String; 5]> = outcome
        .new_events
        .iter()
        .map(|event| {
            [
                event_kind_text(event.kind).to_owned(),
                event.session_id.clone(),
                event
                    .target_id
                    .clone()
                    .unwrap_or_else(|| "-".to_owned()),
                event
                    .pr_url
                    .clone()
                    .or_else(|| event.issue_url.clone())
                    .unwrap_or_else(|| "-".to_owned()),
                committed.to_owned(),
            ]
        })
        .collect();
    write_table(out, &["kind", "session", "target", "url", "committed?"], &rows)?;
    for d in &outcome.deferred {
        writeln!(out, "deferred: {} ({})", d.url, d.reason)?;
    }
    Ok(())
}

fn write_table<W: Write>(out: &mut W, headers: &[&str; 5], rows: &[[String; 5]]) -> Result<()> {
    let widths: [usize; 5] = std::array::from_fn(|i| {
        rows.iter()
            .map(|r| r[i].len())
            .max()
            .unwrap_or(0)
            .max(headers[i].len())
    });
    write_cells(out, headers.iter().copied(), &widths)?;
    for row in rows {
        write_cells(out, row.iter().map(String::as_str), &widths)?;
    }
    Ok(())
}

fn write_cells<'a, W: Write, I>(out: &mut W, cells: I, widths: &[usize; 5]) -> Result<()>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut first = true;
    for (i, cell) in cells.into_iter().enumerate() {
        if !first {
            write!(out, "  ")?;
        }
        first = false;
        if i + 1 == widths.len() {
            write!(out, "{cell}")?;
        } else {
            write!(out, "{cell:<width$}", width = widths[i])?;
        }
    }
    writeln!(out)?;
    Ok(())
}

fn event_kind_text(kind: MaintEventKind) -> &'static str {
    match kind {
        MaintEventKind::PrOpen => "pr_open",
        MaintEventKind::PrMerged => "pr_merged",
        MaintEventKind::PrClosedUnmerged => "pr_closed_unmerged",
        MaintEventKind::PrStale => "pr_stale",
        MaintEventKind::PrForcePushed => "pr_force_pushed",
        MaintEventKind::PrBranchDeleted => "pr_branch_deleted",
        MaintEventKind::IssueOpen => "issue_open",
        MaintEventKind::IssueClosed => "issue_closed",
    }
}

fn commit_and_push(operator: &Path, settings: &crate::settings::Settings) -> Result<()> {
    let env = optimizer_git_env(settings);
    match git::stage_and_commit(
        operator,
        &["maintain.jsonl"],
        "maintain: record lifecycle events",
        &env,
    )? {
        git::CommitOutcome::Committed => {}
        git::CommitOutcome::NothingToCommit => return Ok(()),
    }
    let token = publish::read_publish_token(
        settings
            .publish
            .token_file_required()
            .context("`sbagent maintain` push")?,
    )?;
    let remote = "origin";
    let branch = git::run_git_output(operator, &["rev-parse", "--abbrev-ref", "HEAD"])
        .context("reading operator branch")?;
    if branch == "HEAD" {
        bail!("`sbagent maintain` requires operator repo on a branch, not detached HEAD");
    }
    let remote_url = git::run_git_output(operator, &["remote", "get-url", remote])
        .context("reading operator origin URL")?;
    let auth_username = settings
        .git
        .effective_auth_username();
    let auth_url_prefix = settings
        .git
        .effective_auth_url_prefix()?;
    git::validate_auth_url(&remote_url, &auth_url_prefix, "operator origin")?;
    git::push_with_pat(operator, remote, &branch, &token, &env, auth_username, &auth_url_prefix)
        .context("pushing maintain.jsonl commit")?;
    Ok(())
}
