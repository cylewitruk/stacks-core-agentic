//! Session-start preflight: fast checks for operator/orchestrator
//! drift modes that silently corrupt session output downstream.
//!
//! Three checks:
//!
//! - **Installed binary drift** (`Warn`) — running `sbagent` older than
//!   `<framework>/target/release/sbagent`. Operator forgot to install after
//!   rebuild.
//! - **Load-bearing prompt drift** (`Fail` on `optimizer.md`; `Warn` on other
//!   tunable prompts) — orchestrator's typed-report gate depends on
//!   `optimizer.md`'s contract; stale operator copy makes the agent write the
//!   wrong artifact and looks like "agent crashed" downstream.
//! - **Submodule reachability** (`Fail`) — `repos/<base>` HEAD must be an
//!   ancestor of local `origin/<publish_base_branch>`. No network fetch in v1.
//!
//! Wired into `sbagent session run`, `session optimize run` (incl.
//! `--resume`), and `sbagent check`. `--skip-preflight` opts out.

use std::fmt;
use std::path::Path;

use anyhow::{Context as _, Result};

use crate::cli::CliContext;

/// Severity tier for a preflight finding. `Fail` aborts before
/// session-start; `Warn` surfaces to stderr but lets the session
/// continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Fail,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Warn => f.write_str("WARN"),
            Severity::Fail => f.write_str("FAIL"),
        }
    }
}

/// One preflight finding. Carries enough for an operator to act
/// without grepping source: what's wrong, where, and the concrete
/// remediation command.
#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    pub check: &'static str,
    pub message: String,
    pub remediation: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] {}\n  remediation: {}",
            self.severity, self.check, self.message, self.remediation
        )
    }
}

/// Run all session-start preflight checks. Returns aggregated
/// findings in evaluation order (caller decides whether to bail on
/// any `Fail`).
pub fn collect_findings(ctx: &CliContext) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    check_installed_binary_drift(ctx, &mut findings)?;
    check_critical_prompt_drift(ctx, &mut findings)?;
    check_free_disk(ctx, &mut findings, &Fs4Disk)?;
    check_source_config(ctx, &mut findings);
    Ok(findings)
}

/// v3 Phase 3 cutover: every `session run` materializes a per-session
/// source checkout under
/// `<agent_workspace_root>/sessions/<id>/repos/<cache_id>/`. That requires
/// three config invariants:
///
/// 1. `[source].url` must be set.
/// 2. `[source].branch` must be set.
/// 3. `[source].id` (when set) must validate against the slug regex — settings
///    parsing already enforces this, but preflight re-validates as
///    defense-in-depth so a runtime-constructed settings value can't slip past.
/// 4. `layout.agent_workspace_root` must be set (the cache + session checkout
///    both live under it).
///
/// All four are `Fail` severity — without them, session start cannot
/// materialize the source checkout and every downstream phase that
/// previously consumed `<operator>/repos/<base>/` would crash.
/// Remediation pointers cite `docs/setup.md`'s migration recipe.
pub fn check_source_config(ctx: &CliContext, findings: &mut Vec<Finding>) {
    const CHECK: &str = "source-config";
    let source = &ctx.settings.source;

    if source.url.is_none() {
        findings.push(Finding {
            severity: Severity::Fail,
            check: CHECK,
            message: "`[source].url` is not set; required post-v3-cutover for the per-session \
                      source checkout"
                .to_owned(),
            remediation: "add a `[source]` stanza to config.toml with `url = \"...\"` and `branch \
                          = \"...\"` — see docs/setup.md for the migration recipe"
                .to_owned(),
        });
    }
    if source.branch.is_none() {
        findings.push(Finding {
            severity: Severity::Fail,
            check: CHECK,
            message: "`[source].branch` is not set; required post-v3-cutover".to_owned(),
            remediation: "set `branch = \"...\"` under `[source]` in config.toml — see \
                          docs/setup.md"
                .to_owned(),
        });
    }
    if let Some(id) = source.id.as_deref()
        && let Err(reason) = crate::settings::validate_source_id(id)
    {
        findings.push(Finding {
            severity: Severity::Fail,
            check: CHECK,
            message: format!("`[source].id` (`{id}`) failed validation: {reason}"),
            remediation: "set `id` to a lowercase ASCII slug matching \
                          `^[a-z](?:[a-z0-9-]{0,62}[a-z0-9])?$`, or omit `id` to let sbagent \
                          derive one from `source.url`"
                .to_owned(),
        });
    }
    if ctx
        .settings
        .layout
        .agent_workspace_root
        .is_none()
    {
        findings.push(Finding {
            severity: Severity::Fail,
            check: CHECK,
            message: "`layout.agent_workspace_root` is not set; the v3 per-session source \
                      checkout + bare cache both live under it"
                .to_owned(),
            remediation: "set `agent_workspace_root` under `[layout]` in config.toml to a path \
                          OUTSIDE the operator repo (e.g. `/private/tmp/sbagent-workspaces`)"
                .to_owned(),
        });
    }
}

