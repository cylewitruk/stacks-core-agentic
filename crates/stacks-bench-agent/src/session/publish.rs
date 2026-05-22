//! Phase 5: publish artifacts.
//!
//! Two halves, both running in-process under the agent user:
//! - [`generate`] invokes `pr-writer.md` / `issue-writer.md` per shippable
//!   target, validates the produced sections, and writes
//!   `optimize/<id>/{pr,issue}-{title.txt,body.md}`. Codex receives no token in
//!   any rendered prompt.
//! - [`push`] reads `publish_token_file` into the [`StdGhClient`] (octocrab) at
//!   call time, then performs PR/issue creation via the GitHub REST API and
//!   `git` for the worktree → branch → push hop. The token never leaves
//!   `sbagent`'s memory, never enters its env, and is never written to disk.
//!
//! The token file is expected to live OUTSIDE every directory passed to
//! Codex via `--add-dir` or `[sandbox_workspace_write].writable_roots`,
//! so the LLM has no path that resolves to it.
//!
//! Per-target dispatch:
//! - `normal_pr` — needs `summary.experiments[id].status == "accepted"`, then
//!   runs `pr-writer.md`. Push: PR (draft per `PUBLISH_DRAFT_PRS`,
//!   default-labeled).
//! - `consensus_poc_pr` — needs `optimize/<id>/optimizer-report.json` with
//!   `outcome=implemented`, then runs `pr-writer.md`. Push: PR ALWAYS draft,
//!   with the safety labels `consensus-change,needs-HIP,do-not-merge`.
//! - `consensus_issue` — needs `optimize/<id>/consensus-issue.md`, then runs
//!   `issue-writer.md`. Push: issue with the safety labels
//!   `consensus-change,needs-HIP`. Idempotent via a hidden trace tag in the
//!   issue body.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use crate::harnesses::{AgentHarness, InvokeInputs};
use crate::layout::Layout;
use crate::models::common::DeliveryMode;
use crate::models::optimizer_report::OptimizerReport;
use crate::models::summary::{ExperimentStatus, Summary};
use crate::models::targets::MergedTarget;
use crate::prompts;
use crate::session::{SessionLayout, loader};
use crate::settings::Settings;

// ---------------------------------------------------------------------------
// generate (agent-side)
// ---------------------------------------------------------------------------

/// Inputs to `publish generate`.
pub struct GenerateInputs<'a, H: AgentHarness> {
    /// Resolved per-session layout.
    pub layout: &'a SessionLayout,
    /// Resolved framework + data layout.
    pub framework: &'a Layout,
    /// Settings.
    pub settings: &'a Settings,
    /// Agent harness.
    pub harness: &'a H,
}

/// Outputs of `publish generate`.
#[derive(Debug, Default)]
pub struct GenerateOutputs {
    /// PR-target prompts that succeeded.
    pub pr_count: usize,
    /// Issue-target prompts that succeeded.
    pub issue_count: usize,
    /// Targets that were skipped (with reason logged to stderr).
    pub skip_count: usize,
}

