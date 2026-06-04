//! Bundled triage / analyzer SQL queries + on-disk seeding helpers.
//!
//! The 17 `.sql` files under `<repo>/queries/` (orientation,
//! candidate-ranking, drilldown) plus the operator-facing `README.md`
//! are embedded into the binary via `include_str!`. [`seed_to`] writes
//! them to the operator's
//! [`LayoutSettings::queries_dir`](crate::settings::LayoutSettings::queries_dir) on
//! `sbagent init` and at every CLI startup (don't-replace); [`sync`]
//! rewrites them unconditionally; [`drift`] reports any operator-disk
//! copy that doesn't byte-match the bundle.
//!
//! Symmetry with [`crate::schemas`]: same write semantics. Queries are
//! versioned contract (the typed candidates/analysis output depends on
//! their column names + ordering), not a tuning surface — `sync` takes
//! no flag, and `check` fails on drift. If an operator wants a custom
//! drilldown, they can `.read` it from inside their session prompt
//! without overwriting the bundle.
//!
//! Why bundle queries (vs. read them from a framework checkout):
//! makes the operator dir fully self-contained. Pre-bundle, triage and
//! analyzer required `framework_root` to be set in config so they
//! could read `<framework>/queries/*.sql`. Post-bundle, the operator
//! dir alone is enough.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::{Context as _, Result};

/// `(file_name, contents)` table for every bundled query + the
/// operator-facing README. File names match the on-disk layout
/// exactly. Skips the historical `dump.json` Metabase capture
/// (reference-only, ~322 KB, no runtime consumer).
pub const BUNDLED_QUERIES: &[(&str, &str)] = &[
    ("README.md", include_str!("../../../queries/README.md")),
    (
        "baseline_empty_block_breakdown.sql",
        include_str!("../../../queries/baseline_empty_block_breakdown.sql"),
    ),
    ("block_timing_breakdown.sql", include_str!("../../../queries/block_timing_breakdown.sql")),
    ("profiler_trace_block.sql", include_str!("../../../queries/profiler_trace_block.sql")),
    ("profiler_trace_tx.sql", include_str!("../../../queries/profiler_trace_tx.sql")),
    ("run_summary.sql", include_str!("../../../queries/run_summary.sql")),
    (
        "span_per_block_distribution.sql",
        include_str!("../../../queries/span_per_block_distribution.sql"),
    ),
    (
        "span_per_sample_distribution.sql",
        include_str!("../../../queries/span_per_sample_distribution.sql"),
    ),
    ("span_recurrence.sql", include_str!("../../../queries/span_recurrence.sql")),
    ("span_run_drift.sql", include_str!("../../../queries/span_run_drift.sql")),
    ("top_blocks_for_span.sql", include_str!("../../../queries/top_blocks_for_span.sql")),
    (
        "top_clarity_consumers_by_contract.sql",
        include_str!("../../../queries/top_clarity_consumers_by_contract.sql"),
    ),
    ("top_contract_calls.sql", include_str!("../../../queries/top_contract_calls.sql")),
    ("top_spans_by_call_count.sql", include_str!("../../../queries/top_spans_by_call_count.sql")),
    ("top_spans_by_self_wall.sql", include_str!("../../../queries/top_spans_by_self_wall.sql")),
    ("top_txs_by_duration.sql", include_str!("../../../queries/top_txs_by_duration.sql")),
    ("tx_type_distribution.sql", include_str!("../../../queries/tx_type_distribution.sql")),
    ("txs_for_contract.sql", include_str!("../../../queries/txs_for_contract.sql")),
];

/// Result of a [`seed_to`] call. Same shape as
/// [`crate::schemas::SeedReport`].
#[derive(Debug, Default)]
pub struct SeedReport {
    /// Files written this call (file was missing).
    pub seeded: Vec<&'static str>,
    /// Files left alone (already on disk).
    pub kept: Vec<&'static str>,
}

/// Seed `dir` with every bundled query + README, only writing files
/// that don't already exist. Used by [`crate::cli::init::run`] and
/// every `sbagent` startup. Creates `dir` if missing.
///
/// Uses `O_CREAT|O_EXCL` so concurrent seed calls can't race-corrupt
/// a file (matches [`crate::schemas::seed_to`]).
pub fn seed_to(dir: &Path) -> Result<SeedReport> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating queries dir {}", dir.display()))?;
    let mut report = SeedReport::default();
    for (name, contents) in BUNDLED_QUERIES {
        let path = dir.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(contents.as_bytes())
                    .with_context(|| format!("writing seed query to {}", path.display()))?;
                report.seeded.push(name);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                report.kept.push(name);
            }
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("seeding query to {}", path.display()))
                );
            }
        }
    }
    Ok(report)
}

