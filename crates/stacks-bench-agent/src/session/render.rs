//! Phase 4 markdown rendering.
//!
//! Produces two human-readable artifacts under `results/finalize/`:
//!
//! - `summary.md` — overview table with per-target verdict + a Mermaid xychart
//!   of measured improvement_pct.
//! - `targets.md` — full catalog rendered from `optimization-targets.json`.
//!
//! Both files live in `results/finalize/` and link via `..`-prefixed relative
//! paths to the sibling phase dirs (`../optimize/<id>/` for per-target
//! writeups, `../analysis/<family>/` for analyzer JSONs). GitHub renders
//! these the same as a local markdown viewer.

use std::collections::BTreeMap;

use crate::models::analyze::Analysis;
use crate::models::common::{Bucket, LensDispositionStatus, SchemaVersionV2, SelectionLens};
use crate::models::summary::{Experiment, ExperimentStatus, Summary};
use crate::models::targets::{MergedTarget, OptimizationTargets};
use crate::session::SessionLayout;

/// Short head-of-file extracts for per-target experiment writeups, inlined
/// in `summary.md`'s narrative section. Empty entries are valid (target had
/// no implementation.md / abort.md on disk).
#[derive(Debug, Clone, Default)]
pub struct ExperimentNotes {
    /// First non-heading paragraph of `../optimize/<id>/implementation.md`.
    pub implementation_head: Option<String>,
    /// First non-heading paragraph of `../optimize/<id>/abort.md`.
    pub abort_head: Option<String>,
}

/// Load per-target writeup excerpts from the session tree. Returns one
/// entry per target id (including empty entries when neither file exists),
/// so callers can pass the map straight to [`render_summary_md`].
pub fn load_experiment_notes(
    layout: &SessionLayout,
    targets: &OptimizationTargets,
) -> BTreeMap<String, ExperimentNotes> {
    let mut out = BTreeMap::new();
    for t in &targets.targets {
        let implementation_head = std::fs::read_to_string(layout.experiment_implementation(&t.id))
            .ok()
            .as_deref()
            .map(|s| excerpt_writeup(s, 360));
        let abort_head = std::fs::read_to_string(layout.experiment_abort(&t.id))
            .ok()
            .as_deref()
            .map(|s| excerpt_writeup(s, 280));
        out.insert(
            t.id.clone(),
            ExperimentNotes {
                implementation_head,
                abort_head,
            },
        );
    }
    out
}

/// Render `summary.md`. The document reads top-to-bottom as a narrative of
/// the session:
///
/// 1. Header + meta + improvement chart.
/// 2. `## TL;DR` — auto-generated 1-paragraph synopsis from outcome counts.
/// 3. `## What was found` — per-family digest of triage + analyzer outputs.
/// 4. `## What was chosen — and how it went` — per-target narrative block with
///    hotspot / proposed-change excerpts and outcome.
/// 5. `## Outcomes` — counts table.
/// 6. `## At a glance` — flat experiments table for lookup.
/// 7. `## Coverage matrix` — bucket × selection_lens.
/// 8. `## Real hotspots without an actionable fix` — sidebar.
/// 9. `## Next steps` — short hint.
///
/// `notes` carries per-target writeup excerpts (load via
/// [`load_experiment_notes`]). Passing an empty map degrades gracefully —
/// the narrative blocks will skip the inline writeup excerpts.
pub fn render_summary_md(
    summary: &Summary,
    targets: &OptimizationTargets,
    analyses: &BTreeMap<String, Analysis>,
    notes: &BTreeMap<String, ExperimentNotes>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Session {sid}\n\n- Baseline run id: {b1}\n- Baseline rerun id: {b2}\n- Noise floor: \
         {nf}%\n- Target catalog: [targets.md](targets.md)\n\n",
        sid = summary.session_id,
        b1 = summary.baseline_run_id,
        b2 = summary.baseline_rerun_id,
        nf = summary.noise_floor_pct
    ));

    if let Some(chart) = render_improvement_chart(&summary.experiments) {
        out.push_str("## Improvement vs baseline\n\n");
        out.push_str(&chart);
        out.push('\n');
    }

    out.push_str("## TL;DR\n\n");
    out.push_str(&render_tldr(summary));
    out.push('\n');

    out.push_str(&render_what_was_found(targets, analyses));
    out.push_str(&render_what_was_chosen(summary, targets, analyses, notes));

    out.push_str("## Outcomes\n\n");
    out.push_str("| Delivery mode    | Counts                                       |\n");
    out.push_str("| ---------------- | -------------------------------------------- |\n");
    out.push_str(&format!(
        "| Normal PR        | Accepted {a} · Rejected {r} · Aborted {ab} |\n",
        a = summary
            .outcome_counts
            .normal_pr
            .accepted,
        r = summary
            .outcome_counts
            .normal_pr
            .rejected,
        ab = summary
            .outcome_counts
            .normal_pr
            .aborted
    ));
    out.push_str(&format!(
        "| Consensus PoC PR | PoC landed {p} · Aborted {ab} |\n",
        p = summary
            .outcome_counts
            .consensus_poc_pr
            .poc_landed,
        ab = summary
            .outcome_counts
            .consensus_poc_pr
            .aborted
    ));
    out.push_str(&format!(
        "| Consensus issue  | Routed to issue {r} · Aborted {ab} |\n\n",
        r = summary
            .outcome_counts
            .consensus_issue
            .routed_to_issue,
        ab = summary
            .outcome_counts
            .consensus_issue
            .aborted
    ));

    out.push_str(&render_coverage_matrix(targets, analyses));

    out.push_str("\n## At a glance\n\n");
    out.push_str("| Target | Delivery mode | Status | Improvement | Run ids | Notes |\n");
    out.push_str("| ------ | ------------- | ------ | ----------- | ------- | ----- |\n");
    for e in &summary.experiments {
        let target_link = format!("[{tid}](../optimize/{tid}/)", tid = e.target_id);
        let status_cell = render_status_link(&e.target_id, e.status);
        let improvement = e
            .improvement_pct
            .map(|v| format!("{:.2}%", v))
            .unwrap_or_else(|| "—".to_owned());
        let run_ids = render_run_ids(&e.target_id, e.run_ids.as_deref());
        let mut notes = e
            .reason
            .clone()
            .unwrap_or_default();
        if let Some(bc) = e.breakage_class {
            if !notes.is_empty() {
                notes.push_str("; ");
            }
            notes.push_str(&format!("breakage class: {}", humanize_breakage_class(bc)));
        }
        out.push_str(&format!(
            "| {target_link} | {dm} | {status_cell} | {improvement} | {run_ids} | {notes} |\n",
            dm = humanize_delivery_mode(e.delivery_mode),
        ));
    }

    let n_not_actionable = summary
        .lens_dispositions
        .iter()
        .filter(|d| d.status == LensDispositionStatus::NotActionable)
        .count();
    if n_not_actionable > 0 {
        out.push_str("\n## Real hotspots without an actionable fix\n\n");
        out.push_str(
            "The analyzer drilled into the families below, confirmed the signal at code\n",
        );
        out.push_str(
            "level, and could not find a structural handle. The reasons reflect code-level\n",
        );
        out.push_str(
            "constraints (consensus rules, inherent CPU cost, already-cached paths). These\n",
        );
        out.push_str(
            "are first-class artifacts — surface them to whoever decides what to optimize\n",
        );
        out.push_str("next.\n\n");
        out.push_str("| Family | Lens | Reason |\n");
        out.push_str("| ------ | ---- | ------ |\n");
        for d in &summary.lens_dispositions {
            if d.status == LensDispositionStatus::NotActionable {
                out.push_str(&format!(
                    "| [{fid}](../analysis/{fid}/analysis.json) | {lens} | {reason} |\n",
                    fid = d.family_id,
                    lens = humanize_lens(d.lens),
                    reason = d
                        .reason
                        .as_deref()
                        .unwrap_or("(no reason)"),
                ));
            }
        }
    }

    if let Some(hint) = &summary.next_targets_hint {
        out.push_str(&format!("\n## Next steps\n\n{hint}\n"));
    }

    out
}

