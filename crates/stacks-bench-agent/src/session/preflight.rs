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
    check_submodule_reachable(ctx, &mut findings)?;
    Ok(findings)
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

/// Verify the operator's `repos/<base>` submodule HEAD is reachable
/// from the local `refs/remotes/origin/<publish_base_branch>` ref.
/// No network fetch — catches "operator checked out a non-publish
/// branch by accident" and "submodule moved past the local
/// origin-tracking ref"; does NOT catch "operator hasn't fetched
/// recently." A future strict-network variant could add the fetch.
fn check_submodule_reachable(ctx: &CliContext, findings: &mut Vec<Finding>) -> Result<()> {
    const CHECK: &str = "submodule-reachability";

    // Read the resolved `base` off `ctx.layout`, not `ctx.settings` —
    // Layout already ran the canonical `absolutize(...)` step that the
    // session phases use, so this preflight validates exactly the
    // checkout the optimizer / Phase 0a / Phase 1.8 will read from.
    // Re-resolving from `ctx.settings.base` here would risk validating
    // a different path than the phases (e.g. `<operator>/repos/...`
    // vs `<cwd>/repos/...`).
    let base_abs = match ctx.layout.base.as_deref() {
        Some(b) => b,
        None => return Ok(()),
    };
    let branch = match ctx
        .settings
        .publish
        .base_branch
        .as_deref()
    {
        Some(b) => b,
        None => return Ok(()),
    };
    if !base_abs.exists() {
        return Ok(()); // operator hasn't bootstrapped yet — out of scope here
    }
    let head_sha = match crate::git::rev_parse_head(base_abs) {
        Ok(s) => s,
        Err(e) => {
            findings.push(Finding {
                severity: Severity::Warn,
                check: CHECK,
                message: format!(
                    "could not resolve submodule HEAD at {}: {e:#}",
                    base_abs.display()
                ),
                remediation: "ensure repos/<base> is a valid git checkout".to_owned(),
            });
            return Ok(());
        }
    };
    let origin_ref = format!("refs/remotes/origin/{branch}");
    if !is_reachable_from(base_abs, &head_sha, &origin_ref) {
        findings.push(Finding {
            severity: Severity::Fail,
            check: CHECK,
            message: format!(
                "submodule HEAD {head_sha} at {} is not reachable from local {origin_ref} — the \
                 per-target clones the optimizer creates will branch off the wrong source",
                base_abs.display(),
            ),
            remediation: format!(
                "in {}: `git fetch origin {branch} && git checkout origin/{branch}` (or `git \
                 reset --hard origin/{branch}` if you have no local work to preserve)",
                base_abs.display(),
            ),
        });
    }
    Ok(())
}

/// `git merge-base --is-ancestor <sha> <ref>` exits 0 if `sha` is on
/// `ref`'s history, non-zero otherwise. We use it for the
/// reachability check above.
fn is_reachable_from(dir: &Path, sha: &str, target_ref: &str) -> bool {
    crate::git::run_git_check(dir, &["merge-base", "--is-ancestor", sha, target_ref])
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
}
