//! Configuration loaded from a single `config.toml` file.
//!
//! `config.toml` is the canonical source of truth for every long-lived
//! setting `sbagent` reads. Per-invocation overrides live on the relevant
//! CLI flags (each carries an `env = "..."` attribute via clap, but those
//! exist for one-off ergonomics — there is no `.env` file loader and no
//! bulk env-var layering).
//!
//! Resolution order (full details on [`Settings::load`]):
//! 1. `--config-path <path>` (or `-c <path>`).
//! 2. `$XDG_CONFIG_HOME/sbagent/config.toml` when `XDG_CONFIG_HOME` is set.
//! 3. `$HOME/.config/sbagent/config.toml` (macOS default; Linux fallback when
//!    `XDG_CONFIG_HOME` is unset).
//!
//! Operators land in `~/.config/sbagent/config.toml` (the standard XDG
//! location for per-user config); machine-specific paths stay out of
//! the operator repo.
//!
//! Settings are grouped into stanzas (`[layout]`, `[stacks_bench]`, `[codex]`,
//! `[publish]`, `[git]`, etc.) rather than one flat file. Each sub-struct
//! is its own `Default + Deserialize` with `deny_unknown_fields`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// Top-level settings. Every field is optional; commands either pull
/// defaults from the resolved layout, or surface an error when a
/// required field is unset (e.g. `stacks_bench.source_dir` is required
/// by `session baseline run`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Developer / framework-internal knobs. Operators almost never set
    /// these; they exist for iterating on sbagent itself or for unusual
    /// checkout layouts.
    #[serde(default)]
    pub dev: DevSettings,

    /// Filesystem topology sbagent reads from and writes to —
    /// session/scratch/bundle/memory directories. Per-domain paths
    /// (`stacks_bench.data_dir`, `publish.token_file`) live in their
    /// own stanza; this stanza is the cross-cutting layout.
    #[serde(default)]
    pub layout: LayoutSettings,

    /// stacks-core submodule that builds the `stacks-bench` binary.
    #[serde(default)]
    pub stacks_core: StacksCoreSettings,

    /// `stacks-bench` invocation params (chainstate source, network,
    /// block range, filter).
    #[serde(default)]
    pub stacks_bench: StacksBenchSettings,

    /// Triage-phase tuning (candidate cap, lens weights, noise floor).
    #[serde(default)]
    pub triage: TriageSettings,

    /// Analyzer-phase tuning (parallelism cap).
    #[serde(default)]
    pub analyzer: AnalyzerSettings,

    /// Optimizer-phase tuning (inner-loop attempts + budget).
    #[serde(default)]
    pub optimizer: OptimizerSettings,

    /// Codex CLI invocation knobs (model, reasoning effort, sandbox,
    /// timeout). Shared by every agent-driving phase.
    #[serde(default)]
    pub codex: CodexSettings,

    /// Publish-phase config (PR target, draft mode, labels, branch
    /// shape, PAT location).
    #[serde(default)]
    pub publish: PublishSettings,

    /// Git identity + PAT-via-extraheader auth used by `init` (push /
    /// seed) and by every agent-side commit (optimizer worktrees).
    #[serde(default)]
    pub git: GitSettings,
}

/// `[dev]` — framework-internal knobs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevSettings {
    /// Absolute path to the framework checkout. When unset, layout
    /// derivation walks up from cwd looking for a `prompts/`+`schemas/`
    /// sibling. Set this when running `sbagent schema export` /
    /// `prompt lint` from a non-cwd location.
    #[serde(default)]
    pub framework_root: Option<PathBuf>,
}

