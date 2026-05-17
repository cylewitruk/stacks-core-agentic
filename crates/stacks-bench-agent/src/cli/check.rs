//! `sbagent check` — preflight verification.
//!
//! Concerns, in order of strictness:
//! 1. **Required tools** are installed and on `PATH` (`codex`, `cargo`, `git`,
//!    `sqlite3`).
//! 2. **Codex CLI** is invocable (`--help` succeeds and advertises the flags
//!    the harness depends on).
//! 3. **Bundle drift (schemas)**: every `<schemas_dir>/*.schema.json` on disk
//!    byte-matches the bundled schema baked into the running binary. Fails the
//!    check — stale on-disk schemas mean the operator validates agent output
//!    against a different contract than the binary expects. Fix with `sbagent
//!    sync`.
//! 4. **Bundle drift (queries)**: same fail-on-drift contract as schemas,
//!    against `<queries_dir>/*.sql` + `README.md`. Stale queries silently
//!    change agent input (column ordering, parameter names), so this gate is
//!    just as strict as the schemas one. Fix with `sbagent sync`.
//! 5. **Bundle drift (prompts)**: each operator-on-disk template / reference
//!    doc is compared against the embedded bundle. Drift is **warned only**
//!    (operator edits are legitimate — autoresearch's `program.md` model).
//! 6. **Tool-dev drift** (only when `framework_root` is set): every committed
//!    `<framework>/schemas/*.schema.json` matches what `sbagent schema export`
//!    would emit. Catches "edited a Rust model, forgot to regenerate." This
//!    runs in tool-developer checkouts; operator deployments skip it
//!    automatically.
//! 7. **Publish wiring** (only with `--with-publish`): the configured
//!    `publish_token_file` is non-empty and readable, and the configured
//!    `publish_base_repo` is reachable via the GitHub API with that token.
//!
//! Exits 0 on success; non-zero with a per-finding error report on failure.

use std::process::Command;

use anyhow::{Context as _, Result, bail};
use clap::Args;

use crate::cli::{CliContext, preflight};
use crate::harnesses::codex::CodexHarness;
use crate::schema_export::{self, DriftEntry};
use crate::{queries, schemas};

/// Args for `sbagent check`.
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Skip ALL schema drift gates (bundle-vs-disk + typed-model-vs-committed).
    /// Useful when generating new models before committing the regenerated
    /// `schemas/`.
    #[clap(long)]
    pub skip_schema_drift: bool,
    /// Also verify Phase 5 publish wiring: `publish_token_file` is
    /// readable and the configured `publish_base_repo` is reachable via
    /// the GitHub API with that token.
    #[clap(long)]
    pub with_publish: bool,
}

/// External CLI tools sbagent (or the agents it launches) always shells
/// out to at runtime.
///
/// - `cargo` — `cargo stacks-bench` fallback when the prebuilt release binary
///   is missing, and `cargo nextest run` inside optimizer worktrees.
/// - `git` — optimizer worktree creation, Phase 5 publish (switch / add /
///   commit / push / remote get-url / ls-remote).
/// - `sqlite3` — NOT used by sbagent itself, but the triage and analyzer
///   prompts instruct Codex to drive the bench DB via `sqlite3 -header -csv ...
///   .read <queries>/*.sql`. Probing here means a missing sqlite3 fails
///   preflight rather than silently producing empty `candidates.json` after
///   Phase 1 runs.
/// - Codex CLI — the agent harness.
///
/// `envsubst` and `flock` are intentionally NOT here: sbagent renders
/// prompts internally via [`crate::prompts`] (Askama) and locks bench
/// access via the `fd-lock` crate. `gh` is gone (replaced with
/// `octocrab`); Phase 5's git-side ops still go through `git`.
const REQUIRED_TOOLS: &[&str] = &["cargo", "git", "sqlite3", CodexHarness::COMMAND];

/// Run all preflight checks. Each check appends to `findings`; on any
/// finding the command bails with the aggregated list.
pub async fn run(args: CheckArgs, ctx: &CliContext) -> Result<()> {
    let mut findings: Vec<String> = Vec::new();

    check_required_tools(&mut findings);
    check_codex_compat(&mut findings);
    if !args.skip_schema_drift {
        check_bundle_schema_drift(ctx, &mut findings);
        check_bundle_query_drift(ctx, &mut findings);
        check_bundle_prompt_drift(ctx);
        check_bundle_context_drift(ctx);
        check_context_bundle_consistency(&mut findings);
        if ctx.layout.framework.is_some() {
            check_typed_model_schema_drift(ctx, &mut findings);
        }
    }
    if args.with_publish {
        preflight::collect_publish_findings(ctx, &mut findings).await;
    }

    if findings.is_empty() {
        println!("OK");
        Ok(())
    } else {
        for f in &findings {
            eprintln!("FAIL: {f}");
        }
        bail!("{} check(s) failed", findings.len())
    }
}

