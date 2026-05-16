//! Codex CLI harness.
//!
//! Mirrors the bash flag pile from `_lib.sh::codex_top_level_args` /
//! `codex_exec_args` plus the per-phase `codex exec ...` invocation. The
//! invariants that survive the bash → Rust port:
//!
//! - Top-level: `-m <model>`, `-c features.fast_mode=false`, optional `-c
//!   model_reasoning_effort=<effort>`, then either `--ask-for-approval never`
//!   (default) OR `--dangerously-bypass-approvals-and-sandbox` (when the demo
//!   VM can't support Codex's internal sandboxing).
//! - `exec` subcommand: `--skip-git-repo-check`, `--cd <cwd>`, `--add-dir`
//!   (repeated), `--sandbox workspace-write` (omitted under dangerous bypass),
//!   `--json`, `--output-last-message <path>`, optional `--search`, then the
//!   rendered prompt as the final positional arg.
//! - stdout → events JSONL, stderr → stderr log. Conversation id parsed from
//!   the events JSONL post-invocation.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use tokio::io::AsyncBufReadExt as _;

use crate::harnesses::{AgentHarness, InvokeInputs, InvokeOutputs};

/// Configuration shared across invocations.
///
/// Currently zero-sized; kept as a struct so callers can construct a
/// stable handle (and so future per-instance config has a place to land
/// without churning the trait surface).
#[derive(Debug, Clone, Default)]
pub struct CodexHarness;

impl CodexHarness {
    /// Codex CLI executable name (resolved on PATH).
    pub const COMMAND: &'static str = "codex";

    /// Construct a harness.
    pub fn new() -> Self {
        Self
    }
}

impl AgentHarness for CodexHarness {
    async fn check(&self) -> Result<()> {
        let help_out = tokio::process::Command::new(Self::COMMAND)
            .arg("--help")
            .output()
            .await
            .context("invoking `codex --help`")?;
        if !help_out.status.success() {
            bail!(
                "codex --help exited {}: {}",
                help_out.status,
                String::from_utf8_lossy(&help_out.stderr).trim()
            );
        }
        let help = String::from_utf8_lossy(&help_out.stdout);
        if !help.contains("--ask-for-approval") {
            bail!("codex CLI does not advertise --ask-for-approval; current scripts expect it");
        }
        if !help.contains("--model") && !help.contains("-m") {
            bail!("codex CLI does not advertise --model/-m; current scripts expect it");
        }
        Ok(())
    }

    async fn invoke<'a>(&'a self, inputs: &'a InvokeInputs<'a>) -> Result<InvokeOutputs> {
        if let Some(parent) = inputs.events_jsonl.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        if let Some(parent) = inputs.stderr_log.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let mut cmd = tokio::process::Command::new(Self::COMMAND);
        // Top-level args (before `exec`).
        cmd.arg("-m")
            .arg(inputs.model);
        cmd.arg("-c")
            .arg("features.fast_mode=false");
        if let Some(effort) = inputs.reasoning_effort {
            cmd.arg("-c")
                .arg(format!("model_reasoning_effort={effort}"));
        }
        if inputs.dangerously_bypass_sandbox {
            cmd.arg("--dangerously-bypass-approvals-and-sandbox");
        } else {
            cmd.arg("--ask-for-approval")
                .arg("never");
        }

        // exec subcommand.
        cmd.arg("exec");
        if inputs.skip_git_repo_check {
            cmd.arg("--skip-git-repo-check");
        }
        cmd.arg("--cd")
            .arg(inputs.cwd);
        for d in inputs.add_dirs {
            cmd.arg("--add-dir").arg(d);
        }
        if !inputs.dangerously_bypass_sandbox {
            cmd.arg("--sandbox")
                .arg("workspace-write");
        }
        cmd.arg("--json");
        if inputs.enable_web_search {
            cmd.arg("--search");
        }
        cmd.arg("--output-last-message")
            .arg(inputs.last_message);

        // Codex's CLI takes the prompt as a single positional argv. On
        // Linux MAX_ARG_STRLEN is typically 128 KB; the merge phase
        // inlines accepted-analyses JSON which can grow with N families.
        // Bail with a clear message before `spawn` returns E2BIG so the
        // operator gets an actionable error instead of a generic spawn
        // failure. Threshold = 96 KB to leave headroom for the env block.
        const PROMPT_BYTE_BUDGET: usize = 96 * 1024;
        if inputs.rendered_prompt.len() > PROMPT_BYTE_BUDGET {
            bail!(
                "rendered prompt is {} bytes; codex CLI carries the prompt as a single argv \
                 element and Linux limits this to ~128 KB. The merge phase is the typical culprit \
                 (it inlines ${{ACCEPTED_ANALYSES_JSON}}); shrink the upstream payload or split \
                 the work across smaller invocations.",
                inputs.rendered_prompt.len()
            );
        }
        cmd.arg(inputs.rendered_prompt);

        // Extra env (e.g. GIT_AUTHOR_* / GIT_CONFIG_COUNT injected by
        // the optimizer fan-out for per-process git identity + signing
        // overrides). Inherited by every shell codex spawns.
        for (k, v) in inputs.extra_env {
            cmd.env(k, v);
        }

        let stdout = std::fs::File::create(inputs.events_jsonl)
            .with_context(|| format!("opening events stream {}", inputs.events_jsonl.display()))?;
        let stderr = std::fs::File::create(inputs.stderr_log)
            .with_context(|| format!("opening stderr log {}", inputs.stderr_log.display()))?;
        cmd.stdout(Stdio::from(stdout));
        cmd.stderr(Stdio::from(stderr));

        let timeout = inputs
            .timeout
            .filter(|d| !d.is_zero());
        let status = match timeout {
            Some(d) => run_with_timeout(&mut cmd, d).await?,
            None => cmd
                .status()
                .await
                .context("invoking codex exec")?,
        };
        if !status.success() {
            bail!("codex exec exited with {status}");
        }

        let conversation_id = parse_conversation_id(inputs.events_jsonl).await?;
        Ok(InvokeOutputs { conversation_id })
    }
}

