//! Bundled JSON Schemas + on-disk seeding/sync/drift helpers.
//!
//! The canonical JSON Schemas are embedded into the binary via
//! `include_str!` at compile time. [`seed_to`] writes them to the
//! operator's
//! [`LayoutSettings::schemas_dir`](crate::settings::LayoutSettings::schemas_dir)
//! on `sbagent init` and at every CLI startup (with don't-replace
//! semantics); [`sync`] re-writes them unconditionally; [`drift`]
//! reports any operator-disk copy that doesn't byte-match the bundle.
//!
//! Asymmetry with [`crate::prompts`]:
//! - Prompts are tunable (autoresearch-style program.md). `seed_to` is
//!   don't-replace; `sync_force` requires explicit acknowledgement via
//!   `--force`; drift only warns.
//! - Schemas are versioned contract, not tuning surface. `seed_to` is still
//!   don't-replace (cheap idempotent default for init), but [`sync`] takes no
//!   `--force` flag — schemas should always move to the binary's bundled set —
//!   and drift FAILS the check.
//!
//! The bundle is sourced from `<repo-root>/schemas/*.schema.json`,
//! which is itself generated from the typed Rust models via
//! [`crate::schema_export::write_all`]. The schema-export drift gate
//! (compile-time-ish, run via `sbagent check`) keeps the on-disk
//! committed schemas + the typed models in sync; the operator-side
//! drift gate (this module's [`drift`]) keeps the operator's
//! `.sbagent/schemas/` in sync with the running binary's bundle.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::{Context as _, Result};

/// `(file_name, contents)` table for every bundled schema. File names
/// match the on-disk layout exactly.
///
/// Adding a new schema: create a placeholder so `include_str!`
/// resolves, rebuild, run `sbagent schema export --out schemas`,
/// then rebuild again so the binary embeds the populated bytes (not
/// the stub).
pub const BUNDLED_SCHEMAS: &[(&str, &str)] = &[
    ("candidates.schema.json", include_str!("../../../schemas/candidates.schema.json")),
    ("analysis.schema.json", include_str!("../../../schemas/analysis.schema.json")),
    (
        "optimization-targets.schema.json",
        include_str!("../../../schemas/optimization-targets.schema.json"),
    ),
    ("optimizer-report.schema.json", include_str!("../../../schemas/optimizer-report.schema.json")),
    (
        "coordinator-provenance.schema.json",
        include_str!("../../../schemas/coordinator-provenance.schema.json"),
    ),
    ("summary.schema.json", include_str!("../../../schemas/summary.schema.json")),
    ("session-record.schema.json", include_str!("../../../schemas/session-record.schema.json")),
    ("results-analysis.schema.json", include_str!("../../../schemas/results-analysis.schema.json")),
    ("source.schema.json", include_str!("../../../schemas/source.schema.json")),
    ("timings.schema.json", include_str!("../../../schemas/timings.schema.json")),
    ("publish-feedback.schema.json", include_str!("../../../schemas/publish-feedback.schema.json")),
    ("maintain-event.schema.json", include_str!("../../../schemas/maintain-event.schema.json")),
    ("optimizer-memory.schema.json", include_str!("../../../schemas/optimizer-memory.schema.json")),
];

/// Result of a [`seed_to`] call. Same shape as
/// [`crate::prompts::SeedReport`] — both vectors carry the canonical
/// file name so the caller can log what changed.
#[derive(Debug, Default)]
pub struct SeedReport {
    /// Schemas written this call (file was missing).
    pub seeded: Vec<&'static str>,
    /// Schemas left alone (file already existed on disk).
    pub kept: Vec<&'static str>,
}

/// Seed `dir` with every bundled schema, only writing files that don't
/// already exist. Used by [`crate::cli::init::run`] and by every
/// `sbagent` startup so a fresh operator gets a working baseline and
/// re-runs are no-ops. Creates `dir` if missing.
///
/// Uses `O_CREAT|O_EXCL` so concurrent seed calls can't race-corrupt
/// a file (matches the [`crate::prompts::seed_to`] safety story).
pub fn seed_to(dir: &Path) -> Result<SeedReport> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating schemas dir {}", dir.display()))?;
    let mut report = SeedReport::default();
    for (name, contents) in BUNDLED_SCHEMAS {
        let path = dir.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(contents.as_bytes())
                    .with_context(|| format!("writing seed schema to {}", path.display()))?;
                report.seeded.push(name);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                report.kept.push(name);
            }
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("seeding schema to {}", path.display()))
                );
            }
        }
    }
    Ok(report)
}