/// `[layout]` — cross-cutting filesystem topology.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutSettings {
    /// Root for durable per-session artifacts (`<sessions_root>/<id>/`).
    /// Defaults to `<framework_root>/sessions`.
    #[serde(default)]
    pub sessions_root: Option<PathBuf>,

    /// Absolute path to the operator's git repo — holds `sessions/`,
    /// `repos/`, plus the `sessions.jsonl` ledger on `main` and
    /// `session/<id>` archive branches. Required by `session archive`.
    ///
    /// Defaults to `sessions_root.parent()` ONLY when `sessions_root`
    /// was set explicitly — the conventional `<operator>/sessions/`
    /// layout where the parent IS the operator. When `sessions_root`
    /// itself defaulted (typically to `<agent_workspace_root>/sessions/`),
    /// the parent points at the workspace root, which is NOT the
    /// operator; in that case `operator_repo_root` stays `None` and
    /// the operator must set it explicitly.
    ///
    /// Must be absolute when set (enforced by [`Settings::validate`]).
    #[serde(default)]
    pub operator_repo_root: Option<PathBuf>,

    /// Coordination lockfiles (`benchmark.lock`, `test.lock`). Defaults
    /// to `<framework_root>/data/run/`.
    #[serde(default)]
    pub lock_dir: Option<PathBuf>,

    /// Root for mutable agent scratch (per-target git clones, build
    /// caches). Phase-scoped subdirs: `<root>/optimizers/<sid>/<tid>/`.
    /// Durable session records stay under `sessions_root`; only
    /// ephemeral execution state lands here.
    ///
    /// When unset, optimizer checkouts fall back to the legacy
    /// `<sessions_root>/<id>/worktrees/<target>/`. Macos suggested:
    /// `/private/tmp/sbagent-workspaces/`; Linux: `/var/tmp/...` or
    /// `$XDG_CACHE_HOME/sbagent/`.
    #[serde(default)]
    pub agent_workspace_root: Option<PathBuf>,

    /// Operator-tuned prompt template overrides — autoresearch-style
    /// `program.md` model. For each phase prompt (`triage.md`,
    /// `analyzer.md`, `merge-analyses.md`, `optimizer.md`,
    /// `pr-writer.md`, `issue-writer.md`), a non-empty same-named file
    /// here overrides the bundled template; missing/empty falls back.
    /// Override files use the same `{{ field_name }}` substitution as
    /// the bundle — referencing a missing field is a hard error so
    /// drift is caught at render time.
    ///
    /// Primary operator tuning surface. Typical value: `.sbagent/prompts/`.
    #[serde(default)]
    pub prompt_overrides_dir: Option<PathBuf>,

    /// On-disk mirror of the bundled JSON Schemas. Not a tuning
    /// surface — `sbagent sync` overwrites unconditionally and
    /// `sbagent check` fails on drift. When unset, derived as the
    /// sibling `<prompt_overrides_dir>/../schemas`, else
    /// `<framework_root>/schemas`.
    #[serde(default)]
    pub schemas_dir: Option<PathBuf>,

    /// On-disk mirror of the bundled triage/analyzer SQL queries +
    /// operator README. Same contract as `schemas_dir`. When unset,
    /// derived as sibling of `prompt_overrides_dir`.
    #[serde(default)]
    pub queries_dir: Option<PathBuf>,

    /// Operator-tunable context / reference docs (the agent's
    /// "brainstem"). Each entry: markdown + TOML sidecar declaring
    /// which phases may surface it. Tunable — `sync` refreshes by
    /// default (preserve with `--keep-tunables`); `check` warns on
    /// drift except for the load-bearing `optimizer.md` which fails.
    /// When unset, derived as sibling of `prompt_overrides_dir`.
    #[serde(default)]
    pub context_overrides_dir: Option<PathBuf>,

    /// Operator's cross-session memory (analyzed-rejections ledger
    /// today; manual-review decisions and baseline history planned).
    /// Each file is a JSONL or markdown artifact the operator can
    /// edit/trim directly; `<memory_dir>/.locks/memory.lock` protects
    /// concurrent appends. Lives at the operator-repo top level, NOT
    /// under `.sbagent/` — this is accumulated bot knowledge, not
    /// bundled/synced state. When unset, derived as
    /// `<operator>/memory/` from `prompt_overrides_dir`'s parent.
    #[serde(default)]
    pub memory_dir: Option<PathBuf>,
}

/// `[stacks_core]` — the stacks-core submodule.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StacksCoreSettings {
    /// Path to the stacks-core checkout (typically a submodule at
    /// `<operator>/repos/stacks-core`). Builds the `stacks-bench`
    /// binary and is the source SHA finalize records. Required — no
    /// default.
    #[serde(default)]
    pub base: Option<PathBuf>,

    /// Clone URL for the stacks-core checkout. Read by `sbagent init`
    /// when bootstrapping the submodule; unused at runtime thereafter
    /// (sbagent reads `git -C <base> remote get-url origin` directly).
    #[serde(default)]
    pub base_repo_url: Option<String>,
}

/// `[stacks_bench]` — `stacks-bench` invocation params.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StacksBenchSettings {
    /// Persistent app-data dir for the `stacks-bench` binary (its
    /// own SQLite store, not sbagent's). Defaults to
    /// `<framework_root>/data/stacks-bench`.
    #[serde(default)]
    pub data_dir: Option<PathBuf>,

    /// Chainstate source dir (must contain `chainstate/`). Required by
    /// `session baseline run`.
    #[serde(default)]
    pub source_dir: Option<PathBuf>,

    /// Network identifier passed to `stacks-bench`. Defaults to `mainnet`.
    #[serde(default)]
    pub network: Option<String>,

    /// Parent dir passed via `--shadow-dir-root` for stacks-bench's
    /// reflink shadow copy of the source chainstate. **Must be on the
    /// same filesystem as `source_dir`** (reflinks fail across
    /// filesystems). Set this when the default — `source_dir.parent()`
    /// — isn't writable from the codex sandbox (e.g. `/Volumes/Extern`).
    /// Unset = pass nothing, let stacks-bench pick.
    #[serde(default)]
    pub shadow_dir: Option<PathBuf>,

    /// Block range start.
    #[serde(default)]
    pub start_at: Option<u64>,

    /// Block range count.
    #[serde(default)]
    pub count: Option<u64>,

    /// Number of pre-window blocks to advance before measurement
    /// starts (`--warmup` arg). Lets caches/JIT/IO settle so the
    /// measured `count` reflects steady-state. Unset = no warmup.
    #[serde(default)]
    pub warmup: Option<u64>,

    /// `--filter` arg for `bench run` (e.g. `contract-call`). Restricts
    /// which blocks in the range get replayed — useful for chainstates
    /// with non-canonical forks where iterating every height trips on
    /// missing blocks. Unset = no filter.
    #[serde(default)]
    pub filter: Option<String>,
}

/// `[triage]` — triage-phase tuning.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriageSettings {
    /// Soft cap on candidates per session. Orchestrator warns (doesn't
    /// reject) when triage emits more — catches degenerate sessions
    /// dumping every workload-entry pattern. Defaults to 20.
    #[serde(default)]
    pub candidate_soft_cap: Option<usize>,

    /// Selection-lens weights as `"tx_latency,tenure_throughput,commit_time"`,
    /// e.g. `"0.4,0.4,0.2"`.
    #[serde(default)]
    pub axis_weights: Option<String>,

    /// Conservative noise-floor fallback used when only a single
    /// baseline run was imported (no rerun for noise estimation).
    /// Defaults to 1.0%.
    #[serde(default)]
    pub single_run_noise_floor_pct: Option<f64>,
}

/// `[analyzer]` — analyzer-phase tuning.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalyzerSettings {
    /// Max analyzer subagents the Phase 1.5 fan-out runs in parallel.
    /// Each invocation is expensive (one Codex call + deep reasoning,
    /// minutes per family); 10-20 candidates per session is normal, so
    /// capping prevents N concurrent codex processes from saturating
    /// the host. Defaults to 4.
    #[serde(default)]
    pub concurrency_cap: Option<usize>,
}

