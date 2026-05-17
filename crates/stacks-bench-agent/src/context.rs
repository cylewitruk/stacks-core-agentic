//! Bundled context / reference docs + on-disk seeding/sync/drift helpers.
//!
//! Distinct from [`crate::prompts`] (renderable templates) and
//! [`crate::schemas`] (versioned JSON Schema contracts): context docs are
//! human-readable reference material that agents consult at runtime via
//! absolute paths. Each doc ships as a markdown body + a TOML sidecar that
//! declares which phases may surface it. The orchestrator filters docs by
//! phase and exposes the resolved paths to renderable prompts via named
//! template variables; prompts integrate the references behaviorally where
//! it makes sense (`{{ non_targets_path }}`, etc.).
//!
//! Asymmetry vs. the sibling bundles:
//! - **Prompts** are operator-tunable; [`seed_to`] is don't-replace,
//!   [`sync_force`] rewrites only under explicit user action, drift warns.
//! - **Schemas** are versioned contract; [`crate::schemas::seed_to`] is
//!   don't-replace, [`crate::schemas::sync`] rewrites unconditionally, drift
//!   **fails**.
//! - **Context** sits with prompts: operator-tunable. The sidecar metadata
//!   ships in lockstep with the markdown body, both treated as operator edits
//!   (warn on drift, force on `--force-tunables`).
//!
//! See `<repo>/context/*.md` + `<repo>/context/*.toml` for the bundled set.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;

/// Phases that may declare a dependency on a context doc. Adding a new
/// phase here requires touching no other context-bundle code — sidecars
/// can reference it immediately. Keep in sync with the actual prompt set
/// in [`crate::prompts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Phase 1.
    Triage,
    /// Phase 1.5.
    Analyzer,
    /// Phase 1.7.
    Merge,
    /// Phase 2.
    Optimizer,
    /// Phase 5 — draft PR shipping.
    PrWriter,
    /// Phase 5 — consensus-issue shipping.
    IssueWriter,
}

impl Phase {
    /// Snake-case identifier used in TOML sidecars.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Analyzer => "analyzer",
            Self::Merge => "merge",
            Self::Optimizer => "optimizer",
            Self::PrWriter => "pr_writer",
            Self::IssueWriter => "issue_writer",
        }
    }
}

/// Parsed TOML sidecar metadata for one context doc.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextManifest {
    /// Stable kebab-case identifier (matches the markdown filename's stem).
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// One-line description for `sbagent check` output and any future
    /// auto-rendered doc index.
    pub description: String,
    /// Phases that may declare this doc as required reading. Empty list
    /// is rejected at lint time — a doc nothing references is dead.
    pub phases: Vec<Phase>,
    /// When `true`, every phase in `phases` MUST have a rendered prompt
    /// that references this doc's path; `sbagent check`'s body-grep
    /// validation enforces it.
    pub required: bool,
}

/// One bundled context doc: markdown body + parsed sidecar.
struct BundledDoc {
    /// `<id>.md` filename inside the operator's context dir.
    md_name: &'static str,
    /// `<id>.toml` filename (the sidecar lives next to the markdown).
    toml_name: &'static str,
    /// Markdown body bundled via `include_str!`.
    md_body: &'static str,
    /// Sidecar TOML bundled via `include_str!`.
    toml_body: &'static str,
}

/// Compile-time table of every bundled context doc. Order is the seeding
/// + drift-iteration order.
const BUNDLED: &[BundledDoc] = &[
    BundledDoc {
        md_name: "non-targets.md",
        toml_name: "non-targets.toml",
        md_body: include_str!("../../../context/non-targets.md"),
        toml_body: include_str!("../../../context/non-targets.toml"),
    },
    BundledDoc {
        md_name: "bucket-anchors.md",
        toml_name: "bucket-anchors.toml",
        md_body: include_str!("../../../context/bucket-anchors.md"),
        toml_body: include_str!("../../../context/bucket-anchors.toml"),
    },
    BundledDoc {
        md_name: "stacks-domain-context.md",
        toml_name: "stacks-domain-context.toml",
        md_body: include_str!("../../../context/stacks-domain-context.md"),
        toml_body: include_str!("../../../context/stacks-domain-context.toml"),
    },
];

