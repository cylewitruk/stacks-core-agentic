//! Agent-harness abstractions.
//!
//! [`AgentHarness::invoke`] is the single seam every Codex-launching phase
//! goes through. Production uses [`codex::CodexHarness`]; tests inject a
//! recording fake.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

pub mod codex;

/// Inputs to a single agent invocation. Caller renders the prompt, picks
/// the working directory, and lists the additional readable dirs the
/// sandboxed agent needs.
#[derive(Debug)]
pub struct InvokeInputs<'a> {
    /// Already-rendered prompt body (Askama substitution already applied).
    pub rendered_prompt: &'a str,
    /// Working directory the harness runs in (`--cd` for Codex).
    pub cwd: &'a Path,
    /// Additional readable directories for the sandbox (`--add-dir` for Codex).
    pub add_dirs: &'a [PathBuf],
    /// Captured stdout (Codex JSONL events stream).
    pub events_jsonl: &'a Path,
    /// Captured stderr.
    pub stderr_log: &'a Path,
    /// `--output-last-message` destination.
    pub last_message: &'a Path,
    /// Optional outer timeout. `None` runs without a timeout.
    pub timeout: Option<Duration>,
    /// Model id (`-m` for Codex).
    pub model: &'a str,
    /// Optional reasoning effort (`-c model_reasoning_effort=...`).
    pub reasoning_effort: Option<&'a str>,
    /// Pass `--skip-git-repo-check` (cwd may not be a git repo).
    pub skip_git_repo_check: bool,
    /// Force the dangerous-bypass sandbox flag (mirrors bash
    /// `CODEX_DANGEROUS_BYPASS_SANDBOX=1`). Otherwise uses
    /// `--ask-for-approval never --sandbox workspace-write`.
    pub dangerously_bypass_sandbox: bool,
    /// Disable web search (`--search` is not added when false). The bash
    /// triage script omits search to cut a network dependency; analyzers
    /// / optimizers may want it.
    pub enable_web_search: bool,
    /// Extra environment variables set on the harness child process (and
    /// thus inherited by every shell the agent spawns). Used by the
    /// optimizer to inject `GIT_AUTHOR_*` / `GIT_COMMITTER_*` /
    /// `GIT_CONFIG_COUNT` overrides without mutating any git config file —
    /// see [`crate::session::optimizers::optimizer_git_env`]. Empty for
    /// phases that don't need per-process git config (triage, analyzers,
    /// merge, publish).
    pub extra_env: &'a [(String, String)],
}

/// Output of a successful agent invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokeOutputs {
    /// Conversation/session id parsed from the JSONL events stream. May be
    /// `None` if the harness output didn't carry one (e.g. early failure).
    pub conversation_id: Option<String>,
}

/// Common interface for all agent harnesses (Codex today; Claude / Copilot
/// CLI later). Methods are async to support `tokio::process::Command`-based
/// implementations and trait-bound callers.
pub trait AgentHarness: Send + Sync {
    /// Verify the harness CLI is installed and advertises the flags this
    /// codebase depends on. Wired into `sbagent check`.
    fn check(&self) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Invoke the agent with the given inputs. Captures stdout to
    /// `events_jsonl`, stderr to `stderr_log`, and the agent's last
    /// message to `last_message`. Returns the captured conversation id.
    fn invoke<'a>(
        &'a self,
        inputs: &'a InvokeInputs<'a>,
    ) -> impl std::future::Future<Output = Result<InvokeOutputs>> + Send + 'a;
}