/// `[optimizer]` — optimizer-phase tuning.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizerSettings {
    /// Max inner-loop attempts per merged target before giving up.
    /// Combined with `budget_minutes` as "whichever exhausts first".
    /// Applies to `normal_pr` targets only — `consensus_poc_pr` keeps
    /// the one-shot scoped-tests model. Default: 5.
    #[serde(default)]
    pub attempts: Option<u32>,

    /// Wall-clock budget (minutes) per merged target. Combined with
    /// `attempts` as "whichever exhausts first". Codex's
    /// `codex.exec_timeout_sec` is the hard kill; this is the
    /// prompt-level soft cap the agent self-enforces. Default: 60.
    #[serde(default)]
    pub budget_minutes: Option<u32>,
}

/// `[codex]` — Codex CLI invocation knobs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexSettings {
    /// Codex model id (e.g. `gpt-5.5`).
    #[serde(default)]
    pub model: Option<String>,

    /// Reasoning effort. For GPT-5.x and GPT-5.x-Codex models, `high`
    /// is the workflow's default.
    #[serde(default)]
    pub reasoning_effort: Option<String>,

    /// Per-phase override for merge-phase reasoning effort. Inherits
    /// `reasoning_effort` when unset.
    #[serde(default)]
    pub merge_reasoning_effort: Option<String>,

    /// Disable Codex's internal sandbox. Set true only on demo VMs
    /// that can't support it.
    #[serde(default)]
    pub dangerously_bypass_sandbox: Option<bool>,

    /// Outer timeout (seconds) for each `codex exec`. `0` disables.
    /// Defaults to 3600 (one hour).
    #[serde(default)]
    pub exec_timeout_sec: Option<u64>,

    /// Extra paths granted write access in the codex sandbox, on top
    /// of per-phase `add_dirs`. Codex's `-c` config grammar **replaces**
    /// array values rather than deep-merging, so any `writable_roots`
    /// in `~/.codex/config.toml` would be silently dropped by our
    /// per-invoke override. List those here so the harness folds them
    /// back in. Entries must be absolute paths (enforced by
    /// [`Settings::validate`]); empty list disables the extra grant.
    #[serde(default)]
    pub extra_writable_roots: Vec<PathBuf>,
}

/// `[publish]` — Phase 5 publish targets + PAT location.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishSettings {
    /// Absolute path to the GitHub PAT file. Mode 0600, owned by the
    /// agent user. Defaults to `${HOME}/.config/sbagent/gh_token`.
    ///
    /// MUST live OUTSIDE the framework root: `publish generate` passes
    /// the framework root to Codex via `--add-dir`, so an in-tree token
    /// is reachable by the LLM. Enforced at preflight + Phase 5 startup.
    #[serde(default)]
    pub token_file: Option<PathBuf>,

    /// Git remote name in the optimizer worktrees (e.g. `origin`).
    #[serde(default)]
    pub remote: Option<String>,

    /// `<owner>/<repo>` PRs and issues are filed against. Defaults to
    /// `cylewitruk/stacks-core` (operator's fork; least blast radius).
    #[serde(default)]
    pub base_repo: Option<String>,

    /// Branch name PRs target. Defaults to `feat/stacks-bench`.
    #[serde(default)]
    pub base_branch: Option<String>,

    /// Whether `normal_pr` PRs are created as drafts. `consensus_poc_pr`
    /// is always draft regardless of this. Defaults to true.
    #[serde(default)]
    pub draft_prs: Option<bool>,

    /// Labels added to every created PR. Each entry is passed
    /// individually to `gh pr create --label`.
    #[serde(default)]
    pub pr_labels: Option<Vec<String>>,

    /// Branch prefix for optimizer worktrees pushed to GitHub. Full
    /// branch name: `<prefix>/<session-id>/<target-id>`.
    #[serde(default)]
    pub branch_prefix: Option<String>,

    /// Override for the head owner in `gh pr create --head` (otherwise
    /// derived from the configured remote URL).
    #[serde(default)]
    pub head_owner: Option<String>,
}