/// Generate per-target publish artifacts. Mirrors
/// `scripts/generate-pr-artifacts.sh`.
pub async fn generate<H: AgentHarness>(inputs: &GenerateInputs<'_, H>) -> Result<GenerateOutputs> {
    let targets = loader::read_optimization_targets(inputs.layout)
        .context("loading optimization-targets.json")?;
    if targets.targets.is_empty() {
        return Ok(GenerateOutputs::default());
    }
    // summary.json is required only for normal_pr targets — consensus modes
    // route on per-target marker files. Read it lazily.
    let summary = loader::read_summary(inputs.layout).ok();

    let mut outputs = GenerateOutputs::default();
    for target in &targets.targets {
        match decide_publish(target, inputs.layout, summary.as_ref()) {
            PublishDecision::ShipPr => {
                run_pr_writer(target, inputs).await?;
                outputs.pr_count += 1;
            }
            PublishDecision::ShipIssue => {
                run_issue_writer(target, inputs).await?;
                outputs.issue_count += 1;
            }
            PublishDecision::Skip(reason) => {
                eprintln!("skip {}: {reason}", target.id);
                clear_publish_artifacts(
                    &inputs
                        .layout
                        .experiment_dir(&target.id),
                );
                outputs.skip_count += 1;
            }
        }
    }
    Ok(outputs)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PublishDecision {
    ShipPr,
    ShipIssue,
    Skip(String),
}

fn decide_publish(
    target: &MergedTarget,
    layout: &SessionLayout,
    summary: Option<&Summary>,
) -> PublishDecision {
    let exp_dir = layout.experiment_dir(&target.id);
    match target.delivery_mode {
        DeliveryMode::NormalPr => {
            let Some(summary) = summary else {
                return PublishDecision::Skip("no-summary-file".to_owned());
            };
            let row = summary
                .experiments
                .iter()
                .find(|e| e.target_id == target.id);
            match row.map(|r| r.status) {
                Some(ExperimentStatus::Accepted) => PublishDecision::ShipPr,
                Some(s) => PublishDecision::Skip(format!("bench-status={s:?}")),
                None => PublishDecision::Skip("bench-status=missing".to_owned()),
            }
        }
        DeliveryMode::ConsensusPocPr => {
            // Gate on the typed report's outcome, not the rendered
            // companion file (the markdown might be present-but-stale
            // if a prior phase wrote it before a demotion). Use the
            // context-checking loader so a misbehaving agent can't
            // emit a report claiming a different mode to bypass
            // mode-specific invariants.
            match loader::read_optimizer_report_for_target(layout, &target.id, target.delivery_mode)
            {
                Ok(Some(OptimizerReport::Implemented(_))) => PublishDecision::ShipPr,
                Ok(Some(OptimizerReport::Aborted(_))) | Ok(None) => {
                    PublishDecision::Skip("no-implementation".to_owned())
                }
                Err(e) => PublishDecision::Skip(format!("optimizer-report error: {e}")),
            }
        }
        DeliveryMode::ConsensusIssue => {
            if is_non_empty_file(&exp_dir.join("consensus-issue.md")) {
                PublishDecision::ShipIssue
            } else {
                PublishDecision::Skip("no-consensus-issue-marker".to_owned())
            }
        }
    }
}

async fn run_pr_writer<H: AgentHarness>(
    target: &MergedTarget,
    inputs: &GenerateInputs<'_, H>,
) -> Result<()> {
    let exp_dir = inputs
        .layout
        .experiment_dir(&target.id);
    // Pre-flight: the typed optimizer report must say `implemented`
    // AND its context (target_id/session_id/delivery_mode) must match
    // the merged target. pr-writer only runs after `decide_publish`
    // already cleared this gate, so this check is defense-in-depth.
    match loader::read_optimizer_report_for_target(inputs.layout, &target.id, target.delivery_mode)?
    {
        Some(OptimizerReport::Implemented(_)) => {}
        _ => bail!(
            "missing or non-implemented optimizer-report.json for {} (cannot ship a PR)",
            target.id
        ),
    }
    let target_json = serde_json::to_string_pretty(target)?;
    let summary = loader::read_summary(inputs.layout).ok();
    let experiment_json = summary
        .as_ref()
        .and_then(|s| {
            s.experiments
                .iter()
                .find(|e| e.target_id == target.id)
        })
        .map(serde_json::to_string_pretty)
        .transpose()?
        .unwrap_or_else(|| "{}".to_owned());

    let worktree_dir = inputs
        .framework
        .session_optimizer_checkouts_dir(&inputs.layout.id)
        .join(&target.id);

    clear_publish_artifacts(&exp_dir);

    let prompts_dir = inputs
        .settings
        .require_prompt_overrides_dir()?;
    let rendered = prompts::render(
        "pr-writer",
        &prompts::PrWriterPrompt {
            opt_session_id: inputs
                .layout
                .id
                .as_str()
                .to_owned(),
            target_id: target.id.clone(),
            output_dir: exp_dir
                .to_string_lossy()
                .into_owned(),
            worktree_dir: worktree_dir
                .to_string_lossy()
                .into_owned(),
            target_json,
            experiment_json,
            delivery_mode: delivery_mode_str(target.delivery_mode).to_owned(),
        },
        prompts_dir,
    )?;
    std::fs::write(exp_dir.join("pr-writer-prompt.md"), &rendered)?;

    let timeout = inputs
        .settings
        .codex_exec_timeout_sec
        .filter(|n| *n > 0)
        .map(Duration::from_secs);
    let model = inputs
        .settings
        .codex_model
        .as_deref()
        .unwrap_or("gpt-5.5");
    let reasoning_effort = inputs
        .settings
        .codex_reasoning_effort
        .as_deref();
    let dangerous = inputs
        .settings
        .codex_dangerously_bypass_sandbox
        .unwrap_or(false);
    // pr-writer's rendered prompt is passed inline to codex; the agent
    // writes `pr-title.txt`/`pr-body.md` to cwd (the experiment dir)
    // and reads diff context from the worktree. No framework path
    // needed — schemas/prompts aren't referenced from inside the
    // pr-writer template.
    let mut add_dirs: Vec<PathBuf> = vec![worktree_dir.clone()];
    add_dirs.extend(
        inputs
            .settings
            .codex_extra_writable_roots
            .iter()
            .cloned(),
    );

    inputs
        .harness
        .invoke(&InvokeInputs {
            rendered_prompt: &rendered,
            cwd: exp_dir.as_ref(),
            add_dirs: &add_dirs,
            events_jsonl: &exp_dir.join("pr-writer-events.jsonl"),
            stderr_log: &exp_dir.join("pr-writer-stderr.log"),
            last_message: &exp_dir.join("pr-writer-final-message.md"),
            timeout,
            model,
            reasoning_effort,
            skip_git_repo_check: true,
            dangerously_bypass_sandbox: dangerous,
            enable_web_search: false,
            extra_env: &[],
        })
        .await
        .with_context(|| format!("invoking codex for pr-writer {}", target.id))?;

    let title_path = exp_dir.join("pr-title.txt");
    let body_path = exp_dir.join("pr-body.md");
    if !is_non_empty_file(&title_path) {
        bail!("pr-title.txt missing for {}", target.id);
    }
    if !is_non_empty_file(&body_path) {
        bail!("pr-body.md missing for {}", target.id);
    }
    validate_pr_body_sections(&body_path, target.delivery_mode)
        .with_context(|| format!("validating pr-body.md for {}", target.id))?;
    Ok(())
}

async fn run_issue_writer<H: AgentHarness>(
    target: &MergedTarget,
    inputs: &GenerateInputs<'_, H>,
) -> Result<()> {
    let exp_dir = inputs
        .layout
        .experiment_dir(&target.id);
    if !is_non_empty_file(&exp_dir.join("consensus-issue.md")) {
        bail!("missing consensus-issue.md for {}", target.id);
    }
    let target_json = serde_json::to_string_pretty(target)?;

    clear_publish_artifacts(&exp_dir);

    let prompts_dir = inputs
        .settings
        .require_prompt_overrides_dir()?;
    let rendered = prompts::render(
        "issue-writer",
        &prompts::IssueWriterPrompt {
            opt_session_id: inputs
                .layout
                .id
                .as_str()
                .to_owned(),
            target_id: target.id.clone(),
            output_dir: exp_dir
                .to_string_lossy()
                .into_owned(),
            target_json,
        },
        prompts_dir,
    )?;
    std::fs::write(exp_dir.join("issue-writer-prompt.md"), &rendered)?;

    let timeout = inputs
        .settings
        .codex_exec_timeout_sec
        .filter(|n| *n > 0)
        .map(Duration::from_secs);
    let model = inputs
        .settings
        .codex_model
        .as_deref()
        .unwrap_or("gpt-5.5");
    let reasoning_effort = inputs
        .settings
        .codex_reasoning_effort
        .as_deref();
    let dangerous = inputs
        .settings
        .codex_dangerously_bypass_sandbox
        .unwrap_or(false);
    // Issue-writer reads `consensus-issue.md` from cwd (the experiment
    // dir) and writes the rendered issue body alongside it. No extra
    // add_dirs needed beyond cwd (which codex always grants); the
    // operator-side extras still merge in.
    let mut add_dirs: Vec<PathBuf> = vec![];
    add_dirs.extend(
        inputs
            .settings
            .codex_extra_writable_roots
            .iter()
            .cloned(),
    );

    inputs
        .harness
        .invoke(&InvokeInputs {
            rendered_prompt: &rendered,
            cwd: exp_dir.as_ref(),
            add_dirs: &add_dirs,
            events_jsonl: &exp_dir.join("issue-writer-events.jsonl"),
            stderr_log: &exp_dir.join("issue-writer-stderr.log"),
            last_message: &exp_dir.join("issue-writer-final-message.md"),
            timeout,
            model,
            reasoning_effort,
            skip_git_repo_check: true,
            dangerously_bypass_sandbox: dangerous,
            enable_web_search: false,
            extra_env: &[],
        })
        .await
        .with_context(|| format!("invoking codex for issue-writer {}", target.id))?;

    let title_path = exp_dir.join("issue-title.txt");
    let body_path = exp_dir.join("issue-body.md");
    if !is_non_empty_file(&title_path) {
        bail!("issue-title.txt missing for {}", target.id);
    }
    if !is_non_empty_file(&body_path) {
        bail!("issue-body.md missing for {}", target.id);
    }
    validate_issue_body_sections(&body_path)
        .with_context(|| format!("validating issue-body.md for {}", target.id))?;
    Ok(())
}

/// Required `## ` headings on a PR body. Mirrors the bash check.
fn validate_pr_body_sections(path: &Path, delivery_mode: DeliveryMode) -> Result<()> {
    let mut required = vec!["Summary", "What changed", "Benchmark result", "Validation"];
    if delivery_mode == DeliveryMode::ConsensusPocPr {
        required.push("Consensus / HIP coordination");
    }
    check_sections(path, &required)
}

fn validate_issue_body_sections(path: &Path) -> Result<()> {
    check_sections(
        path,
        &[
            "Summary",
            "Breakage class",
            "Proposed change",
            "Expected impact",
            "HIP / coordination concerns",
            "Why an issue, not a PR",
            "Reference: target id",
        ],
    )
}

fn check_sections(path: &Path, sections: &[&str]) -> Result<()> {
    let body =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut missing: Vec<String> = Vec::new();
    for section in sections {
        let want = section.to_ascii_lowercase();
        let mut found = false;
        for line in body.lines() {
            let line = line.trim_end();
            if !line.starts_with("## ") {
                continue;
            }
            // Normalize: strip leading `## `, lowercase, collapse whitespace.
            let heading = line[3..]
                .trim()
                .to_ascii_lowercase();
            let normalized: String = heading
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let want_normalized: String = want
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if normalized == want_normalized {
                found = true;
                break;
            }
        }
        if !found {
            missing.push((*section).to_owned());
        }
    }
    if !missing.is_empty() {
        bail!("missing required ## sections: {}", missing.join(", "));
    }
    Ok(())
}

/// Names of every per-experiment file Phase 5 (`publish generate` /
/// `publish push`) produces. Public so `sbagent publish clean` can
/// enumerate them without duplicating the list.
pub const PUBLISH_ARTIFACT_FILE_NAMES: &[&str] = &[
    "pr-title.txt",
    "pr-body.md",
    "pr-writer-prompt.md",
    "pr-writer-events.jsonl",
    "pr-writer-stderr.log",
    "pr-writer-final-message.md",
    "issue-title.txt",
    "issue-body.md",
    "issue-writer-prompt.md",
    "issue-writer-events.jsonl",
    "issue-writer-stderr.log",
    "issue-writer-final-message.md",
];

fn clear_publish_artifacts(exp_dir: &Path) {
    for name in PUBLISH_ARTIFACT_FILE_NAMES {
        let _ = std::fs::remove_file(exp_dir.join(name));
    }
}

fn is_non_empty_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

fn delivery_mode_str(mode: DeliveryMode) -> &'static str {
    match mode {
        DeliveryMode::NormalPr => "normal_pr",
        DeliveryMode::ConsensusPocPr => "consensus_poc_pr",
        DeliveryMode::ConsensusIssue => "consensus_issue",
    }
}

