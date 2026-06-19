//! `sbagent history report` — v17 markdown digest of archived sessions.
//!
//! Phase 1 defined the typed view + range selection helpers; Phase 2
//! (this addition) plugs in [`render_markdown`], a pure
//! `&ReportViewV1 -> String` renderer that produces the operator
//! digest. Phase 3 will wire it into a CLI subcommand.
//!
//! v17 consumes [`HistoryProjectionV1`] only — it does not read
//! `sessions.jsonl` or `maintain.jsonl` directly, and it does not look
//! at per-session archive branch artifacts. Phase 1 verified that v12
//! dedup decisions are NOT visible through the projection (they live
//! in `optimization-targets.json` under `rejected_by_merge[]` in the
//! archive branch, not in `sessions.jsonl`). v17 therefore omits the
//! `Dedup skips` section; surfacing it is tracked as a follow-up.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Args;

use crate::cli::CliContext;
use crate::models::common::DeliveryMode;
use crate::models::maintain_event::MaintEventKind;
use crate::models::session_record::{SessionRecord, SessionStatus, TargetStatus};
use crate::session::history_projection::{self, HistoryProjectionV1};
use crate::session::ledger_reader::session_total_secs;

/// `sbagent history report` args.
#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Inclusive lower bound on `started_at`. Accepts the same forms
    /// as `history list --since`: ISO 8601 calendar date
    /// (`YYYY-MM-DD`) or ISO 8601 week id (`YYYY-Www`, e.g.
    /// `2026-W23`). When omitted, the default is the Monday of the
    /// ISO week containing the most recent archived session.
    #[clap(long)]
    pub since: Option<String>,

    /// Destination file for the rendered markdown. When set, the
    /// renderer writes the report to this path (creating parent
    /// directories as needed) and keeps stdout empty. When absent,
    /// the report prints to stdout.
    #[clap(long)]
    pub out: Option<PathBuf>,
}

/// Dispatch entry for `sbagent history report`.
pub(super) fn run_report(args: ReportArgs, ctx: &CliContext) -> Result<()> {
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

    let cutoff = match args.since.as_deref() {
        Some(input) => parse_explicit_cutoff(input)?,
        None => default_cutoff_for_projection(&history.projection),
    };
    let view = ReportViewV1::build_v1(&history.projection, cutoff);
    let markdown = render_markdown(&view);

    match args.out {
        Some(path) => {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("creating parent directories for --out {}", path.display())
                })?;
            }
            std::fs::write(&path, &markdown)
                .with_context(|| format!("writing history report to --out {}", path.display()))?;
            // Stdout stays empty so scripted callers see exactly the
            // path they passed; warnings on stderr are still allowed.
        }
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(markdown.as_bytes())?;
        }
    }
    Ok(())
}

/// What `--since` resolved to. Phase 2 renders the report header with
/// both the date and how it was chosen so operators can tell a default
/// digest apart from one they explicitly filtered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinceCutoff {
    /// Inclusive lower bound as `YYYY-MM-DD`. Compared lexicographically
    /// against [`SessionRecord::started_at`] truncated to the same width.
    pub date: String,
    /// How this cutoff was selected.
    pub source: CutoffSource,
}

/// Provenance of a [`SinceCutoff`]. Determines the report header label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutoffSource {
    /// Operator passed `--since <ref>`.
    Explicit,
    /// Default selection: Monday of the ISO week containing the most-recent
    /// archived session's `started_at`. Falls back to [`Self::EmptyLedger`]
    /// when the projection has zero sessions.
    DefaultLatestWeek,
    /// Default selection but the projection is empty. Cutoff date is the
    /// epoch sentinel `0000-00-00`; renderer prints "no sessions archived
    /// yet" and the rest of the view is empty.
    EmptyLedger,
}

/// Typed view that the Phase 2 markdown renderer consumes. Read-only.
///
/// Every field is derived deterministically from
/// [`HistoryProjectionV1`] + a [`SinceCutoff`]. Re-running
/// [`Self::build_v1`] against the same inputs produces an equal view.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportViewV1 {
    /// Range selection that produced this view.
    pub cutoff: SinceCutoff,
    /// Session-level outcome rollup across the selected sessions.
    pub session_counts: SessionCounts,
    /// Selected sessions in newest-first order by `started_at`. Same
    /// columns as `history list` so operators recognize the schema.
    pub sessions: Vec<ReportSession>,
    /// Target outcome rollup across all selected sessions. Matches
    /// `history list`'s A/R/Ab convention: `Failed` folds into
    /// `aborted`.
    pub target_outcomes: TargetOutcomeCounts,
    /// Total wall-clock seconds summed across all selected sessions.
    pub total_wall_clock_secs: f64,
    /// PR lifecycle rows with latest observation state. Includes only
    /// URLs that maintain has observed; never synthesizes state from
    /// URL presence alone.
    pub pr_lifecycle: Vec<LifecycleRow>,
    /// Issue lifecycle rows with latest observation state. Same shape
    /// + presence rule as [`Self::pr_lifecycle`].
    pub issue_lifecycle: Vec<LifecycleRow>,
    /// Per-target rows for accepted targets whose `reason_code` is a
    /// `mixed:` verdict. Sorted by `(session_id, target_id)`. Empty
    /// when no selected session carries a mixed verdict; renderer
    /// omits the section entirely in that case.
    pub mixed_verdicts: Vec<MixedVerdictRow>,
}

/// One row in the `Mixed verdicts` section. Identifies the session,
/// the target, and the raw `mixed: ...` reason string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixedVerdictRow {
    pub session_id: String,
    pub target_id: String,
    /// Raw `reason_code` value (starts with `mixed:`). Rendered verbatim
    /// so operators see the exact caveat finalize recorded.
    pub reason: String,
}