/// `[git]` — commit identity + PAT-via-extraheader auth.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitSettings {
    /// Author identity for every agent-generated commit. Injected as
    /// `GIT_AUTHOR_NAME` + `GIT_COMMITTER_NAME` env vars (and as a
    /// `user.name` override via `GIT_CONFIG_COUNT`) so it applies to
    /// every `git` invocation inside the agent's process tree WITHOUT
    /// mutating any git config file. Should match the bot account
    /// whose PAT lives in `publish.token_file`. Defaults to
    /// `"stacks-bench-bot"`. See
    /// [`crate::session::optimizers::optimizer_git_env`] for the full
    /// env shape.
    #[serde(default)]
    pub author_name: Option<String>,

    /// Author email for agent commits. Same env-var injection as
    /// `author_name`. For GitHub commit attribution, use
    /// `<numeric-id>+<username>@users.noreply.github.com` (visible in
    /// Settings → Emails when "Keep my email addresses private" is
    /// enabled). Falls back to
    /// `"stacks-bench-bot@users.noreply.github.com"` (no numeric prefix
    /// → commits show as "unverified email" on GitHub but still
    /// attribute by name).
    #[serde(default)]
    pub author_email: Option<String>,

    /// Username component of the HTTP Basic credential injected for
    /// `init --push` and `--seed-from`. Combined with the PAT from
    /// `publish.token_file` as `<auth_username>:<token>` then
    /// base64-encoded into an `AUTHORIZATION: basic ...` header.
    ///
    /// Defaults to `x-access-token` (the magic username GitHub
    /// fine-grained PATs accept). Forge-specific values:
    /// - GitHub fine-grained PATs: `x-access-token` (default).
    /// - GitHub classic PATs: any non-blank username.
    /// - GitLab PATs: `oauth2` (or the actual GitLab username).
    /// - Bitbucket Cloud app passwords: the Bitbucket account username.
    /// - Self-hosted Gitea/Forgejo: typically `git` or the bot's account.
    #[serde(default)]
    pub auth_username: Option<String>,

    /// URL prefix the PAT-via-extraheader auth is scoped to. Used as
    /// (a) the `http.<prefix>.extraheader` git config key (so the token
    /// is only sent to URLs matching this prefix), and (b) the
    /// validation gate for `init --push` origin and `init --seed-from`
    /// `stacks_core.base_repo_url`.
    ///
    /// Defaults to `https://github.com/`. Set to a different forge's
    /// HTTPS root (with trailing slash) to use the same mechanism
    /// against GitLab, Bitbucket, etc.
    ///
    /// **Expert / advanced mode**: setting this to the empty string
    /// (`""`) drops the URL-prefix qualifier from the git config key.
    /// The token is then attached as `http.extraheader` (unqualified)
    /// and **MAY BE SENT TO ANY HTTPS REMOTE THAT GIT CONTACTS DURING
    /// THE INVOCATION**. Use only with audited destinations.
    ///
    /// Trailing slashes are normalized internally — `"https://gitlab.com"`
    /// and `"https://gitlab.com/"` resolve identically — so a missing
    /// slash can't be exploited by an attacker registering a similarly-
    /// named domain (e.g. `https://gitlab.com.evil.example/`).
    #[serde(default)]
    pub auth_url_prefix: Option<String>,
}

/// Default username for the PAT-via-extraheader Basic credential.
pub const DEFAULT_GIT_AUTH_USERNAME: &str = "x-access-token";

/// Default URL prefix the PAT-via-extraheader auth is scoped to.
pub const DEFAULT_GIT_AUTH_URL_PREFIX: &str = "https://github.com/";

/// Default network identifier passed to `stacks-bench`.
pub const DEFAULT_STACKS_BENCH_NETWORK: &str = "mainnet";

/// Default codex model id used by every phase when `codex.model` is unset.
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";

/// Default selection-lens weights for the triage phase.
pub const DEFAULT_TRIAGE_AXIS_WEIGHTS: &str = "0.4,0.4,0.2";

/// Default conservative noise-floor (percent) when only a single
/// baseline run was imported.
pub const DEFAULT_SINGLE_RUN_NOISE_FLOOR_PCT: f64 = 1.0;

/// Default soft cap on candidate count emitted by triage.
pub const DEFAULT_TRIAGE_CANDIDATE_SOFT_CAP: usize = 20;

/// Default analyzer-phase parallelism cap.
pub const DEFAULT_ANALYZER_CONCURRENCY_CAP: usize = 4;

/// Default optimizer inner-loop attempt cap.
pub const DEFAULT_OPTIMIZER_ATTEMPTS: u32 = 5;

/// Default optimizer wall-clock budget per target (minutes).
pub const DEFAULT_OPTIMIZER_BUDGET_MINUTES: u32 = 60;

/// Default outer timeout (seconds) for each `codex exec`.
pub const DEFAULT_CODEX_EXEC_TIMEOUT_SEC: u64 = 3600;

/// Default author identity for agent-generated commits.
pub const DEFAULT_GIT_AUTHOR_NAME: &str = "stacks-bench-bot";

/// Default author email for agent-generated commits.
pub const DEFAULT_GIT_AUTHOR_EMAIL: &str = "stacks-bench-bot@users.noreply.github.com";

impl StacksBenchSettings {
    /// Resolve [`StacksBenchSettings::source_dir`] or bail. Callers add
    /// phase context via [`anyhow::Context::context`] when useful.
    pub fn source_dir_required(&self) -> Result<&Path> {
        self.source_dir
            .as_deref()
            .context(
                "`stacks_bench.source_dir` not set in config; required by baseline / bench / \
                 calibration phases (or set `SOURCE_DIR` env var)",
            )
    }

    /// Network passed to `stacks-bench`. Defaults to `mainnet`.
    pub fn effective_network(&self) -> &str {
        self.network
            .as_deref()
            .unwrap_or(DEFAULT_STACKS_BENCH_NETWORK)
    }
}

impl TriageSettings {
    /// Effective selection-lens weights (defaults to `0.4,0.4,0.2`).
    pub fn effective_axis_weights(&self) -> &str {
        self.axis_weights
            .as_deref()
            .unwrap_or(DEFAULT_TRIAGE_AXIS_WEIGHTS)
    }

    /// Effective single-run noise floor percent (defaults to 1.0).
    pub fn effective_single_run_noise_floor_pct(&self) -> f64 {
        self.single_run_noise_floor_pct
            .unwrap_or(DEFAULT_SINGLE_RUN_NOISE_FLOOR_PCT)
    }

    /// Effective candidate soft cap (defaults to 20).
    pub fn effective_candidate_soft_cap(&self) -> usize {
        self.candidate_soft_cap
            .unwrap_or(DEFAULT_TRIAGE_CANDIDATE_SOFT_CAP)
    }
}

impl AnalyzerSettings {
    /// Effective analyzer parallelism cap (defaults to 4).
    pub fn effective_concurrency_cap(&self) -> usize {
        self.concurrency_cap
            .unwrap_or(DEFAULT_ANALYZER_CONCURRENCY_CAP)
    }
}

impl OptimizerSettings {
    /// Effective inner-loop attempt cap (defaults to 5).
    pub fn effective_attempts(&self) -> u32 {
        self.attempts
            .unwrap_or(DEFAULT_OPTIMIZER_ATTEMPTS)
    }

