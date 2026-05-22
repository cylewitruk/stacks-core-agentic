//! `<session>/results/baseline/bin/manifest.json` — typed provenance for
//! the archived Phase 0a baseline binary. Lives next to the binary in
//! the session results so every downstream phase (Phase 0b, 1.8,
//! preflight resume gate, finalize audit trail) reads identical
//! semantics off one source.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Provenance for the archived `stacks-bench` binary. Written by
/// Phase 0a, consumed by every later phase that needs to confirm the
/// binary's source identity (preflight resume gate compares
/// `source_sha` against per-target provenance; finalize quotes both
/// fields in the audit chain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BaselineBinaryManifest {
    /// 40-char hex SHA — `git rev-parse HEAD` on `repos/<base>` at
    /// archive time.
    pub source_sha: String,
    /// True when the worktree had uncommitted changes at archive
    /// time; `source_sha` alone does not identify the build in that
    /// case. Surfaced by audit trails so reviewers know to look at
    /// `cargo_version` + `archived_at` for additional context.
    pub dirty: bool,
    /// `cargo --version` output at archive time. Doesn't change the
    /// SHA but does change the produced binary; recorded for the
    /// "why does identical source produce different timings" case.
    pub cargo_version: String,
    /// The exact `cargo build` flags Phase 0a used.
    pub build_flags: Vec<String>,
    /// ISO 8601 timestamp at archive time.
    pub archived_at: String,
    /// Absolute path of the archived binary on the operator's disk.
    pub archived_path: PathBuf,
}

impl BaselineBinaryManifest {
    /// Read + parse `<session>/results/baseline/bin/manifest.json`.
    /// Fails on missing file, malformed JSON, schema mismatch, or a
    /// `source_sha` that isn't a 40-char hex string (resume gates
    /// trust this value, so reject obvious garbage at the boundary).
    pub fn read(path: &Path) -> Result<Self> {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let m: Self =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        m.validate()
            .with_context(|| format!("validating {}", path.display()))?;
        Ok(m)
    }

    fn validate(&self) -> Result<()> {
        if self.source_sha.len() != 40
            || !self
                .source_sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            bail!("source_sha must be a 40-char hex string, got {:?}", self.source_sha);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BaselineBinaryManifest {
        BaselineBinaryManifest {
            source_sha: "0ad33704c259da4102b5f195617760003ac89c18".to_owned(),
            dirty: false,
            cargo_version: "cargo 1.94.1 (29ea6fb6a 2026-03-24)".to_owned(),
            build_flags: vec!["--release".to_owned(), "-p".to_owned(), "stacks-bench".to_owned()],
            archived_at: "2026-05-20T21:01:10Z".to_owned(),
            archived_path: PathBuf::from("/Users/x/sessions/y/results/baseline/bin/stacks-bench"),
        }
    }

    #[test]
    fn round_trips_through_serde() {
        let m = sample();
        let s = serde_json::to_string_pretty(&m).unwrap();
        let back: BaselineBinaryManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn rejects_unknown_fields() {
        let bad = r#"{"source_sha":"0ad33704c259da4102b5f195617760003ac89c18","dirty":false,
            "cargo_version":"x","build_flags":[],"archived_at":"x","archived_path":"/x",
            "extra":"nope"}"#;
        assert!(serde_json::from_str::<BaselineBinaryManifest>(bad).is_err());
    }

    #[test]
    fn read_rejects_non_hex_source_sha() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("manifest.json");
        let mut m = sample();
        m.source_sha = "not-a-real-sha".to_owned();
        std::fs::write(&path, serde_json::to_string(&m).unwrap()).unwrap();
        let err = BaselineBinaryManifest::read(&path).unwrap_err();
        assert!(
            format!("{err:#}").contains("source_sha must be a 40-char hex string"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn read_rejects_wrong_length_source_sha() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("manifest.json");
        let mut m = sample();
        m.source_sha = "0ad33704".to_owned();
        std::fs::write(&path, serde_json::to_string(&m).unwrap()).unwrap();
        assert!(BaselineBinaryManifest::read(&path).is_err());
    }
}