/// Per-session row matching the `history list` column set.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportSession {
    pub id: String,
    pub started_at: String,
    pub status: SessionStatus,
    pub target_outcomes: TargetOutcomeCounts,
    pub wall_clock_secs: f64,
    /// Count of targets in this session that carry a `pr_url`. Matches
    /// `history list`'s `prs` column — counts publishes, not lifecycle.
    pub pr_count: usize,
    /// Count of targets in this session that carry an `issue_url`.
    pub issue_count: usize,
}

/// Session-status rollup for the Summary section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionCounts {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub aborted: usize,
}

/// Target-outcome rollup. `Failed` targets fold into `aborted`, matching
/// `history list`'s A/R/Ab convention. `mixed` is a SUBSET of
/// `accepted` (mixed-verdict targets shipped but the verdict carried a
/// caveat) — it never adds to the A/R/Ab buckets and is surfaced
/// separately in the renderer (summary annotation + `Mixed verdicts`
/// section).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TargetOutcomeCounts {
    pub accepted: usize,
    pub rejected: usize,
    pub aborted: usize,
    /// Subset of [`Self::accepted`]: targets with status `accepted`
    /// whose `reason_code` starts with `mixed:`. Matches the
    /// classification rule `history show` uses for the per-target
    /// status cell.
    pub mixed: usize,
}

/// One PR or issue lifecycle row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleRow {
    /// PR or issue URL.
    pub url: String,
    /// Session that published the artifact.
    pub session_id: String,
    /// Latest observed state string ("open", "merged", "closed_unmerged",
    /// "stale", "branch_deleted", "force_pushed", "closed", etc.). Comes
    /// from `maintain`'s `new_state` field via the projection.
    pub latest_state: String,
    /// Kind of the latest lifecycle event.
    pub latest_kind: MaintEventKind,
    /// ISO 8601 timestamp of the latest observation.
    pub observed_at: String,
    /// Delivery mode of the target that published this artifact, when
    /// the report can derive it from the per-session target row.
    pub delivery_mode: Option<DeliveryMode>,
}

impl ReportViewV1 {
    /// Build the v1 typed view from a projection + cutoff. Pure function:
    /// same inputs → same view.
    pub fn build_v1(projection: &HistoryProjectionV1, since: SinceCutoff) -> Self {
        // Select sessions on or after the cutoff date (10-char prefix
        // compare against `started_at`).
        let mut selected: Vec<&SessionRecord> = projection
            .sessions()
            .iter()
            .filter(|s| started_at_prefix(&s.started_at) >= since.date.as_str())
            .collect();
        // Newest-first by `started_at`. ISO 8601 sorts lexicographically.
        selected.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
        });

        let mut session_counts = SessionCounts::default();
        let mut target_outcomes = TargetOutcomeCounts::default();
        let mut total_wall_clock_secs = 0.0_f64;
        let mut sessions = Vec::with_capacity(selected.len());
        // Build lifecycle rows from each selected session's targets +
        // the projection's `latest_artifact_state`. Skip targets whose
        // URL has no maintain observation — the v17 contract is "state
        // comes from projection, not from URL presence alone."
        let mut pr_lifecycle: Vec<LifecycleRow> = Vec::new();
        let mut issue_lifecycle: Vec<LifecycleRow> = Vec::new();
        let mut mixed_verdicts: Vec<MixedVerdictRow> = Vec::new();

        for s in &selected {
            session_counts.total += 1;
            match s.status {
                SessionStatus::Succeeded => session_counts.succeeded += 1,
                SessionStatus::Failed => session_counts.failed += 1,
                SessionStatus::Aborted => session_counts.aborted += 1,
            }
            let wall_clock_secs = session_total_secs(s);
            total_wall_clock_secs += wall_clock_secs;

            let mut per_session_outcomes = TargetOutcomeCounts::default();
            let mut pr_count = 0usize;
            let mut issue_count = 0usize;
            for t in &s.targets {
                match t.status {
                    TargetStatus::Accepted => per_session_outcomes.accepted += 1,
                    TargetStatus::Rejected => per_session_outcomes.rejected += 1,
                    TargetStatus::Aborted | TargetStatus::Failed => {
                        per_session_outcomes.aborted += 1;
                    }
                }
                // Mixed verdict: accepted target with a `mixed:` reason
                // code. Same classification rule as
                // `super::show::target_status_text`. Mixed is a SUBSET
                // of accepted — it does not change the A/R/Ab buckets.
                if t.status == TargetStatus::Accepted
                    && let Some(reason) = t.reason_code.as_deref()
                    && reason.starts_with("mixed:")
                {
                    per_session_outcomes.mixed += 1;
                    mixed_verdicts.push(MixedVerdictRow {
                        session_id: s.id.clone(),
                        target_id: t.id.clone(),
                        reason: reason.to_owned(),
                    });
                }
                if let Some(url) = t.pr_url.as_deref() {
                    pr_count += 1;
                    if let Some(state) = projection.latest_artifact_state(url) {
                        pr_lifecycle.push(LifecycleRow {
                            url: url.to_owned(),
                            session_id: s.id.clone(),
                            latest_state: state.new_state.clone(),
                            latest_kind: state.kind,
                            observed_at: state.observed_at.clone(),
                            delivery_mode: Some(t.delivery_mode),
                        });
                    }
                }
                if let Some(url) = t.issue_url.as_deref() {
                    issue_count += 1;
                    if let Some(state) = projection.latest_artifact_state(url) {
                        issue_lifecycle.push(LifecycleRow {
                            url: url.to_owned(),
                            session_id: s.id.clone(),
                            latest_state: state.new_state.clone(),
                            latest_kind: state.kind,
                            observed_at: state.observed_at.clone(),
                            delivery_mode: Some(t.delivery_mode),
                        });
                    }
                }
            }
            target_outcomes.accepted += per_session_outcomes.accepted;
            target_outcomes.rejected += per_session_outcomes.rejected;
            target_outcomes.aborted += per_session_outcomes.aborted;
            target_outcomes.mixed += per_session_outcomes.mixed;
            sessions.push(ReportSession {
                id: s.id.clone(),
                started_at: s.started_at.clone(),
                status: s.status,
                target_outcomes: per_session_outcomes,
                wall_clock_secs,
                pr_count,
                issue_count,
            });
        }

        // Stable ordering for lifecycle sections: group by latest state,
        // then by URL within each state group. State grouping is by the
        // raw `new_state` string ordered lexicographically.
        pr_lifecycle.sort_by(|a, b| {
            a.latest_state
                .cmp(&b.latest_state)
                .then_with(|| a.url.cmp(&b.url))
        });
        issue_lifecycle.sort_by(|a, b| {
            a.latest_state
                .cmp(&b.latest_state)
                .then_with(|| a.url.cmp(&b.url))
        });
        // Stable ordering for mixed verdicts: group by session, then by
        // target id. Session order alone would be ambiguous when one
        // session has multiple mixed targets.
        mixed_verdicts.sort_by(|a, b| {
            a.session_id
                .cmp(&b.session_id)
                .then_with(|| a.target_id.cmp(&b.target_id))
        });

        Self {
            cutoff: since,
            session_counts,
            sessions,
            target_outcomes,
            total_wall_clock_secs,
            pr_lifecycle,
            issue_lifecycle,
            mixed_verdicts,
        }
    }
}