/// Backing source for free-disk lookups. Real runs use [`Fs4Disk`];
/// tests inject a stub returning whatever byte count they want to
/// assert against, so the warn/fail boundary is testable without
/// allocating gibibytes of real disk.
pub trait DiskInfo {
    /// Bytes currently free on the filesystem holding `path`. Errors
    /// (`path` doesn't exist, EACCES on the parent) propagate so the
    /// preflight can surface them as a `Warn` rather than silently
    /// claim disk is fine.
    fn available_bytes(&self, path: &Path) -> Result<u64>;
}

/// Production [`DiskInfo`] backed by [`fs4::available_space`].
pub struct Fs4Disk;

impl DiskInfo for Fs4Disk {
    fn available_bytes(&self, path: &Path) -> Result<u64> {
        fs4::available_space(path)
            .with_context(|| format!("querying free disk for {}", path.display()))
    }
}

/// One gibibyte in bytes — used both to render the human-readable
/// findings and to compute the warn-threshold default.
const GIB: u64 = 1024 * 1024 * 1024;

/// Conservative warn threshold when `preflight.min_free_gib` is unset:
/// 10 GiB. Below this the preflight emits a `Warn` (operator can still
/// run); when the config is set, that value drives the `Fail`
/// boundary instead.
const DEFAULT_WARN_FLOOR_GIB: u64 = 10;