// ---------------------------------------------------------------------------
// push (in-process: octocrab + git)
// ---------------------------------------------------------------------------

/// Publishing config. Mirrors the `publish_*` keys in `Settings`.
#[derive(Debug, Clone)]
pub struct PublishConfig {
    pub publish_remote: String,
    pub publish_base_repo: String,
    pub publish_base_branch: String,
    pub publish_draft_prs: bool,
    pub publish_pr_labels: Vec<String>,
    pub publish_branch_prefix: String,
    pub publish_token_file: PathBuf,
    /// Override for the head owner (otherwise derived from the configured
    /// remote URL).
    pub publish_head_owner: Option<String>,
}

impl PublishConfig {
    /// Build a [`PublishConfig`] from the loaded `Settings`, falling back to
    /// the workflow's documented defaults for any unset field.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            publish_remote: settings
                .publish_remote
                .clone()
                .unwrap_or_else(|| "origin".to_owned()),
            publish_base_repo: settings
                .publish_base_repo
                .clone()
                .unwrap_or_else(|| "cylewitruk/stacks-core".to_owned()),
            publish_base_branch: settings
                .publish_base_branch
                .clone()
                .unwrap_or_else(|| "feat/stacks-bench".to_owned()),
            publish_draft_prs: settings
                .publish_draft_prs
                .unwrap_or(true),
            publish_pr_labels: settings
                .publish_pr_labels
                .clone()
                .unwrap_or_default(),
            publish_branch_prefix: settings
                .publish_branch_prefix
                .clone()
                .unwrap_or_else(|| "agentic".to_owned()),
            publish_token_file: settings
                .publish_token_file
                .clone()
                .unwrap_or_else(default_publish_token_path),
            publish_head_owner: settings
                .publish_head_owner
                .clone(),
        }
    }
}