/// Resolve the default cutoff: Monday of the ISO week containing the
/// most-recent archived session's `started_at`. Falls back to
/// [`CutoffSource::EmptyLedger`] when the projection has no sessions.
///
/// Operators with sparse archives get the latest week containing
/// activity, not "now − 7 days." For a digest, "what HAPPENED" is the
/// right framing.
pub fn default_cutoff_for_projection(projection: &HistoryProjectionV1) -> SinceCutoff {
    let latest = projection
        .sessions()
        .iter()
        .map(|s| started_at_prefix(&s.started_at))
        .max();
    let Some(latest) = latest else {
        return SinceCutoff {
            date: "0000-00-00".to_owned(),
            source: CutoffSource::EmptyLedger,
        };
    };
    // Convert the latest `YYYY-MM-DD` into its ISO week's Monday. Reuse
    // the existing parsers from the parent module so list / show /
    // report all interpret the calendar the same way.
    let date = monday_of_iso_week_containing(latest).unwrap_or_else(|| latest.to_owned());
    SinceCutoff {
        date,
        source: CutoffSource::DefaultLatestWeek,
    }
}

/// Resolve an explicit `--since` value. Reuses the parent module's
/// `parse_since` helper so list and report accept identical syntax;
/// the returned cutoff carries [`CutoffSource::Explicit`].
pub fn parse_explicit_cutoff(input: &str) -> anyhow::Result<SinceCutoff> {
    let date = super::parse_since(input)?;
    Ok(SinceCutoff {
        date,
        source: CutoffSource::Explicit,
    })
}

/// Render the v17 markdown report from a typed view. Pure function;
/// no IO, no clock reads.
///
/// Output is deterministic, pure ASCII, and never contains ANSI
/// escapes — the report is meant for piping into files, commits, and
/// copy/paste. Section order: `Summary`, `Sessions`, `Pull requests`
/// (when present), `Issues` (when present). The `Dedup skips`
/// section is omitted in v17 — see the module doc.
///
/// When the view selects zero sessions (empty ledger OR no rows
/// after `--since`), the renderer emits just the title and the
/// `no sessions archived yet` notice — matching `history list`'s
/// empty-ledger phrasing so the two commands stay coherent.
pub fn render_markdown(view: &ReportViewV1) -> String {
    let mut out = String::new();
    write_title(&mut out, &view.cutoff);
    if view.sessions.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "no sessions archived yet").unwrap();
        return out;
    }
    write_summary(&mut out, view);
    write_sessions(&mut out, view);
    if !view.pr_lifecycle.is_empty() {
        write_lifecycle_section(&mut out, "Pull requests", &view.pr_lifecycle);
    }
    if !view
        .issue_lifecycle
        .is_empty()
    {
        write_lifecycle_section(&mut out, "Issues", &view.issue_lifecycle);
    }
    if !view.mixed_verdicts.is_empty() {
        write_mixed_verdicts_section(&mut out, &view.mixed_verdicts);
    }
    out
}

fn write_title(out: &mut String, cutoff: &SinceCutoff) {
    match cutoff.source {
        CutoffSource::Explicit => {
            writeln!(out, "# sbagent history report: since {}", cutoff.date).unwrap();
        }
        CutoffSource::DefaultLatestWeek => {
            writeln!(
                out,
                "# sbagent history report: since {} (default: latest ISO week)",
                cutoff.date,
            )
            .unwrap();
        }
        CutoffSource::EmptyLedger => {
            writeln!(out, "# sbagent history report: empty ledger").unwrap();
        }
    }
}

fn write_summary(out: &mut String, view: &ReportViewV1) {
    let SessionCounts {
        total,
        succeeded,
        failed,
        aborted,
    } = view.session_counts;
    let TargetOutcomeCounts {
        accepted,
        rejected,
        aborted: t_aborted,
        mixed,
    } = view.target_outcomes;
    writeln!(out).unwrap();
    writeln!(out, "## Summary").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "- Total sessions: {total} (succeeded: {succeeded}, failed: {failed}, aborted: {aborted})",
    )
    .unwrap();
    // Mixed annotation is omitted when no mixed verdicts shipped, so
    // the dense common case keeps the canonical `A / R / Ab` shape.
    let accepted_cell =
        if mixed > 0 { format!("{accepted} (mixed {mixed})") } else { accepted.to_string() };
    writeln!(
        out,
        "- Targets: accepted {accepted_cell} / rejected {rejected} / aborted {t_aborted}",
    )
    .unwrap();
    writeln!(out, "- Total wall-clock: {}", super::format_wall_clock(view.total_wall_clock_secs),)
        .unwrap();
}