/// Verify every required tool is on `PATH`.
fn check_required_tools(findings: &mut Vec<String>) {
    for name in REQUIRED_TOOLS {
        if which::which(name).is_err() {
            findings.push(format!("missing required command: {name}"));
        }
    }
}

/// Verify the Codex CLI is invocable and advertises the full surface the
/// harness uses. Probes both `codex --help` (top-level) and `codex exec
/// --help` (subcommand) so a CLI that renamed/dropped any of these flags
/// fails preflight rather than the first triage call.
///
/// Top-level flags probed: `--ask-for-approval`, `--model`/`-m`, the `exec`
/// subcommand. Exec flags probed: `--cd`, `--add-dir`, `--sandbox`,
/// `--json`, `--output-last-message`.
///
/// `-c <key>=<value>` config overrides and the prompt positional aren't
/// probed because they don't appear as discrete `--help` strings.
fn check_codex_compat(findings: &mut Vec<String>) {
    let top = match capture_help(&[]) {
        Ok(h) => h,
        Err(e) => {
            findings.push(format!("codex --help failed: {e}"));
            return;
        }
    };
    if !top.contains("--ask-for-approval") {
        findings.push(
            "codex CLI does not advertise --ask-for-approval; the harness sets it on every \
             invocation"
                .to_owned(),
        );
    }
    if !top.contains("--model") && !top.contains(" -m ") && !top.contains(", -m") {
        findings.push(
            "codex CLI does not advertise --model/-m; the harness sets it on every invocation"
                .to_owned(),
        );
    }
    // The harness uses `codex exec`. If the subcommand vanished, every
    // phase will fail at spawn time.
    if !top.contains("exec") {
        findings.push(
            "codex --help does not list the `exec` subcommand; the harness depends on it"
                .to_owned(),
        );
        return;
    }

    let exec = match capture_help(&["exec"]) {
        Ok(h) => h,
        Err(e) => {
            findings.push(format!("codex exec --help failed: {e}"));
            return;
        }
    };
    for flag in ["--cd", "--add-dir", "--sandbox", "--json", "--output-last-message"] {
        if !exec.contains(flag) {
            findings.push(format!(
                "codex exec --help does not advertise {flag}; the harness passes it on every \
                 invocation"
            ));
        }
    }
}