/// Render `targets.md` — the full per-target catalog. Pulled from
/// `optimization-targets.json` (already loaded as `targets`) and cross-linked
/// to per-family analyses + per-experiment writeups.
pub fn render_targets_md(
    targets: &OptimizationTargets,
    analyses: &BTreeMap<String, Analysis>,
) -> String {
    let _ = SchemaVersionV2;
    let mut out = String::new();
    out.push_str(&format!("# Optimization targets — session {sid}\n\n", sid = targets.session_id));
    out.push_str(
        "> Catalog of merged optimization targets produced by Phase 1.7 (merge). For pass/fail \
         verdicts and benchmark deltas, see [summary.md](summary.md).\n\n",
    );
    out.push_str(&format!("- Baseline run id: `{}`\n", targets.baseline_run_id));
    out.push_str(&format!("- Baseline rerun id: `{}`\n", targets.baseline_rerun_id));
    out.push_str(&format!("- Noise floor: `{}%`\n", targets.noise_floor_pct));
    out.push_str(&format!("- Merge method: `{}`\n", humanize_merge_method(targets.merge_method)));
    out.push_str(&format!("- Merge model: `{}`\n\n", targets.merge_model));

    // Table of contents — one anchor per target, kebab ids are already
    // valid GitHub anchors.
    if !targets.targets.is_empty() {
        out.push_str("## Contents\n\n");
        for t in &targets.targets {
            out.push_str(&format!(
                "- [{id}](#{anchor}) — {dm}, bucket=`{bucket}`, risk=`{risk}`\n",
                id = t.id,
                anchor = t.id,
                dm = humanize_delivery_mode(t.delivery_mode),
                bucket = humanize_bucket(t.bucket),
                risk = humanize_risk(t.risk),
            ));
        }
        out.push('\n');
    }

    for t in &targets.targets {
        out.push_str(&render_target_section(t, analyses));
    }

    if !targets
        .rejected_by_merge
        .is_empty()
    {
        out.push_str("## Rejected by merge\n\n");
        out.push_str(
            "Analyzer-emitted targets the merger affirmatively dropped (duplicates of an \
             already-shipped fix, subsumed by a stronger target, etc.).\n\n",
        );
        out.push_str("| Family | Target index | Reason |\n");
        out.push_str("| ------ | ------------ | ------ |\n");
        for r in &targets.rejected_by_merge {
            out.push_str(&format!(
                "| [{fid}](../analysis/{fid}/analysis.json) | {idx} | {reason} |\n",
                fid = r.family_id,
                idx = r.target_index,
                reason = r.reason,
            ));
        }
        out.push('\n');
    }

    if !targets
        .lens_dispositions
        .is_empty()
    {
        out.push_str("## Lens dispositions\n\n");
        out.push_str("| Family | Lens | Status | Reason |\n");
        out.push_str("| ------ | ---- | ------ | ------ |\n");
        for d in &targets.lens_dispositions {
            out.push_str(&format!(
                "| [{fid}](../analysis/{fid}/analysis.json) | {lens} | {status} | {reason} |\n",
                fid = d.family_id,
                lens = humanize_lens(d.lens),
                status = humanize_lens_disposition_status(d.status),
                reason = d
                    .reason
                    .as_deref()
                    .unwrap_or("—"),
            ));
        }
    }

    out
}

