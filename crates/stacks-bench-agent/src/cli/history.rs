//! `sbagent history ...` — read-only views over the archived
//! `sessions.jsonl` ledger.
//!
//! Phase 3 / v6 ships `list` (per-session leaderboard). Phase 4 will add
//! `show <id>` (per-session detail). The shared typed reader lives at
//! [`crate::session::ledger_reader`].
//!
//! Output contract — **ASCII-only by default**, no Unicode glyphs, no
//! ANSI escape codes when stdout is piped. Color (status column only,
//! green/red/yellow) lights up only when stdout is a TTY AND `NO_COLOR`
//! is unset. Fixture tests rely on this for byte-equality assertions.

use std::io::{IsTerminal as _, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::models::common::DeliveryMode;
use crate::models::maintain_event::{MaintEvent, MaintEventKind};
use crate::models::session_record::{SessionRecord, SessionStatus, TargetRecord, TargetStatus};
use crate::session::ledger_reader::{LedgerReadReport, read_all, session_total_secs};
use crate::session::maintain_ledger::read_all as read_maintain;

/// `sbagent history ...`.
#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// Subcommand.
    #[clap(subcommand)]
    pub command: HistoryCommand,
}

/// `sbagent history` subcommands.
#[derive(Debug, Subcommand)]
pub enum HistoryCommand {
    /// One-row-per-session leaderboard view.
    List(ListArgs),
    /// Per-session detail: header + phase-duration bar chart +
    /// per-target table.
    Show(ShowArgs),
}

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

/// `sbagent history show <session-id>` args.
#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Session id (full string, as it appears in
    /// `sessions.jsonl`). Linear scan — operator ledgers stay small
    /// in practice.
    pub session_id: String,
}

/// Dispatch for `sbagent history ...`.
pub async fn run(args: HistoryArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        HistoryCommand::List(a) => run_list(a, ctx),
        HistoryCommand::Show(a) => run_show(a, ctx),
    }
}