/// Force-rewrite every bundled query to disk. Queries are versioned
/// contract, not a tuning surface, so `sync` takes no `--force` flag.
pub fn sync(dir: &Path) -> Result<Vec<&'static str>> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating queries dir {}", dir.display()))?;
    let mut written = Vec::with_capacity(BUNDLED_QUERIES.len());
    for (name, contents) in BUNDLED_QUERIES {
        let path = dir.join(name);
        std::fs::write(&path, contents)
            .with_context(|| format!("syncing query to {}", path.display()))?;
        written.push(*name);
    }
    Ok(written)
}

/// One drift finding from comparing `dir` against the embedded bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftEntry {
    /// File in the bundle is missing under `<dir>/<file_name>`.
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

/// Compare every bundled file against `<dir>/<file_name>`. Empty list
/// = no drift. `sbagent check` fails on any non-empty result.
pub fn drift(dir: &Path) -> Result<Vec<DriftEntry>> {
    let mut out = Vec::new();
    for (name, contents) in BUNDLED_QUERIES {
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
                    .context(format!("reading on-disk query {}", path.display())));
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
        assert_eq!(first.seeded.len(), BUNDLED_QUERIES.len());
        assert!(first.kept.is_empty());

        let touched = dir.join("run_summary.sql");
        std::fs::write(&touched, "-- OPERATOR EDIT\n").unwrap();
        let second = seed_to(dir).expect("second seed");
        assert!(second.seeded.is_empty());
        assert_eq!(second.kept.len(), BUNDLED_QUERIES.len());
        assert_eq!(std::fs::read_to_string(&touched).unwrap(), "-- OPERATOR EDIT\n");
    }

    #[test]
    fn sync_overwrites_unconditionally() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_to(dir).expect("seed");
        let touched = dir.join("run_summary.sql");
        std::fs::write(&touched, "STALE\n").unwrap();

        sync(dir).expect("sync");
        let after = std::fs::read_to_string(&touched).unwrap();
        assert!(
            after.contains("SELECT") || after.contains("select"),
            "sync must restore bundle content (expected SQL); got: {after}",
        );
    }

    #[test]
    fn drift_reports_missing_and_differs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let d = drift(dir).expect("drift empty");
        assert_eq!(d.len(), BUNDLED_QUERIES.len());
        assert!(
            d.iter()
                .all(|e| matches!(e, DriftEntry::Missing { .. }))
        );

        seed_to(dir).expect("seed");
        let d = drift(dir).expect("drift after seed");
        assert!(d.is_empty(), "no drift expected after seed; got: {d:?}");

        std::fs::write(dir.join("run_summary.sql"), "STALE\n").unwrap();
        let d = drift(dir).expect("drift after edit");
        assert_eq!(d.len(), 1);
        assert!(matches!(d[0], DriftEntry::Differs { file_name: "run_summary.sql" }));
    }

    /// Every PRERENDERED query in `session::triage_queries` MUST exist
    /// in the bundle — otherwise `triage_queries::prerender` would
    /// silently log "missing on disk" and the triage agent would see
    /// empty CSVs for the orientation set. This locks down the
    /// bundle-vs-prerender list contract.
    #[test]
    fn bundle_covers_every_prerendered_triage_query() {
        let bundled: std::collections::HashSet<&str> = BUNDLED_QUERIES
            .iter()
            .map(|(name, _)| *name)
            .collect();
        for name in [
            "run_summary.sql",
            "tx_type_distribution.sql",
            "block_timing_breakdown.sql",
            "baseline_empty_block_breakdown.sql",
            "span_recurrence.sql",
            "top_spans_by_self_wall.sql",
            "top_spans_by_call_count.sql",
            "top_contract_calls.sql",
            "top_clarity_consumers_by_contract.sql",
            "top_txs_by_duration.sql",
        ] {
            assert!(
                bundled.contains(name),
                "PRERENDERED query {name} is not in BUNDLED_QUERIES — `triage_queries::prerender` \
                 would log a missing-on-disk warning at runtime",
            );
        }
    }
}