/// Bundle of args for [`GhClient::create_pr`]. Bundling avoids the
/// `clippy::too_many_arguments` lint and keeps the call site readable.
#[derive(Debug)]
pub struct CreatePrArgs<'a> {
    /// `owner/repo` slug.
    pub repo: &'a str,
    /// Base branch.
    pub base: &'a str,
    /// `head_owner:branch` (cross-repo PR ready).
    pub head: &'a str,
    /// Open as draft.
    pub draft: bool,
    /// Labels to add post-creation.
    pub labels: &'a [String],
    /// PR title.
    pub title: &'a str,
    /// PR body content (already-read from disk).
    pub body: &'a str,
}

/// Operations `publish::push` needs. Git ops shell out to `git`; GitHub
/// API ops go through octocrab in-process.
///
/// The git ops stay sync (they're well-isolated, leverage the user's
/// existing auth setup, and `git push` over libgit2 means a separate auth
/// callback rabbit-hole). The GitHub API ops are async because that's
/// octocrab's surface.
pub trait GhClient: Send + Sync {
    fn worktree_remote_url(&self, worktree: &Path, remote: &str) -> Result<String>;
    fn switch_branch(&self, worktree: &Path, branch: &str) -> Result<()>;
    fn add_modified(&self, worktree: &Path) -> Result<()>;
    fn commit_if_staged(&self, worktree: &Path, message: &str) -> Result<()>;
    fn push_branch(&self, worktree: &Path, remote: &str, branch: &str) -> Result<()>;

