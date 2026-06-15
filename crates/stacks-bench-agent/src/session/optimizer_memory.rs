//! Advisory cross-session optimizer memory.
//!
//! v13 uses the durable ledgers that already exist (`sessions.jsonl` and
//! `maintain.jsonl`) to build a compact, session-scoped memory artifact. The
//! artifact is prompt context only. It must never remove targets or replace
//! v12's deterministic dedup gate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result};

use crate::analyzed_rejections::now_utc_iso8601;
use crate::models::FromJsonValidated;
use crate::models::candidates::Candidates;
use crate::models::maintain_event::{MaintEvent, MaintEventKind};
use crate::models::optimizer_memory::{
    OptimizerMemoryAttempt, OptimizerMemoryFamily, OptimizerMemoryJson, OptimizerMemorySignature,
};
use crate::models::session_record::{SessionRecord, TargetRecord};
use crate::session::{SessionLayout, ledger_reader, maintain_ledger};

/// Default max attempts retained per exact signature.
pub const MAX_ATTEMPTS_PER_SIGNATURE: usize = 5;
/// Default max historical sibling signatures retained per family.
pub const MAX_SIGNATURES_PER_FAMILY: usize = 3;
/// Roughly 2k tokens at ~4 chars/token. This is a prompt-rendering guard, not
/// a schema invariant.
pub const MEMORY_RENDER_BUDGET_CHARS: usize = 8_000;

/// Build and write the current session's optimizer-memory artifact.
pub fn write_for_current_candidates(
    layout: &SessionLayout,
    operator_repo_root: &Path,
    current_source_sha: Option<&str>,
) -> Result<OptimizerMemoryJson> {
    let candidates = Candidates::from_json_validated(
        &std::fs::read_to_string(layout.candidates_json()).with_context(|| {
            format!(
                "reading {}",
                layout
                    .candidates_json()
                    .display()
            )
        })?,
    )
    .context("loading candidates.json for optimizer memory")?;
    let family_ids: BTreeSet<String> = candidates
        .candidates
        .iter()
        .map(|c| c.id.clone())
        .collect();

    let sessions = ledger_reader::read_all(&operator_repo_root.join("sessions.jsonl"))
        .context("reading sessions.jsonl for optimizer memory")?;
    for skipped in &sessions.skipped {
        eprintln!(
            "optimizer memory: skipping malformed sessions.jsonl line {}: {}",
            skipped.line_number, skipped.error
        );
    }
    let maintain = maintain_ledger::read_all(&operator_repo_root.join("maintain.jsonl"))
        .context("reading maintain.jsonl for optimizer memory")?;
    for skipped in &maintain.skipped {
        eprintln!(
            "optimizer memory: skipping malformed maintain.jsonl line {}: {}",
            skipped.line_number, skipped.error
        );
    }

    let memory = build_for_families(
        &family_ids,
        &sessions.records,
        &maintain.events,
        current_source_sha.map(str::to_owned),
        now_utc_iso8601(),
    );
    memory.write_atomic(&layout.optimizer_memory_json())?;
    Ok(memory)
}

/// Build a memory artifact from already-read ledgers.
pub fn build_for_families(
    family_ids: &BTreeSet<String>,
    sessions: &[SessionRecord],
    maintain_events: &[MaintEvent],
    current_source_sha: Option<String>,
    generated_at: String,
) -> OptimizerMemoryJson {
    let lifecycle = project_latest_lifecycle(maintain_events);
    let mut by_family: BTreeMap<String, BTreeMap<String, Vec<OptimizerMemoryAttempt>>> =
        BTreeMap::new();

    for session in sessions {
        for target in &session.targets {
            if !family_ids.contains(&target.family_id) {
                continue;
            }
            by_family
                .entry(target.family_id.clone())
                .or_default()
                .entry(target.id.clone())
                .or_default()
                .push(attempt_from_target(session, target, &lifecycle));
        }
    }

    let families = family_ids
        .iter()
        .map(|family_id| {
            let mut signatures: Vec<OptimizerMemorySignature> = by_family
                .get(family_id)
                .map(|by_sig| {
                    by_sig
                        .iter()
                        .map(|(fix_signature, attempts)| signature_row(fix_signature, attempts))
                        .collect()
                })
                .unwrap_or_default();
            signatures.sort_by(|a, b| {
                latest_finished_at(b)
                    .cmp(latest_finished_at(a))
                    .then_with(|| {
                        a.fix_signature
                            .cmp(&b.fix_signature)
                    })
            });
            let omitted_sibling_signatures = signatures
                .len()
                .saturating_sub(MAX_SIGNATURES_PER_FAMILY);
            signatures.truncate(MAX_SIGNATURES_PER_FAMILY);
            OptimizerMemoryFamily {
                family_id: family_id.clone(),
                signatures,
                omitted_sibling_signatures,
            }
        })
        .collect();

    OptimizerMemoryJson {
        schema_version: crate::models::common::SchemaVersionV1,
        generated_at,
        current_source_sha,
        families,
    }
}

