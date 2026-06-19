//! `sbagent history list` — one-row-per-session leaderboard view.

use std::io::{IsTerminal as _, Write};

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::cli::history::{
    colorize_status, format_wall_clock, ledger_path, parse_since, session_status_text,
    started_at_date,
};
use crate::models::session_record::{SessionRecord, SessionStatus, TargetStatus};
use crate::session::ledger_reader::{LedgerReadReport, read_all, session_total_secs};

/// `sbagent history list` args.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Maximum rows to print, most-recent first (sorted by
    /// `started_at` descending). Default 20.
    #[clap(long, default_value_t = 20)]
    pub limit: usize,

    /// Filter to sessions started on or after this date. Accepts
    /// either an ISO 8601 calendar date (`YYYY-MM-DD`) or an ISO 8601
    /// week id (`YYYY-Www`, e.g. `2026-W23`).
    #[clap(long)]
    pub since: Option<String>,
}

pub(super) fn run_list(args: ListArgs, ctx: &CliContext) -> Result<()> {
    let ledger_path = ledger_path(ctx)?;
    let LedgerReadReport { records, skipped } = read_all(&ledger_path)?;

    // Skipped-line warnings are the CLI's job (the reader stays silent
    // so library consumers don't get accidental stderr noise). One
    // line per skipped, before any stdout output.
    for s in &skipped {
        eprintln!(
            "sbagent history: skipping malformed sessions.jsonl line {}: {}",
            s.line_number, s.error,
        );
    }

    if records.is_empty() {
        println!("no sessions archived yet");
        return Ok(());
    }

    let mut filtered = match args.since.as_deref() {
        Some(since) => {
            let cutoff = parse_since(since)?;
            records
                .into_iter()
                .filter(|r| started_at_date(r) >= cutoff.as_str())
                .collect::<Vec<_>>()
        }
        None => records,
    };

    if filtered.is_empty() {
        // Filter excluded everything. Same exit-0 "nothing to show"
        // shape as the empty-ledger case.
        println!("no sessions archived yet");
        return Ok(());
    }

    // Most-recent first; ISO 8601 sorts lexicographically.
    filtered.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
    });
    filtered.truncate(args.limit);

    let mut out = std::io::stdout().lock();
    let use_color = out.is_terminal() && std::env::var_os("NO_COLOR").is_none();
    render_table(&mut out, &filtered, use_color)?;
    Ok(())
}

/// One row of the rendered table. Built ahead of column-width math so
/// header + body share the same widths.
struct Row {
    id: String,
    started_at: String,
    status: SessionStatus,
    status_text: &'static str,
    targets: String,
    wall_clock: String,
    prs: String,
    issues: String,
}

fn build_row(record: &SessionRecord) -> Row {
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    // `Failed` targets fold into the `Ab` (aborted) bucket — the
    // Phase 3 spec defines exactly three counts (A/R/Ab). Both
    // `Aborted` and `Failed` represent non-accept-non-reject outcomes,
    // so collapsing them keeps the column honest without inventing a
    // fourth count the spec doesn't budget for.
    let mut aborted = 0usize;
    let mut prs = 0usize;
    let mut issues = 0usize;
    for t in &record.targets {
        match t.status {
            TargetStatus::Accepted => accepted += 1,
            TargetStatus::Rejected => rejected += 1,
            TargetStatus::Aborted | TargetStatus::Failed => aborted += 1,
        }
        if t.pr_url.is_some() {
            prs += 1;
        }
        if t.issue_url.is_some() {
            issues += 1;
        }
    }
    let status_text = session_status_text(record.status);
    Row {
        id: record.id.clone(),
        started_at: started_at_date(record).to_owned(),
        status: record.status,
        status_text,
        targets: format!("{accepted}/{rejected}/{aborted}"),
        wall_clock: format_wall_clock(session_total_secs(record)),
        prs: prs.to_string(),
        issues: issues.to_string(),
    }
}

const HEADERS: [&str; 7] = ["id", "started_at", "status", "targets", "wall-clock", "prs", "issues"];

fn render_table<W: Write>(out: &mut W, records: &[SessionRecord], use_color: bool) -> Result<()> {
    let rows: Vec<Row> = records
        .iter()
        .map(build_row)
        .collect();

    // Column widths from the longest of (header, each row cell). Done
    // before any writing so header + body share one source of truth.
    let cell_widths: [usize; 7] = std::array::from_fn(|col| {
        let header_w = HEADERS[col].len();
        rows.iter()
            .map(|r| cell(r, col).len())
            .max()
            .unwrap_or(0)
            .max(header_w)
    });

    write_row(out, HEADERS.iter().copied(), &cell_widths)?;
    for r in &rows {
        write_data_row(out, r, &cell_widths, use_color)?;
    }
    Ok(())
}

/// Project a row to its `col`-indexed cell as a string slice (for
/// width math — actual rendering goes through `write_data_row`).
fn cell(r: &Row, col: usize) -> &str {
    match col {
        0 => &r.id,
        1 => &r.started_at,
        2 => r.status_text,
        3 => &r.targets,
        4 => &r.wall_clock,
        5 => &r.prs,
        6 => &r.issues,
        _ => unreachable!(),
    }
}

fn write_row<'a, W: Write, I>(out: &mut W, cells: I, widths: &[usize; 7]) -> Result<()>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut first = true;
    for (i, c) in cells.into_iter().enumerate() {
        if !first {
            write!(out, "  ")?;
        }
        first = false;
        // Last column: skip trailing padding so the rendered output
        // has no trailing whitespace (matters for byte-equality
        // tests that don't want to be sensitive to invisible
        // padding).
        if i + 1 == widths.len() {
            write!(out, "{c}")?;
        } else {
            write!(out, "{c:<width$}", width = widths[i])?;
        }
    }
    writeln!(out)?;
    Ok(())
}

fn write_data_row<W: Write>(
    out: &mut W,
    row: &Row,
    widths: &[usize; 7],
    use_color: bool,
) -> Result<()> {
    // Column 2 is `status`; the only colored cell. We render it
    // padded-to-width FIRST as plain text, then optionally wrap that
    // padded slice in ANSI codes — keeps width math identical
    // whether color is on or off.
    for (i, _h) in HEADERS.iter().enumerate() {
        if i > 0 {
            write!(out, "  ")?;
        }
        let raw = cell(row, i);
        let is_last = i + 1 == HEADERS.len();
        let mut padded = if is_last { raw.to_owned() } else { format!("{raw:<w$}", w = widths[i]) };
        if i == 2 && use_color {
            padded = colorize_status(&padded, row.status);
        }
        write!(out, "{padded}")?;
    }
    writeln!(out)?;
    Ok(())
}