    /// Effective wall-clock budget per target in minutes (defaults to 60).
    pub fn effective_budget_minutes(&self) -> u32 {
        self.budget_minutes
            .unwrap_or(DEFAULT_OPTIMIZER_BUDGET_MINUTES)
    }
}

impl CodexSettings {
    /// Effective codex model. Defaults to `gpt-5.5` when unset; once
    /// `codex.model` is set, every phase respects it.
    pub fn effective_model(&self) -> &str {
        self.model
            .as_deref()
            .unwrap_or(DEFAULT_CODEX_MODEL)
    }

    /// Reasoning effort used by the merge phase. Falls back to the
    /// general `reasoning_effort` when `merge_reasoning_effort` is unset.
    pub fn effective_merge_reasoning_effort(&self) -> Option<&str> {
        self.merge_reasoning_effort
            .as_deref()
            .or(self
                .reasoning_effort
                .as_deref())
    }

    /// Effective codex exec timeout (seconds). Defaults to 3600.
    /// Returns `0` when the operator explicitly disabled the timeout.
    pub fn effective_exec_timeout_sec(&self) -> u64 {
        self.exec_timeout_sec
            .unwrap_or(DEFAULT_CODEX_EXEC_TIMEOUT_SEC)
    }

    /// Effective codex exec timeout as a [`Duration`], or `None` when the
    /// operator explicitly disabled it (`codex.exec_timeout_sec = 0`).
    /// Used by every agent-driving phase to bound `codex exec`.
    pub fn effective_exec_timeout(&self) -> Option<std::time::Duration> {
        match self.effective_exec_timeout_sec() {
            0 => None,
            n => Some(std::time::Duration::from_secs(n)),
        }
    }
}

impl PublishSettings {
    /// Resolve [`PublishSettings::token_file`] or bail. Use this at
    /// callsites that require the PAT (`init --push`, `publish push`,
    /// `session archive` push, `sync --push`). Callers without a
    /// hardcoded fallback to `~/.config/sbagent/gh_token` use this;
    /// callers that DO want the fallback use
    /// [`crate::session::publish::default_publish_token_path`].
    pub fn token_file_required(&self) -> Result<&Path> {
        self.token_file
            .as_deref()
            .context(
                "`publish.token_file` not set in config; required for `--push` / `--seed-from` / \
                 `publish push` / `session archive` (push step)",
            )
    }
}

impl GitSettings {
    /// Effective author identity for agent commits.
    pub fn effective_author_name(&self) -> &str {
        self.author_name
            .as_deref()
            .unwrap_or(DEFAULT_GIT_AUTHOR_NAME)
    }

    /// Effective author email for agent commits.
    pub fn effective_author_email(&self) -> &str {
        self.author_email
            .as_deref()
            .unwrap_or(DEFAULT_GIT_AUTHOR_EMAIL)
    }

    /// Effective PAT-via-extraheader Basic credential username.
    /// Defaults to `x-access-token` (GitHub fine-grained PATs).
    pub fn effective_auth_username(&self) -> &str {
        self.auth_username
            .as_deref()
            .unwrap_or(DEFAULT_GIT_AUTH_USERNAME)
    }

    /// Effective PAT-via-extraheader URL prefix, normalized to end
    /// in `/` when non-empty. Defaults to `https://github.com/`; an
    /// explicit empty string is preserved (expert mode).
    ///
    /// Rejects non-empty prefixes that aren't `https://...` — the
    /// PAT-via-extraheader mechanism only sends the Basic credential
    /// over HTTPS, and `http.<prefix>.extraheader` doesn't apply to
    /// `ssh://` / SCP-style remotes. A typo'd `http://` prefix would
    /// silently leak the PAT over plaintext, so we fail fast.
    pub fn effective_auth_url_prefix(&self) -> Result<String> {
        let prefix = match self
            .auth_url_prefix
            .as_deref()
        {
            Some(s) => normalize_auth_url_prefix(s),
            None => DEFAULT_GIT_AUTH_URL_PREFIX.to_owned(),
        };
        if !prefix.is_empty() && !prefix.starts_with("https://") {
            anyhow::bail!(
                "`git.auth_url_prefix = {:?}` is invalid: a non-empty prefix MUST be an HTTPS URL \
                 (the PAT-via-extraheader mechanism only sends the Basic credential over HTTPS, \
                 and `http.<prefix>.extraheader` cannot apply to SSH or other non-HTTP remotes). \
                 Either set it to your forge's `https://...` root (e.g. `https://gitlab.com/`) or \
                 set it to `\"\"` for expert / unqualified mode.",
                self.auth_url_prefix
                    .as_deref()
                    .unwrap_or(""),
            );
        }
        Ok(prefix)
    }
}

/// Normalize an operator-supplied URL prefix: when non-empty, ensure it
/// ends with `/` so `https://gitlab.com` and `https://gitlab.com/`
/// resolve identically. Defends against typosquat-style attacks where a
/// missing trailing slash would let `https://gitlab.com.evil.example/`
/// pass `starts_with("https://gitlab.com")`. Empty input (expert /
/// unqualified mode) is returned as-is.
pub fn normalize_auth_url_prefix(raw: &str) -> String {
    if raw.is_empty() || raw.ends_with('/') { raw.to_owned() } else { format!("{raw}/") }
}

/// Operator-side default for `layout.schemas_dir`, applied identically
/// by `sbagent init` (for seeding + initial-commit staging) and
/// `Layout::from_settings` (for runtime resolution). Single source of
/// truth so an operator can't end up with one schemas dir committed
/// and a different one created at runtime.
pub fn default_schemas_dir(settings: &Settings) -> Option<PathBuf> {
    default_bundle_sibling(
        settings
            .layout
            .schemas_dir
            .as_ref(),
        settings,
        "schemas",
    )
}