fn run_list(args: ListArgs, ctx: &CliContext) -> Result<()> {
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

/// Resolve `<operator_repo_root>/sessions.jsonl`.
fn ledger_path(ctx: &CliContext) -> Result<PathBuf> {
    let root = ctx
        .layout
        .require_operator_repo_root()?;
    Ok(root.join("sessions.jsonl"))
}

/// Truncate `started_at` to its 10-char ISO date prefix
/// (`YYYY-MM-DD`). For malformed values shorter than 10 chars, we
/// just return the whole string — `--since` filtering then becomes
/// a no-op for that record, which is preferable to panicking.
fn started_at_date(record: &SessionRecord) -> &str {
    let s = record.started_at.as_str();
    if s.len() >= 10 { &s[..10] } else { s }
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

/// Render `secs` as `mm:ss` (under an hour) or `hh:mm:ss` (one hour
/// or more). Sub-second values render as `0:00` to keep the column
/// width predictable.
fn format_wall_clock(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let hours = total / 3600;
    let mins = (total % 3600) / 60;
    let s = total % 60;
    if hours > 0 { format!("{hours}:{mins:02}:{s:02}") } else { format!("{mins}:{s:02}") }
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

/// Wrap `s` in the SGR escape for the given session status. Padding
/// is already applied to `s`; the escape codes don't perturb column
/// alignment because they have zero display width.
fn colorize_status(s: &str, status: SessionStatus) -> String {
    let code = match status {
        SessionStatus::Succeeded => "32", // green
        SessionStatus::Failed => "31",    // red
        SessionStatus::Aborted => "33",   // yellow
    };
    format!("\x1b[{code}m{s}\x1b[0m")
}

/// Parse `--since` input into a `YYYY-MM-DD` string for prefix
/// comparison against `started_at`. Accepts:
///
/// - `YYYY-MM-DD` — calendar date, used verbatim.
/// - `YYYY-Www` — ISO 8601 week, converted to the Monday of that week.
fn parse_since(input: &str) -> Result<String> {
    if let Some((y, w)) = parse_iso_week(input) {
        let date = iso_week_monday(y, w)?;
        return Ok(date);
    }
    if is_iso_date(input) {
        return Ok(input.to_owned());
    }
    anyhow::bail!(
        "`--since {input}` is not a recognized form; expected YYYY-MM-DD (e.g. 2026-06-01) or \
         YYYY-Www (e.g. 2026-W23)",
    );
}

/// `YYYY-MM-DD` validator: 10 chars, digits + dashes in the right
/// slots, month 01..=12, day 01..=31. Not a calendar-correctness
/// check — `started_at` is itself ISO 8601, so a syntactically valid
/// prefix is enough for the lexicographic compare.
fn is_iso_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 10 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        match i {
            0..=3 | 5..=6 | 8..=9 => {
                if !b.is_ascii_digit() {
                    return false;
                }
            }
            4 | 7 => {
                if *b != b'-' {
                    return false;
                }
            }
            _ => unreachable!(),
        }
    }
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// Parse `YYYY-Www` into `(year, week)`. Returns `None` if the input
/// isn't in that shape; defers calendar validity (1..=53) to
/// [`iso_week_monday`].
fn parse_iso_week(s: &str) -> Option<(i32, u32)> {
    // Shape: 4 digits, `-W`, 2 digits.
    if s.len() != 8 {
        return None;
    }
    let bytes = s.as_bytes();
    if bytes[4] != b'-' || (bytes[5] != b'W' && bytes[5] != b'w') {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let week: u32 = s[6..8].parse().ok()?;
    Some((year, week))
}

/// Monday of ISO week `week` of `year`, rendered as `YYYY-MM-DD`.
///
/// Uses Howard Hinnant's days-from-civil algorithm
/// (`https://howardhinnant.github.io/date_algorithms.html`) for the
/// gregorian-to-serial conversion in both directions. Valid for
/// year >= 1 and week 1..=53; rejects anything else with a clear
/// error.
fn iso_week_monday(year: i32, week: u32) -> Result<String> {
    if year < 1 {
        anyhow::bail!("--since year {year} is before 0001; expected a 4-digit gregorian year");
    }
    if !(1..=53).contains(&week) {
        anyhow::bail!("--since week {week} out of range; ISO weeks are 01..=53");
    }
    // Jan 4 is always in ISO week 1 (the week containing the first
    // Thursday of the year).
    let jan4_serial = days_from_civil(year, 1, 4);
    // Weekday for the serial: 1970-01-01 was a Thursday. Days are
    // counted from civil epoch; (serial + offset) % 7 gives Mon=0.
    let weekday = weekday_from_serial(jan4_serial); // Mon=0
    let week1_monday_serial = jan4_serial - i64::from(weekday);
    let target_serial = week1_monday_serial + i64::from(week - 1) * 7;
    let (y, m, d) = civil_from_days(target_serial);
    Ok(format!("{y:04}-{m:02}-{d:02}"))
}

/// Days since 1970-01-01 for the given gregorian date. Hinnant's
/// algorithm — exact, branch-light, handles negative years.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(if m <= 2 { y - 1 } else { y });
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let m_u = u64::from(m);
    let d_u = u64::from(d);
    let doy = (153 * if m_u > 2 { m_u - 3 } else { m_u + 9 } + 2) / 5 + d_u - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe as i64 - 719468
}

/// Inverse of [`days_from_civil`]. Returns `(year, month, day)`.
fn civil_from_days(serial: i64) -> (i32, u32, u32) {
    let z = serial + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = (y + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// 0=Mon..6=Sun for a serial returned by [`days_from_civil`].
/// 1970-01-01 was a Thursday (Mon=0 ⇒ Thu=3), and `days_from_civil(1970,1,1) ==
/// 0`.
fn weekday_from_serial(serial: i64) -> u32 {
    let m = serial.rem_euclid(7);
    // m=0 ⇒ Thursday (=3 in Mon=0 mapping), so add 3 then mod 7.
    ((m + 3) % 7) as u32
}

// ─────────────────────────────────────────────────────────────────────
// `sbagent history show <session-id>` — per-session detail view.
// Three sections (header, phase durations, targets); ASCII-only by
// default. Shares wall-clock + color helpers with `history list`.
// ─────────────────────────────────────────────────────────────────────

/// Bar-chart width cap (in `#` characters) for the phase-durations
/// section. Spec calls for `min(terminal_width, 60)`; we use 60
/// unconditionally — for piped output that's the correct ceiling, and
/// for narrow TTYs an occasional wrap is better than pulling in a
/// terminal-size dep just for this.
const BAR_WIDTH: usize = 60;

fn run_show(args: ShowArgs, ctx: &CliContext) -> Result<()> {
    let ledger_path = ledger_path(ctx)?;
    let LedgerReadReport { records, skipped } = read_all(&ledger_path)?;
    for s in &skipped {
        eprintln!(
            "sbagent history: skipping malformed sessions.jsonl line {}: {}",
            s.line_number, s.error,
        );
    }

    let record = match records
        .into_iter()
        .find(|r| r.id == args.session_id)
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
    let maintain_path = ctx
        .layout
        .require_operator_repo_root()?
        .join("maintain.jsonl");
    let maintain = read_maintain(&maintain_path)?;
    for s in &maintain.skipped {
        eprintln!(
            "sbagent history: skipping malformed maintain.jsonl line {}: {}",
            s.line_number, s.error,
        );
    }
    let maint_events = maintain
        .events
        .into_iter()
        .filter(|e| e.session_id == record.id)
        .collect::<Vec<_>>();
    render_show(&mut out, &record, &maint_events, use_color)?;
    Ok(())
}

fn render_show<W: Write>(
    out: &mut W,
    record: &SessionRecord,
    maint_events: &[MaintEvent],
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

fn render_maintenance_section<W: Write>(out: &mut W, events: &[MaintEvent]) -> Result<()> {
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

fn build_maintenance_row(event: &MaintEvent) -> MaintenanceRow {
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

fn session_status_text(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Succeeded => "succeeded",
        SessionStatus::Failed => "failed",
        SessionStatus::Aborted => "aborted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_wall_clock_under_one_hour() {
        assert_eq!(format_wall_clock(0.0), "0:00");
        assert_eq!(format_wall_clock(59.9), "0:59");
        assert_eq!(format_wall_clock(60.0), "1:00");
        assert_eq!(format_wall_clock(125.4), "2:05");
        assert_eq!(format_wall_clock(3599.0), "59:59");
    }

    #[test]
    fn format_wall_clock_over_one_hour() {
        assert_eq!(format_wall_clock(3600.0), "1:00:00");
        assert_eq!(format_wall_clock(3661.0), "1:01:01");
        assert_eq!(format_wall_clock(45_296.0), "12:34:56");
    }

    #[test]
    fn is_iso_date_accepts_well_formed() {
        assert!(is_iso_date("2026-06-01"));
        assert!(is_iso_date("2026-12-31"));
    }

    #[test]
    fn is_iso_date_rejects_garbage() {
        assert!(!is_iso_date("2026/06/01"));
        assert!(!is_iso_date("2026-13-01"));
        assert!(!is_iso_date("2026-06-32"));
        assert!(!is_iso_date("2026-06-1"));
        assert!(!is_iso_date(""));
    }

    #[test]
    fn parse_iso_week_round_trips() {
        // 2026-W23 begins on Monday 2026-06-01 (verified externally).
        assert_eq!(iso_week_monday(2026, 23).unwrap(), "2026-06-01");
        // 2026-W01 begins on Monday 2025-12-29 (ISO week 1 contains
        // Jan 4 — for 2026 that's a Sunday in week 1, so Monday of
        // week 1 falls in the prior calendar year).
        assert_eq!(iso_week_monday(2026, 1).unwrap(), "2025-12-29");
        // 2020-W01 begins on Monday 2019-12-30.
        assert_eq!(iso_week_monday(2020, 1).unwrap(), "2019-12-30");
        // 2025-W01 begins on Monday 2024-12-30.
        assert_eq!(iso_week_monday(2025, 1).unwrap(), "2024-12-30");
    }

    #[test]
    fn parse_since_accepts_both_forms() {
        assert_eq!(parse_since("2026-06-01").unwrap(), "2026-06-01");
        assert_eq!(parse_since("2026-W23").unwrap(), "2026-06-01");
        assert_eq!(parse_since("2026-w23").unwrap(), "2026-06-01"); // lowercase tolerated
    }

    #[test]
    fn parse_since_rejects_garbage() {
        assert!(parse_since("garbage").is_err());
        assert!(parse_since("2026/06/01").is_err());
        assert!(parse_since("2026-W00").is_err());
        assert!(parse_since("2026-W54").is_err());
    }

    #[test]
    fn days_from_civil_round_trips() {
        // Epoch sanity.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // Random midpoint date.
        let serial = days_from_civil(2026, 6, 11);
        assert_eq!(civil_from_days(serial), (2026, 6, 11));
        // 1970-01-01 was Thursday (Mon=0 ⇒ Thu=3).
        assert_eq!(weekday_from_serial(0), 3);
        // 2026-06-01 was Monday.
        assert_eq!(weekday_from_serial(days_from_civil(2026, 6, 1)), 0);
    }
}