/// Render a compact family-scoped memory snippet for analyzer/merge.
pub fn render_family_memory(memory: Option<&OptimizerMemoryJson>, family_id: &str) -> String {
    let Some(memory) = memory else {
        return format!("No optimizer memory artifact is available for family `{family_id}`.");
    };
    let Some(family) = memory
        .families
        .iter()
        .find(|f| f.family_id == family_id)
    else {
        return format!("No cross-session optimizer memory for family `{family_id}`.");
    };
    render_family(memory, family, None)
}

/// Render all memory rows for the merge prompt.
pub fn render_all_memory(memory: Option<&OptimizerMemoryJson>) -> String {
    let Some(memory) = memory else {
        return "No optimizer memory artifact is available.".to_owned();
    };
    if memory.families.is_empty() {
        return "No relevant cross-session optimizer memory.".to_owned();
    }
    let mut out = String::from(
        "Optimizer memory is advisory context only; v12 dedup owns hard skips.\nMissing \
         source_sha means codebase drift is unknown.\n",
    );
    for family in &memory.families {
        out.push('\n');
        out.push_str(&render_family(memory, family, None));
    }
    truncate_with_marker(out, MEMORY_RENDER_BUDGET_CHARS, "merge memory")
}

/// Render target-scoped memory for the optimizer prompt. Exact signature rows
/// are shown first when present; same-family sibling rows follow as context.
pub fn render_target_memory(
    memory: Option<&OptimizerMemoryJson>,
    family_id: &str,
    fix_signature: &str,
) -> String {
    let Some(memory) = memory else {
        return format!("No optimizer memory artifact is available for target `{fix_signature}`.");
    };
    let Some(family) = memory
        .families
        .iter()
        .find(|f| f.family_id == family_id)
    else {
        return format!("No cross-session optimizer memory for family `{family_id}`.");
    };
    render_family(memory, family, Some(fix_signature))
}

fn signature_row(
    fix_signature: &str,
    attempts: &[OptimizerMemoryAttempt],
) -> OptimizerMemorySignature {
    let mut attempts = attempts.to_vec();
    attempts.sort_by(|a, b| {
        b.finished_at
            .cmp(&a.finished_at)
            .then_with(|| {
                b.session_id
                    .cmp(&a.session_id)
            })
            .then_with(|| b.target_id.cmp(&a.target_id))
    });
    let omitted_attempts = attempts
        .len()
        .saturating_sub(MAX_ATTEMPTS_PER_SIGNATURE);
    attempts.truncate(MAX_ATTEMPTS_PER_SIGNATURE);
    OptimizerMemorySignature {
        fix_signature: fix_signature.to_owned(),
        attempts,
        omitted_attempts,
    }
}

fn latest_finished_at(sig: &OptimizerMemorySignature) -> &str {
    sig.attempts
        .first()
        .map(|a| a.finished_at.as_str())
        .unwrap_or("")
}

#[derive(Debug, Clone)]
struct LifecycleState {
    kind: MaintEventKind,
    observed_at: String,
    new_state: String,
    head_sha: Option<String>,
}