/// Spawn and await a `tokio::process::Command` with an outer timeout. On
/// timeout, kills the child and surfaces the timeout as an error.
async fn run_with_timeout(
    cmd: &mut tokio::process::Command,
    duration: Duration,
) -> Result<std::process::ExitStatus> {
    let mut child = cmd
        .spawn()
        .context("spawning codex")?;
    match tokio::time::timeout(duration, child.wait()).await {
        Ok(res) => res.context("waiting for codex"),
        Err(_) => {
            let _ = child.kill().await;
            bail!("codex exec exceeded timeout of {duration:?}");
        }
    }
}

/// Read the JSONL events stream and extract the first
/// `conversation_id` (or `session_id` fallback) emitted by the harness.
/// Mirrors `_lib.sh::capture_codex_conversation_id`.
async fn parse_conversation_id(path: &Path) -> Result<Option<String>> {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(
                anyhow::Error::new(e).context(format!("opening events stream {}", path.display()))
            );
        }
    };
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // tolerate non-JSON banners
        };
        if let Some(id) = parsed
            .get("conversation_id")
            .and_then(|v| v.as_str())
        {
            return Ok(Some(id.to_owned()));
        }
        if let Some(id) = parsed
            .get("session_id")
            .and_then(|v| v.as_str())
        {
            return Ok(Some(id.to_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[tokio::test]
    async fn parse_conversation_id_finds_first_match() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = std::fs::File::create(tmp.path()).unwrap();
        writeln!(f, "{}", serde_json::json!({"type": "banner"})).unwrap();
        writeln!(f, "{}", serde_json::json!({"conversation_id": "conv-abc"})).unwrap();
        writeln!(f, "{}", serde_json::json!({"conversation_id": "conv-xyz"})).unwrap();
        drop(f);

        let id = parse_conversation_id(tmp.path())
            .await
            .unwrap();
        assert_eq!(id.as_deref(), Some("conv-abc"));
    }

    #[tokio::test]
    async fn parse_conversation_id_falls_back_to_session_id() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut f = std::fs::File::create(tmp.path()).unwrap();
        writeln!(f, "{}", serde_json::json!({"session_id": "sess-1"})).unwrap();
        drop(f);

        let id = parse_conversation_id(tmp.path())
            .await
            .unwrap();
        assert_eq!(id.as_deref(), Some("sess-1"));
    }

    #[tokio::test]
    async fn parse_conversation_id_returns_none_for_empty_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::File::create(tmp.path()).unwrap();
        let id = parse_conversation_id(tmp.path())
            .await
            .unwrap();
        assert_eq!(id, None);
    }
}