fn write_sessions(out: &mut String, view: &ReportViewV1) {
    writeln!(out).unwrap();
    writeln!(out, "## Sessions").unwrap();
    writeln!(out).unwrap();
    let rows: Vec<[String; 7]> = view
        .sessions
        .iter()
        .map(|s| {
            [
                s.id.clone(),
                started_at_prefix(&s.started_at).to_owned(),
                super::session_status_text(s.status).to_owned(),
                format!(
                    "{}/{}/{}",
                    s.target_outcomes.accepted,
                    s.target_outcomes.rejected,
                    s.target_outcomes.aborted,
                ),
                super::format_wall_clock(s.wall_clock_secs),
                s.pr_count.to_string(),
                s.issue_count.to_string(),
            ]
        })
        .collect();
    const HEADERS: [&str; 7] =
        ["id", "started_at", "status", "targets", "wall-clock", "prs", "issues"];
    write_markdown_table(out, &HEADERS, &rows);
}

fn write_lifecycle_section(out: &mut String, title: &str, rows: &[LifecycleRow]) {
    writeln!(out).unwrap();
    writeln!(out, "## {title}").unwrap();
    writeln!(out).unwrap();
    let body: Vec<[String; 4]> = rows
        .iter()
        .map(|r| {
            [r.latest_state.clone(), r.url.clone(), r.session_id.clone(), r.observed_at.clone()]
        })
        .collect();
    const HEADERS: [&str; 4] = ["state", "url", "session", "observed_at"];
    write_markdown_table(out, &HEADERS, &body);
}

fn write_mixed_verdicts_section(out: &mut String, rows: &[MixedVerdictRow]) {
    writeln!(out).unwrap();
    writeln!(out, "## Mixed verdicts").unwrap();
    writeln!(out).unwrap();
    let body: Vec<[String; 3]> = rows
        .iter()
        .map(|r| [r.session_id.clone(), r.target_id.clone(), r.reason.clone()])
        .collect();
    const HEADERS: [&str; 3] = ["session", "target", "reason"];
    write_markdown_table(out, &HEADERS, &body);
}

/// Render a GitHub-flavored markdown table with column widths sized
/// to the longest cell in each column. Header separator uses at least
/// 4 dashes per column so the raw text is readable when grepped.
fn write_markdown_table<const N: usize>(
    out: &mut String,
    headers: &[&str; N],
    rows: &[[String; N]],
) {
    let widths: [usize; N] = std::array::from_fn(|col| {
        let header_w = headers[col].len();
        rows.iter()
            .map(|r| r[col].len())
            .max()
            .unwrap_or(0)
            .max(header_w)
            .max(4)
    });
    write_table_row(
        out,
        headers
            .iter()
            .copied()
            .map(str::to_owned),
        &widths,
    );
    let dashes: Vec<String> = widths
        .iter()
        .map(|w| "-".repeat(*w))
        .collect();
    write_table_row(out, dashes, &widths);
    for row in rows {
        write_table_row(out, row.iter().cloned(), &widths);
    }
}

fn write_table_row<I, const N: usize>(out: &mut String, cells: I, widths: &[usize; N])
where
    I: IntoIterator<Item = String>,
{
    out.push('|');
    for (i, cell) in cells.into_iter().enumerate() {
        write!(out, " {cell:<w$} |", w = widths[i]).unwrap();
    }
    out.push('\n');
}

/// Truncate `started_at` to its 10-char ISO date prefix. Matches
/// [`super::started_at_date`] but operates on `&str` directly so this
/// module can pull it out of any projection field that exposes the
/// timestamp as a string.
fn started_at_prefix(s: &str) -> &str {
    if s.len() >= 10 { &s[..10] } else { s }
}