fn project_latest_lifecycle(events: &[MaintEvent]) -> BTreeMap<String, LifecycleState> {
    let mut sorted: Vec<&MaintEvent> = events.iter().collect();
    sorted.sort_by(|a, b| {
        a.observed_at
            .cmp(&b.observed_at)
            .then_with(|| a.pr_url.cmp(&b.pr_url))
            .then_with(|| a.issue_url.cmp(&b.issue_url))
    });

    let mut out = BTreeMap::new();
    for event in sorted {
        let Some(url) = event
            .pr_url
            .as_ref()
            .or(event.issue_url.as_ref())
        else {
            continue;
        };
        out.insert(
            url.clone(),
            LifecycleState {
                kind: event.kind,
                observed_at: event.observed_at.clone(),
                new_state: event.new_state.clone(),
                head_sha: event.head_sha.clone(),
            },
        );
    }
    out
}

fn attempt_from_target(
    session: &SessionRecord,
    target: &TargetRecord,
    lifecycle: &BTreeMap<String, LifecycleState>,
) -> OptimizerMemoryAttempt {
    let lifecycle_state = target
        .pr_url
        .as_ref()
        .or(target.issue_url.as_ref())
        .and_then(|url| lifecycle.get(url));
    OptimizerMemoryAttempt {
        session_id: session.id.clone(),
        target_id: target.id.clone(),
        finished_at: session.finished_at.clone(),
        status: target.status,
        delivery_mode: target.delivery_mode,
        reason_code: target.reason_code.clone(),
        source_sha: session.source_sha.clone(),
        pr_url: target.pr_url.clone(),
        issue_url: target.issue_url.clone(),
        lifecycle_kind: lifecycle_state.map(|s| s.kind),
        lifecycle_state: lifecycle_state.map(|s| s.new_state.clone()),
        lifecycle_observed_at: lifecycle_state.map(|s| s.observed_at.clone()),
        head_sha: lifecycle_state
            .and_then(|s| s.head_sha.clone())
            .or_else(|| target.head_sha.clone()),
    }
}

fn render_family(
    memory: &OptimizerMemoryJson,
    family: &OptimizerMemoryFamily,
    exact_signature: Option<&str>,
) -> String {
    let mut out = format!(
        "### Family `{}`\n\nMemory is advisory context only; do not drop current targets unless \
         coordinator dedup already did. Missing source_sha means codebase drift is unknown.\n",
        family.family_id
    );
    if family.signatures.is_empty() {
        out.push_str("\nNo prior attempts for this family in the compact memory view.\n");
        return out;
    }

    let mut signatures: Vec<&OptimizerMemorySignature> = Vec::new();
    if let Some(exact) = exact_signature {
        if let Some(sig) = family
            .signatures
            .iter()
            .find(|s| s.fix_signature == exact)
        {
            signatures.push(sig);
        } else {
            out.push_str(&format!(
                "\nNo exact prior signature row for `{exact}` in this compact view; same-family \
                 rows below are context, not a match.\n",
            ));
        }
    }
    for sig in &family.signatures {
        if !signatures
            .iter()
            .any(|existing| existing.fix_signature == sig.fix_signature)
        {
            signatures.push(sig);
        }
    }

    for sig in signatures {
        out.push_str(&format!("\n- Signature `{}`", sig.fix_signature));
        if sig.omitted_attempts > 0 {
            out.push_str(&format!(" ({} older attempt(s) omitted)", sig.omitted_attempts));
        }
        out.push('\n');
        for attempt in &sig.attempts {
            out.push_str("  - ");
            out.push_str(&format!(
                "session `{}` target `{}`: status={:?}, mode={:?}, finished_at={}",
                attempt.session_id,
                attempt.target_id,
                attempt.status,
                attempt.delivery_mode,
                attempt.finished_at,
            ));
            if let Some(reason) = &attempt.reason_code {
                out.push_str(&format!(", reason={reason}"));
            }
            if let Some(source_sha) = &attempt.source_sha {
                let relation = memory
                    .current_source_sha
                    .as_ref()
                    .map(|current| {
                        if current == source_sha {
                            "same as current"
                        } else {
                            "different from current"
                        }
                    })
                    .unwrap_or("current source unknown");
                out.push_str(&format!(", source_sha={source_sha} ({relation})"));
            } else {
                out.push_str(", source_sha=unknown (drift unknown)");
            }
            if let Some(kind) = attempt.lifecycle_kind {
                out.push_str(&format!(", lifecycle={kind:?}"));
            }
            if let Some(state) = &attempt.lifecycle_state {
                out.push_str(&format!(":{state}"));
            }
            if let Some(url) = attempt
                .pr_url
                .as_ref()
                .or(attempt.issue_url.as_ref())
            {
                out.push_str(&format!(", url={url}"));
            }
            out.push('\n');
        }
    }
    if family.omitted_sibling_signatures > 0 {
        out.push_str(&format!(
            "\n... ({} more sibling signature(s) omitted for prompt budget)\n",
            family.omitted_sibling_signatures
        ));
    }

    truncate_with_marker(out, MEMORY_RENDER_BUDGET_CHARS, &family.family_id)
}

