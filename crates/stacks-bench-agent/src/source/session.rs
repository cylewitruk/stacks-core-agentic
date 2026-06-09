//! High-level orchestration: materialize the per-session source
//! checkout at session start, write `source.json`, and return the
//! resolved provenance + checkout path for downstream phases.
//!
//! Used by `cli::session::run` between preflight + Phase 0a. Every
//! downstream phase that previously consumed
//! `<operator>/repos/<base>/` (Phase 0a baseline build, Phase 0
//! baseline bench cwd, Phase 1.8 calibration cwd, Phase 2 optimizer
//! fan-out, Phase 3 candidate-bench cargo cwd) now reads
//! [`ResolvedSource::session_checkout`].

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result, bail};

use crate::analyzed_rejections::now_utc_iso8601;
use crate::models::common::SchemaVersionV1;
use crate::models::source::SourceJson;
use crate::settings::SourceSettings;
use crate::source::cache_id::resolve_cache_id;
use crate::source::repo::{
    MaterializeInputs, SourceRepo, cache_dir_for, materialize_unlocked, session_repo_dir_for,
    with_materialization_lock,
};

/// Everything a phase needs from the source materialization: the
/// per-session checkout path it should read source from, plus the
/// SourceJson provenance written to disk.
#[derive(Debug, Clone)]
pub struct ResolvedSource {
    /// `<workspace>/sessions/<id>/repos/<cache_id>/` — what every
    /// downstream phase reads.
    pub session_checkout: PathBuf,
    /// `<workspace>/cache/<cache_id>.git/` — kept in the struct for
    /// observability + ops debugging.
    pub cache_dir: PathBuf,
    /// The provenance record that was either freshly written or read
    /// from `<session>/results/source.json` on a resume.
    pub source: SourceJson,
}

/// Read an already-written `source.json` and derive the per-session
/// source checkout path from its recorded `cache_id`. Used by every
/// standalone CLI subcommand that consumes the source state set up by
/// a prior `session run` (triage, analysis, analyze-results,
/// optimize). The session checkout itself is NOT required to exist —
/// that's the caller's concern.
///
/// **Critical**: derive from `source.cache_id`, NOT from current
/// settings. If the operator changes `[source].id` between
/// `session run` and a standalone subcommand, current-settings
/// derivation would point at the wrong path. Same fix as the resume
/// path in [`materialize_session_source`].
pub fn read_session_source(
    workspace_root: &Path,
    session_id: &str,
    source_json_path: &Path,
) -> Result<ResolvedSource> {
    let source = SourceJson::read(source_json_path).with_context(|| {
        format!(
            "reading source.json at {} — has `sbagent session run` materialized this session?",
            source_json_path.display(),
        )
    })?;
    let cache_dir = cache_dir_for(workspace_root, &source.cache_id);
    let session_checkout = session_repo_dir_for(workspace_root, session_id, &source.cache_id);
    Ok(ResolvedSource {
        session_checkout,
        cache_dir,
        source,
    })
}