fn render_target_section(t: &MergedTarget, _analyses: &BTreeMap<String, Analysis>) -> String {
    let mut out = String::new();
    out.push_str(&format!("## {id}\n\n", id = t.id));

    out.push_str("**Meta**\n\n");
    out.push_str(&format!("- Delivery mode: {}\n", humanize_delivery_mode(t.delivery_mode)));
    out.push_str(&format!("- Bucket: {}\n", humanize_bucket(t.bucket)));
    out.push_str(&format!("- Risk: {}\n", humanize_risk(t.risk)));
    if let Some(rank) = t.rank {
        out.push_str(&format!("- Rank: `{}`\n", rank));
    }
    out.push_str(&format!("- Convergence: `{}` contributor(s)\n", t.convergence_count));
    out.push_str(&format!("- Bench eligible: {}\n", if t.bench_eligible { "yes" } else { "no" },));
    out.push('\n');

    out.push_str("**Hotspot**\n\n");
    out.push_str(&format!("- Target span: `{}`\n", t.target_span));
    out.push_str(&format!("- Profiler span: `{}`\n", t.hotspot.span));
    out.push_str(&format!("- Location: `{}`\n", t.hotspot.location));
    out.push_str(&format!(
        "- self_wall: `{}` µs · total_wall: `{}` µs · calls: `{}`\n",
        t.hotspot.self_wall_us, t.hotspot.total_wall_us, t.hotspot.calls,
    ));
    out.push('\n');

    out.push_str("**Expected improvement**\n\n");
    out.push_str(&format!(
        "- tx_latency: `{:.2}%`\n",
        t.expected_improvement
            .tx_latency
    ));
    out.push_str(&format!(
        "- tenure_throughput: `{:.2}%`\n",
        t.expected_improvement
            .tenure_throughput,
    ));
    out.push_str(&format!(
        "- commit_time: `{:.2}%`\n\n",
        t.expected_improvement
            .commit_time
    ));

    if !t.files.is_empty() {
        out.push_str("**Files**\n\n");
        for f in &t.files {
            out.push_str(&format!("- `{}`\n", f));
        }
        out.push('\n');
    }

    out.push_str("**Evidence**\n\n");
    out.push_str(&blockquote(&t.evidence));
    out.push('\n');

    out.push_str("**Proposed change**\n\n");
    out.push_str(&blockquote(&t.proposed_change));
    out.push('\n');

    out.push_str("**Verification plan**\n\n");
    out.push_str(&blockquote(&t.verification_plan));
    out.push('\n');

    if let Some(notes) = &t.merge_notes {
        out.push_str("**Merge notes**\n\n");
        out.push_str(&blockquote(notes));
        out.push('\n');
    }

    if let Some(diffs) = &t.contributor_differences
        && !diffs.is_empty()
    {
        out.push_str("**Contributor differences**\n\n");
        for d in diffs {
            out.push_str(&format!("- {}\n", d));
        }
        out.push('\n');
    }

    if t.consensus_breaking {
        out.push_str("**Consensus-breaking**\n\n");
        if let Some(bc) = t.breakage_class {
            out.push_str(&format!("- Breakage class: {}\n", humanize_breakage_class(bc)));
        }
        if let Some(p) = t.poc_implementable {
            out.push_str(&format!("- PoC implementable: {}\n", if p { "yes" } else { "no" }));
        }
        if let Some(scope) = &t.poc_test_scope
            && !scope.is_empty()
        {
            out.push_str("- PoC test scope:\n");
            for s in scope {
                out.push_str(&format!("  - `{}`\n", s));
            }
        }
        out.push('\n');
        if let Some(writeup) = &t.consensus_writeup {
            out.push_str("**Consensus writeup**\n\n");
            out.push_str(&blockquote(writeup));
            out.push('\n');
        }
    }

    out.push_str("**Contributors**\n\n");
    for mf in &t.merged_from {
        out.push_str(&format!(
            "- [{fid}](../analysis/{fid}/analysis.json) (target_index `{idx}`)\n",
            fid = mf.family_id,
            idx = mf.target_index,
        ));
    }
    out.push('\n');

    out.push_str("**Outputs**\n\n");
    out.push_str(&format!(
        "- Experiment dir: [`../optimize/{id}/`](../optimize/{id}/)\n",
        id = t.id
    ));
    out.push_str(&format!(
        "- [implementation.md](../optimize/{id}/implementation.md) · \
         [side-observations.md](../optimize/{id}/side-observations.md) · \
         [abort.md](../optimize/{id}/abort.md)\n",
        id = t.id
    ));
    out.push('\n');

    out
}

/// Render the `## TL;DR` body — one short paragraph synthesized from the
/// outcome counts. Omits clauses whose count is zero.
fn render_tldr(summary: &Summary) -> String {
    let np = &summary
        .outcome_counts
        .normal_pr;
    let cp = &summary
        .outcome_counts
        .consensus_poc_pr;
    let ci = &summary
        .outcome_counts
        .consensus_issue;
    let measured: Vec<f64> = summary
        .experiments
        .iter()
        .filter_map(|e| e.improvement_pct)
        .collect();

    let mut clauses: Vec<String> = Vec::new();
    if np.accepted > 0 {
        let avg = if measured.is_empty() {
            0.0
        } else {
            measured.iter().sum::<f64>() / measured.len() as f64
        };
        let range = if measured.len() > 1 {
            let lo = measured
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let hi = measured
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            format!(" (range {lo:.1}%..{hi:.1}%)")
        } else {
            String::new()
        };
        clauses.push(format!(
            "{} normal-PR target(s) measurably improved (avg +{avg:.2}%{range})",
            np.accepted,
        ));
    }
    if np.rejected > 0 {
        clauses.push(format!("{} rejected within noise or as regressions", np.rejected));
    }
    if np.aborted > 0 {
        clauses.push(format!("{} aborted before producing a viable change", np.aborted));
    }
    if cp.poc_landed > 0 {
        clauses.push(format!("{} consensus PoC PR(s) landed for HIP discussion", cp.poc_landed,));
    }
    if cp.aborted > 0 {
        clauses.push(format!("{} consensus PoC PR(s) aborted", cp.aborted));
    }
    if ci.routed_to_issue > 0 {
        clauses.push(format!("{} routed to a tracking issue", ci.routed_to_issue));
    }
    if ci.aborted > 0 {
        clauses.push(format!("{} consensus issue target(s) aborted", ci.aborted));
    }

    let total = summary.experiments.len();
    let prefix = format!("Out of **{total}** merged optimization target(s), ");
    let body = if clauses.is_empty() {
        "no measurable outcome was recorded — see the per-target section below.".to_owned()
    } else {
        format!("{}.", clauses.join("; "))
    };
    format!("{prefix}{body}\n")
}