/// Check free disk on the filesystem holding
/// `layout.agent_workspace_root`. Behavior:
///
/// - `preflight.min_free_gib = Some(n)` and `available < n GiB` → `Fail`. Error
///   body suggests the exact `sbagent workspace prune` invocation.
/// - `preflight.min_free_gib = None` and `available < DEFAULT_WARN_FLOOR_GIB` →
///   `Warn`. The operator hasn't picked a floor; we still flag obvious
///   tight-disk situations.
/// - Otherwise no finding.
///
/// Skipped entirely when `layout.agent_workspace_root` isn't
/// resolvable (operator deployments that pin everything inline).
pub fn check_free_disk(
    ctx: &CliContext,
    findings: &mut Vec<Finding>,
    disk: &dyn DiskInfo,
) -> Result<()> {
    const CHECK: &str = "free-disk";

    let workspace_root = match ctx
        .settings
        .layout
        .agent_workspace_root
        .as_deref()
    {
        Some(p) => p,
        None => return Ok(()),
    };
    // The probe needs an existing path. Fall back to the nearest
    // ancestor that does exist — a fresh operator may not have
    // materialized the workspace dir yet on the very first run.
    let probe_path: &Path = {
        let mut p = workspace_root;
        loop {
            if p.exists() {
                break p;
            }
            match p.parent() {
                Some(parent) => p = parent,
                None => return Ok(()),
            }
        }
    };

    let available = match disk.available_bytes(probe_path) {
        Ok(b) => b,
        Err(e) => {
            findings.push(Finding {
                severity: Severity::Warn,
                check: CHECK,
                message: format!("free-disk probe failed: {e:#}"),
                remediation: format!(
                    "ensure {} (or its first existing ancestor) is readable; preflight cannot \
                     confirm available disk space",
                    workspace_root.display(),
                ),
            });
            return Ok(());
        }
    };

    let available_gib = available / GIB;
    let configured_min_gib = ctx
        .settings
        .preflight
        .min_free_gib;

    match configured_min_gib {
        Some(min_gib) if available_gib < min_gib => {
            findings.push(Finding {
                severity: Severity::Fail,
                check: CHECK,
                message: format!(
                    "free disk on {} is {} GiB; preflight.min_free_gib requires {} GiB",
                    workspace_root.display(),
                    available_gib,
                    min_gib,
                ),
                remediation: "free space (e.g. `sbagent workspace prune --older-than 7d \
                              --archived-only`) or lower `preflight.min_free_gib` in config.toml"
                    .to_owned(),
            });
        }
        None if available_gib < DEFAULT_WARN_FLOOR_GIB => {
            findings.push(Finding {
                severity: Severity::Warn,
                check: CHECK,
                message: format!(
                    "free disk on {} is {} GiB (no `preflight.min_free_gib` configured; warning \
                     below {} GiB)",
                    workspace_root.display(),
                    available_gib,
                    DEFAULT_WARN_FLOOR_GIB,
                ),
                remediation: "set `preflight.min_free_gib` in config.toml once you know a \
                              session's peak per-target disk, or run `sbagent workspace prune \
                              --older-than 7d --archived-only` to reclaim space"
                    .to_owned(),
            });
        }
        _ => {}
    }
    Ok(())
}

/// True iff any finding is a hard failure. Used by session-start to
/// decide whether to abort.
pub fn has_failures(findings: &[Finding]) -> bool {
    findings
        .iter()
        .any(|f| f.severity == Severity::Fail)
}

/// Emit findings to stderr, one per line. Returns
/// `Err(anyhow::anyhow!(...))` when any finding is `Fail`, so the
/// caller can `?` it at session-start.
pub fn report(findings: &[Finding]) -> Result<()> {
    if findings.is_empty() {
        return Ok(());
    }
    for f in findings {
        eprintln!("preflight {f}");
    }
    if has_failures(findings) {
        let n_fail = findings
            .iter()
            .filter(|f| f.severity == Severity::Fail)
            .count();
        anyhow::bail!(
            "session-start preflight: {n_fail} blocking finding(s) — fix the above and re-run, or \
             pass --skip-preflight to bypass (unsafe)"
        );
    }
    Ok(())
}

/// Compare the running binary's mtime to the framework's
/// `target/release/sbagent`, if both can be located. Older
/// running-binary warns; newer running-binary is fine (operator
/// installed a release).
///
/// Skipped entirely when `framework_root` isn't set (operator
/// deployments without a workspace nearby) or when
/// `std::env::current_exe()` can't be resolved (e.g. running under a
/// stripped binary).
fn check_installed_binary_drift(ctx: &CliContext, findings: &mut Vec<Finding>) -> Result<()> {
    const CHECK: &str = "installed-binary-drift";

    let framework = match ctx.layout.framework.as_ref() {
        Some(f) => f,
        None => return Ok(()),
    };
    let workspace_bin = framework
        .root()
        .join("target/release/sbagent");
    let workspace_mtime = match std::fs::metadata(&workspace_bin) {
        Ok(m) => m
            .modified()
            .with_context(|| format!("reading mtime of {}", workspace_bin.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("stat {}", workspace_bin.display())));
        }
    };

    let running_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    // If the running binary IS the workspace build, nothing to flag.
    if running_path == workspace_bin {
        return Ok(());
    }
    let running_mtime = match std::fs::metadata(&running_path) {
        Ok(m) => m
            .modified()
            .with_context(|| format!("reading mtime of {}", running_path.display()))?,
        Err(_) => return Ok(()),
    };

    if workspace_mtime > running_mtime {
        findings.push(Finding {
            severity: Severity::Warn,
            check: CHECK,
            message: format!(
                "running sbagent at {} is older than workspace build at {}",
                running_path.display(),
                workspace_bin.display(),
            ),
            remediation: format!(
                "cp {} {} (or `cargo install --path crates/stacks-bench-agent --force`)",
                workspace_bin.display(),
                running_path.display(),
            ),
        });
    }
    Ok(())
}