pub fn materialize_session_source<R: SourceRepo>(
    repo: &R,
    workspace_root: &Path,
    session_id: &str,
    settings: &SourceSettings,
    source_json_path: &Path,
) -> Result<ResolvedSource> {
    // Resume path: source.json is the truth. Read it (validates cache_id
    // as a slug), then derive cache + checkout paths from the RECORDED
    // cache_id rather than recomputing from current settings. This is
    // Codex's High-#1 fix: an operator who changes/removes
    // `[source].id` between `session run` invocations must still see
    // the same checkout the session originally materialized against.
    //
    // Done BEFORE lock acquisition so a resume is cheap (no fetch, no
    // exclusive lock contention against concurrent fresh starts of
    // unrelated sessions).
    if source_json_path.exists() {
        let source = SourceJson::read(source_json_path)?;
        let cache_dir = cache_dir_for(workspace_root, &source.cache_id);
        let session_checkout = session_repo_dir_for(workspace_root, session_id, &source.cache_id);
        if !session_checkout.exists() {
            bail!(
                "source.json exists at {} but the per-session checkout at {} is missing — this \
                 iteration does not support rebuilding the checkout on resume. To recover, wipe \
                 source.json (and re-run to materialize fresh) OR restore the checkout manually \
                 from the bare cache at {}",
                source_json_path.display(),
                session_checkout.display(),
                cache_dir.display(),
            );
        }
        return Ok(ResolvedSource {
            session_checkout,
            cache_dir,
            source,
        });
    }

    // Fresh path. Resolve config-driven cache_id ONCE, then hold the
    // materialization lock across fetch → resolve → clone →
    // source.json write. Codex's Medium #3 fix: the source.json write
    // must be inside the lock so a concurrent process can't prune +
    // re-clone the session_checkout between our materialize() return
    // and our source.json write.
    let (source_url, branch) = settings
        .require_url_and_branch()
        .context("resolving [source] config for session-start materialization")?;
    let cache_id =
        resolve_cache_id(settings.id.as_deref(), source_url).map_err(|e| anyhow::anyhow!(e))?;

    // Hold the materialization lock across the **entire** fresh
    // window: re-check (race after blocking), materialize, write
    // source.json. The scoped helper is the only way to construct a
    // `MaterializationLockWitness`, so `materialize_unlocked` can't
    // be called outside this critical section.
    with_materialization_lock(workspace_root, &cache_id, |witness| {
        // Re-check after acquiring the lock: another process may have
        // raced and written source.json while we were blocked. In that
        // case treat this as a resume.
        if source_json_path.exists() {
            let source = SourceJson::read(source_json_path)?;
            let cache_dir = cache_dir_for(workspace_root, &source.cache_id);
            let session_checkout =
                session_repo_dir_for(workspace_root, session_id, &source.cache_id);
            if !session_checkout.exists() {
                bail!(
                    "source.json appeared at {} during materialization (race with another \
                     `session run`?) but per-session checkout at {} is missing",
                    source_json_path.display(),
                    session_checkout.display(),
                );
            }
            return Ok(ResolvedSource {
                session_checkout,
                cache_dir,
                source,
            });
        }

        let mat = materialize_unlocked(
            repo,
            &MaterializeInputs {
                workspace_root,
                session_id,
                cache_id: &cache_id,
                source_url,
                branch,
                now: SystemTime::now(),
            },
            witness,
        )
        .context("materializing per-session source checkout")?;

        let source = SourceJson {
            schema_version: SchemaVersionV1,
            url: mat.source_url.clone(),
            branch: mat.branch.clone(),
            sha: mat.sha.clone(),
            fetched_at: now_utc_iso8601(),
            cache_id: cache_id.clone(),
        };
        source
            .write(source_json_path)
            .with_context(|| format!("writing source.json at {}", source_json_path.display()))?;

        Ok(ResolvedSource {
            session_checkout: mat.session_checkout,
            cache_dir: mat.cache_dir,
            source,
        })
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use anyhow::Result;

    use super::*;
    use crate::source::repo::{SourceRepo, cache_dir_for};

    #[derive(Default, Clone)]
    struct StubRepo {
        sha: Arc<Mutex<String>>,
    }

    impl StubRepo {
        fn with_sha(sha: &str) -> Self {
            Self {
                sha: Arc::new(Mutex::new(sha.to_owned())),
            }
        }
    }

    impl SourceRepo for StubRepo {
        fn ensure_cache(&self, cache_dir: &Path, _source_url: &str, _branch: &str) -> Result<()> {
            std::fs::create_dir_all(cache_dir)?;
            std::fs::write(cache_dir.join("HEAD"), b"ref: refs/heads/main\n")?;
            Ok(())
        }
        fn clone_session_checkout(
            &self,
            _cache_dir: &Path,
            session_checkout: &Path,
            _branch: &str,
            _source_url: &str,
        ) -> Result<()> {
            std::fs::create_dir_all(session_checkout)?;
            std::fs::write(session_checkout.join(".stub"), b"x")?;
            Ok(())
        }
        fn resolve_head_sha(&self, _repo: &Path) -> Result<String> {
            Ok(self
                .sha
                .lock()
                .unwrap()
                .clone())
        }
        fn prune_session_checkout(&self, session_checkout: &Path) -> Result<bool> {
            match std::fs::remove_dir_all(session_checkout) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(anyhow::Error::new(e)),
            }
        }
    }

    fn valid_sha() -> String {
        "0ad33704c259da4102b5f195617760003ac89c18".to_owned()
    }

    fn full_settings() -> SourceSettings {
        SourceSettings {
            url: Some("https://example.com/owner/repo.git".to_owned()),
            branch: Some("main".to_owned()),
            id: Some("test-cache".to_owned()),
        }
    }

    #[test]
    fn fresh_materialization_writes_source_json_and_returns_resolved_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let source_json = workspace.join("source.json");

        let resolved = materialize_session_source(
            &StubRepo::with_sha(&valid_sha()),
            workspace,
            "20260607-104400",
            &full_settings(),
            &source_json,
        )
        .expect("fresh materialize");

        assert!(source_json.exists(), "source.json must be written on fresh path");
        assert!(
            resolved
                .session_checkout
                .exists()
        );
        assert_eq!(resolved.cache_dir, cache_dir_for(workspace, "test-cache"));
        assert_eq!(resolved.source.sha, valid_sha());
        assert_eq!(resolved.source.branch, "main");
        assert_eq!(resolved.source.url, "https://example.com/owner/repo.git");

        // Round-trip: re-read source.json and confirm it matches.
        let on_disk = SourceJson::read(&source_json).unwrap();
        assert_eq!(on_disk, resolved.source);
    }

    #[test]
    fn resume_with_existing_source_json_returns_existing_provenance() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let source_json = workspace.join("source.json");

        // First call materializes + writes source.json.
        let first = materialize_session_source(
            &StubRepo::with_sha(&valid_sha()),
            workspace,
            "20260607-104400",
            &full_settings(),
            &source_json,
        )
        .expect("first materialize");

        // Second call: source.json already exists; stub would report
        // a different SHA if we asked it, but resume should NOT call
        // the stub — it should just read source.json.
        let stub_with_drift = StubRepo::with_sha("ffffffffffffffffffffffffffffffffffffffff");
        let second = materialize_session_source(
            &stub_with_drift,
            workspace,
            "20260607-104400",
            &full_settings(),
            &source_json,
        )
        .expect("resume materialize");

        assert_eq!(
            first.source, second.source,
            "resume must return the originally-written provenance, not whatever the stub would \
             produce now",
        );
        // Specifically: the recorded sha is the first one, not the drift.
        assert_eq!(second.source.sha, valid_sha());
    }

    #[test]
    fn resume_fails_loud_when_checkout_was_deleted_after_source_json_was_written() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let source_json = workspace.join("source.json");

        let first = materialize_session_source(
            &StubRepo::with_sha(&valid_sha()),
            workspace,
            "20260607-104400",
            &full_settings(),
            &source_json,
        )
        .unwrap();

        // Wipe the checkout, leaving source.json behind.
        std::fs::remove_dir_all(&first.session_checkout).unwrap();
        assert!(source_json.exists());
        assert!(
            !first
                .session_checkout
                .exists()
        );

        let err = materialize_session_source(
            &StubRepo::with_sha(&valid_sha()),
            workspace,
            "20260607-104400",
            &full_settings(),
            &source_json,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("per-session checkout"), "expected checkout-missing error: {msg}");
        assert!(msg.contains("wipe source.json"), "expected recovery hint: {msg}");
    }

    #[test]
    fn missing_url_or_branch_returns_a_useful_error_with_remediation_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let source_json = workspace.join("source.json");

        let empty = SourceSettings::default();
        let err = materialize_session_source(
            &StubRepo::with_sha(&valid_sha()),
            workspace,
            "20260607-104400",
            &empty,
            &source_json,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("[source].url"), "error should name the missing config field: {msg}",);
        assert!(
            msg.contains("migration recipe") || msg.contains("docs/setup.md"),
            "error should point at the migration recipe: {msg}",
        );
        assert!(!source_json.exists(), "no source.json should be written when config is missing");
    }

    /// v3 Phase 3 Codex High-#1: source.json is the truth. If the
    /// operator changes `[source].id` between `session run` and a
    /// later resume (or standalone phase), the resume must still see
    /// the ORIGINAL cache_id + checkout path, not the new one derived
    /// from current settings.
    #[test]
    fn resume_uses_cache_id_from_source_json_not_current_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let source_json = workspace.join("source.json");

        // Materialize with pinned id `original-id`.
        let original = SourceSettings {
            url: Some("https://example.com/owner/repo.git".to_owned()),
            branch: Some("main".to_owned()),
            id: Some("original-id".to_owned()),
        };
        let first = materialize_session_source(
            &StubRepo::with_sha(&valid_sha()),
            workspace,
            "20260607-104400",
            &original,
            &source_json,
        )
        .expect("first materialize");
        assert_eq!(first.source.cache_id, "original-id");

        // Resume with a DIFFERENT id — the recorded `original-id` must
        // win. If we used current settings we'd look at the wrong path
        // and bail with "checkout missing".
        let drifted = SourceSettings {
            url: Some("https://example.com/owner/repo.git".to_owned()),
            branch: Some("main".to_owned()),
            id: Some("operator-changed-it".to_owned()),
        };
        let resumed = materialize_session_source(
            &StubRepo::with_sha("ffffffffffffffffffffffffffffffffffffffff"),
            workspace,
            "20260607-104400",
            &drifted,
            &source_json,
        )
        .expect("resume with drifted settings must succeed against original cache_id");
        assert_eq!(
            resumed.source.cache_id, "original-id",
            "resume must use the recorded cache_id, not the live setting",
        );
        assert_eq!(first.session_checkout, resumed.session_checkout);
        assert_eq!(first.cache_dir, resumed.cache_dir);
    }

    /// Also covers: resume when the operator REMOVES `[source].id`
    /// (forcing live derivation) — the recorded cache_id still wins.
    #[test]
    fn resume_uses_cache_id_from_source_json_when_settings_id_is_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let source_json = workspace.join("source.json");

        let with_id = SourceSettings {
            url: Some("https://example.com/owner/repo.git".to_owned()),
            branch: Some("main".to_owned()),
            id: Some("pinned-id".to_owned()),
        };
        let first = materialize_session_source(
            &StubRepo::with_sha(&valid_sha()),
            workspace,
            "20260607-104400",
            &with_id,
            &source_json,
        )
        .unwrap();

        // Operator removed `[source].id`. Live derivation would
        // produce something like `example-com-owner-repo-<hash>`,
        // which would point at a DIFFERENT (non-existent) checkout.
        let without_id = SourceSettings {
            url: Some("https://example.com/owner/repo.git".to_owned()),
            branch: Some("main".to_owned()),
            id: None,
        };
        let resumed = materialize_session_source(
            &StubRepo::with_sha(&valid_sha()),
            workspace,
            "20260607-104400",
            &without_id,
            &source_json,
        )
        .expect("resume must succeed");
        assert_eq!(resumed.source.cache_id, "pinned-id");
        assert_eq!(first.session_checkout, resumed.session_checkout);
    }
}
