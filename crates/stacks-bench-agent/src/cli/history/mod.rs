//! `sbagent history ...` — read-only views over the archived
//! `sessions.jsonl` ledger.
//!
//! Submodules:
//! - [`list`] — per-session leaderboard table.
//! - [`show`] — per-session detail (header, phase bars, targets, maintenance
//!   events).
//! - [`report`] — v17 markdown digest (typed `ReportViewV1` + range selection
//!   helpers).
//!
//! Output contract — **ASCII-only by default**, no Unicode glyphs, no
//! ANSI escape codes when stdout is piped. Color (status column only,
//! green/red/yellow) lights up only when stdout is a TTY AND `NO_COLOR`
//! is unset. Fixture tests rely on this for byte-equality assertions.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::CliContext;
use crate::models::session_record::{SessionRecord, SessionStatus};

pub mod list;
pub mod report;
pub mod show;

pub use list::ListArgs;
pub use report::ReportArgs;
pub use show::ShowArgs;

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
    /// Markdown digest across multiple sessions, suitable for
    /// stdout, commits, and copy/paste.
    Report(ReportArgs),
}

/// Dispatch for `sbagent history ...`.
pub async fn run(args: HistoryArgs, ctx: &CliContext) -> Result<()> {
    match args.command {
        HistoryCommand::List(a) => list::run_list(a, ctx),
        HistoryCommand::Show(a) => show::run_show(a, ctx),
        HistoryCommand::Report(a) => report::run_report(a, ctx),
    }
}

/// Resolve `<operator_repo_root>/sessions.jsonl`.
pub(super) fn ledger_path(ctx: &CliContext) -> Result<PathBuf> {
    let root = ctx
        .layout
        .require_operator_repo_root()?;
    Ok(root.join("sessions.jsonl"))
}

/// Truncate `started_at` to its 10-char ISO date prefix
/// (`YYYY-MM-DD`). For malformed values shorter than 10 chars, we
/// just return the whole string — `--since` filtering then becomes
/// a no-op for that record, which is preferable to panicking.
pub(super) fn started_at_date(record: &SessionRecord) -> &str {
    let s = record.started_at.as_str();
    if s.len() >= 10 { &s[..10] } else { s }
}

/// Render `secs` as `mm:ss` (under an hour) or `hh:mm:ss` (one hour
/// or more). Sub-second values render as `0:00` to keep the column
/// width predictable.
pub(super) fn format_wall_clock(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let hours = total / 3600;
    let mins = (total % 3600) / 60;
    let s = total % 60;
    if hours > 0 { format!("{hours}:{mins:02}:{s:02}") } else { format!("{mins}:{s:02}") }
}

/// Stable snake_case rendering of [`SessionStatus`].
pub(super) fn session_status_text(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Succeeded => "succeeded",
        SessionStatus::Failed => "failed",
        SessionStatus::Aborted => "aborted",
    }
}

/// Wrap `s` in the SGR escape for the given session status. Padding
/// is already applied to `s`; the escape codes don't perturb column
/// alignment because they have zero display width.
pub(super) fn colorize_status(s: &str, status: SessionStatus) -> String {
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
///
/// Shared by `history list` and `history report`. Do NOT duplicate
/// this parser; both subcommands must accept identical inputs so
/// operators can carry the same `--since` value across views.
pub(super) fn parse_since(input: &str) -> Result<String> {
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
pub(super) fn iso_week_monday(year: i32, week: u32) -> Result<String> {
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
pub(super) fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
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
pub(super) fn civil_from_days(serial: i64) -> (i32, u32, u32) {
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
pub(super) fn weekday_from_serial(serial: i64) -> u32 {
    let m = serial.rem_euclid(7);
    // m=0 ⇒ Thursday (=3 in Mon=0 mapping), so add 3 then mod 7.
    ((m + 3) % 7) as u32
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