/// Force-rewrite every bundled schema to disk, **overwriting** whatever
/// was there. Schemas are versioned contract, not a tuning surface, so
/// `sync` takes no `--force` flag (unlike [`crate::prompts::sync_force`]):
/// the operator's `.sbagent/schemas/` is always a mirror of the binary's
/// bundle. Used by `sbagent sync`.
pub fn sync(dir: &Path) -> Result<Vec<&'static str>> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating schemas dir {}", dir.display()))?;
    let mut written = Vec::with_capacity(BUNDLED_SCHEMAS.len());
    for (name, contents) in BUNDLED_SCHEMAS {
        let path = dir.join(name);
        std::fs::write(&path, contents)
            .with_context(|| format!("syncing schema to {}", path.display()))?;
        written.push(*name);
    }
    Ok(written)
}

/// One drift finding from comparing `dir` against the embedded bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftEntry {
    /// File exists in the bundle but not at `<dir>/<file_name>`.
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

/// Compare every bundled schema against `<dir>/<file_name>`, returning
/// a list of [`DriftEntry`] for every mismatch. Empty list = no drift.
///
/// `sbagent check` calls this and **fails** on any non-empty result:
/// stale on-disk schemas mean the operator is validating agent output
/// against a different contract than the running binary expects.
/// Comparison is byte-exact — the bundle is canonical (it came from
/// the typed models via [`crate::schema_export::write_all`]), so
/// there's no normalization step needed.
pub fn drift(dir: &Path) -> Result<Vec<DriftEntry>> {
    let mut out = Vec::new();
    for (name, contents) in BUNDLED_SCHEMAS {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                if s != *contents {
                    out.push(DriftEntry::Differs { file_name: name });
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                out.push(DriftEntry::Missing { file_name: name });
            }
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("reading on-disk schema {}", path.display())));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_writes_missing_and_keeps_existing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let first = seed_to(dir).expect("first seed");
        assert_eq!(first.seeded.len(), BUNDLED_SCHEMAS.len());
        assert!(first.kept.is_empty());

        // Touching one schema with operator-edited content + re-seeding must
        // preserve the (broken) edit — seed is don't-replace.
        let candidates = dir.join("candidates.schema.json");
        std::fs::write(&candidates, "{\"hand-edited\":true}\n").unwrap();

        let second = seed_to(dir).expect("second seed");
        assert!(second.seeded.is_empty());
        assert_eq!(second.kept.len(), BUNDLED_SCHEMAS.len());
        assert_eq!(
            std::fs::read_to_string(&candidates).unwrap(),
            "{\"hand-edited\":true}\n",
            "seed_to must not overwrite",
        );
    }

    /// `sync` overwrites unconditionally — schemas are not a tuning
    /// surface, so the operator's on-disk copy should always reflect
    /// the running binary's bundle.
    #[test]
    fn sync_overwrites_unconditionally() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_to(dir).expect("seed");
        let candidates = dir.join("candidates.schema.json");
        std::fs::write(&candidates, "STALE\n").unwrap();

        sync(dir).expect("sync");
        let after = std::fs::read_to_string(&candidates).unwrap();
        assert!(after.contains("\"$defs\""), "sync must restore the bundled schema: {after}");
    }

    /// `drift` reports missing + differing files.
    #[test]
    fn drift_reports_missing_and_differs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        // Empty dir: every bundled file is Missing.
        let d = drift(dir).expect("drift on empty");
        assert_eq!(d.len(), BUNDLED_SCHEMAS.len());
        assert!(
            d.iter()
                .all(|e| matches!(e, DriftEntry::Missing { .. }))
        );

        seed_to(dir).expect("seed");
        // After seed: no drift.
        let d = drift(dir).expect("drift after seed");
        assert!(d.is_empty(), "expected no drift after seed, got: {d:?}");

        // Mutate one file: one Differs entry.
        std::fs::write(dir.join("candidates.schema.json"), "STALE\n").unwrap();
        let d = drift(dir).expect("drift after edit");
        assert_eq!(d.len(), 1);
        assert!(matches!(
            d[0],
            DriftEntry::Differs {
                file_name: "candidates.schema.json"
            }
        ));
    }

    /// Bundle contents must match the canonical schema-export output —
    /// catches the case where someone edits a Rust model without
    /// regenerating `<repo>/schemas/*.schema.json`. The committed
    /// files are the include_str! source; if they're stale, the
    /// bundle ships stale schemas.
    #[test]
    fn bundle_matches_schema_export() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        crate::schema_export::write_all(dir).expect("schema export");
        for (name, contents) in BUNDLED_SCHEMAS {
            let fresh = std::fs::read_to_string(dir.join(name)).expect("read exported");
            assert_eq!(
                fresh, *contents,
                "bundled {name} drifted from schema_export::write_all; run `sbagent schema \
                 export` and commit the result",
            );
        }
    }
}