/// Render the `## What was found` section — a table mapping each analyzed
/// family to its outcome (contributed to a merged target, or recorded as
/// not_actionable, or rejected by the analyzer).
fn render_what_was_found(
    targets: &OptimizationTargets,
    analyses: &BTreeMap<String, Analysis>,
) -> String {
    if analyses.is_empty() {
        return String::new();
    }

    // Build family_id -> merged target id (first occurrence) for the
    // "contributed to" pointers.
    let mut family_to_target: BTreeMap<String, String> = BTreeMap::new();
    for t in &targets.targets {
        for mf in &t.merged_from {
            family_to_target
                .entry(mf.family_id.clone())
                .or_insert_with(|| t.id.clone());
        }
    }

    let mut out = String::new();
    out.push_str("## What was found\n\n");
    out.push_str(&format!(
        "Triage promoted **{n}** family(ies) to per-family analysis. Their dispositions:\n\n",
        n = analyses.len(),
    ));
    out.push_str("| Family | Lens | Disposition |\n");
    out.push_str("| ------ | ---- | ----------- |\n");
    for (fid, a) in analyses {
        let lens = match a {
            Analysis::Accepted(acc) => humanize_lens(acc.selection_lens).to_owned(),
            Analysis::Rejected(_) => "—".to_owned(),
        };
        let disposition = match a {
            Analysis::Accepted(acc) => match acc.lens_disposition.status {
                LensDispositionStatus::Addressed => match family_to_target.get(fid) {
                    Some(tid) => format!(
                        "→ contributed to **[{tid}](#{anchor})** ({n} target(s) proposed)",
                        anchor = anchor_from_target_id(tid),
                        n = acc.targets.len(),
                    ),
                    None => format!(
                        "→ analyzer proposed {n} target(s) but none merged — see \
                         [rejected-by-merge](targets.md#rejected-by-merge)",
                        n = acc.targets.len(),
                    ),
                },
                LensDispositionStatus::NotActionable => {
                    let reason = acc
                        .lens_disposition
                        .reason
                        .as_deref()
                        .map(|r| excerpt_paragraph(r, 200))
                        .unwrap_or_else(|| "no reason recorded".to_owned());
                    format!("not_actionable — {reason}")
                }
            },
            Analysis::Rejected(rej) => {
                format!("rejected by analyzer — {}", excerpt_paragraph(&rej.reason, 200))
            }
        };
        out.push_str(&format!(
            "| [{fid}](../analysis/{fid}/analysis.json) | {lens} | {disposition} |\n",
        ));
    }
    out.push('\n');
    out
}

/// Render the `## What was chosen — and how it went` section — one block
/// per merged target with hotspot / proposed-change excerpts, contributor
/// links, and outcome (with inline excerpt from implementation.md or
/// abort.md when available).
fn render_what_was_chosen(
    summary: &Summary,
    targets: &OptimizationTargets,
    _analyses: &BTreeMap<String, Analysis>,
    notes: &BTreeMap<String, ExperimentNotes>,
) -> String {
    if targets.targets.is_empty() {
        return String::new();
    }

    // experiment_by_id lookup for status + improvement.
    let exp_by_id: BTreeMap<&str, &Experiment> = summary
        .experiments
        .iter()
        .map(|e| (e.target_id.as_str(), e))
        .collect();

    let mut out = String::new();
    out.push_str("## What was chosen — and how it went\n\n");
    out.push_str(
        "Each merged optimization target below carries the hotspot evidence the analyzer \
         identified, the proposed change, and what the optimizer actually shipped. Excerpts are \
         short — follow the links to the full writeups.\n\n",
    );

    for t in &targets.targets {
        let exp = exp_by_id.get(t.id.as_str());
        out.push_str(&render_target_narrative(t, exp.copied(), notes.get(&t.id)));
    }
    out
}

fn render_target_narrative(
    t: &MergedTarget,
    exp: Option<&Experiment>,
    note: Option<&ExperimentNotes>,
) -> String {
    let mut out = String::new();

    // Heading: emoji prefix + id + delivery mode + status badge.
    let badge = exp
        .map(|e| status_badge(e.status, e.improvement_pct))
        .unwrap_or_else(|| "(no experiment record)".to_owned());
    let mut prefix = String::from(delivery_mode_emoji(t.delivery_mode));
    if let Some(e) = exp {
        prefix.push(' ');
        prefix.push_str(status_emoji(e.status));
    }
    out.push_str(&format!(
        "### {prefix} {id} — {dm} · {badge}\n\n",
        id = t.id,
        dm = humanize_delivery_mode(t.delivery_mode),
    ));

    // Hotspot metadata — single line, compact units.
    out.push_str(&format!(
        "`{span}` at `{loc}` · self_wall {sw} · {calls} calls · risk: {risk} · {conv} \
         contributor(s)\n\n",
        span = t.hotspot.span,
        loc = t.hotspot.location,
        sw = format_us(t.hotspot.self_wall_us),
        calls = format_count(t.hotspot.calls),
        risk = humanize_risk(t.risk),
        conv = t.convergence_count,
    ));

    // Evidence / Proposed change / Outcome — inline prose with bold leading
    // labels (no sub-heading + blockquote treatment) to keep each target
    // card compact.
    let evidence = excerpt_paragraph(&t.evidence, 360);
    if !evidence.is_empty() {
        out.push_str(&format!("**Evidence.** {evidence}\n\n"));
    }
    let proposed = excerpt_paragraph(&t.proposed_change, 280);
    if !proposed.is_empty() {
        out.push_str(&format!("**Proposed.** {proposed}\n\n"));
    }
    if let Some(n) = note {
        if let Some(text) = n
            .implementation_head
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            out.push_str(&format!("**Outcome.** {text}\n\n"));
        } else if let Some(text) = n
            .abort_head
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            out.push_str(&format!("**Outcome (aborted).** {text}\n\n"));
        }
    }

    // Footer line: contributors first, then detail links. One line each.
    if !t.merged_from.is_empty() {
        let links: Vec<String> = t
            .merged_from
            .iter()
            .map(|mf| format!("[{fid}](../analysis/{fid}/analysis.json)", fid = mf.family_id))
            .collect();
        out.push_str(&format!("Contributors: {}\n\n", links.join(", ")));
    }
    let mut detail_links: Vec<String> = vec![
        format!("[experiment dir](../optimize/{id}/)", id = t.id),
        format!("[target catalog](targets.md#{anchor})", anchor = anchor_from_target_id(&t.id),),
    ];
    if let Some(n) = note {
        if n.implementation_head
            .is_some()
        {
            detail_links.push(format!(
                "[implementation.md](../optimize/{id}/implementation.md)",
                id = t.id,
            ));
        }
        if n.abort_head.is_some() {
            detail_links.push(format!("[abort.md](../optimize/{id}/abort.md)", id = t.id));
        }
    }
    out.push_str(&format!("Details: {}\n\n", detail_links.join(" · ")));

    // Card separator. Trailing rule after each target.
    out.push_str("---\n\n");

    out
}