    fn pr_exists(
        &self,
        repo: &str,
        head_owner: &str,
        branch: &str,
        base: &str,
    ) -> impl std::future::Future<Output = Result<bool>> + Send;
    fn issue_exists(
        &self,
        repo: &str,
        trace_tag: &str,
    ) -> impl std::future::Future<Output = Result<bool>> + Send;
    fn create_pr<'a>(
        &'a self,
        args: CreatePrArgs<'a>,
    ) -> impl std::future::Future<Output = Result<()>> + Send + 'a;
    fn create_issue<'a>(
        &'a self,
        repo: &'a str,
        labels: &'a [String],
        title: &'a str,
        body: &'a str,
    ) -> impl std::future::Future<Output = Result<()>> + Send + 'a;
}

/// Default impl: git ops shell out to `git`; GitHub API ops go through
/// an authenticated [`octocrab::Octocrab`] in-process.
pub struct StdGhClient {
    pub api: octocrab::Octocrab,
}

impl StdGhClient {
    /// Construct from a personal access token. The token is held in the
    /// octocrab client's memory only — never written to disk and never
    /// exported into this process's env.
    pub fn from_token(token: &str) -> Result<Self> {
        let api = octocrab::Octocrab::builder()
            .personal_token(token.to_owned())
            .build()
            .context("constructing octocrab client")?;
        Ok(Self { api })
    }
}

/// Split an `owner/repo` slug, bailing on a malformed value.
fn split_repo(slug: &str) -> Result<(&str, &str)> {
    slug.split_once('/')
        .ok_or_else(|| anyhow::anyhow!("expected `owner/repo`, got `{slug}`"))
}

impl GhClient for StdGhClient {
    fn worktree_remote_url(&self, worktree: &Path, remote: &str) -> Result<String> {
        crate::git::get_remote_url(worktree, remote)
    }
    fn switch_branch(&self, worktree: &Path, branch: &str) -> Result<()> {
        crate::git::run_git(worktree, &["switch", "-C", branch])
    }
    fn add_modified(&self, worktree: &Path) -> Result<()> {
        crate::git::run_git(worktree, &["add", "-u"])
    }
    fn commit_if_staged(&self, worktree: &Path, message: &str) -> Result<()> {
        if !crate::git::has_staged_changes(worktree)? {
            return Ok(());
        }
        crate::git::run_git(worktree, &["commit", "-m", message])
    }
    fn push_branch(&self, worktree: &Path, remote: &str, branch: &str) -> Result<()> {
        crate::git::run_git(worktree, &["push", "-u", remote, branch])
    }