/// Capture stdout of `codex [args] --help`, bailing on non-zero exit.
fn capture_help(args: &[&str]) -> Result<String> {
    let output = Command::new(CodexHarness::COMMAND)
        .args(args)
        .arg("--help")
        .output()
        .with_context(|| format!("spawning codex {} --help", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "codex {} --help exited {} (stderr: {})",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Verify the typed models still emit byte-equal JSON Schemas to whatever's
/// committed under `<framework>/schemas/`. Tool-developer-only check — skipped
/// in operator deployments where `framework_root` isn't set.
fn check_typed_model_schema_drift(ctx: &CliContext, findings: &mut Vec<String>) {
    let framework = match ctx.layout.framework.as_ref() {
        Some(f) => f,
        None => return, // caller already gated on this, but defensive
    };
    let dir = framework.schemas_dir();
    let drifts = match schema_export::drift(&dir)
        .with_context(|| format!("comparing schemas against {}", dir.display()))
    {
        Ok(v) => v,
        Err(e) => {
            findings.push(format!("schema drift check failed: {e:#}"));
            return;
        }
    };
    for d in drifts {
        match d {
            DriftEntry::Missing { file_name } => {
                findings.push(format!(
                    "tool-dev schema drift: committed {} is missing — run `sbagent schema export`",
                    file_name
                ));
            }
            DriftEntry::Differs { file_name } => {
                findings.push(format!(
                    "tool-dev schema drift: committed {} differs from typed model — run `sbagent \
                     schema export`",
                    file_name
                ));
            }
        }
    }
}

/// Verify the operator's on-disk schemas (under [`Layout::schemas_dir`])
/// byte-match the bundle embedded in the running binary. Drift here is
/// always a failure — stale schemas mean the operator validates agent
/// output against a different contract than what the binary knows
/// about. Fix with `sbagent sync`.
fn check_bundle_schema_drift(ctx: &CliContext, findings: &mut Vec<String>) {
    let dir = &ctx.layout.schemas_dir;
    let drifts = match schemas::drift(dir).with_context(|| {
        format!("comparing on-disk schemas at {} against binary bundle", dir.display())
    }) {
        Ok(v) => v,
        Err(e) => {
            findings.push(format!("bundle schema drift check failed: {e:#}"));
            return;
        }
    };
    for d in drifts {
        match d {
            schemas::DriftEntry::Missing { file_name } => {
                findings.push(format!(
                    "bundle schema drift: {} missing under {} — run `sbagent sync`",
                    file_name,
                    dir.display(),
                ));
            }
            schemas::DriftEntry::Differs { file_name } => {
                findings.push(format!(
                    "bundle schema drift: {} under {} differs from binary's embedded bundle — run \
                     `sbagent sync`",
                    file_name,
                    dir.display(),
                ));
            }
        }
    }
}

/// Verify the operator's on-disk SQL queries (under
/// [`Layout::queries_dir`]) byte-match the bundle. Same fail-on-drift
/// contract as [`check_bundle_schema_drift`] — stale queries can
/// silently change agent input (different columns / ordering),
/// breaking the typed candidates/analysis pipeline downstream.
fn check_bundle_query_drift(ctx: &CliContext, findings: &mut Vec<String>) {
    let dir = &ctx.layout.queries_dir;
    let drifts = match queries::drift(dir).with_context(|| {
        format!("comparing on-disk queries at {} against binary bundle", dir.display())
    }) {
        Ok(v) => v,
        Err(e) => {
            findings.push(format!("bundle query drift check failed: {e:#}"));
            return;
        }
    };
    for d in drifts {
        match d {
            queries::DriftEntry::Missing { file_name } => {
                findings.push(format!(
                    "bundle query drift: {} missing under {} — run `sbagent sync`",
                    file_name,
                    dir.display(),
                ));
            }
            queries::DriftEntry::Differs { file_name } => {
                findings.push(format!(
                    "bundle query drift: {} under {} differs from binary's embedded bundle — run \
                     `sbagent sync`",
                    file_name,
                    dir.display(),
                ));
            }
        }
    }
}

/// Compare operator's on-disk prompt templates against the embedded
/// bundle. **Warns only** (operator edits are legitimate — the
/// autoresearch `program.md` model). The warning surfaces on stderr
/// so an operator who picked up a new sbagent version sees that their
/// prompt tunes may now lag the new bundled defaults; they decide
/// whether to merge or `sbagent sync --force-tunables`.
fn check_bundle_prompt_drift(ctx: &CliContext) {
    let dir = match ctx
        .settings
        .prompt_overrides_dir
        .as_deref()
    {
        Some(d) => d,
        None => return,
    };
    let drifts = match crate::prompts::drift(dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("note: prompt bundle drift check failed: {e:#}");
            return;
        }
    };
    if drifts.is_empty() {
        return;
    }
    eprintln!(
        "note: {} prompt template(s) under {} differ from the binary's bundled defaults (operator \
         edits are legitimate; run `sbagent sync --force-tunables` to reset):",
        drifts.len(),
        dir.display(),
    );
    for d in &drifts {
        eprintln!(
            "  - {}: {}",
            d.file_name(),
            match d {
                crate::prompts::DriftEntry::Missing { .. } => "missing on disk",
                crate::prompts::DriftEntry::Differs { .. } => "differs from bundle",
            }
        );
    }
}

/// Compare operator's on-disk context docs against the embedded bundle.
/// Same warn-only contract as [`check_bundle_prompt_drift`]: context
/// docs are operator-tunable (the bot's "brainstem"), so drift is
/// informational — operator who upgrades sbagent sees that their tunes
/// may lag the new defaults and decides whether to merge or
/// `sbagent sync --force-tunables`.
fn check_bundle_context_drift(ctx: &CliContext) {
    let dir = &ctx.layout.context_dir;
    let drifts = match crate::context::drift(dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("note: context bundle drift check failed: {e:#}");
            return;
        }
    };
    if drifts.is_empty() {
        return;
    }
    eprintln!(
        "note: {} context file(s) under {} differ from the binary's bundled defaults (operator \
         edits are legitimate; run `sbagent sync --force-tunables` to reset):",
        drifts.len(),
        dir.display(),
    );
    for d in &drifts {
        eprintln!(
            "  - {}: {}",
            d.file_name(),
            match d {
                crate::context::DriftEntry::Missing { .. } => "missing on disk",
                crate::context::DriftEntry::Differs { .. } => "differs from bundle",
            }
        );
    }
}

/// Validate the bundled context manifest is internally consistent
/// (unique ids, non-empty titles/descriptions/phases, ids match
/// filenames). Surfaced as a hard failure because a malformed bundle
/// is a sbagent bug, not an operator-tunable condition.
fn check_context_bundle_consistency(findings: &mut Vec<String>) {
    if let Err(e) = crate::context::lint_bundle() {
        findings.push(format!("context bundle manifest invalid: {e:#}"));
    }
}
