//! `sbagent history show <session-id>` — per-session detail view.
//!
//! Three sections (header, phase durations, targets) plus an optional
//! Maintenance events section when v10 lifecycle observations exist
//! for the session. ASCII-only by default. Shares wall-clock + color
//! helpers with `history list` via [`super`].

use std::io::{IsTerminal as _, Write};

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::cli::history::{colorize_status, format_wall_clock, session_status_text};
use crate::models::common::DeliveryMode;
use crate::models::maintain_event::MaintEventKind;
use crate::models::session_record::{SessionRecord, TargetRecord, TargetStatus};
use crate::session::history_projection::{self, ProjectedMaintenanceEventV1};

/// `sbagent history show <session-id>` args.
#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Session id (full string, as it appears in
    /// `sessions.jsonl`). Linear scan — operator ledgers stay small
    /// in practice.
    pub session_id: String,
}

/// Bar-chart width cap (in `#` characters) for the phase-durations
/// section. Spec calls for `min(terminal_width, 60)`; we use 60
/// unconditionally — for piped output that's the correct ceiling, and
/// for narrow TTYs an occasional wrap is better than pulling in a
/// terminal-size dep just for this.
const BAR_WIDTH: usize = 60;

pub(super) fn run_show(args: ShowArgs, ctx: &CliContext) -> Result<()> {
    let operator = ctx
        .layout
        .require_operator_repo_root()?;
    let history = history_projection::read_operator_projection_v1(operator)?;
    for s in &history.skipped_sessions {
        eprintln!(
            "sbagent history: skipping malformed sessions.jsonl line {}: {}",
            s.line_number, s.error,
        );
    }
    for s in &history.skipped_maintain {
        eprintln!(
            "sbagent history: skipping malformed maintain.jsonl line {}: {}",
            s.line_number, s.error,
        );
    }

    let record = match history
        .projection
        .session(&args.session_id)
    {
        Some(r) => r,
        None => {
            anyhow::bail!(
                "no such session id `{}` in sessions.jsonl; run `sbagent history list` to see \
                 what is archived",
                args.session_id,
            );
        }
    };

    let mut out = std::io::stdout().lock();
    let use_color = out.is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let maint_events = history
        .projection
        .maintenance_events_for_session(&record.id);
    render_show(&mut out, record, maint_events, use_color)?;
    Ok(())
}

fn render_show<W: Write>(
    out: &mut W,
    record: &SessionRecord,
    maint_events: &[ProjectedMaintenanceEventV1],
    use_color: bool,
) -> Result<()> {
    render_header_section(out, record, use_color)?;
    writeln!(out)?;
    render_phase_durations_section(out, record)?;
    writeln!(out)?;
    render_targets_section(out, record)?;
    if !maint_events.is_empty() {
        writeln!(out)?;
        render_maintenance_section(out, maint_events)?;
    }
    Ok(())
}

fn render_header_section<W: Write>(
    out: &mut W,
    record: &SessionRecord,
    use_color: bool,
) -> Result<()> {
    writeln!(out, "Session {}", record.id)?;
    writeln!(out, "  started_at -> finished_at:  {} -> {}", record.started_at, record.finished_at,)?;
    let status_text = session_status_text(record.status);
    let status_render = if use_color {
        colorize_status(status_text, record.status)
    } else {
        status_text.to_owned()
    };
    writeln!(out, "  status:                     {status_render}")?;
    if let Some(phase) = record
        .failure_phase
        .as_deref()
    {
        writeln!(out, "  failure_phase:              {phase}")?;
    }
    if let Some(reason) = record
        .failure_reason
        .as_deref()
    {
        writeln!(out, "  failure_reason:             {reason}")?;
    }
    Ok(())
}

fn render_phase_durations_section<W: Write>(out: &mut W, record: &SessionRecord) -> Result<()> {
    writeln!(out, "Phase durations")?;
    if record
        .phase_durations_secs
        .is_empty()
    {
        writeln!(out, "  (no phase durations recorded)")?;
        return Ok(());
    }

    // Sort by descending seconds so the widest bars sit at top.
    let mut entries: Vec<(&str, f64)> = record
        .phase_durations_secs
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect();
    entries.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let name_w = entries
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    // Reserve a 7-char field for "1234.56" (i.e. up to four-digit
    // seconds with two-decimal precision); falls back gracefully on
    // wider values.
    let secs_w = entries
        .iter()
        .map(|(_, s)| format!("{s:.2}").len())
        .max()
        .unwrap_or(7)
        .max(7);

    let max_secs = entries
        .iter()
        .map(|(_, s)| *s)
        .fold(0.0_f64, f64::max);

    for (name, secs) in &entries {
        let bar = if *secs < 1.0 {
            "< 1s".to_owned()
        } else if max_secs <= 0.0 {
            String::new()
        } else {
            let scaled = (*secs / max_secs) * (BAR_WIDTH as f64);
            // At least one `#` for any non-sub-second phase, so a
            // phase that's tiny relative to the max still produces a
            // visible row.
            let n = (scaled.round() as usize).max(1);
            "#".repeat(n)
        };
        writeln!(out, "  {name:<name_w$}  {secs:>secs_w$.2}s  {bar}")?;
    }
    Ok(())
}