/// Monday of the ISO week containing `date` (a `YYYY-MM-DD` string).
/// Returns `None` for malformed input — the caller falls back to the
/// raw date, which is still a valid cutoff (just not week-aligned).
fn monday_of_iso_week_containing(date: &str) -> Option<String> {
    if date.len() < 10 {
        return None;
    }
    let y: i32 = date[0..4].parse().ok()?;
    let m: u32 = date[5..7].parse().ok()?;
    let d: u32 = date[8..10].parse().ok()?;
    let serial = super::days_from_civil(y, m, d);
    // weekday: 0=Mon..6=Sun. Monday of this week = serial - weekday.
    let weekday = super::weekday_from_serial(serial);
    let monday_serial = serial - i64::from(weekday);
    let (my, mm, md) = super::civil_from_days(monday_serial);
    Some(format!("{my:04}-{mm:02}-{md:02}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::models::common::{DeliveryMode, SchemaVersionV1, SchemaVersionV3};
    use crate::models::maintain_event::{MaintEvent, MaintEventKind};
    use crate::models::session_record::{
        SessionRange, SessionRecord, SessionRecordKind, SessionStatus, TargetRecord, TargetStatus,
    };

    fn session(id: &str, started_at: &str, status: SessionStatus) -> SessionRecord {
        SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: SchemaVersionV3,
            id: id.to_owned(),
            artifact_branch: format!("session/{id}"),
            artifact_sha: "0".repeat(40),
            artifact_url: None,
            started_at: started_at.to_owned(),
            finished_at: started_at.to_owned(),
            status,
            failure_phase: None,
            failure_reason: None,
            sbagent_version: "0.0.0".to_owned(),
            sbagent_git_sha: None,
            range: SessionRange {
                start_at: None,
                count: None,
                warmup: None,
                filter: None,
                network: "mainnet".to_owned(),
            },
            baseline_run_ids: vec![],
            phase_durations_secs: BTreeMap::new(),
            targets: vec![],
            source_url: None,
            source_branch: None,
            source_sha: None,
            source_fetched_at: None,
        }
    }

    fn target(id: &str, family: &str, status: TargetStatus, pr: Option<&str>) -> TargetRecord {
        TargetRecord {
            id: id.to_owned(),
            family_id: family.to_owned(),
            bucket: "block_processing".to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            status,
            status_stage: match status {
                TargetStatus::Accepted => None,
                TargetStatus::Rejected => {
                    Some(crate::models::session_record::TargetStatusStage::Bench)
                }
                TargetStatus::Aborted | TargetStatus::Failed => {
                    Some(crate::models::session_record::TargetStatusStage::Optimizer)
                }
            },
            reason_code: None,
            head_sha: None,
            pr_url: pr.map(|s| s.to_owned()),
            issue_url: None,
            bench: None,
        }
    }

    fn maint_event(
        session_id: &str,
        pr_url: &str,
        kind: MaintEventKind,
        new_state: &str,
        observed_at: &str,
    ) -> MaintEvent {
        MaintEvent {
            schema_version: SchemaVersionV1,
            kind,
            observed_at: observed_at.to_owned(),
            session_id: session_id.to_owned(),
            target_id: None,
            family_id: None,
            fix_signature: None,
            pr_url: Some(pr_url.to_owned()),
            issue_url: None,
            prior_state: None,
            new_state: new_state.to_owned(),
            head_sha: None,
        }
    }

    #[test]
    fn default_cutoff_empty_projection_renders_epoch_sentinel() {
        let projection = HistoryProjectionV1::from_ledgers_v1(&[], &[]);
        let cutoff = default_cutoff_for_projection(&projection);
        assert_eq!(cutoff.date, "0000-00-00");
        assert_eq!(cutoff.source, CutoffSource::EmptyLedger);
    }

    #[test]
    fn default_cutoff_picks_monday_of_iso_week_containing_latest_session() {
        // 2026-06-15 is a Monday; ISO week 25's Monday is 2026-06-15 itself.
        // 2026-06-17 is the Wednesday of week 25; Monday of that week is 2026-06-15.
        let sessions = vec![
            session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded),
            session("b", "2026-06-17T00:00:00Z", SessionStatus::Succeeded),
        ];
        let projection = HistoryProjectionV1::from_ledgers_v1(&sessions, &[]);
        let cutoff = default_cutoff_for_projection(&projection);
        assert_eq!(cutoff.date, "2026-06-15");
        assert_eq!(cutoff.source, CutoffSource::DefaultLatestWeek);
    }

    #[test]
    fn build_v1_selects_sessions_on_or_after_cutoff_newest_first() {
        let sessions = vec![
            session("old", "2026-05-01T00:00:00Z", SessionStatus::Succeeded),
            session("mid", "2026-06-01T00:00:00Z", SessionStatus::Failed),
            session("new", "2026-06-10T00:00:00Z", SessionStatus::Succeeded),
        ];
        let projection = HistoryProjectionV1::from_ledgers_v1(&sessions, &[]);
        let cutoff = SinceCutoff {
            date: "2026-06-01".to_owned(),
            source: CutoffSource::Explicit,
        };
        let view = ReportViewV1::build_v1(&projection, cutoff);
        assert_eq!(view.sessions.len(), 2);
        // Newest first.
        assert_eq!(view.sessions[0].id, "new");
        assert_eq!(view.sessions[1].id, "mid");
        assert_eq!(view.session_counts.total, 2);
        assert_eq!(view.session_counts.succeeded, 1);
        assert_eq!(view.session_counts.failed, 1);
        assert_eq!(view.session_counts.aborted, 0);
    }

    #[test]
    fn build_v1_rolls_up_target_outcomes_with_failed_folded_into_aborted() {
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets
            .push(target("t1", "fam", TargetStatus::Accepted, None));
        s.targets
            .push(target("t2", "fam", TargetStatus::Rejected, None));
        s.targets
            .push(target("t3", "fam", TargetStatus::Aborted, None));
        s.targets
            .push(target("t4", "fam", TargetStatus::Failed, None));
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &[]);
        let cutoff = SinceCutoff {
            date: "2026-01-01".to_owned(),
            source: CutoffSource::Explicit,
        };
        let view = ReportViewV1::build_v1(&projection, cutoff);
        assert_eq!(view.target_outcomes.accepted, 1);
        assert_eq!(view.target_outcomes.rejected, 1);
        assert_eq!(view.target_outcomes.aborted, 2); // Failed folded in.
        assert_eq!(
            view.sessions[0]
                .target_outcomes
                .accepted,
            1
        );
        assert_eq!(
            view.sessions[0]
                .target_outcomes
                .rejected,
            1
        );
        assert_eq!(
            view.sessions[0]
                .target_outcomes
                .aborted,
            2
        );
    }

    #[test]
    fn build_v1_lifecycle_rows_require_projection_state_not_just_url_presence() {
        // Two PR URLs in sessions.jsonl, only one observed by maintain.
        // The observed one shows up in pr_lifecycle; the other does not.
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets.push(target(
            "t1",
            "fam",
            TargetStatus::Accepted,
            Some("https://example.com/pull/1"),
        ));
        s.targets.push(target(
            "t2",
            "fam",
            TargetStatus::Accepted,
            Some("https://example.com/pull/2"),
        ));
        let maintain = vec![maint_event(
            "a",
            "https://example.com/pull/1",
            MaintEventKind::PrMerged,
            "merged",
            "2026-06-12T00:00:00Z",
        )];
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &maintain);
        let cutoff = SinceCutoff {
            date: "2026-01-01".to_owned(),
            source: CutoffSource::Explicit,
        };
        let view = ReportViewV1::build_v1(&projection, cutoff);
        // Per-session table counts BOTH PRs (matches history list's `prs`).
        assert_eq!(view.sessions[0].pr_count, 2);
        // Lifecycle section only has the observed one.
        assert_eq!(view.pr_lifecycle.len(), 1);
        assert_eq!(view.pr_lifecycle[0].url, "https://example.com/pull/1");
        assert_eq!(view.pr_lifecycle[0].latest_state, "merged");
        assert_eq!(view.pr_lifecycle[0].latest_kind, MaintEventKind::PrMerged);
    }

    #[test]
    fn build_v1_lifecycle_rows_sort_by_state_then_url() {
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        for (i, url) in ["https://x/pull/1", "https://x/pull/2", "https://x/pull/3"]
            .iter()
            .enumerate()
        {
            s.targets.push(target(
                &format!("t{}", i + 1),
                "fam",
                TargetStatus::Accepted,
                Some(url),
            ));
        }
        let maintain = vec![
            maint_event(
                "a",
                "https://x/pull/2",
                MaintEventKind::PrOpen,
                "open",
                "2026-06-11T00:00:00Z",
            ),
            maint_event(
                "a",
                "https://x/pull/3",
                MaintEventKind::PrMerged,
                "merged",
                "2026-06-13T00:00:00Z",
            ),
            maint_event(
                "a",
                "https://x/pull/1",
                MaintEventKind::PrOpen,
                "open",
                "2026-06-12T00:00:00Z",
            ),
        ];
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &maintain);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-01-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        // Grouped by state ("merged" < "open" lexicographically), then by URL.
        assert_eq!(view.pr_lifecycle.len(), 3);
        assert_eq!(view.pr_lifecycle[0].latest_state, "merged");
        assert_eq!(view.pr_lifecycle[0].url, "https://x/pull/3");
        assert_eq!(view.pr_lifecycle[1].latest_state, "open");
        assert_eq!(view.pr_lifecycle[1].url, "https://x/pull/1");
        assert_eq!(view.pr_lifecycle[2].latest_state, "open");
        assert_eq!(view.pr_lifecycle[2].url, "https://x/pull/2");
    }

    #[test]
    fn parse_explicit_cutoff_supports_iso_date_and_iso_week() {
        let c = parse_explicit_cutoff("2026-06-01").unwrap();
        assert_eq!(c.date, "2026-06-01");
        assert_eq!(c.source, CutoffSource::Explicit);

        let c = parse_explicit_cutoff("2026-W23").unwrap();
        assert_eq!(c.date, "2026-06-01");
        assert_eq!(c.source, CutoffSource::Explicit);
    }

    #[test]
    fn monday_of_iso_week_containing_returns_monday() {
        // 2026-06-17 is Wednesday; Monday of that week is 2026-06-15.
        assert_eq!(monday_of_iso_week_containing("2026-06-17"), Some("2026-06-15".to_owned()));
        // 2026-06-15 is Monday; Monday of week is itself.
        assert_eq!(monday_of_iso_week_containing("2026-06-15"), Some("2026-06-15".to_owned()));
        // 2026-06-21 is Sunday; Monday of week is 2026-06-15.
        assert_eq!(monday_of_iso_week_containing("2026-06-21"), Some("2026-06-15".to_owned()));
    }

    #[test]
    fn iso_week_monday_helper_is_consistent_with_default_cutoff() {
        // Sanity: iso_week_monday(2026, 23) == 2026-06-01.
        let week_start = super::super::iso_week_monday(2026, 23).unwrap();
        assert_eq!(week_start, "2026-06-01");
        // And the default cutoff for a session on 2026-06-03 lands on
        // the same Monday.
        let sessions = vec![session("x", "2026-06-03T00:00:00Z", SessionStatus::Succeeded)];
        let projection = HistoryProjectionV1::from_ledgers_v1(&sessions, &[]);
        let cutoff = default_cutoff_for_projection(&projection);
        assert_eq!(cutoff.date, week_start);
    }

    // -----------------------------------------------------------------
    // Phase 2: markdown renderer tests.
    // -----------------------------------------------------------------

    fn target_with_urls(
        id: &str,
        status: TargetStatus,
        pr: Option<&str>,
        issue: Option<&str>,
    ) -> TargetRecord {
        let mut t = target(id, "fam", status, pr);
        t.issue_url = issue.map(|s| s.to_owned());
        t
    }

    #[test]
    fn render_markdown_empty_ledger_renders_title_and_empty_message() {
        let projection = HistoryProjectionV1::from_ledgers_v1(&[], &[]);
        let cutoff = default_cutoff_for_projection(&projection);
        let view = ReportViewV1::build_v1(&projection, cutoff);
        let md = render_markdown(&view);
        assert_eq!(md, "# sbagent history report: empty ledger\n\nno sessions archived yet\n",);
    }

    #[test]
    fn render_markdown_explicit_cutoff_with_no_matches_still_emits_empty_message() {
        // One session exists, but cutoff excludes it.
        let projection = HistoryProjectionV1::from_ledgers_v1(
            &[session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded)],
            &[],
        );
        let cutoff = SinceCutoff {
            date: "2030-01-01".to_owned(),
            source: CutoffSource::Explicit,
        };
        let view = ReportViewV1::build_v1(&projection, cutoff);
        let md = render_markdown(&view);
        assert_eq!(md, "# sbagent history report: since 2030-01-01\n\nno sessions archived yet\n",);
    }

    #[test]
    fn render_markdown_header_labels_default_cutoff_distinctly() {
        let cutoff = SinceCutoff {
            date: "2026-06-15".to_owned(),
            source: CutoffSource::DefaultLatestWeek,
        };
        let view = ReportViewV1 {
            cutoff,
            session_counts: SessionCounts::default(),
            sessions: vec![],
            target_outcomes: TargetOutcomeCounts::default(),
            total_wall_clock_secs: 0.0,
            pr_lifecycle: vec![],
            issue_lifecycle: vec![],
            mixed_verdicts: vec![],
        };
        let md = render_markdown(&view);
        assert!(
            md.starts_with(
                "# sbagent history report: since 2026-06-15 (default: latest ISO week)\n",
            ),
            "default-cutoff header missing default suffix:\n{md}",
        );
    }

    #[test]
    fn render_markdown_full_view_matches_byte_equality_fixture() {
        // Two sessions; sess-a succeeded with one accepted target +
        // merged PR, sess-b failed with one aborted target + open PR
        // + open issue. Cutoff is Explicit so the title carries no
        // default suffix. wall-clock comes from phase_durations_secs
        // so the totals are non-zero and deterministic.
        let mut sess_a = session("sess-a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        sess_a
            .phase_durations_secs
            .insert("baseline".to_owned(), 60.0);
        sess_a
            .phase_durations_secs
            .insert("optimize".to_owned(), 600.0);
        sess_a
            .targets
            .push(target_with_urls(
                "t1",
                TargetStatus::Accepted,
                Some("https://example.com/pull/1"),
                None,
            ));

        let mut sess_b = session("sess-b", "2026-06-17T00:00:00Z", SessionStatus::Failed);
        sess_b
            .phase_durations_secs
            .insert("baseline".to_owned(), 30.0);
        sess_b
            .targets
            .push(target_with_urls(
                "t2",
                TargetStatus::Aborted,
                Some("https://example.com/pull/2"),
                Some("https://example.com/issues/9"),
            ));

        let maintain = vec![
            maint_event(
                "sess-a",
                "https://example.com/pull/1",
                MaintEventKind::PrMerged,
                "merged",
                "2026-06-12T00:00:00Z",
            ),
            maint_event(
                "sess-b",
                "https://example.com/pull/2",
                MaintEventKind::PrOpen,
                "open",
                "2026-06-18T00:00:00Z",
            ),
            MaintEvent {
                issue_url: Some("https://example.com/issues/9".to_owned()),
                pr_url: None,
                ..maint_event(
                    "sess-b",
                    "ignored",
                    MaintEventKind::IssueOpen,
                    "open",
                    "2026-06-18T12:00:00Z",
                )
            },
        ];

        let projection = HistoryProjectionV1::from_ledgers_v1(&[sess_a, sess_b], &maintain);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-06-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        let md = render_markdown(&view);

        // Hand-rolled expected output. Column widths are
        // longest-cell-or-header-or-4. Raw string preserves exact
        // bytes; rustfmt skip keeps the table aligned.
        #[rustfmt::skip]
        const EXPECTED: &str = "\
# sbagent history report: since 2026-06-01

## Summary

- Total sessions: 2 (succeeded: 1, failed: 1, aborted: 0)
- Targets: accepted 1 / rejected 0 / aborted 1
- Total wall-clock: 11:30

## Sessions

| id     | started_at | status    | targets | wall-clock | prs  | issues |
| ------ | ---------- | --------- | ------- | ---------- | ---- | ------ |
| sess-b | 2026-06-17 | failed    | 0/0/1   | 0:30       | 1    | 1      |
| sess-a | 2026-06-10 | succeeded | 1/0/0   | 11:00      | 1    | 0      |

## Pull requests

| state  | url                        | session | observed_at          |
| ------ | -------------------------- | ------- | -------------------- |
| merged | https://example.com/pull/1 | sess-a  | 2026-06-12T00:00:00Z |
| open   | https://example.com/pull/2 | sess-b  | 2026-06-18T00:00:00Z |

## Issues

| state | url                          | session | observed_at          |
| ----- | ---------------------------- | ------- | -------------------- |
| open  | https://example.com/issues/9 | sess-b  | 2026-06-18T12:00:00Z |
";
        assert_eq!(md, EXPECTED, "rendered markdown did not match fixture");

        // Pure ASCII + no ANSI escape — the digest is meant for piping
        // into files / commits / chat.
        for b in md.as_bytes() {
            assert!(*b < 0x80, "non-ASCII byte 0x{b:02x} in rendered markdown");
        }
        assert!(!md.contains("\x1b["), "ANSI escape leaked into rendered markdown");
    }

    #[test]
    fn render_markdown_omits_pr_section_when_no_pr_lifecycle_rows() {
        // Session has a PR URL but maintain never observed it →
        // pr_lifecycle stays empty → no Pull requests section.
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets
            .push(target_with_urls(
                "t1",
                TargetStatus::Accepted,
                Some("https://example.com/pull/1"),
                None,
            ));
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &[]);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-01-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        let md = render_markdown(&view);
        assert!(md.contains("## Sessions"), "Sessions section missing:\n{md}");
        assert!(!md.contains("## Pull requests"), "Pull requests section should be omitted:\n{md}");
        assert!(!md.contains("## Issues"), "Issues section should be omitted:\n{md}");
    }

    #[test]
    fn render_markdown_omits_issues_section_when_only_prs_present() {
        // PR observed → Pull requests renders; no issue rows → Issues
        // section is omitted.
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets
            .push(target_with_urls(
                "t1",
                TargetStatus::Accepted,
                Some("https://example.com/pull/1"),
                None,
            ));
        let maintain = vec![maint_event(
            "a",
            "https://example.com/pull/1",
            MaintEventKind::PrOpen,
            "open",
            "2026-06-11T00:00:00Z",
        )];
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &maintain);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-01-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        let md = render_markdown(&view);
        assert!(md.contains("## Pull requests"), "Pull requests should render:\n{md}");
        assert!(!md.contains("## Issues"), "Issues section should be omitted:\n{md}");
    }

    #[test]
    fn render_markdown_never_renders_dedup_section() {
        // Phase 1 verified dedup data is not visible through the
        // projection; v17 must never emit a "Dedup skips" heading.
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets
            .push(target_with_urls("t1", TargetStatus::Accepted, None, None));
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &[]);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-01-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        let md = render_markdown(&view);
        assert!(!md.contains("Dedup"), "Dedup section must not render:\n{md}");
    }

    // -----------------------------------------------------------------
    // Mixed-verdict visibility (Phase 2 follow-up after Codex review).
    // -----------------------------------------------------------------

    fn target_with_reason(
        id: &str,
        status: TargetStatus,
        reason: Option<&str>,
        pr: Option<&str>,
    ) -> TargetRecord {
        let mut t = target(id, "fam", status, pr);
        t.reason_code = reason.map(|s| s.to_owned());
        t
    }

    #[test]
    fn build_v1_classifies_accepted_with_mixed_reason_as_mixed_subset() {
        // One session with two accepted targets — one plain, one with
        // a `mixed:` reason code. `mixed` is a SUBSET of `accepted`;
        // the A/R/Ab buckets stay at 2/0/0 and the dedicated `mixed`
        // count rises to 1.
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets
            .push(target_with_reason("t-clean", TargetStatus::Accepted, None, None));
        s.targets
            .push(target_with_reason(
                "t-mix",
                TargetStatus::Accepted,
                Some("mixed: magnitude below expected band"),
                None,
            ));
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &[]);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-01-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        assert_eq!(view.target_outcomes.accepted, 2);
        assert_eq!(view.target_outcomes.rejected, 0);
        assert_eq!(view.target_outcomes.aborted, 0);
        assert_eq!(view.target_outcomes.mixed, 1);
        assert_eq!(view.mixed_verdicts.len(), 1);
        assert_eq!(view.mixed_verdicts[0].target_id, "t-mix");
        assert_eq!(view.mixed_verdicts[0].reason, "mixed: magnitude below expected band");
        // Per-session row carries the same subset bookkeeping.
        assert_eq!(
            view.sessions[0]
                .target_outcomes
                .accepted,
            2
        );
        assert_eq!(
            view.sessions[0]
                .target_outcomes
                .mixed,
            1
        );
    }

    #[test]
    fn build_v1_does_not_classify_non_mixed_reason_codes_as_mixed() {
        // `reason_code` values that don't start with `mixed:` (e.g. the
        // finalize verdict codes) are NOT counted as mixed.
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets
            .push(target_with_reason("t1", TargetStatus::Accepted, Some("noise_floor_pass"), None));
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &[]);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-01-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        assert_eq!(view.target_outcomes.mixed, 0);
        assert!(view.mixed_verdicts.is_empty());
    }

    #[test]
    fn render_markdown_surfaces_mixed_verdicts_in_summary_and_section() {
        // Single-session fixture focused on mixed visibility:
        // 2 accepted targets, 1 of them mixed. Expected output pins
        // both the `(mixed 1)` summary annotation and the `## Mixed
        // verdicts` section.
        let mut s = session("sess-x", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets
            .push(target_with_reason("t-clean", TargetStatus::Accepted, None, None));
        s.targets
            .push(target_with_reason(
                "t-mix",
                TargetStatus::Accepted,
                Some("mixed: magnitude below expected band"),
                None,
            ));
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &[]);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-06-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        let md = render_markdown(&view);

        #[rustfmt::skip]
        const EXPECTED: &str = "\
# sbagent history report: since 2026-06-01

## Summary

- Total sessions: 1 (succeeded: 1, failed: 0, aborted: 0)
- Targets: accepted 2 (mixed 1) / rejected 0 / aborted 0
- Total wall-clock: 0:00

## Sessions

| id     | started_at | status    | targets | wall-clock | prs  | issues |
| ------ | ---------- | --------- | ------- | ---------- | ---- | ------ |
| sess-x | 2026-06-10 | succeeded | 2/0/0   | 0:00       | 0    | 0      |

## Mixed verdicts

| session | target | reason                               |
| ------- | ------ | ------------------------------------ |
| sess-x  | t-mix  | mixed: magnitude below expected band |
";
        assert_eq!(md, EXPECTED, "rendered markdown did not match mixed-verdict fixture");
    }

    #[test]
    fn render_markdown_omits_mixed_section_when_no_mixed_verdicts() {
        // Phase 2 fixture exercises the no-mixed common case
        // (`render_markdown_full_view_matches_byte_equality_fixture`).
        // This narrower assertion pins the discipline: when
        // `mixed_verdicts` is empty, neither the summary annotation
        // nor the section heading appears.
        let mut s = session("a", "2026-06-10T00:00:00Z", SessionStatus::Succeeded);
        s.targets
            .push(target_with_urls("t1", TargetStatus::Accepted, None, None));
        let projection = HistoryProjectionV1::from_ledgers_v1(&[s], &[]);
        let view = ReportViewV1::build_v1(
            &projection,
            SinceCutoff {
                date: "2026-01-01".to_owned(),
                source: CutoffSource::Explicit,
            },
        );
        let md = render_markdown(&view);
        assert!(!md.contains("(mixed"), "summary should omit mixed annotation:\n{md}");
        assert!(
            !md.contains("## Mixed verdicts"),
            "Mixed verdicts section should be omitted:\n{md}"
        );
    }
}