/// File name pairs for every bundled doc. Used by drift / sync / seed
/// iteration in the same shape as [`crate::prompts::BUNDLED_TEMPLATES`]
/// and [`crate::schemas::BUNDLED_SCHEMAS`].
pub fn bundled_file_names() -> Vec<&'static str> {
    let mut out = Vec::with_capacity(BUNDLED.len() * 2);
    for d in BUNDLED {
        out.push(d.md_name);
        out.push(d.toml_name);
    }
    out
}

/// Parse every bundled sidecar at the cost of one allocation per doc.
/// Returns a map `id → (manifest, md_body)` for callers that need to
/// resolve phase membership or surface metadata.
///
/// Errors only when a bundled sidecar fails to parse — that's a
/// development-time bug (the sidecars ship with the binary), so we
/// surface it loudly rather than silently degrading.
pub fn bundled_manifests() -> Result<BTreeMap<String, (ContextManifest, &'static str)>> {
    let mut out = BTreeMap::new();
    for d in BUNDLED {
        let manifest: ContextManifest = toml::from_str(d.toml_body).with_context(|| {
            format!("parsing bundled sidecar {} (this is a sbagent bug)", d.toml_name)
        })?;
        if manifest.id != stem(d.md_name) {
            return Err(anyhow!(
                "bundled sidecar {}: `id` (`{}`) must match the markdown file stem (`{}`)",
                d.toml_name,
                manifest.id,
                stem(d.md_name),
            ));
        }
        out.insert(manifest.id.clone(), (manifest, d.md_body));
    }
    Ok(out)
}

/// Look up a context-doc absolute path by its manifest id in a
/// [`paths_for_phase`] result. Surfaces a clear error when the manifest
/// didn't include the doc for the phase the orchestrator queried — that
/// usually means the operator dropped the entry from the sidecar's
/// `phases` field.
pub fn ctx_path(paths: &BTreeMap<String, PathBuf>, id: &str) -> Result<String> {
    Ok(paths
        .get(id)
        .with_context(|| {
            format!(
                "context doc `{id}` is not declared for this phase in any sidecar — check the \
                 `phases` field of `<operator>/.sbagent/context/{id}.toml`",
            )
        })?
        .to_string_lossy()
        .into_owned())
}

/// Resolve `<dir>/<id>.md` for every bundled doc whose sidecar lists
/// `phase` in its `phases`. Used by phase orchestrators to populate
/// the per-doc template variables (`{{ non_targets_path }}`,
/// `{{ bucket_anchors_path }}`, `{{ domain_context_path }}`, ...).
///
/// Returns `id → absolute_path`. The path is built unconditionally; the
/// file may or may not exist on disk at call time (operator may have
/// deleted it). Drift checking is the operator-facing gate; here we
/// just compute paths.
pub fn paths_for_phase(dir: &Path, phase: Phase) -> Result<BTreeMap<String, PathBuf>> {
    let manifests = bundled_manifests()?;
    let mut out = BTreeMap::new();
    for (id, (m, _)) in manifests {
        if m.phases.contains(&phase) {
            // Find the matching md file name (id + ".md") in the bundle.
            // Could derive from id but go through the bundle table for
            // safety against id/filename divergence.
            for d in BUNDLED {
                if stem(d.md_name) == id {
                    out.insert(id.clone(), dir.join(d.md_name));
                    break;
                }
            }
        }
    }
    Ok(out)
}

/// Result of a [`seed_to`] call. Mirrors [`crate::prompts::SeedReport`].
#[derive(Debug, Default)]
pub struct SeedReport {
    /// Files written this call (didn't exist on disk).
    pub seeded: Vec<&'static str>,
    /// Files left alone (already existed).
    pub kept: Vec<&'static str>,
}