/// Operator-side default for `layout.queries_dir`. Same shape +
/// contract as [`default_schemas_dir`].
pub fn default_queries_dir(settings: &Settings) -> Option<PathBuf> {
    default_bundle_sibling(
        settings
            .layout
            .queries_dir
            .as_ref(),
        settings,
        "queries",
    )
}

/// Operator-side default for `layout.context_overrides_dir`. Same shape +
/// contract as [`default_schemas_dir`].
pub fn default_context_dir(settings: &Settings) -> Option<PathBuf> {
    default_bundle_sibling(
        settings
            .layout
            .context_overrides_dir
            .as_ref(),
        settings,
        "context",
    )
}

/// Operator-side default for `layout.memory_dir`. Same sibling-of-tunables
/// derivation as [`default_schemas_dir`].
pub fn default_memory_dir(settings: &Settings) -> Option<PathBuf> {
    default_bundle_sibling(
        settings
            .layout
            .memory_dir
            .as_ref(),
        settings,
        "memory",
    )
}

/// Shared bundle-dir resolution: explicit override wins; otherwise
/// derive a `<parent>/<leaf>` sibling of `prompt_overrides_dir`;
/// otherwise `None` so the caller can pick its own fallback (typically
/// `<framework>/<leaf>` or the conventional `.sbagent/<leaf>`).
fn default_bundle_sibling(
    explicit: Option<&PathBuf>,
    settings: &Settings,
    leaf: &str,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.clone());
    }
    let prompts = settings
        .layout
        .prompt_overrides_dir
        .as_ref()?;
    match prompts.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Some(parent.join(leaf)),
        // Bare filename (`prompts`, `./prompts`) — put the bundle dir
        // next to it in the same parent.
        _ => Some(PathBuf::from(leaf)),
    }
}

impl Settings {
    /// Resolve [`LayoutSettings::prompt_overrides_dir`] or return a
    /// clear error. Required by every render callsite since sbagent
    /// loads prompts from disk.
    pub fn require_prompt_overrides_dir(&self) -> Result<&Path> {
        self.layout
            .prompt_overrides_dir
            .as_deref()
            .context(
                "`layout.prompt_overrides_dir` not set in config; required since sbagent loads \
                 prompts from disk. Typical operator value: `.sbagent/prompts/`. sbagent seeds \
                 bundled defaults into this dir on startup; the operator can edit them in place.",
            )
    }