    async fn pr_exists(
        &self,
        repo: &str,
        head_owner: &str,
        branch: &str,
        base: &str,
    ) -> Result<bool> {
        let (owner, repo_name) = split_repo(repo)?;
        // octocrab's `head` filter expects `owner:branch` for cross-repo PRs.
        let head_filter = format!("{head_owner}:{branch}");
        let page = self
            .api
            .pulls(owner, repo_name)
            .list()
            .state(octocrab::params::State::All)
            .head(&head_filter)
            .base(base)
            .per_page(1)
            .send()
            .await
            .with_context(|| format!("octocrab pulls list for {repo}"))?;
        Ok(!page.items.is_empty())
    }

    async fn issue_exists(&self, repo: &str, trace_tag: &str) -> Result<bool> {
        // GitHub search syntax. `is:issue` excludes PRs (which the search
        // API also indexes); the trace tag is HTML-comment markup we
        // embedded on issue creation.
        let query = format!(r#"repo:{repo} is:issue "{trace_tag}" in:body"#);
        let page = self
            .api
            .search()
            .issues_and_pull_requests(&query)
            .per_page(1)
            .send()
            .await
            .with_context(|| format!("octocrab search for {trace_tag} in {repo}"))?;
        Ok(!page.items.is_empty())
    }

    async fn create_pr<'a>(&'a self, args: CreatePrArgs<'a>) -> Result<()> {
        let (owner, repo_name) = split_repo(args.repo)?;
        let pr = self
            .api
            .pulls(owner, repo_name)
            .create(args.title, args.head, args.base)
            .body(args.body)
            .draft(args.draft)
            .send()
            .await
            .with_context(|| format!("octocrab pulls create for {}", args.repo))?;
        if !args.labels.is_empty() {
            // Label endpoint lives under issues/<number>; PRs are issues
            // for label purposes.
            self.api
                .issues(owner, repo_name)
                .add_labels(pr.number, args.labels)
                .await
                .with_context(|| format!("octocrab add_labels for PR #{}", pr.number))?;
        }
        println!("{}", pr.html_url);
        Ok(())
    }

    async fn create_issue<'a>(
        &'a self,
        repo: &'a str,
        labels: &'a [String],
        title: &'a str,
        body: &'a str,
    ) -> Result<()> {
        let (owner, repo_name) = split_repo(repo)?;
        let issue = self
            .api
            .issues(owner, repo_name)
            .create(title)
            .body(body)
            .labels(labels.to_vec())
            .send()
            .await
            .with_context(|| format!("octocrab issues create for {repo}"))?;
        println!("{}", issue.html_url);
        Ok(())
    }
}

/// Inputs to `publish push`.
pub struct PushInputs<'a, G: GhClient> {
    pub layout: &'a SessionLayout,
    pub framework: &'a Layout,
    pub config: &'a PublishConfig,
    pub gh: &'a G,
}

#[derive(Debug, Default)]
pub struct PushOutputs {
    pub pr_count: usize,
    pub issue_count: usize,
    pub skip_count: usize,
}

const CONSENSUS_PR_LABELS: &[&str] = &["consensus-change", "needs-HIP", "do-not-merge"];
const CONSENSUS_ISSUE_LABELS: &[&str] = &["consensus-change", "needs-HIP"];