/// Format a microsecond duration with a unit that keeps 3 significant
/// digits visible. Used in the per-target hotspot one-liner.
fn format_us(us: i64) -> String {
    let abs = us.unsigned_abs();
    if abs < 1_000 {
        format!("{us} µs")
    } else if abs < 1_000_000 {
        format!("{:.1} ms", us as f64 / 1_000.0)
    } else if abs < 60_000_000 {
        format!("{:.2} s", us as f64 / 1_000_000.0)
    } else {
        let secs = us as f64 / 1_000_000.0;
        let mins = (secs / 60.0).floor();
        let rem = secs - mins * 60.0;
        format!("{mins:.0}m {rem:.1}s")
    }
}

/// Compact integer count with k/M suffix. `1234` → `1.2k`, `123_456` → `123k`,
/// `2_500_000` → `2.5M`.
fn format_count(n: i64) -> String {
    let abs = n.unsigned_abs();
    if abs < 1_000 {
        n.to_string()
    } else if abs < 1_000_000 {
        if abs < 10_000 { format!("{:.1}k", n as f64 / 1_000.0) } else { format!("{}k", n / 1_000) }
    } else if abs < 10_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else {
        format!("{}M", n / 1_000_000)
    }
}

/// Delivery-mode emoji for the per-target narrative heading. The user
/// asked for type-representative emojis here so the section reads at a
/// glance — outside this section, the codebase otherwise avoids emojis.
fn delivery_mode_emoji(mode: crate::models::common::DeliveryMode) -> &'static str {
    use crate::models::common::DeliveryMode::*;
    match mode {
        NormalPr => "🔧",
        ConsensusPocPr => "⚠️",
        ConsensusIssue => "📋",
    }
}

/// Status emoji paired with the delivery-mode emoji on per-target headings.
fn status_emoji(status: ExperimentStatus) -> &'static str {
    match status {
        ExperimentStatus::Accepted => "✅",
        ExperimentStatus::Rejected => "❌",
        ExperimentStatus::Aborted => "🚫",
        ExperimentStatus::PocLanded => "✅",
        ExperimentStatus::RoutedToIssue => "✅",
    }
}

/// Short text label combining status + measured improvement, used as the
/// per-target narrative heading badge.
fn status_badge(status: ExperimentStatus, improvement_pct: Option<f64>) -> String {
    let label = humanize_status(status);
    match (status, improvement_pct) {
        (ExperimentStatus::Accepted, Some(v)) => format!("**{label}** (+{v:.2}%)"),
        (ExperimentStatus::Rejected, Some(v)) if v < 0.0 => {
            format!("**{label}** — regression ({v:.2}%)")
        }
        (ExperimentStatus::Rejected, Some(v)) => format!("**{label}** — within noise ({v:.2}%)"),
        _ => format!("**{label}**"),
    }
}

/// Best-effort excerpt of a free-text field. Takes the first paragraph
/// (split on `\n\n`), collapses internal whitespace, and caps at
/// `max_chars` with a trailing ellipsis when truncated. Empty / blank
/// input returns an empty string so callers can branch on `is_empty()`.
fn excerpt_paragraph(text: &str, max_chars: usize) -> String {
    let first_para = text
        .split("\n\n")
        .next()
        .unwrap_or("")
        .trim();
    let normalized: String = first_para
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return String::new();
    }
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut out: String = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    out.push('…');
    out
}

/// Excerpt for a markdown writeup file (`implementation.md`, `abort.md`).
/// Skips leading ATX/Setext headings to land on the first prose paragraph
/// before applying [`excerpt_paragraph`].
fn excerpt_writeup(text: &str, max_chars: usize) -> String {
    for para in text.split("\n\n") {
        let p = para.trim();
        if p.is_empty() || p.starts_with('#') {
            continue;
        }
        return excerpt_paragraph(p, max_chars);
    }
    String::new()
}

/// Convert a kebab-case target id to its GitHub-rendered anchor. Kebab
/// case ids are already valid anchors, but this gives us one place to
/// adjust if the rule ever changes.
fn anchor_from_target_id(id: &str) -> String {
    id.to_owned()
}