fn render_targets_section<W: Write>(out: &mut W, record: &SessionRecord) -> Result<()> {
    writeln!(out, "Targets")?;
    if record.targets.is_empty() {
        writeln!(out, "  (no targets recorded)")?;
        return Ok(());
    }

    let rows: Vec<TargetRow> = record
        .targets
        .iter()
        .map(build_target_row)
        .collect();

    const TARGET_HEADERS: [&str; 6] =
        ["id", "status", "delivery_mode", "improvement_pct", "bench", "url"];

    let widths: [usize; 6] = std::array::from_fn(|col| {
        let header_w = TARGET_HEADERS[col].len();
        rows.iter()
            .map(|r| target_cell(r, col).len())
            .max()
            .unwrap_or(0)
            .max(header_w)
    });

    write!(out, "  ")?;
    write_padded_cells(out, TARGET_HEADERS.iter().copied(), &widths)?;
    for r in &rows {
        write!(out, "  ")?;
        write_padded_cells(out, (0..TARGET_HEADERS.len()).map(|c| target_cell(r, c)), &widths)?;
    }
    Ok(())
}

struct TargetRow {
    id: String,
    status: String,
    delivery_mode: String,
    improvement_pct: String,
    bench: String,
    url: String,
}

fn build_target_row(t: &TargetRecord) -> TargetRow {
    let status = target_status_text(t).to_owned();
    let delivery_mode = match t.delivery_mode {
        DeliveryMode::NormalPr => "normal_pr",
        DeliveryMode::ConsensusPocPr => "consensus_poc_pr",
        DeliveryMode::ConsensusIssue => "consensus_issue",
    }
    .to_owned();
    let (improvement_pct, bench_wall) = match &t.bench {
        Some(b) => {
            let pct = format!("{:+.2}%", b.improvement_pct);
            let bench_wall = format_wall_clock((b.candidate_total_us as f64) / 1_000_000.0);
            (pct, bench_wall)
        }
        None => ("-".to_owned(), "-".to_owned()),
    };
    let url = t
        .pr_url
        .as_deref()
        .or(t.issue_url.as_deref())
        .unwrap_or("-")
        .to_owned();
    TargetRow {
        id: t.id.clone(),
        status,
        delivery_mode,
        improvement_pct,
        bench: bench_wall,
        url,
    }
}

fn target_status_text(t: &TargetRecord) -> &'static str {
    if t.status == TargetStatus::Accepted
        && t.reason_code
            .as_deref()
            .is_some_and(|reason| reason.starts_with("mixed:"))
    {
        return "mixed";
    }
    match t.status {
        TargetStatus::Accepted => "accepted",
        TargetStatus::Rejected => "rejected",
        TargetStatus::Aborted => "aborted",
        TargetStatus::Failed => "failed",
    }
}

fn target_cell(r: &TargetRow, col: usize) -> &str {
    match col {
        0 => &r.id,
        1 => &r.status,
        2 => &r.delivery_mode,
        3 => &r.improvement_pct,
        4 => &r.bench,
        5 => &r.url,
        _ => unreachable!(),
    }
}

fn write_padded_cells<'a, W, I>(out: &mut W, cells: I, widths: &[usize; 6]) -> Result<()>
where
    W: Write,
    I: IntoIterator<Item = &'a str>,
{
    let mut first = true;
    let n = widths.len();
    for (i, c) in cells.into_iter().enumerate() {
        if !first {
            write!(out, "  ")?;
        }
        first = false;
        if i + 1 == n {
            write!(out, "{c}")?;
        } else {
            write!(out, "{c:<w$}", w = widths[i])?;
        }
    }
    writeln!(out)?;
    Ok(())
}

fn render_maintenance_section<W: Write>(
    out: &mut W,
    events: &[ProjectedMaintenanceEventV1],
) -> Result<()> {
    writeln!(out, "Maintenance events")?;
    let mut rows = events
        .iter()
        .map(build_maintenance_row)
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        a.observed_at
            .cmp(&b.observed_at)
    });

    const HEADERS: [&str; 5] = ["observed_at", "kind", "target", "state", "url"];
    let widths: [usize; 5] = std::array::from_fn(|col| {
        let header_w = HEADERS[col].len();
        rows.iter()
            .map(|r| maintenance_cell(r, col).len())
            .max()
            .unwrap_or(0)
            .max(header_w)
    });
    write!(out, "  ")?;
    write_maintenance_cells(out, HEADERS.iter().copied(), &widths)?;
    for row in &rows {
        write!(out, "  ")?;
        write_maintenance_cells(
            out,
            (0..HEADERS.len()).map(|c| maintenance_cell(row, c)),
            &widths,
        )?;
    }
    Ok(())
}

struct MaintenanceRow {
    observed_at: String,
    kind: String,
    target: String,
    state: String,
    url: String,
}

fn build_maintenance_row(event: &ProjectedMaintenanceEventV1) -> MaintenanceRow {
    MaintenanceRow {
        observed_at: event.observed_at.clone(),
        kind: maint_kind_text(event.kind).to_owned(),
        target: event
            .target_id
            .clone()
            .unwrap_or_else(|| "-".to_owned()),
        state: event.new_state.clone(),
        url: event
            .pr_url
            .clone()
            .or_else(|| event.issue_url.clone())
            .unwrap_or_else(|| "-".to_owned()),
    }
}

fn maint_kind_text(kind: MaintEventKind) -> &'static str {
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

fn maintenance_cell(r: &MaintenanceRow, col: usize) -> &str {
    match col {
        0 => &r.observed_at,
        1 => &r.kind,
        2 => &r.target,
        3 => &r.state,
        4 => &r.url,
        _ => unreachable!(),
    }
}

fn write_maintenance_cells<'a, W, I>(out: &mut W, cells: I, widths: &[usize; 5]) -> Result<()>
where
    W: Write,
    I: IntoIterator<Item = &'a str>,
{
    let mut first = true;
    let n = widths.len();
    for (i, c) in cells.into_iter().enumerate() {
        if !first {
            write!(out, "  ")?;
        }
        first = false;
        if i + 1 == n {
            write!(out, "{c}")?;
        } else {
            write!(out, "{c:<w$}", w = widths[i])?;
        }
    }
    writeln!(out)?;
    Ok(())
}
