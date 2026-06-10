//! `<session>/results/source.json` — typed source-provenance record
//! written exactly once at session start and read by every later
//! phase that needs to confirm the source state.
//!
//! Decoupled from the `[source]` config stanza
//! ([`crate::settings::SourceSettings`]): `source.json` carries the
//! **resolved** values (the exact URL the cache was fetched from, the exact
//! branch + SHA + fetched-at instant the session checkout was
//! clone-and-resolved against), while `[source]` carries the operator's
//! **intended** URL + branch. The two diverge at most by interpretation
//! (alias/redirect resolution) and by time-of-day (the SHA the branch resolved
//! to at that moment).
//!
//! Sister fields land on `Summary` ([`crate::models::summary::Summary`])
//! and `SessionRecord`
//! ([`crate::models::session_record::SessionRecord`]) — both gained
//! `source_url` / `source_branch` / `source_sha` / `source_fetched_at`
//! as part of the v3 iteration schema bumps. `source.json` is the
//! per-session canonical writer; the other two carry copies for ledger
//! + summary self-containment.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::SchemaVersionV1;
use crate::models::{FromJsonValidated, ValidateModel};

/// Per-session source-provenance record. v1 of the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceJson {
    /// Constant: 1.
    pub schema_version: SchemaVersionV1,
    /// Clone URL passed to `git fetch` at session start. Canonical form
    /// matches the operator's `[source].url` setting; sbagent does
    /// **not** rewrite the URL (so an SSH config alias remains an SSH
    /// config alias in the record).
    pub url: String,
    /// Branch fetched + checked out. Matches the operator's
    /// `[source].branch`.
    pub branch: String,
    /// 40-char hex SHA the branch resolved to at session start. The
    /// load-bearing anchor: this SHA is what every Phase 2 per-target
    /// optimizer clone forks from, and what archive + finalize quote
    /// as the source identity.
    pub sha: String,
    /// ISO 8601 UTC timestamp of the `git fetch` resolution. Useful
    /// when the same SHA reappears in a later session and you want
    /// to know whether the operator was on a recent or stale view.
    pub fetched_at: String,
    /// Cache id used to derive both the bare-cache path
    /// (`<workspace>/cache/<cache_id>.git/`) and the per-session
    /// source checkout (`<workspace>/sessions/<id>/repos/<cache_id>/`).
    /// Recorded here so resume + standalone commands derive the same
    /// paths the session originally used — even if the operator
    /// removes or changes `[source].id` in config between invocations.
    /// Slug-validated by [`SourceJson::validate_model`].
    pub cache_id: String,
}

impl SourceJson {
    /// Write `self` to `path` atomically AND write-once. Refuses to
    /// overwrite an existing file: the v3 contract is that
    /// `source.json` is written once at session start and never
    /// mutated. The atomic-temp-then-link approach ensures a reader
    /// that interrupts a writer never sees a half-written file.
    ///
    /// Returns `Err` (without touching the existing file) if `path`
    /// already exists. Callers that legitimately need to re-write
    /// after a crash + manual cleanup should remove `path` first.
    pub fn write(&self, path: &Path) -> Result<()> {
        self.validate_model()?;
        let parent = path
            .parent()
            .with_context(|| format!("source.json path has no parent: {}", path.display()))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating source.json parent {}", parent.display()))?;
        let pretty =
            serde_json::to_string_pretty(self).context("serializing SourceJson to JSON")?;
        // Atomic write-once: write to a temp file in the same dir,
        // then `link()` onto the final path via `persist_noclobber` —
        // refuses if the destination exists. Same-dir link is atomic
        // on every POSIX filesystem we care about, and the noclobber
        // variant enforces the v3 "written once, never mutated"
        // contract at the lowest level.
        let mut tmp = tempfile::Builder::new()
            .prefix(".source.json.")
            .suffix(".tmp")
            .tempfile_in(parent)
            .with_context(|| format!("opening temp for {}", path.display()))?;
        {
            use std::io::Write as _;
            tmp.write_all(pretty.as_bytes())
                .with_context(|| format!("writing temp for {}", path.display()))?;
            tmp.as_file_mut()
                .sync_all()
                .with_context(|| format!("fsync temp for {}", path.display()))?;
        }
        tmp.persist_noclobber(path)
            .map_err(|e| {
                anyhow::anyhow!(
                    "source.json at {} already exists; refusing to overwrite (write-once \
                     contract): {}",
                    path.display(),
                    e.error,
                )
            })?;
        Ok(())
    }

    /// Read + parse + validate `<session>/results/source.json`. Fails
    /// loud on:
    /// - missing file
    /// - malformed JSON
    /// - schema mismatch (`schema_version != 1`, unknown fields, wrong types)
    /// - `sha` not a 40-char hex string
    /// - empty `url` / `branch` / `fetched_at`
    pub fn read(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading source.json at {}", path.display()))?;
        Self::from_json_validated(&raw)
            .with_context(|| format!("parsing/validating source.json at {}", path.display()))
    }
}