/// Build a Mermaid xychart-beta block for the per-target improvement_pct
/// values. Returns `None` if no experiment carries a measured improvement.
/// Label width is capped to avoid overflowing the rendered chart for
/// long kebab ids — the full id is in the experiments table below.
fn render_improvement_chart(experiments: &[Experiment]) -> Option<String> {
    let measured: Vec<(&str, f64)> = experiments
        .iter()
        .filter_map(|e| {
            e.improvement_pct
                .map(|v| (e.target_id.as_str(), v))
        })
        .collect();
    if measured.is_empty() {
        return None;
    }

    // Pick a y-axis range that fits the data tightly:
    // - all values >= 0 → 0 .. ceil(max/5)*5
    // - all values <= 0 → floor(min/5)*5 .. 0
    // - mixed → floor(min/5)*5 .. ceil(max/5)*5 (asymmetric; we don't pad the
    //   unused side just to look symmetric)
    // Floor of 10% on the spanned side so a tiny chart still has tick labels.
    let max = measured
        .iter()
        .map(|(_, v)| *v)
        .fold(f64::NEG_INFINITY, f64::max);
    let min = measured
        .iter()
        .map(|(_, v)| *v)
        .fold(f64::INFINITY, f64::min);
    let upper = if max > 0.0 { ((max / 5.0).ceil() * 5.0).max(10.0) } else { 0.0 };
    let lower = if min < 0.0 { ((min / 5.0).floor() * 5.0).min(-10.0) } else { 0.0 };

    let labels: String = measured
        .iter()
        .map(|(id, _)| format!("\"{}\"", truncate_label(id, 24)))
        .collect::<Vec<_>>()
        .join(", ");
    let bars: String = measured
        .iter()
        .map(|(_, v)| format!("{:.2}", v))
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::new();
    out.push_str("```mermaid\n");
    out.push_str("xychart-beta\n");
    out.push_str("    title \"Per-target improvement vs baseline (%)\"\n");
    out.push_str(&format!("    x-axis [{labels}]\n"));
    out.push_str(&format!("    y-axis \"% improvement\" {lower} --> {upper}\n"));
    out.push_str(&format!("    bar [{bars}]\n"));
    out.push_str("```\n");
    Some(out)
}

fn render_status_link(target_id: &str, status: ExperimentStatus) -> String {
    let label = humanize_status(status);
    let file = match status {
        ExperimentStatus::Accepted | ExperimentStatus::Rejected | ExperimentStatus::PocLanded => {
            "implementation.md"
        }
        ExperimentStatus::RoutedToIssue => "consensus-issue.md",
        ExperimentStatus::Aborted => "abort.md",
    };
    format!("[{label}](../optimize/{target_id}/{file})")
}

fn render_run_ids(target_id: &str, run_ids: Option<&[i64]>) -> String {
    let Some(ids) = run_ids else { return "—".to_owned() };
    if ids.is_empty() {
        return "—".to_owned();
    }
    ids.iter()
        .enumerate()
        .map(|(idx, id)| {
            // `run-N` directories are 1-indexed by Phase 3 ordering, which
            // is the same order Phase 4 reads from `run-ids`.
            format!("[{id}](../optimize/{target_id}/run-{n}/bench-run.json)", n = idx + 1)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Coverage matrix bucket × selection_lens. Cell counts use each merged
/// target's primary lens (= first contributor's selection_lens). Carried
/// over from the previous in-finalize implementation.
fn render_coverage_matrix(
    targets: &OptimizationTargets,
    analyses: &BTreeMap<String, Analysis>,
) -> String {
    let mut lens_by_family: BTreeMap<String, SelectionLens> = BTreeMap::new();
    for (fid, a) in analyses {
        if let Analysis::Accepted(acc) = a {
            lens_by_family.insert(fid.clone(), acc.selection_lens);
        }
    }

    let mut counts: BTreeMap<(Bucket, Option<SelectionLens>), u32> = BTreeMap::new();
    for t in &targets.targets {
        let lens = t
            .merged_from
            .first()
            .and_then(|mf| {
                lens_by_family
                    .get(&mf.family_id)
                    .copied()
            });
        *counts
            .entry((t.bucket, lens))
            .or_insert(0) += 1;
    }

    let mut out = String::new();
    out.push_str("## Coverage matrix (bucket × selection lens)\n\n");
    out.push_str("|                  | Tx latency | Tenure throughput | Commit time |\n");
    out.push_str("| ---------------- | ---------- | ----------------- | ----------- |\n");
    for bucket in [Bucket::BlockProcessing, Bucket::BlockCommit] {
        let bucket_label = humanize_bucket(bucket);
        let cell = |lens: SelectionLens| -> String {
            counts
                .get(&(bucket, Some(lens)))
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_owned())
        };
        out.push_str(&format!(
            "| {bucket_label} | {tx} | {tt} | {ct} |\n",
            tx = cell(SelectionLens::TxLatency),
            tt = cell(SelectionLens::TenureThroughput),
            ct = cell(SelectionLens::CommitTime),
        ));
    }
    out.push_str(
        "\n> Cell counts use each merged target's primary lens (the first contributor's selection \
         lens). Targets with cross-lens convergence are counted once; see the \
         `contributor_differences` field of `optimization-targets.json` for cross-lens cases.\n",
    );
    out
}

/// Render `text` as a markdown blockquote, prefixing every line with `> `.
/// Empty input collapses to a single `> —` marker so the section is still
/// visually anchored.
fn blockquote(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "> —\n".to_owned();
    }
    let mut out = String::new();
    for line in trimmed.lines() {
        if line.is_empty() {
            out.push_str(">\n");
        } else {
            out.push_str(&format!("> {line}\n"));
        }
    }
    out
}

fn truncate_label(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut out: String = s
        .chars()
        .take(max.saturating_sub(1))
        .collect();
    out.push('…');
    out
}

// ---------- humanize: enum → proper English ----------
//
// Each helper match is exhaustive (no `_` arm) so adding a new enum variant
// upstream forces a code update here. Labels are sentence case where the
// enum is rendered standalone (table cells, headings); callers that need
// inline lower case (mid-sentence prose) wrap as needed.

fn humanize_delivery_mode(m: crate::models::common::DeliveryMode) -> &'static str {
    use crate::models::common::DeliveryMode::*;
    match m {
        NormalPr => "Normal PR",
        ConsensusPocPr => "Consensus PoC PR",
        ConsensusIssue => "Consensus issue",
    }
}

fn humanize_bucket(b: Bucket) -> &'static str {
    match b {
        Bucket::BlockProcessing => "Block processing",
        Bucket::BlockCommit => "Block commit",
    }
}

fn humanize_risk(r: crate::models::common::Risk) -> &'static str {
    use crate::models::common::Risk::*;
    match r {
        Low => "Low",
        Medium => "Medium",
        High => "High",
    }
}

fn humanize_lens(l: SelectionLens) -> &'static str {
    match l {
        SelectionLens::TxLatency => "Tx latency",
        SelectionLens::TenureThroughput => "Tenure throughput",
        SelectionLens::CommitTime => "Commit time",
    }
}