/// Drift check for operator-on-disk prompts vs the bundled defaults.
/// `optimizer.md` is `Fail` (its content is load-bearing for the
/// orchestrator's typed-report gate; stale content breaks Phase 2
/// invisibly). Other operator-tunable prompts are `Warn` — drift is
/// legitimate but worth surfacing once per session.
fn check_critical_prompt_drift(ctx: &CliContext, findings: &mut Vec<Finding>) -> Result<()> {
    const CHECK: &str = "prompt-drift";

    let dir = match ctx
        .settings
        .layout
        .prompt_overrides_dir
        .as_deref()
    {
        Some(d) => d,
        None => return Ok(()),
    };
    let drifts = match crate::prompts::drift(dir) {
        Ok(v) => v,
        Err(e) => {
            findings.push(Finding {
                severity: Severity::Warn,
                check: CHECK,
                message: format!("prompt drift probe failed: {e:#}"),
                remediation: format!(
                    "investigate {} — agent may be running against stale templates",
                    dir.display()
                ),
            });
            return Ok(());
        }
    };
    for d in &drifts {
        let file_name = d.file_name();
        let severity = if file_name == "optimizer.md" { Severity::Fail } else { Severity::Warn };
        let kind = match d {
            crate::prompts::DriftEntry::Missing { .. } => "missing on disk",
            crate::prompts::DriftEntry::Differs { .. } => "differs from bundled default",
        };
        findings.push(Finding {
            severity,
            check: CHECK,
            message: format!("prompt {file_name}: {kind} ({})", dir.display()),
            remediation: if severity == Severity::Fail {
                "sbagent sync (load-bearing prompt — orchestrator's typed-report gate depends on \
                 bundled contract)"
                    .to_owned()
            } else {
                "sbagent sync (or merge the bundled changes if your edits should be preserved)"
                    .to_owned()
            },
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_display_renders_word() {
        assert_eq!(Severity::Warn.to_string(), "WARN");
        assert_eq!(Severity::Fail.to_string(), "FAIL");
    }

    #[test]
    fn has_failures_returns_false_for_warns_only() {
        let findings = vec![Finding {
            severity: Severity::Warn,
            check: "x",
            message: "y".into(),
            remediation: "z".into(),
        }];
        assert!(!has_failures(&findings));
    }

    #[test]
    fn has_failures_returns_true_when_any_fail_present() {
        let findings = vec![
            Finding {
                severity: Severity::Warn,
                check: "x",
                message: "y".into(),
                remediation: "z".into(),
            },
            Finding {
                severity: Severity::Fail,
                check: "a",
                message: "b".into(),
                remediation: "c".into(),
            },
        ];
        assert!(has_failures(&findings));
    }

    #[test]
    fn finding_display_includes_severity_check_and_remediation() {
        let f = Finding {
            severity: Severity::Fail,
            check: "test-check",
            message: "stale binary".into(),
            remediation: "cp x y".into(),
        };
        let s = f.to_string();
        assert!(s.contains("FAIL"), "{s}");
        assert!(s.contains("[test-check]"), "{s}");
        assert!(s.contains("stale binary"), "{s}");
        assert!(s.contains("remediation: cp x y"), "{s}");
    }

    #[test]
    fn report_returns_ok_on_empty_findings() {
        report(&[]).expect("empty findings → Ok");
    }

    #[test]
    fn report_returns_err_when_any_fail() {
        let findings = vec![Finding {
            severity: Severity::Fail,
            check: "x",
            message: "y".into(),
            remediation: "z".into(),
        }];
        report(&findings).expect_err("Fail → Err");
    }

    #[test]
    fn report_returns_ok_when_only_warns() {
        let findings = vec![Finding {
            severity: Severity::Warn,
            check: "x",
            message: "y".into(),
            remediation: "z".into(),
        }];
        report(&findings).expect("warns alone don't fail");
    }

    // ─────────────────────────────────────────────────────────────
    // Phase 4 of v2-cleanup-and-workspace-hygiene: check_free_disk
    // ─────────────────────────────────────────────────────────────

    use crate::layout::Layout;
    use crate::settings::{LayoutSettings, PreflightSettings, Settings};

    /// Stub `DiskInfo` returning a configured byte count.
    struct StubDisk(u64);
    impl DiskInfo for StubDisk {
        fn available_bytes(&self, _path: &Path) -> Result<u64> {
            Ok(self.0)
        }
    }

    /// `DiskInfo` that always errors.
    struct ErrDisk;
    impl DiskInfo for ErrDisk {
        fn available_bytes(&self, _path: &Path) -> Result<u64> {
            anyhow::bail!("simulated EACCES")
        }
    }

    fn ctx_with_workspace(tmp: &std::path::Path, min_free_gib: Option<u64>) -> CliContext {
        let settings = Settings {
            layout: LayoutSettings {
                agent_workspace_root: Some(tmp.to_path_buf()),
                ..LayoutSettings::default()
            },
            preflight: PreflightSettings { min_free_gib },
            ..Settings::default()
        };
        // Bypass the normal Layout::from_settings — it requires more
        // wiring than this test needs. Construct a minimal layout
        // pointing at the same tmp dir.
        let layout = Layout::from_settings(&settings).expect("layout");
        CliContext { settings, layout }
    }

    #[test]
    fn check_free_disk_no_finding_when_above_warn_floor_and_no_config() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workspace(tmp.path(), None);
        let mut findings = Vec::new();
        check_free_disk(&ctx, &mut findings, &StubDisk((DEFAULT_WARN_FLOOR_GIB + 1) * GIB))
            .unwrap();
        assert!(findings.is_empty(), "well above floor → no finding: {findings:?}");
    }

    #[test]
    fn check_free_disk_warns_below_default_floor_when_unconfigured() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workspace(tmp.path(), None);
        let mut findings = Vec::new();
        check_free_disk(&ctx, &mut findings, &StubDisk(GIB)).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warn);
        assert!(
            findings[0]
                .message
                .contains("1 GiB"),
            "{}",
            findings[0].message
        );
        assert!(
            findings[0]
                .remediation
                .contains("workspace prune")
        );
    }

    #[test]
    fn check_free_disk_fails_below_configured_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workspace(tmp.path(), Some(50));
        let mut findings = Vec::new();
        check_free_disk(&ctx, &mut findings, &StubDisk(20 * GIB)).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Fail);
        assert!(
            findings[0]
                .message
                .contains("requires 50 GiB"),
            "{}",
            findings[0].message
        );
        assert!(
            findings[0]
                .remediation
                .contains("workspace prune"),
            "{}",
            findings[0].remediation
        );
    }

    #[test]
    fn check_free_disk_passes_above_configured_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workspace(tmp.path(), Some(10));
        let mut findings = Vec::new();
        check_free_disk(&ctx, &mut findings, &StubDisk(100 * GIB)).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn check_free_disk_warns_on_probe_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workspace(tmp.path(), Some(10));
        let mut findings = Vec::new();
        check_free_disk(&ctx, &mut findings, &ErrDisk).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warn);
        assert!(
            findings[0]
                .message
                .contains("probe failed"),
            "{}",
            findings[0].message
        );
    }

    #[test]
    fn check_free_disk_skipped_when_workspace_root_unset() {
        // No layout.agent_workspace_root — the check should no-op,
        // not panic.
        let settings = Settings::default();
        let layout = Layout::from_settings(&settings).expect("layout");
        let ctx = CliContext { settings, layout };
        let mut findings = Vec::new();
        check_free_disk(&ctx, &mut findings, &StubDisk(0)).unwrap();
        assert!(findings.is_empty());
    }

    // ─────────────────────────────────────────────────────────────────
    // v3 Phase 3: check_source_config — settings-side preflight gate.
    // ─────────────────────────────────────────────────────────────────

    use crate::settings::SourceSettings;

    fn ctx_with_source(workspace: Option<&Path>, source: SourceSettings) -> CliContext {
        let settings = Settings {
            layout: LayoutSettings {
                agent_workspace_root: workspace.map(|p| p.to_path_buf()),
                ..LayoutSettings::default()
            },
            source,
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        CliContext { settings, layout }
    }

    #[test]
    fn check_source_config_passes_when_url_branch_and_workspace_all_set() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_source(
            Some(tmp.path()),
            SourceSettings {
                url: Some("https://example.com/owner/repo.git".to_owned()),
                branch: Some("main".to_owned()),
                id: Some("test-cache".to_owned()),
            },
        );
        let mut findings = Vec::new();
        check_source_config(&ctx, &mut findings);
        assert!(findings.is_empty(), "valid config → no findings: {findings:?}");
    }

    #[test]
    fn check_source_config_fails_when_url_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_source(
            Some(tmp.path()),
            SourceSettings {
                url: None,
                branch: Some("main".to_owned()),
                id: None,
            },
        );
        let mut findings = Vec::new();
        check_source_config(&ctx, &mut findings);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Fail);
        assert!(
            findings[0]
                .message
                .contains("`[source].url`")
        );
        assert!(
            findings[0]
                .remediation
                .contains("docs/setup.md"),
            "remediation should point at the migration recipe",
        );
    }

    #[test]
    fn check_source_config_fails_when_branch_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_source(
            Some(tmp.path()),
            SourceSettings {
                url: Some("https://example.com/x.git".to_owned()),
                branch: None,
                id: None,
            },
        );
        let mut findings = Vec::new();
        check_source_config(&ctx, &mut findings);
        assert!(
            findings.iter().any(|f| f
                .message
                .contains("`[source].branch`")),
            "{findings:?}",
        );
    }

    #[test]
    fn check_source_config_fails_on_invalid_source_id_defense_in_depth() {
        // Settings parsing already rejects bad slugs, but preflight
        // re-validates so a runtime-constructed Settings value can't
        // slip past.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_source(
            Some(tmp.path()),
            SourceSettings {
                url: Some("https://example.com/x.git".to_owned()),
                branch: Some("main".to_owned()),
                id: Some("trailing-".to_owned()), // bad: trailing hyphen
            },
        );
        let mut findings = Vec::new();
        check_source_config(&ctx, &mut findings);
        assert!(
            findings.iter().any(|f| f
                .message
                .contains("`[source].id`")
                && f.message
                    .contains("trailing-")),
            "{findings:?}",
        );
    }

    #[test]
    fn check_source_config_fails_when_agent_workspace_root_unset() {
        let ctx = ctx_with_source(
            None,
            SourceSettings {
                url: Some("https://example.com/x.git".to_owned()),
                branch: Some("main".to_owned()),
                id: None,
            },
        );
        let mut findings = Vec::new();
        check_source_config(&ctx, &mut findings);
        assert!(
            findings.iter().any(|f| f
                .message
                .contains("agent_workspace_root")),
            "{findings:?}",
        );
    }

    #[test]
    fn check_source_config_reports_multiple_failures_in_one_pass() {
        // No url, no branch, no workspace → 3 findings.
        let ctx = ctx_with_source(None, SourceSettings::default());
        let mut findings = Vec::new();
        check_source_config(&ctx, &mut findings);
        assert_eq!(findings.len(), 3, "{findings:?}");
        assert!(
            findings
                .iter()
                .all(|f| f.severity == Severity::Fail)
        );
    }
}