pub async fn push<G: GhClient>(inputs: &PushInputs<'_, G>) -> Result<PushOutputs> {
    let targets = loader::read_optimization_targets(inputs.layout)
        .context("loading optimization-targets.json")?;
    if targets.targets.is_empty() {
        return Ok(PushOutputs::default());
    }

    let head_owner = match &inputs
        .config
        .publish_head_owner
    {
        Some(o) => o.clone(),
        None => derive_head_owner(
            inputs.gh,
            inputs
                .framework
                .require_base()?,
            &inputs.config.publish_remote,
        )?,
    };

    let mut outputs = PushOutputs::default();
    for target in &targets.targets {
        let exp_dir = inputs
            .layout
            .experiment_dir(&target.id);
        match target.delivery_mode {
            DeliveryMode::NormalPr | DeliveryMode::ConsensusPocPr => {
                if !is_non_empty_file(&exp_dir.join("pr-title.txt"))
                    || !is_non_empty_file(&exp_dir.join("pr-body.md"))
                {
                    eprintln!("skip {}: pr artifacts not generated", target.id);
                    outputs.skip_count += 1;
                    continue;
                }
                match push_pr(target, inputs, &exp_dir, &head_owner).await {
                    Ok(()) => outputs.pr_count += 1,
                    Err(e) => {
                        eprintln!("publish push: failed for {}: {e:#}", target.id);
                        outputs.skip_count += 1;
                    }
                }
            }
            DeliveryMode::ConsensusIssue => {
                if !is_non_empty_file(&exp_dir.join("issue-title.txt"))
                    || !is_non_empty_file(&exp_dir.join("issue-body.md"))
                {
                    eprintln!("skip {}: issue artifacts not generated", target.id);
                    outputs.skip_count += 1;
                    continue;
                }
                match push_issue(target, inputs, &exp_dir).await {
                    Ok(()) => outputs.issue_count += 1,
                    Err(e) => {
                        eprintln!("publish push: failed for {}: {e:#}", target.id);
                        outputs.skip_count += 1;
                    }
                }
            }
        }
    }
    Ok(outputs)
}

/// Read `<publish_token_file>` and return the trimmed token. Bails if
/// the file is missing, unreadable, or empty. Used by both
/// `cli::publish::push` and `cli::session::run` (Phase 5) to construct
/// the [`StdGhClient`] without ever exporting the token into the
/// environment.
pub fn read_publish_token(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading token file {}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("token file {} is empty", path.display());
    }
    Ok(trimmed.to_owned())
}

/// Default path for `publish_token_file` when unset:
/// `${HOME}/.config/sbagent/gh_token`. Falls back to a relative
/// `.config/sbagent/gh_token` if `HOME` isn't set, which will surface
/// a clear "file not found" at read time.
///
/// Deliberately points OUTSIDE the framework root: `publish generate`
/// passes the framework root into Codex via `--add-dir`, so any token
/// path inside that tree is reachable by the LLM.
pub fn default_publish_token_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return PathBuf::from(".config/sbagent/gh_token");
    }
    PathBuf::from(home)
        .join(".config")
        .join("sbagent")
        .join("gh_token")
}

/// Enforce that `token_path` does NOT live inside `framework_root`.
/// Codex receives the framework root via `--add-dir` during
/// `publish generate`, so a token inside the tree is reachable by the
/// LLM. Bails with an actionable message when the rule is violated.
///
/// Both paths are canonicalized when possible so a `data/run/gh_token`
/// vs. an absolute `<framework>/data/run/gh_token` form is caught
/// consistently.
pub fn ensure_token_outside_framework(
    token_path: &Path,
    framework_root: Option<&Path>,
) -> Result<()> {
    let Some(framework_root) = framework_root else {
        // No framework dir in play means `publish generate` won't
        // pass one to Codex via `--add-dir`. The token-leak vector
        // we're guarding against (LLM reading the token via the
        // framework's add_dir scope) doesn't exist here, so the
        // check is vacuous. The other add_dirs `publish generate`
        // uses (worktree dir, experiment dir) live under
        // `agent_workspace_root` / sessions_root — operators should
        // keep PATs out of those by convention, but enforcing that
        // belongs in a separate, more general check.
        return Ok(());
    };
    let token_canon =
        std::fs::canonicalize(token_path).unwrap_or_else(|_| token_path.to_path_buf());
    let frame_canon =
        std::fs::canonicalize(framework_root).unwrap_or_else(|_| framework_root.to_path_buf());
    if token_canon.starts_with(&frame_canon) {
        bail!(
            "publish_token_file ({}) is inside the framework root ({}). `publish generate` passes \
             the framework root to Codex via --add-dir, so the token would be reachable by the \
             LLM. Move it outside the framework — the default `~/.config/sbagent/gh_token` is the \
             recommended location.",
            token_path.display(),
            framework_root.display(),
        );
    }
    Ok(())
}