/// Seed `dir` with every bundled doc + sidecar, only writing files that
/// don't already exist. Idempotent. Uses `O_CREAT|O_EXCL` to avoid
/// concurrent-seed corruption, same pattern as [`crate::prompts::seed_to`]
/// and [`crate::schemas::seed_to`].
pub fn seed_to(dir: &Path) -> Result<SeedReport> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating context dir {}", dir.display()))?;
    let mut report = SeedReport::default();
    for d in BUNDLED {
        for (name, body) in [(d.md_name, d.md_body), (d.toml_name, d.toml_body)] {
            let path = dir.join(name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    f.write_all(body.as_bytes())
                        .with_context(|| {
                            format!("writing seed context file to {}", path.display())
                        })?;
                    report.seeded.push(name);
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    report.kept.push(name);
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e)
                        .context(format!("seeding context file to {}", path.display())));
                }
            }
        }
    }
    Ok(report)
}

/// Force-rewrite every bundled doc + sidecar to disk. Same operator-tunable
/// contract as [`crate::prompts::sync_force`]: intended only for explicit
/// invocation (`sbagent sync --force-tunables` and friends).
pub fn sync_force(dir: &Path) -> Result<Vec<&'static str>> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating context dir {}", dir.display()))?;
    let mut written = Vec::with_capacity(BUNDLED.len() * 2);
    for d in BUNDLED {
        for (name, body) in [(d.md_name, d.md_body), (d.toml_name, d.toml_body)] {
            let path = dir.join(name);
            std::fs::write(&path, body)
                .with_context(|| format!("force-syncing context file to {}", path.display()))?;
            written.push(name);
        }
    }
    Ok(written)
}

/// One drift finding. Same shape as [`crate::prompts::DriftEntry`] so the
/// `sbagent check` reporter can render both bundles uniformly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftEntry {
    /// Bundle file doesn't exist at `<dir>/<file_name>`.
    Missing {
        /// Bundle file name.
        file_name: &'static str,
    },
    /// File exists on disk but doesn't byte-match the bundle.
    Differs {
        /// Bundle file name.
        file_name: &'static str,
    },
}

impl DriftEntry {
    /// File name this drift entry refers to.
    pub fn file_name(&self) -> &'static str {
        match self {
            Self::Missing { file_name } | Self::Differs { file_name } => file_name,
        }
    }
}

/// Compare every bundled file against `<dir>/<file_name>`. Drift is
/// warn-class per the operator-tunable contract (same as prompts).
pub fn drift(dir: &Path) -> Result<Vec<DriftEntry>> {
    let mut out = Vec::new();
    for d in BUNDLED {
        for (name, body) in [(d.md_name, d.md_body), (d.toml_name, d.toml_body)] {
            let path = dir.join(name);
            match std::fs::read_to_string(&path) {
                Ok(s) => {
                    if s != *body {
                        out.push(DriftEntry::Differs { file_name: name });
                    }
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    out.push(DriftEntry::Missing { file_name: name });
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e)
                        .context(format!("reading on-disk context file {}", path.display())));
                }
            }
        }
    }
    Ok(out)
}

/// Validate the bundle is internally consistent: every sidecar parses,
/// every `id` is unique, every `id` matches its markdown stem, every
/// `phases` list is non-empty, every `title`/`description` is non-empty.
///
/// Called from `sbagent check` AND used as an invariant test in
/// [`tests::bundle_is_internally_consistent`].
pub fn lint_bundle() -> Result<()> {
    let manifests = bundled_manifests()?;
    let mut seen_ids = BTreeSet::new();
    for (id, (m, body)) in &manifests {
        if !seen_ids.insert(id.clone()) {
            return Err(anyhow!("duplicate context-doc id `{id}` in bundle"));
        }
        if m.title.trim().is_empty() {
            return Err(anyhow!("context doc `{id}`: empty title"));
        }
        if m.description
            .trim()
            .is_empty()
        {
            return Err(anyhow!("context doc `{id}`: empty description"));
        }
        if m.phases.is_empty() {
            return Err(anyhow!(
                "context doc `{id}`: empty `phases` list — a doc no phase references is dead"
            ));
        }
        if body.trim().is_empty() {
            return Err(anyhow!("context doc `{id}`: empty markdown body"));
        }
    }
    Ok(())
}