impl ValidateModel for SourceJson {
    fn validate_model(&self) -> Result<()> {
        if self.url.is_empty() {
            bail!("source.json: url must not be empty");
        }
        if self.branch.is_empty() {
            bail!("source.json: branch must not be empty");
        }
        if self.fetched_at.is_empty() {
            bail!("source.json: fetched_at must not be empty");
        }
        if self.sha.len() != 40
            || !self
                .sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            bail!("source.json: sha must be a 40-char hex string, got {:?}", self.sha,);
        }
        // cache_id is a path segment under `<workspace>/cache/` and
        // `<workspace>/sessions/<id>/repos/` — same slug constraint as
        // `[source].id` (path-escape proof).
        crate::settings::validate_source_id(&self.cache_id).map_err(|e| {
            anyhow::anyhow!("source.json: cache_id `{}` is not a valid slug: {}", self.cache_id, e)
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SourceJson {
        SourceJson {
            schema_version: SchemaVersionV1,
            url: "https://github.com/stacks-network/stacks-core.git".to_owned(),
            branch: "feat/stacks-bench".to_owned(),
            sha: "0ad33704c259da4102b5f195617760003ac89c18".to_owned(),
            fetched_at: "2026-06-07T12:00:00Z".to_owned(),
            cache_id: "stacks-core-feat-stacks-bench".to_owned(),
        }
    }

    #[test]
    fn round_trip_through_disk_preserves_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.json");
        let s = sample();
        s.write(&path).unwrap();
        let back = SourceJson::read(&path).unwrap();
        assert_eq!(back, s);
    }

    /// v3 write-once contract: the second write against an existing
    /// path must fail and must NOT mutate the original file.
    #[test]
    fn write_refuses_to_overwrite_an_existing_source_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.json");

        let first = sample();
        first.write(&path).unwrap();
        let first_bytes = std::fs::read(&path).unwrap();

        // Second writer with a DIFFERENT sha — a real bug would
        // silently flip the provenance on disk.
        let mut second = sample();
        second.sha = "ffffffffffffffffffffffffffffffffffffffff".to_owned();
        let err = second
            .write(&path)
            .expect_err("second write must fail under write-once contract");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("already exists") && msg.contains("refusing to overwrite"),
            "expected overwrite-refusal message, got: {msg}",
        );

        // Original bytes preserved verbatim — no half-write, no flip.
        let after_bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            first_bytes, after_bytes,
            "existing source.json must be byte-for-byte unchanged after a refused write",
        );

        // And the parsed contents still match the FIRST writer, not
        // the second.
        let parsed = SourceJson::read(&path).unwrap();
        assert_eq!(parsed.sha, first.sha);
        assert_ne!(parsed.sha, second.sha);
    }

    #[test]
    fn write_is_atomic_via_tempfile_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.json");
        let s = sample();
        s.write(&path).unwrap();
        // No stray `.source.json.*` temp file left behind after a
        // successful write.
        for entry in std::fs::read_dir(tmp.path()).unwrap() {
            let name = entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .into_owned();
            if name == "source.json" {
                continue;
            }
            assert!(
                !name.starts_with(".source.json."),
                "atomic write left a temp file behind: {name}",
            );
        }
    }

    #[test]
    fn read_fails_loud_on_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let err = SourceJson::read(
            &tmp.path()
                .join("missing.json"),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("reading source.json")
                && (msg.contains("No such file")
                    || msg
                        .to_lowercase()
                        .contains("not found")),
            "unexpected error: {msg}",
        );
    }

    #[test]
    fn read_fails_loud_on_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.json");
        std::fs::write(&path, b"{ this isn't JSON").unwrap();
        let err = SourceJson::read(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("parsing/validating"), "expected parser error: {msg}");
    }

    #[test]
    fn read_fails_loud_on_wrong_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.json");
        // Hand-roll a v0 line (schema_version=0) → SchemaVersionV1
        // deserializer should reject.
        std::fs::write(
            &path,
            br#"{"schema_version":0,"url":"https://x","branch":"main","sha":"0000000000000000000000000000000000000000","fetched_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let err = SourceJson::read(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("schema_version"), "expected schema_version error: {msg}",);
    }

    #[test]
    fn read_fails_loud_on_unknown_field() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.json");
        std::fs::write(
            &path,
            br#"{"schema_version":1,"url":"https://x","branch":"main","sha":"0ad33704c259da4102b5f195617760003ac89c18","fetched_at":"2026-01-01T00:00:00Z","stray":"nope"}"#,
        )
        .unwrap();
        let err = SourceJson::read(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("stray") || msg.contains("unknown"), "{msg}");
    }

    #[test]
    fn validate_rejects_non_hex_sha() {
        let mut s = sample();
        s.sha = "not-a-real-sha".to_owned();
        let err = s
            .validate_model()
            .unwrap_err();
        assert!(format!("{err:#}").contains("sha must be a 40-char hex"), "{err:#}",);
    }

    #[test]
    fn validate_rejects_wrong_length_sha() {
        let mut s = sample();
        s.sha = "0ad33704".to_owned();
        assert!(s.validate_model().is_err());
    }

    #[test]
    fn validate_rejects_empty_url_branch_or_fetched_at() {
        for mutate in [
            |s: &mut SourceJson| s.url = "".to_owned(),
            |s: &mut SourceJson| s.branch = "".to_owned(),
            |s: &mut SourceJson| s.fetched_at = "".to_owned(),
        ] {
            let mut s = sample();
            mutate(&mut s);
            assert!(
                s.validate_model().is_err(),
                "expected validation failure after mutating to empty field",
            );
        }
    }
}