async fn push_pr<G: GhClient>(
    target: &MergedTarget,
    inputs: &PushInputs<'_, G>,
    exp_dir: &Path,
    head_owner: &str,
) -> Result<()> {
    let worktree = inputs
        .framework
        .session_optimizer_checkouts_dir(&inputs.layout.id)
        .join(&target.id);
    if !worktree.is_dir() {
        bail!("missing worktree {}", worktree.display());
    }
    let title_file = exp_dir.join("pr-title.txt");
    let body_file = exp_dir.join("pr-body.md");
    let title = std::fs::read_to_string(&title_file)?
        .lines()
        .next()
        .unwrap_or("")
        .to_owned();
    let body = std::fs::read_to_string(&body_file)
        .with_context(|| format!("reading {}", body_file.display()))?;

    let branch = format!(
        "{}/{}/{}",
        inputs
            .config
            .publish_branch_prefix,
        inputs.layout.id,
        target.id
    );

    if inputs
        .gh
        .pr_exists(
            &inputs
                .config
                .publish_base_repo,
            head_owner,
            &branch,
            &inputs
                .config
                .publish_base_branch,
        )
        .await?
    {
        eprintln!("publish push: PR already exists for {}; skipping git ops.", target.id);
        return Ok(());
    }

    inputs
        .gh
        .switch_branch(&worktree, &branch)?;
    inputs
        .gh
        .add_modified(&worktree)?;
    let commit_msg =
        if title.is_empty() { format!("perf: optimize {}", target.id) } else { title.clone() };
    inputs
        .gh
        .commit_if_staged(&worktree, &commit_msg)?;
    inputs
        .gh
        .push_branch(&worktree, &inputs.config.publish_remote, &branch)?;

    let draft = matches!(target.delivery_mode, DeliveryMode::ConsensusPocPr)
        || inputs
            .config
            .publish_draft_prs;
    let mut labels: Vec<String> = inputs
        .config
        .publish_pr_labels
        .clone();
    if matches!(target.delivery_mode, DeliveryMode::ConsensusPocPr) {
        labels.extend(
            CONSENSUS_PR_LABELS
                .iter()
                .map(|s| (*s).to_owned()),
        );
    }
    let head = format!("{head_owner}:{branch}");
    inputs
        .gh
        .create_pr(CreatePrArgs {
            repo: &inputs
                .config
                .publish_base_repo,
            base: &inputs
                .config
                .publish_base_branch,
            head: &head,
            draft,
            labels: &labels,
            title: &title,
            body: &body,
        })
        .await
}

async fn push_issue<G: GhClient>(
    target: &MergedTarget,
    inputs: &PushInputs<'_, G>,
    exp_dir: &Path,
) -> Result<()> {
    let title_file = exp_dir.join("issue-title.txt");
    let body_file = exp_dir.join("issue-body.md");
    let title = std::fs::read_to_string(&title_file)?
        .lines()
        .next()
        .unwrap_or("")
        .to_owned();
    let body = std::fs::read_to_string(&body_file)?;

    let trace_tag = format!("agentic-{}-{}", inputs.layout.id, target.id);
    if inputs
        .gh
        .issue_exists(
            &inputs
                .config
                .publish_base_repo,
            &trace_tag,
        )
        .await?
    {
        eprintln!("publish push: issue already exists for {} ({trace_tag}); skipping.", target.id);
        return Ok(());
    }

    let body_with_trace = format!("{body}\n\n<!-- {trace_tag} -->\n");
    let labels: Vec<String> = CONSENSUS_ISSUE_LABELS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    inputs
        .gh
        .create_issue(
            &inputs
                .config
                .publish_base_repo,
            &labels,
            &title,
            &body_with_trace,
        )
        .await
}

fn derive_head_owner<G: GhClient>(gh: &G, base: &Path, remote: &str) -> Result<String> {
    let url = gh.worktree_remote_url(base, remote)?;
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .ok_or_else(|| anyhow::anyhow!("unable to derive head owner from remote URL: {url}"))?;
    // Need both an owner segment AND a repo segment — `git@github.com:foo`
    // (no slash) would otherwise return the whole tail as `owner` and
    // produce a `<owner>:<branch>` head ref the GitHub API rejects with
    // an opaque error.
    let (owner, _) = rest
        .split_once('/')
        .ok_or_else(|| {
            anyhow::anyhow!(
                "remote URL {url} is missing the `owner/repo` separator; expected \
                 `git@github.com:OWNER/REPO[.git]` or `https://github.com/OWNER/REPO`"
            )
        })?;
    if owner.is_empty() {
        bail!("remote URL {url} has an empty owner segment");
    }
    Ok(owner.to_owned())
}