fn humanize_status(s: ExperimentStatus) -> &'static str {
    match s {
        ExperimentStatus::Accepted => "Accepted",
        ExperimentStatus::Rejected => "Rejected",
        ExperimentStatus::Aborted => "Aborted",
        ExperimentStatus::PocLanded => "PoC landed",
        ExperimentStatus::RoutedToIssue => "Routed to issue",
    }
}

fn humanize_lens_disposition_status(s: LensDispositionStatus) -> &'static str {
    match s {
        LensDispositionStatus::Addressed => "Addressed",
        LensDispositionStatus::NotActionable => "Not actionable",
    }
}

fn humanize_breakage_class(b: crate::models::common::BreakageClass) -> &'static str {
    use crate::models::common::BreakageClass::*;
    match b {
        ClarityCostWeight => "Clarity cost weight",
        ClarityVmBehavior => "Clarity VM behavior",
        MiningFlow => "Mining flow",
        BlockValidation => "Block validation",
        MarfLayout => "MARF layout",
        OnChainFormat => "On-chain format",
    }
}

fn humanize_merge_method(m: crate::models::targets::MergeMethod) -> &'static str {
    use crate::models::targets::MergeMethod::*;
    match m {
        Llm => "LLM",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::analyze::Analysis;
    use crate::models::common::{
        BreakageClass, Bucket, DeliveryMode, Hotspot, ImprovementVector, LensDispositionEntry,
        LensDispositionStatus, Risk, SchemaVersionV2, SelectionLens,
    };
    use crate::models::summary::{
        ConsensusIssueCounts, ConsensusPocPrCounts, Experiment, ExperimentStatus, NormalPrCounts,
        OutcomeCounts, Summary,
    };
    use crate::models::targets::{MergeMethod, MergedFrom, MergedTarget, OptimizationTargets};

    fn fixture_targets() -> OptimizationTargets {
        OptimizationTargets {
            schema_version: SchemaVersionV2,
            session_id: "20260511-063216".into(),
            baseline_run_id: 4,
            baseline_rerun_id: 4,
            noise_floor_pct: 1.0,
            merge_method: MergeMethod::Llm,
            merge_model: "gpt-5.5".into(),
            targets: vec![
                MergedTarget {
                    id: "sqlite-side-store-batched-replace".into(),
                    merged_from: vec![MergedFrom {
                        family_id: "marf-block-commit-seal-family".into(),
                        target_index: 0,
                    }],
                    convergence_count: 1,
                    rank: Some(1),
                    target_span: "put".into(),
                    bucket: Bucket::BlockCommit,
                    hotspot: Hotspot {
                        span: "put".into(),
                        self_wall_us: 1_000_000,
                        total_wall_us: 1_500_000,
                        calls: 250,
                        location: "src/sqlite_side_store.rs:120".into(),
                    },
                    files: vec![
                        "stackslib/src/chainstate/stacks/index/sqlite_side_store.rs".into(),
                    ],
                    evidence: "Repeated REPLACE INTO ...".into(),
                    proposed_change: "Batch REPLACE INTO into transactions of 256 rows".into(),
                    expected_improvement: ImprovementVector {
                        tx_latency: 0.0,
                        tenure_throughput: 0.0,
                        commit_time: 8.5,
                    },
                    risk: Risk::Low,
                    verification_plan: "Full nextest pass + bench rerun".into(),
                    verification_replay: None,
                    merge_notes: None,
                    contributor_differences: None,
                    consensus_breaking: false,
                    breakage_class: None,
                    poc_implementable: None,
                    poc_test_scope: None,
                    consensus_writeup: None,
                    delivery_mode: DeliveryMode::NormalPr,
                    bench_eligible: true,
                },
                MergedTarget {
                    id: "clarity-borrowed-tuple-get-cost".into(),
                    merged_from: vec![MergedFrom {
                        family_id: "v0-market-supply-collateral-runtime-family".into(),
                        target_index: 0,
                    }],
                    convergence_count: 1,
                    rank: Some(2),
                    target_span: "lookup_variable".into(),
                    bucket: Bucket::BlockProcessing,
                    hotspot: Hotspot {
                        span: "lookup_variable".into(),
                        self_wall_us: 50_000,
                        total_wall_us: 75_000,
                        calls: 1000,
                        location: "clarity/src/vm/functions/tuples.rs:80".into(),
                    },
                    files: vec!["clarity/src/vm/functions/tuples.rs".into()],
                    evidence: "Tuple-get charges LookupVariableSize for the whole tuple".into(),
                    proposed_change: "Borrow-aware path that charges per selected field".into(),
                    expected_improvement: ImprovementVector {
                        tx_latency: 12.0,
                        tenure_throughput: 8.0,
                        commit_time: 0.0,
                    },
                    risk: Risk::Medium,
                    verification_plan: "Scoped nextest under stackslib::clarity_vm::tests".into(),
                    verification_replay: None,
                    merge_notes: Some("Cost weight change requires HIP".into()),
                    contributor_differences: None,
                    consensus_breaking: true,
                    breakage_class: Some(BreakageClass::ClarityCostWeight),
                    poc_implementable: Some(true),
                    poc_test_scope: Some(vec![
                        "package(stackslib) & test(/clarity_vm::tests::costs::/)".into(),
                    ]),
                    consensus_writeup: Some("Pre-sanitized epochs charge whole-tuple cost".into()),
                    delivery_mode: DeliveryMode::ConsensusPocPr,
                    bench_eligible: false,
                },
            ],
            rejected_by_merge: vec![],
            lens_dispositions: vec![LensDispositionEntry {
                family_id: "blocksurvey-proof-submission-write-budget-family".into(),
                lens: SelectionLens::TenureThroughput,
                status: LensDispositionStatus::NotActionable,
                reason: Some("Write budget bound to deployed contract".into()),
            }],
        }
    }

    fn fixture_summary() -> Summary {
        Summary {
            schema_version: SchemaVersionV2,
            session_id: "20260511-063216".into(),
            baseline_run_id: 4,
            baseline_rerun_id: 4,
            noise_floor_pct: 1.0,
            experiments: vec![
                Experiment {
                    target_id: "sqlite-side-store-batched-replace".into(),
                    delivery_mode: DeliveryMode::NormalPr,
                    status: ExperimentStatus::Accepted,
                    run_ids: Some(vec![11, 12]),
                    baseline_run_ids: None,
                    improvement_pct: Some(24.99),
                    breakage_class: None,
                    base_sha: None,
                    head_sha: None,
                    reason: None,
                },
                Experiment {
                    target_id: "clarity-borrowed-tuple-get-cost".into(),
                    delivery_mode: DeliveryMode::ConsensusPocPr,
                    status: ExperimentStatus::PocLanded,
                    run_ids: None,
                    baseline_run_ids: None,
                    improvement_pct: None,
                    breakage_class: Some(BreakageClass::ClarityCostWeight),
                    base_sha: None,
                    head_sha: None,
                    reason: None,
                },
            ],
            outcome_counts: OutcomeCounts {
                normal_pr: NormalPrCounts {
                    accepted: 1,
                    rejected: 0,
                    aborted: 0,
                },
                consensus_poc_pr: ConsensusPocPrCounts { poc_landed: 1, aborted: 0 },
                consensus_issue: ConsensusIssueCounts { routed_to_issue: 0, aborted: 0 },
            },
            lens_dispositions: vec![LensDispositionEntry {
                family_id: "blocksurvey-proof-submission-write-budget-family".into(),
                lens: SelectionLens::TenureThroughput,
                status: LensDispositionStatus::NotActionable,
                reason: Some("Write budget bound to deployed contract".into()),
            }],
            next_targets_hint: Some("1 PR(s) + 1 PoC PR(s) + 0 issue(s) of 2 target(s)".into()),
        }
    }

    fn fixture_analyses() -> BTreeMap<String, Analysis> {
        // Empty map is sufficient for the rendering tests — the coverage
        // matrix degrades gracefully to "-" cells, and target sections
        // do not consume analyses content (only the contributors block,
        // which uses the merged target's `merged_from` list).
        BTreeMap::new()
    }

    #[test]
    fn summary_md_includes_chart_and_links() {
        let summary = fixture_summary();
        let targets = fixture_targets();
        let analyses = fixture_analyses();
        let md = render_summary_md(&summary, &targets, &analyses, &BTreeMap::new());

        assert!(md.contains("[targets.md](targets.md)"));
        assert!(md.contains("```mermaid"));
        assert!(md.contains("xychart-beta"));
        // Chart label is truncated at 24 chars for readability.
        assert!(md.contains("\"sqlite-side-store-batch…\""));
        // Full id appears unmodified in the experiments table.
        assert!(md.contains(
            "[sqlite-side-store-batched-replace](../optimize/sqlite-side-store-batched-replace/)"
        ));
        assert!(md.contains(
            "[Accepted](../optimize/sqlite-side-store-batched-replace/implementation.md)"
        ));
        assert!(
            md.contains("[11](../optimize/sqlite-side-store-batched-replace/run-1/bench-run.json)")
        );
        assert!(
            md.contains("[12](../optimize/sqlite-side-store-batched-replace/run-2/bench-run.json)")
        );
        assert!(md.contains(
            "[blocksurvey-proof-submission-write-budget-family](../analysis/\
             blocksurvey-proof-submission-write-budget-family/analysis.json)"
        ));
    }

    #[test]
    fn summary_md_omits_chart_when_no_measurements() {
        let mut summary = fixture_summary();
        for e in &mut summary.experiments {
            e.improvement_pct = None;
        }
        let md =
            render_summary_md(&summary, &fixture_targets(), &fixture_analyses(), &BTreeMap::new());
        assert!(!md.contains("```mermaid"));
    }

    #[test]
    fn targets_md_renders_each_target_with_contributors_and_outputs() {
        let targets = fixture_targets();
        let analyses = fixture_analyses();
        let md = render_targets_md(&targets, &analyses);

        assert!(md.contains("# Optimization targets — session 20260511-063216"));
        assert!(md.contains("## Contents"));
        assert!(
            md.contains("[sqlite-side-store-batched-replace](#sqlite-side-store-batched-replace)")
        );
        assert!(md.contains("## sqlite-side-store-batched-replace"));
        assert!(md.contains("## clarity-borrowed-tuple-get-cost"));

        // Consensus block on the second target.
        assert!(md.contains("**Consensus-breaking**"));
        assert!(md.contains("Breakage class: Clarity cost weight"));
        assert!(md.contains("**Consensus writeup**"));

        // Contributors link out to the analyses dir.
        assert!(md.contains(
            "[marf-block-commit-seal-family](../analysis/marf-block-commit-seal-family/analysis.\
             json)"
        ));

        // Outputs links into the experiments dir.
        assert!(md.contains(
            "[`../optimize/sqlite-side-store-batched-replace/`](../optimize/\
             sqlite-side-store-batched-replace/)"
        ));
        assert!(md.contains(
            "[implementation.md](../optimize/sqlite-side-store-batched-replace/implementation.md)"
        ));

        // Lens dispositions table appears.
        assert!(md.contains("## Lens dispositions"));
    }

    #[test]
    fn improvement_chart_skips_when_no_measured_targets() {
        let experiments = vec![Experiment {
            target_id: "x".into(),
            delivery_mode: DeliveryMode::ConsensusPocPr,
            status: ExperimentStatus::PocLanded,
            run_ids: None,
            baseline_run_ids: None,
            improvement_pct: None,
            breakage_class: None,
            base_sha: None,
            head_sha: None,
            reason: None,
        }];
        assert!(render_improvement_chart(&experiments).is_none());
    }

    #[test]
    fn truncate_label_appends_ellipsis_when_overflow() {
        assert_eq!(truncate_label("short", 10), "short");
        let long = "this-is-a-very-long-kebab-id";
        let out = truncate_label(long, 12);
        assert_eq!(out.chars().count(), 12);
        assert!(out.ends_with('…'));
    }
}