/// Strip the `.md` / `.toml` extension from a filename to produce the
/// expected `id` value. Internal helper.
fn stem(filename: &str) -> &str {
    filename
        .rsplit_once('.')
        .map_or(filename, |(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundle ships with internally-consistent metadata: every sidecar
    /// parses, ids are unique + match filenames, phases lists are non-empty.
    #[test]
    fn bundle_is_internally_consistent() {
        lint_bundle().expect("bundle lint clean");
    }

    /// Seed writes missing files + keeps existing.
    #[test]
    fn seed_writes_missing_and_keeps_existing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let first = seed_to(dir).expect("first seed");
        assert_eq!(first.seeded.len(), BUNDLED.len() * 2);
        assert!(first.kept.is_empty());

        // Hand-edit one doc + one sidecar to simulate operator tunes.
        let non_targets = dir.join("non-targets.md");
        std::fs::write(&non_targets, "OPERATOR EDIT\n").unwrap();
        let sidecar = dir.join("non-targets.toml");
        std::fs::write(&sidecar, "OPERATOR EDIT\n").unwrap();

        let second = seed_to(dir).expect("second seed");
        assert!(second.seeded.is_empty());
        assert_eq!(second.kept.len(), BUNDLED.len() * 2);
        assert_eq!(std::fs::read_to_string(&non_targets).unwrap(), "OPERATOR EDIT\n");
        assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), "OPERATOR EDIT\n");
    }

    /// `sync_force` overwrites operator edits. Both the markdown body and
    /// the sidecar are rewritten — keeping them in lockstep is the whole
    /// point of the bundle.
    #[test]
    fn sync_force_overwrites_operator_edits() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_to(dir).expect("seed");
        let sidecar = dir.join("non-targets.toml");
        std::fs::write(&sidecar, "OPERATOR EDIT\n").unwrap();

        sync_force(dir).expect("sync");
        let after = std::fs::read_to_string(&sidecar).unwrap();
        assert!(after.contains("id = \"non-targets\""), "sync_force must restore: {after}");
    }

    /// Drift reports missing files (empty dir → every file Missing).
    #[test]
    fn drift_reports_missing_for_empty_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let d = drift(tmp.path()).expect("drift on empty");
        assert_eq!(d.len(), BUNDLED.len() * 2);
        assert!(
            d.iter()
                .all(|e| matches!(e, DriftEntry::Missing { .. }))
        );
    }

    /// Drift reports byte differences after operator edits.
    #[test]
    fn drift_reports_differs_after_edit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_to(dir).expect("seed");
        assert!(drift(dir).unwrap().is_empty());
        std::fs::write(dir.join("non-targets.md"), "EDIT\n").unwrap();
        let d = drift(dir).unwrap();
        assert_eq!(d.len(), 1);
        assert!(matches!(d[0], DriftEntry::Differs { file_name: "non-targets.md" }));
    }

    /// `paths_for_phase(Triage)` returns the docs whose sidecar lists
    /// `triage` — currently all three (non-targets, bucket-anchors,
    /// stacks-domain-context).
    #[test]
    fn paths_for_phase_triage_returns_three_docs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = paths_for_phase(tmp.path(), Phase::Triage).expect("paths");
        let ids: BTreeSet<&str> = p
            .keys()
            .map(String::as_str)
            .collect();
        assert!(ids.contains("non-targets"));
        assert!(ids.contains("bucket-anchors"));
        assert!(ids.contains("stacks-domain-context"));
        // Every path is `<dir>/<id>.md`, NOT the sidecar.
        for (id, path) in &p {
            assert!(path.ends_with(format!("{id}.md")), "{id} → {}", path.display());
        }
    }

    /// `paths_for_phase(Merge)` returns ONLY bucket-anchors per the
    /// current sidecar phase lists.
    #[test]
    fn paths_for_phase_merge_returns_bucket_anchors_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = paths_for_phase(tmp.path(), Phase::Merge).expect("paths");
        let ids: BTreeSet<&str> = p
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            ids,
            ["bucket-anchors"]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
    }

    /// `paths_for_phase(IssueWriter)` returns empty for now — no current
    /// bundle doc declares the issue_writer phase, but the enum accepts
    /// it so future docs can target it without a code change.
    #[test]
    fn paths_for_phase_issue_writer_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = paths_for_phase(tmp.path(), Phase::IssueWriter).expect("paths");
        assert!(p.is_empty());
    }
}