    /// Load settings from a TOML file. Path resolution, in order of
    /// precedence (first match wins):
    ///
    /// 1. **`--config-path <path>`** (the `path` argument). Must exist. Use
    ///    this for non-standard layouts or in tests.
    /// 2. **`$XDG_CONFIG_HOME/sbagent/config.toml`** (Linux + macOS operators
    ///    who set `XDG_CONFIG_HOME` explicitly).
    /// 3. **`$HOME/.config/sbagent/config.toml`** (macOS default; also the
    ///    Linux fallback when `XDG_CONFIG_HOME` is unset).
    ///
    /// If none match, returns [`Settings::default`] — every field None.
    /// That's almost certainly not what an operator wants, but it lets
    /// non-session commands (`prompt lint`, `schema export`) run
    /// without a config in place.
    ///
    /// Deserialize errors (typo'd field, wrong type) are surfaced
    /// rather than swallowed — silently dropping a config is a footgun.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let resolved = Self::resolve_config_path(path);
        let mut builder = config::Config::builder();
        let explicit_source = resolved.is_some();
        if let Some(ref p) = resolved {
            builder = builder.add_source(config::File::from(p.as_path()));
        }
        let cfg = builder
            .build()
            .context("building config")?;
        if !explicit_source {
            return Ok(Self::default());
        }
        let settings: Settings = cfg
            .try_deserialize()
            .with_context(|| {
                format!(
                    "parsing {}",
                    resolved
                        .as_ref()
                        .unwrap()
                        .display()
                )
            })?;
        settings
            .validate()
            .with_context(|| {
                format!(
                    "validating settings loaded from {}",
                    resolved
                        .as_ref()
                        .unwrap()
                        .display(),
                )
            })?;
        Ok(settings)
    }

    /// Reject syntactically-valid but semantically-dangerous settings.
    /// Runs after deserialization in [`Settings::load`]; tests can
    /// call it directly on hand-built instances.
    ///
    /// Checks:
    /// - [`CodexSettings::extra_writable_roots`] entries must be **absolute**.
    ///   Relative paths grant sandbox access relative to whatever cwd the codex
    ///   process inherits.
    /// - [`LayoutSettings::operator_repo_root`] must be **absolute** when set.
    ///   The archive flow runs git from absolute paths.
    pub fn validate(&self) -> Result<()> {
        for (i, path) in self
            .codex
            .extra_writable_roots
            .iter()
            .enumerate()
        {
            if !path.is_absolute() {
                anyhow::bail!(
                    "codex.extra_writable_roots[{i}] = {:?} is a relative path; sandbox grants \
                     must be absolute so they don't depend on the codex process's cwd",
                    path.display(),
                );
            }
        }
        if let Some(p) = &self.layout.operator_repo_root
            && !p.is_absolute()
        {
            anyhow::bail!(
                "layout.operator_repo_root = {:?} is a relative path; the archive flow runs git \
                 from absolute paths so its behavior doesn't depend on the caller's cwd",
                p.display(),
            );
        }
        Ok(())
    }

    /// Apply the resolution order documented on [`Settings::load`] and
    /// return the first config path that exists, or `None`.
    pub fn resolve_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
        if let Some(p) = explicit {
            return Some(p.to_path_buf());
        }
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            let p = PathBuf::from(xdg)
                .join("sbagent")
                .join("config.toml");
            if p.is_file() {
                return Some(p);
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home)
                .join(".config")
                .join("sbagent")
                .join("config.toml");
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_default_when_no_path_and_no_xdg_or_home_config() {
        // Scrub XDG_CONFIG_HOME / HOME so resolution can't pick up
        // the developer machine's `~/.config/sbagent/config.toml`.
        // SAFETY: see `resolve_config_path_precedence_order` for the
        // env-var-in-tests rationale (single-thread per-test under
        // nextest).
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("HOME");
        }
        let result = Settings::load(None);
        unsafe {
            if let Some(v) = prev_xdg {
                std::env::set_var("XDG_CONFIG_HOME", v);
            }
            if let Some(v) = prev_home {
                std::env::set_var("HOME", v);
            }
        }
        let settings = result.unwrap();
        assert!(
            settings
                .dev
                .framework_root
                .is_none()
        );
        assert!(settings.codex.model.is_none());
        assert!(
            settings
                .publish
                .token_file
                .is_none()
        );
    }

    #[test]
    fn load_parses_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
                [stacks_bench]
                source_dir = "/mnt/chainstate/mainnet"
                start_at   = 5_000_000
                count      = 200_000

                [publish]
                token_file = "/etc/sbagent/gh_token"
                pr_labels  = ["needs-bench-review", "auto-generated"]
                draft_prs  = false
            "#,
        )
        .unwrap();
        let s = Settings::load(Some(&path)).expect("load");
        assert_eq!(s.stacks_bench.source_dir, Some(PathBuf::from("/mnt/chainstate/mainnet")));
        assert_eq!(s.stacks_bench.start_at, Some(5_000_000));
        assert_eq!(s.stacks_bench.count, Some(200_000));
        assert_eq!(s.publish.token_file, Some(PathBuf::from("/etc/sbagent/gh_token")));
        assert_eq!(s.publish.draft_prs, Some(false));
        let labels = s
            .publish
            .pr_labels
            .as_deref()
            .unwrap();
        assert_eq!(labels, ["needs-bench-review", "auto-generated"]);
    }

    #[test]
    fn normalize_auth_url_prefix_appends_trailing_slash_when_missing() {
        assert_eq!(normalize_auth_url_prefix("https://gitlab.com"), "https://gitlab.com/");
        assert_eq!(normalize_auth_url_prefix("https://github.com/"), "https://github.com/");
        assert_eq!(normalize_auth_url_prefix(""), "");
    }

    #[test]
    fn effective_git_auth_defaults_to_github_x_access_token() {
        let s = Settings::default();
        assert_eq!(
            s.git
                .effective_auth_username(),
            "x-access-token"
        );
        assert_eq!(
            s.git
                .effective_auth_url_prefix()
                .unwrap(),
            "https://github.com/"
        );
    }

    #[test]
    fn effective_git_auth_normalizes_operator_prefix() {
        let s = Settings {
            git: GitSettings {
                auth_url_prefix: Some("https://gitlab.example".into()),
                auth_username: Some("oauth2".into()),
                ..GitSettings::default()
            },
            ..Settings::default()
        };
        // Trailing slash appended so `starts_with(prefix)` cannot be defeated
        // by a typosquat sibling host.
        assert_eq!(
            s.git
                .effective_auth_url_prefix()
                .unwrap(),
            "https://gitlab.example/"
        );
        assert_eq!(
            s.git
                .effective_auth_username(),
            "oauth2"
        );
    }

    #[test]
    fn effective_git_auth_preserves_empty_expert_mode() {
        let s = Settings {
            git: GitSettings {
                auth_url_prefix: Some(String::new()),
                ..GitSettings::default()
            },
            ..Settings::default()
        };
        assert_eq!(
            s.git
                .effective_auth_url_prefix()
                .unwrap(),
            ""
        );
    }

    /// Shared `default_schemas_dir` helper covers every combination
    /// of `schemas_dir` / `prompt_overrides_dir`:
    /// - explicit `schemas_dir` wins.
    /// - conventional `.sbagent/prompts` → `.sbagent/schemas`.
    /// - bare-filename `prompts` → bare `schemas` (next to it).
    /// - both unset → None.
    #[test]
    fn default_schemas_dir_covers_all_cases() {
        // 1. Explicit setting wins.
        let s = Settings {
            layout: LayoutSettings {
                schemas_dir: Some(PathBuf::from("/abs/custom")),
                prompt_overrides_dir: Some(PathBuf::from(".sbagent/prompts")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        assert_eq!(default_schemas_dir(&s), Some(PathBuf::from("/abs/custom")));

        // 2. Conventional sibling layout.
        let s = Settings {
            layout: LayoutSettings {
                prompt_overrides_dir: Some(PathBuf::from(".sbagent/prompts")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        assert_eq!(default_schemas_dir(&s), Some(PathBuf::from(".sbagent/schemas")));

        // 3. Bare filename → next to it (NOT under .sbagent/).
        // This is the case Codex flagged: init.rs and layout.rs used to
        // disagree here. The shared helper makes them agree.
        let s = Settings {
            layout: LayoutSettings {
                prompt_overrides_dir: Some(PathBuf::from("prompts")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        assert_eq!(default_schemas_dir(&s), Some(PathBuf::from("schemas")));

        // 4. Both unset → None (caller decides the fallback).
        assert_eq!(default_schemas_dir(&Settings::default()), None);
    }

    /// Non-empty prefix that isn't `https://...` MUST be rejected: a
    /// typo'd `http://forge.example/` would silently send the PAT over
    /// plaintext, and a `git@host:` prefix would pass `starts_with`
    /// matching for SSH URLs even though `http.<prefix>.extraheader`
    /// can never apply to them. The destination-URL gate in `init` is
    /// a secondary defense; this is the primary one.
    #[test]
    fn effective_git_auth_url_prefix_rejects_non_https_prefix() {
        for bad in ["http://forge.example/", "git@github.com:", "ssh://git@host/"] {
            let s = Settings {
                git: GitSettings {
                    auth_url_prefix: Some(bad.into()),
                    ..GitSettings::default()
                },
                ..Settings::default()
            };
            let err = s
                .git
                .effective_auth_url_prefix()
                .expect_err(&format!("prefix {bad:?} must be rejected"));
            let msg = format!("{err:#}");
            assert!(
                msg.contains(bad) && (msg.contains("HTTPS") || msg.contains("https")),
                "rejection must cite the bad prefix + HTTPS requirement; got: {msg}"
            );
        }
    }

    #[test]
    fn load_rejects_unknown_field() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "definitely_not_a_real_setting = true\n").unwrap();
        let err = Settings::load(Some(&path)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("definitely_not_a_real_setting") || msg.contains("unknown field"),
            "msg={msg}"
        );
    }

    /// `resolve_config_path` walks three locations in order: explicit
    /// `--config-path`, `$XDG_CONFIG_HOME/sbagent/config.toml`,
    /// `$HOME/.config/sbagent/config.toml`. This test exercises all
    /// three from a single body so env-var manipulation (global state)
    /// doesn't race other tests.
    #[test]
    fn resolve_config_path_precedence_order() {
        let tmp = tempfile::tempdir().unwrap();
        let explicit = tmp
            .path()
            .join("explicit.toml");
        let xdg_root = tmp.path().join("xdg");
        let home_root = tmp.path().join("home");
        for d in [&xdg_root, &home_root] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&explicit, "").unwrap();
        let xdg_path = xdg_root
            .join("sbagent")
            .join("config.toml");
        std::fs::create_dir_all(xdg_path.parent().unwrap()).unwrap();
        std::fs::write(&xdg_path, "").unwrap();
        let home_path = home_root
            .join(".config")
            .join("sbagent")
            .join("config.toml");
        std::fs::create_dir_all(home_path.parent().unwrap()).unwrap();
        std::fs::write(&home_path, "").unwrap();

        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");

        // SAFETY: changing process-wide env vars is safe here because
        // cargo nextest runs each test in its own process; within a
        // single test body these `set_var` calls don't race other
        // tests.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &xdg_root);
            std::env::set_var("HOME", &home_root);
        }
        assert_eq!(
            Settings::resolve_config_path(Some(&explicit)),
            Some(explicit.clone()),
            "explicit --config-path must beat all fallbacks",
        );

        assert_eq!(
            Settings::resolve_config_path(None),
            Some(xdg_path.clone()),
            "XDG_CONFIG_HOME/sbagent/config.toml must beat $HOME fallback",
        );

        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        assert_eq!(
            Settings::resolve_config_path(None),
            Some(home_path.clone()),
            "$HOME/.config/sbagent/config.toml must be the last fallback",
        );

        unsafe {
            std::env::remove_var("HOME");
        }
        assert_eq!(
            Settings::resolve_config_path(None),
            None,
            "with no env vars set, resolution returns None",
        );

        unsafe {
            if let Some(v) = prev_xdg {
                std::env::set_var("XDG_CONFIG_HOME", v);
            }
            if let Some(v) = prev_home {
                std::env::set_var("HOME", v);
            }
        }
    }

    #[test]
    fn validate_accepts_absolute_writable_roots() {
        let s = Settings {
            codex: CodexSettings {
                extra_writable_roots: vec![
                    PathBuf::from("/Users/op/Library/Caches/sccache"),
                    PathBuf::from("/var/cache/sbagent"),
                ],
                ..CodexSettings::default()
            },
            ..Settings::default()
        };
        s.validate()
            .expect("absolute paths must pass validation");
    }

    #[test]
    fn validate_rejects_relative_writable_root() {
        let s = Settings {
            codex: CodexSettings {
                extra_writable_roots: vec![PathBuf::from("../shared-cache")],
                ..CodexSettings::default()
            },
            ..Settings::default()
        };
        let err = s
            .validate()
            .expect_err("relative path must fail validation");
        let msg = format!("{err:#}");
        assert!(msg.contains("codex.extra_writable_roots"), "msg: {msg}");
        assert!(msg.contains("relative path"), "msg: {msg}");
    }

    #[test]
    fn validate_rejects_relative_root_alongside_absolute() {
        let s = Settings {
            codex: CodexSettings {
                extra_writable_roots: vec![
                    PathBuf::from("/var/cache/sbagent"),
                    PathBuf::from("relative/sneaky"),
                ],
                ..CodexSettings::default()
            },
            ..Settings::default()
        };
        let err = s
            .validate()
            .expect_err("relative path among absolute paths must fail validation");
        let msg = format!("{err:#}");
        assert!(msg.contains("relative/sneaky"), "msg: {msg}");
    }

    #[test]
    fn validate_rejects_relative_operator_repo_root() {
        let s = Settings {
            layout: LayoutSettings {
                operator_repo_root: Some(PathBuf::from("repos/operator")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let err = s
            .validate()
            .expect_err("relative operator_repo_root must fail validation");
        let msg = format!("{err:#}");
        assert!(msg.contains("operator_repo_root"), "msg: {msg}");
        assert!(msg.contains("relative path"), "msg: {msg}");
    }

    #[test]
    fn validate_accepts_absolute_operator_repo_root() {
        let s = Settings {
            layout: LayoutSettings {
                operator_repo_root: Some(PathBuf::from("/Users/op/operator")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        s.validate()
            .expect("absolute operator_repo_root must pass validation");
    }
}