fn truncate_with_marker(s: String, max_chars: usize, label: &str) -> String {
    if s.chars().count() <= max_chars {
        return s;
    }
    let mut truncated: String = s
        .chars()
        .take(max_chars)
        .collect();
    truncated
        .push_str(&format!("\n... (optimizer memory for {label} truncated; more rows omitted)\n"));
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::common::{DeliveryMode, SchemaVersionV1, SchemaVersionV3};
    use crate::models::session_record::{
        SessionRange, SessionRecordKind, SessionStatus, TargetStatus, TargetStatusStage,
    };

    #[test]
    fn projection_records_open_pr_context() {
        let pr = "https://github.com/owner/repo/pull/1";
        let family_ids = families(["family-a"]);
        let memory = build_for_families(
            &family_ids,
            &[session(
                "s1",
                "2026-06-12T00:00:00Z",
                Some("abc"),
                vec![target_pr("fix-a", pr, TargetStatus::Accepted)],
            )],
            &[event(MaintEventKind::PrOpen, pr, "2026-06-13T00:00:00Z")],
            Some("abc".to_owned()),
            "2026-06-14T00:00:00Z".to_owned(),
        );
        let attempt = &memory.families[0].signatures[0].attempts[0];
        assert_eq!(attempt.lifecycle_kind, Some(MaintEventKind::PrOpen));
        assert_eq!(attempt.pr_url.as_deref(), Some(pr));
        assert_eq!(attempt.source_sha.as_deref(), Some("abc"));
    }

    #[test]
    fn observed_at_order_wins_over_file_order() {
        let pr = "https://github.com/owner/repo/pull/1";
        let memory = build_for_families(
            &families(["family-a"]),
            &[session(
                "s1",
                "2026-06-12T00:00:00Z",
                None,
                vec![target_pr("fix-a", pr, TargetStatus::Accepted)],
            )],
            &[
                event(MaintEventKind::PrOpen, pr, "2026-06-13T02:00:00Z"),
                event(MaintEventKind::PrMerged, pr, "2026-06-13T01:00:00Z"),
            ],
            None,
            "2026-06-14T00:00:00Z".to_owned(),
        );
        assert_eq!(
            memory.families[0].signatures[0].attempts[0].lifecycle_kind,
            Some(MaintEventKind::PrOpen),
            "latest by observed_at should win, not file order"
        );
    }

    #[test]
    fn stale_pr_is_context_not_removed() {
        let pr = "https://github.com/owner/repo/pull/1";
        let memory = build_for_families(
            &families(["family-a"]),
            &[session(
                "s1",
                "2026-06-12T00:00:00Z",
                None,
                vec![target_pr("fix-a", pr, TargetStatus::Accepted)],
            )],
            &[event(MaintEventKind::PrStale, pr, "2026-06-13T00:00:00Z")],
            None,
            "2026-06-14T00:00:00Z".to_owned(),
        );
        let rendered = render_family_memory(Some(&memory), "family-a");
        assert!(rendered.contains("PrStale"));
        assert!(rendered.contains("advisory context only"));
    }

    #[test]
    fn unrelated_families_are_not_included() {
        let memory = build_for_families(
            &families(["family-a"]),
            &[
                session(
                    "s1",
                    "2026-06-12T00:00:00Z",
                    None,
                    vec![target("fix-a", "family-a", TargetStatus::Accepted)],
                ),
                session(
                    "s2",
                    "2026-06-13T00:00:00Z",
                    None,
                    vec![target("fix-b", "family-b", TargetStatus::Accepted)],
                ),
            ],
            &[],
            None,
            "2026-06-14T00:00:00Z".to_owned(),
        );
        assert_eq!(memory.families.len(), 1);
        assert_eq!(memory.families[0].family_id, "family-a");
        assert_eq!(memory.families[0].signatures[0].fix_signature, "fix-a");
    }

    #[test]
    fn compact_bounds_are_enforced() {
        let mut sessions = Vec::new();
        for i in 0..5 {
            sessions.push(session(
                &format!("sig-session-{i}"),
                &format!("2026-06-1{i}T00:00:00Z"),
                None,
                vec![target(&format!("fix-{i}"), "family-a", TargetStatus::Accepted)],
            ));
        }
        let memory = build_for_families(
            &families(["family-a"]),
            &sessions,
            &[],
            None,
            "2026-06-14T00:00:00Z".to_owned(),
        );
        assert_eq!(
            memory.families[0]
                .signatures
                .len(),
            MAX_SIGNATURES_PER_FAMILY
        );
        assert_eq!(memory.families[0].omitted_sibling_signatures, 2);
    }

    #[test]
    fn attempts_are_bounded_per_signature() {
        let sessions: Vec<_> = (0..7)
            .map(|i| {
                session(
                    &format!("s{i}"),
                    &format!("2026-06-1{i}T00:00:00Z"),
                    None,
                    vec![target("fix-a", "family-a", TargetStatus::Failed)],
                )
            })
            .collect();
        let memory = build_for_families(
            &families(["family-a"]),
            &sessions,
            &[],
            None,
            "2026-06-14T00:00:00Z".to_owned(),
        );
        let sig = &memory.families[0].signatures[0];
        assert_eq!(sig.attempts.len(), MAX_ATTEMPTS_PER_SIGNATURE);
        assert_eq!(sig.omitted_attempts, 2);
    }

    fn families<const N: usize>(ids: [&str; N]) -> BTreeSet<String> {
        ids.into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn session(
        id: &str,
        finished_at: &str,
        source_sha: Option<&str>,
        targets: Vec<TargetRecord>,
    ) -> SessionRecord {
        SessionRecord {
            kind: SessionRecordKind::SessionCompleted,
            schema_version: SchemaVersionV3,
            id: id.to_owned(),
            artifact_branch: format!("session/{id}"),
            artifact_sha: "a".repeat(40),
            artifact_url: None,
            started_at: "2026-06-11T17:29:55Z".to_owned(),
            finished_at: finished_at.to_owned(),
            status: SessionStatus::Succeeded,
            failure_phase: None,
            failure_reason: None,
            sbagent_version: "0.1.0".to_owned(),
            sbagent_git_sha: None,
            range: SessionRange {
                start_at: None,
                count: None,
                warmup: None,
                filter: None,
                network: "mainnet".to_owned(),
            },
            baseline_run_ids: vec![],
            phase_durations_secs: Default::default(),
            targets,
            source_url: None,
            source_branch: None,
            source_sha: source_sha.map(str::to_owned),
            source_fetched_at: None,
        }
    }

    fn target(id: &str, family: &str, status: TargetStatus) -> TargetRecord {
        TargetRecord {
            id: id.to_owned(),
            family_id: family.to_owned(),
            bucket: "block_processing".to_owned(),
            delivery_mode: DeliveryMode::NormalPr,
            status,
            status_stage: (status != TargetStatus::Accepted).then_some(TargetStatusStage::Bench),
            reason_code: (status != TargetStatus::Accepted).then_some("no_signal".to_owned()),
            head_sha: None,
            pr_url: None,
            issue_url: None,
            bench: None,
        }
    }

    fn target_pr(id: &str, pr: &str, status: TargetStatus) -> TargetRecord {
        let mut t = target(id, "family-a", status);
        t.pr_url = Some(pr.to_owned());
        t
    }

    fn event(kind: MaintEventKind, url: &str, observed_at: &str) -> MaintEvent {
        MaintEvent {
            schema_version: SchemaVersionV1,
            kind,
            observed_at: observed_at.to_owned(),
            session_id: "s1".to_owned(),
            target_id: Some("fix-a".to_owned()),
            family_id: Some("family-a".to_owned()),
            fix_signature: Some("fix-a".to_owned()),
            pr_url: Some(url.to_owned()),
            issue_url: None,
            prior_state: None,
            new_state: format!("{kind:?}"),
            head_sha: Some("abc123".to_owned()),
        }
    }
}
