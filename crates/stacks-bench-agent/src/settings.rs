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
//! 2. `./config.toml` in the current directory (legacy default, kept for
//!    backward compatibility with operators whose config sits next to their
//!    `sessions/` dir).
//! 3. `$XDG_CONFIG_HOME/sbagent/config.toml` when `XDG_CONFIG_HOME` is set.
//! 4. `$HOME/.config/sbagent/config.toml` (macOS default; Linux fallback when
//!    `XDG_CONFIG_HOME` is unset).
//!
//! The XDG/HOME fallbacks let new operators land in
//! `~/.config/sbagent/config.toml` (the standard XDG location for
//! per-user config) without committing machine-specific paths into the
//! operator repo. The cwd fallback exists for legacy/test workflows; an
//! `sbagent init` bootstrap doesn't write a `./config.toml` —
//! operators are expected to place their config under `~/.config/sbagent/`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// Top-level settings. Every field is optional; commands either pull defaults
/// from the resolved layout, or surface an error when a required field is
/// unset (e.g. `source_dir` is required by `session baseline run`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Absolute path to the framework checkout root. When unset, the layout
    /// derivation walks up from cwd looking for a directory with `prompts/`
    /// and `schemas/`.
    #[serde(default)]
    pub framework_root: Option<PathBuf>,

    /// Sessions root. When unset, defaults to `<framework_root>/sessions`.
    #[serde(default)]
    pub sessions_root: Option<PathBuf>,

    /// Persistent stacks-bench app-data directory. When unset, defaults to
    /// `<framework_root>/data/stacks-bench`.
    #[serde(default)]
    pub stacks_bench_data_dir: Option<PathBuf>,

    /// Directory holding sbagent's coordination lockfiles
    /// (`benchmark.lock` for `bench run`/`rerun`, `test.lock` for
    /// optimizer `cargo nextest` serialization). Defaults to
    /// `<framework_root>/data/run/`.
    #[serde(default)]
    pub lock_dir: Option<PathBuf>,

    /// Root for mutable agent scratch state (per-target git clones,
    /// build caches, future per-phase scratch dirs). **Durable
    /// session records** — results, summaries, events, prompts —
    /// continue to live under `sessions_root`; only ephemeral
    /// execution state lands here.
    ///
    /// Layout: `<agent_workspace_root>/<phase>/<session>/<...>`. Today
    /// only the optimizer phase populates it, at
    /// `<root>/optimizers/<session_id>/<target_id>/` (each target's
    /// per-target git clone). Future phases can claim sibling subdirs
    /// without rearranging the operator's setup.
    ///
    /// When unset, optimizer checkouts fall back to the legacy path
    /// `<sessions_root>/<id>/worktrees/<target>/` for backward
    /// compatibility. Operators on macOS should typically set this to
    /// `/private/tmp/sbagent-workspaces/` (cleared on boot, outside
    /// Spotlight + Time Machine, no Seatbelt quirks); on Linux, an
    /// `/var/tmp/sbagent-workspaces/` or `$XDG_CACHE_HOME/sbagent/`
    /// path makes sense. The choice is operator-configured, NOT
    /// platform-magic.
    #[serde(default)]
    pub agent_workspace_root: Option<PathBuf>,

    /// Stacks-core checkout that builds + ships the `stacks-bench` binary.
    /// Owned by the operator (typically a submodule at
    /// `<operator_root>/repos/stacks-core/`). Required — no default.
    #[serde(default)]
    pub base: Option<PathBuf>,

    /// Clone URL for the stacks-core checkout. Used by `sbagent init`
    /// to bootstrap `base` as a submodule when the operator dir is
    /// first set up. Once `base` exists locally, this field is
    /// unused at runtime — read directly from
    /// `git -C <base> remote get-url origin` thereafter.
    /// Typically `https://github.com/<owner>/stacks-core.git`.
    #[serde(default)]
    pub base_repo_url: Option<String>,

    /// Username component of the HTTP Basic credential pair injected
    /// for `sbagent init --push` and `--seed-from`. Combined with the
    /// PAT from `publish_token_file` as
    /// `<git_auth_username>:<token>` then base64-encoded into an
    /// `AUTHORIZATION: basic ...` header.
    ///
    /// Defaults to `x-access-token` (GitHub fine-grained PATs accept
    /// this magic username). Forge-specific values:
    /// - **GitHub fine-grained PATs**: `x-access-token` (default) — works.
    /// - **GitHub classic PATs**: any non-blank username works.
    /// - **GitLab PATs**: `oauth2` (or the actual GitLab username).
    /// - **Bitbucket Cloud app passwords**: the Bitbucket account username.
    /// - **Self-hosted Gitea/Forgejo**: typically `git` or the bot's account
    ///   name.
    #[serde(default)]
    pub git_auth_username: Option<String>,

    /// URL prefix the PAT-via-extraheader auth is scoped to. Used as
    /// (a) the `http.<prefix>.extraheader` config key (so the token
    /// is only sent to URLs matching this prefix), and (b) the
    /// validation gate for `init --push` origin and `init
    /// --seed-from` `base_repo_url`.
    ///
    /// Defaults to `https://github.com/`. Set to a different forge's
    /// HTTPS root (with trailing slash) to use the same mechanism
    /// against GitLab, Bitbucket, etc. — e.g.
    /// `git_auth_url_prefix = "https://gitlab.com/"`.
    ///
    /// **Expert / advanced mode**: setting this to the empty string
    /// (`""`) drops the URL-prefix qualifier from the git config key.
    /// The token is then attached as `http.extraheader` (unqualified)
    /// and **MAY BE SENT TO ANY HTTPS REMOTE THAT GIT CONTACTS DURING
    /// THE INVOCATION**. Use only if you know what you're doing
    /// (e.g. you're running against a Git host with a non-trivial
    /// URL structure that defeats prefix matching, AND you've audited
    /// that the only remotes git will touch are the ones you control).
    ///
    /// Trailing slashes are normalized internally — `"https://gitlab.com"`
    /// and `"https://gitlab.com/"` resolve identically — so a missing
    /// slash can't be exploited by an attacker registering a similarly-
    /// named domain (e.g. `https://gitlab.com.evil.example/`).
    #[serde(default)]
    pub git_auth_url_prefix: Option<String>,

    /// Directory holding the operator's on-disk mirror of the bundled
    /// triage / analyzer SQL queries (`run_summary.sql`,
    /// `top_spans_by_self_wall.sql`, etc.) plus the operator-facing
    /// `README.md`. Same contract as
    /// [`Settings::schemas_dir`] — versioned bundle, not a tuning
    /// surface; `sbagent sync` overwrites unconditionally and
    /// `sbagent check` fails on drift.
    ///
    /// Resolution when unset: sibling of `prompt_overrides_dir`
    /// (`<parent>/queries`), then `<framework_root>/queries` (back-compat
    /// for operators still on the pre-bundle layout). Conventional
    /// layout: `.sbagent/prompts` + `.sbagent/schemas` + `.sbagent/queries`.
    #[serde(default)]
    pub queries_dir: Option<PathBuf>,

    /// Directory holding the operator's on-disk mirror of the bundled
    /// JSON Schemas (`candidates.schema.json`, `analysis.schema.json`,
    /// `optimization-targets.schema.json`, `summary.schema.json`).
    /// Unlike [`Settings::prompt_overrides_dir`], this is NOT a tuning
    /// surface — `sbagent sync` overwrites it unconditionally on every
    /// invocation, and `sbagent check` fails on any drift between the
    /// on-disk copy and the running binary's embedded bundle.
    ///
    /// Resolution when unset:
    /// 1. If `prompt_overrides_dir` is set, derives the sibling
    ///    `<prompt_overrides_dir>/../schemas` (so the conventional
    ///    `.sbagent/prompts/` + `.sbagent/schemas/` layout works without a
    ///    config change).
    /// 2. Otherwise falls back to `<framework_root>/schemas` for backward
    ///    compatibility with operators who haven't run `sbagent init`/`sbagent
    ///    sync` yet.
    ///
    /// Agents read schema files from this dir at session time
    /// (validation paths are embedded in the rendered prompts).
    #[serde(default)]
    pub schemas_dir: Option<PathBuf>,

    /// Directory of operator-tuned prompt templates that override the
    /// bundled defaults shipped with `sbagent`. For each Phase prompt
    /// (`triage.md`, `analyzer.md`, `merge-analyses.md`, `optimizer.md`,
    /// `pr-writer.md`, `issue-writer.md`), if a file with the matching
    /// name exists under this dir and is non-empty, its contents are
    /// used in place of the bundled template. Missing or empty files
    /// fall back to the bundle. Override files support the same
    /// `{{ field_name }}` substitution as the bundled Askama templates;
    /// referencing a field that doesn't exist on the struct is a hard
    /// error so drift is caught at render time.
    ///
    /// This is the operator's primary tuning surface — autoresearch-style
    /// "iterate on program.md" pattern.
    #[serde(default)]
    pub prompt_overrides_dir: Option<PathBuf>,

    /// Directory holding the operator's accumulated cross-session
    /// memory — currently the analyzed-rejections ledger, with
    /// room for future memory types (manual review decisions,
    /// performance baseline history, etc.). Each memory file is a
    /// JSONL or markdown artifact the operator can edit or trim
    /// directly; a single `<memory_dir>/.locks/memory.lock` protects
    /// concurrent writes from coordinator-side appends.
    ///
    /// Resolution when unset: `<operator>/memory/` derived from
    /// `prompt_overrides_dir`'s parent (same sibling-of-tunables
    /// pattern as `schemas_dir` / `context_overrides_dir`).
    ///
    /// Operator-visible top-level; NOT under `.sbagent/` because
    /// this is accumulated bot knowledge, not bundled/synced state.
    #[serde(default)]
    pub memory_dir: Option<PathBuf>,

    /// Maximum number of analyzer subagents the Phase 1.5 fan-out
    /// runs in parallel. The analyzer is expensive (one Codex
    /// invocation + a model with deep reasoning, several minutes
    /// per family). The triage-rejects-only-on-quality-grounds
    /// architecture (Axis 2) can produce 10-20 candidates per session
    /// on early runs; capping parallelism prevents N concurrent
    /// codex processes from saturating the host. Defaults to 4.
    #[serde(default)]
    pub analyzer_concurrency_cap: Option<usize>,

    /// Soft cap on the number of triage candidates per session. The
    /// orchestrator warns (but doesn't reject) when triage emits
    /// more than this. Catches degenerate sessions where the agent
    /// dumps every workload-entry pattern as a candidate. Defaults
    /// to 20.
    #[serde(default)]
    pub triage_candidate_soft_cap: Option<usize>,

    /// Directory of operator-tunable context / reference docs (the
    /// agent's "brainstem"). Each entry is a markdown file + a TOML
    /// sidecar declaring which phases may surface it. See
    /// [`crate::context`] for the bundle.
    ///
    /// Resolution when unset: sibling of `prompt_overrides_dir`
    /// (`<parent>/context`) — same derivation rule as
    /// [`Settings::schemas_dir`] / [`Settings::queries_dir`], so the
    /// conventional `.sbagent/prompts` + `.sbagent/context` layout
    /// works without a config change.
    ///
    /// Operator-tunable: `sbagent sync` rewrites only with
    /// `--force-tunables` (or the legacy `--force-prompts`),
    /// `sbagent check` warns (not fails) on drift between the on-disk
    /// copy and the running binary's bundle.
    #[serde(default)]
    pub context_overrides_dir: Option<PathBuf>,

    /// Chainstate source dir (must contain `chainstate/`). Required by
    /// `session baseline run`.
    #[serde(default)]
    pub source_dir: Option<PathBuf>,

    /// Network identifier passed to `cargo stacks-bench`. Defaults to
    /// `mainnet`.
    #[serde(default)]
    pub stacks_bench_network: Option<String>,

    /// Parent directory passed via `--shadow-dir-root <DIR>` to
    /// `stacks-bench bench run`. stacks-bench creates its reflink shadow
    /// copy of the source chainstate under this dir; auto-named +
    /// auto-cleaned. **Must be on the same filesystem as `source_dir`**
    /// (reflinks fail across filesystems and stacks-bench refuses to
    /// proceed). Set this when the default — the source dir's parent —
    /// is not writable from the codex sandbox (e.g. `/Volumes/Extern`).
    /// Unset = pass nothing, let stacks-bench fall back to the source
    /// parent.
    #[serde(default)]
    pub stacks_bench_shadow_dir: Option<PathBuf>,

    /// Block range start.
    #[serde(default)]
    pub stacks_bench_start_at: Option<u64>,

    /// Block range count.
    #[serde(default)]
    pub stacks_bench_count: Option<u64>,

    /// Number of pre-window blocks to advance through before measurement
    /// starts (`--warmup` arg). Lets caches/JIT/IO settle so the
    /// measured `count` reflects steady-state, not start-up. Unset =
    /// no warmup.
    #[serde(default)]
    pub stacks_bench_warmup: Option<u64>,

    /// Optional `--filter` arg for `bench run` (e.g. `contract-call`).
    /// Restricts which blocks in the range are actually replayed —
    /// useful for chainstates with non-canonical forks where iterating
    /// every height trips on missing blocks. Unset = no filter.
    #[serde(default)]
    pub stacks_bench_filter: Option<String>,

    /// Conservative noise-floor fallback used when a single baseline run is
    /// imported (no rerun for noise estimation). Defaults to 1.0%.
    #[serde(default)]
    pub single_run_noise_floor_pct: Option<f64>,

    /// Triage selection-lens weights, e.g. `"0.4,0.4,0.2"` for
    /// `tx_latency,tenure_throughput,commit_time`.
    #[serde(default)]
    pub stacks_bench_axis_weights: Option<String>,

    /// Codex model id (e.g. `gpt-5.5`).
    #[serde(default)]
    pub codex_model: Option<String>,

    /// Codex reasoning effort. Valid values depend on the model family;
    /// for GPT-5.x and GPT-5.x-Codex models, `high` is the workflow's
    /// default.
    #[serde(default)]
    pub codex_reasoning_effort: Option<String>,

    /// Optional override for the merge phase's reasoning effort. Inherits
    /// `codex_reasoning_effort` when unset.
    #[serde(default)]
    pub codex_merge_reasoning_effort: Option<String>,

    /// Set to true on demo VMs that cannot support Codex's internal
    /// sandboxing.
    #[serde(default)]
    pub codex_dangerously_bypass_sandbox: Option<bool>,

    /// Outer timeout, in seconds, for each `codex exec` invocation. `0`
    /// disables. Defaults to 3600 (one hour).
    #[serde(default)]
    pub codex_exec_timeout_sec: Option<u64>,

    /// Maximum number of inner-loop attempts the Phase 2 optimizer may
    /// burn on one merged target before giving up. Combined with
    /// [`Settings::optimizer_budget_minutes`] as "whichever exhausts
    /// first." Default: `5`. Applies to `normal_pr` targets only —
    /// `consensus_poc_pr` keeps the one-shot scoped-tests model.
    #[serde(default)]
    pub optimizer_attempts: Option<u32>,

    /// Wall-clock budget (in minutes) the Phase 2 optimizer may spend on
    /// the inner loop for one merged target. Combined with
    /// [`Settings::optimizer_attempts`] as "whichever exhausts first."
    /// Default: `60`. Codex's outer `codex_exec_timeout_sec` is the
    /// hard kill; this budget is the prompt-level soft cap the agent
    /// self-enforces.
    #[serde(default)]
    pub optimizer_budget_minutes: Option<u32>,

    /// Absolute path to the GitHub PAT file `sbagent` reads at Phase 5
    /// time. Should be mode 0600, owned by the agent user. Defaults to
    /// `${HOME}/.config/sbagent/gh_token`.
    ///
    /// MUST live OUTSIDE the framework root: `publish generate` passes
    /// the framework root to Codex via `--add-dir`, so a token inside
    /// the tree is reachable by the LLM. `sbagent` enforces this at
    /// preflight and at Phase 5 startup; configuring an in-tree path
    /// surfaces an actionable bail rather than a quiet leak.
    #[serde(default)]
    pub publish_token_file: Option<PathBuf>,

    /// Git remote name in the optimizer worktrees (e.g. `origin`).
    #[serde(default)]
    pub publish_remote: Option<String>,

    /// `<owner>/<repo>` PRs and issues are filed against. Defaults to
    /// `cylewitruk/stacks-core` (operator's fork; least blast radius).
    #[serde(default)]
    pub publish_base_repo: Option<String>,

    /// Branch name PRs target. Defaults to `feat/stacks-bench`.
    #[serde(default)]
    pub publish_base_branch: Option<String>,

    /// Whether `normal_pr` PRs are created as drafts. `consensus_poc_pr`
    /// is always draft regardless of this setting. Defaults to true.
    #[serde(default)]
    pub publish_draft_prs: Option<bool>,

    /// Optional labels added to every created PR. Each entry is passed
    /// individually to `gh pr create --label`.
    #[serde(default)]
    pub publish_pr_labels: Option<Vec<String>>,

    /// Branch prefix for optimizer worktrees pushed to GitHub. The full
    /// branch name is `<prefix>/<session-id>/<target-id>`.
    #[serde(default)]
    pub publish_branch_prefix: Option<String>,

    /// Override for the head owner used in `gh pr create --head` (otherwise
    /// derived from the configured remote URL).
    #[serde(default)]
    pub publish_head_owner: Option<String>,

    /// Author identity for every agent-generated commit. Injected as
    /// `GIT_AUTHOR_NAME` + `GIT_COMMITTER_NAME` environment variables
    /// on the optimizer codex invocation (and as a `user.name`
    /// override via `GIT_CONFIG_COUNT`) so it applies to every `git`
    /// invocation inside the agent's process tree WITHOUT mutating any
    /// git config file. Should match the bot account whose PAT lives
    /// in `publish_token_file`. Defaults to `"stacks-bench-bot"` when
    /// unset. See [`crate::session::optimizers::optimizer_git_env`]
    /// for the full env shape.
    #[serde(default)]
    pub git_author_name: Option<String>,

    /// Author email for agent commits. Plumbed the same way as
    /// [`Settings::git_author_name`] (env-var, not git config file).
    /// For GitHub commit attribution, use the bot account's
    /// `<numeric-id>+<username>@users.noreply.github.com` form
    /// (visible in Settings → Emails when "Keep my email addresses
    /// private" is enabled). Falls back to
    /// `"stacks-bench-bot@users.noreply.github.com"` (no numeric
    /// prefix → commits show as "unverified email" on GitHub but still
    /// attribute by name).
    #[serde(default)]
    pub git_author_email: Option<String>,
}

/// Default username for the PAT-via-extraheader Basic credential. Magic
/// value GitHub fine-grained PATs accept; works on most other forges too
/// (GitLab/Bitbucket may want a different value — see
/// [`Settings::git_auth_username`]).
pub const DEFAULT_GIT_AUTH_USERNAME: &str = "x-access-token";

/// Default URL prefix the PAT-via-extraheader auth is scoped to. Same
/// host pattern `init --push` and `--seed-from` validate against.
pub const DEFAULT_GIT_AUTH_URL_PREFIX: &str = "https://github.com/";

/// Normalize an operator-supplied URL prefix: when non-empty, ensure it
/// ends with `/` so `https://gitlab.com` and `https://gitlab.com/`
/// resolve identically. Defends against typosquat-style attacks where a
/// missing trailing slash would let `https://gitlab.com.evil.example/`
/// pass `starts_with("https://gitlab.com")`. Empty input (expert /
/// unqualified mode) is returned as-is.
pub fn normalize_auth_url_prefix(raw: &str) -> String {
    if raw.is_empty() || raw.ends_with('/') { raw.to_owned() } else { format!("{raw}/") }
}

/// Operator-side default for `schemas_dir`, applied **identically** by
/// `sbagent init` (for seeding + initial-commit staging) and
/// `Layout::from_settings` (for runtime resolution). Single source of
/// truth so an operator can't end up with one schemas dir committed
/// and a different one created at runtime.
///
/// Returns:
/// 1. `Settings::schemas_dir` verbatim when set.
/// 2. Sibling of `prompt_overrides_dir`: `<parent>/schemas` for the
///    conventional `.sbagent/prompts` → `.sbagent/schemas` layout. For a
///    bare-filename `prompt_overrides_dir` (e.g. `"prompts"`, no parent),
///    returns `"schemas"` next to it.
/// 3. `None` when `prompt_overrides_dir` is also unset — callers fall back to
///    whatever path makes sense (framework-side `<framework>/schemas` for
///    [`crate::layout::Layout`], the conventional `.sbagent/schemas` for
///    tooling that lacks framework context).
pub fn default_schemas_dir(settings: &Settings) -> Option<PathBuf> {
    default_bundle_sibling(settings.schemas_dir.as_ref(), settings, "schemas")
}

/// Operator-side default for `queries_dir`. Same shape + contract as
/// [`default_schemas_dir`]; see [`Settings::queries_dir`] for the
/// full doc.
pub fn default_queries_dir(settings: &Settings) -> Option<PathBuf> {
    default_bundle_sibling(settings.queries_dir.as_ref(), settings, "queries")
}

/// Operator-side default for `context_overrides_dir`. Same shape +
/// contract as [`default_schemas_dir`]; see
/// [`Settings::context_overrides_dir`] for the full doc.
pub fn default_context_dir(settings: &Settings) -> Option<PathBuf> {
    default_bundle_sibling(
        settings
            .context_overrides_dir
            .as_ref(),
        settings,
        "context",
    )
}

/// Operator-side default for `memory_dir`. Same sibling-of-tunables
/// derivation as [`default_schemas_dir`]; see [`Settings::memory_dir`]
/// for the full doc.
pub fn default_memory_dir(settings: &Settings) -> Option<PathBuf> {
    default_bundle_sibling(settings.memory_dir.as_ref(), settings, "memory")
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
    /// Effective [`Settings::git_auth_username`], with the
    /// `x-access-token` default applied.
    pub fn effective_git_auth_username(&self) -> &str {
        self.git_auth_username
            .as_deref()
            .unwrap_or(DEFAULT_GIT_AUTH_USERNAME)
    }

    /// Effective [`Settings::git_auth_url_prefix`], normalized to always
    /// end in `/` when non-empty. Defaults to `https://github.com/` when
    /// unset; an explicit empty string is preserved (expert mode).
    ///
    /// Rejects non-empty prefixes that aren't `https://...` — the
    /// PAT-via-extraheader mechanism only sends the Basic credential
    /// over HTTPS, and `http.<prefix>.extraheader` doesn't apply to
    /// `ssh://` / SCP-style remotes. A typo'd `http://` prefix would
    /// silently leak the PAT over plaintext, so we fail fast here
    /// rather than relying on the destination-URL gate alone. Empty
    /// prefix (expert mode) skips this check; the destination URL is
    /// independently required to be HTTPS by
    /// [`crate::cli::init::run`].
    pub fn effective_git_auth_url_prefix(&self) -> Result<String> {
        let prefix = match self
            .git_auth_url_prefix
            .as_deref()
        {
            Some(s) => normalize_auth_url_prefix(s),
            None => DEFAULT_GIT_AUTH_URL_PREFIX.to_owned(),
        };
        if !prefix.is_empty() && !prefix.starts_with("https://") {
            anyhow::bail!(
                "`git_auth_url_prefix = {:?}` is invalid: a non-empty prefix MUST be an HTTPS URL \
                 (the PAT-via-extraheader mechanism only sends the Basic credential over HTTPS, \
                 and `http.<prefix>.extraheader` cannot apply to SSH or other non-HTTP remotes). \
                 Either set it to your forge's `https://...` root (e.g. `https://gitlab.com/`) or \
                 set it to `\"\"` for expert / unqualified mode.",
                self.git_auth_url_prefix
                    .as_deref()
                    .unwrap_or(""),
            );
        }
        Ok(prefix)
    }

    /// Resolve [`Settings::prompt_overrides_dir`] or return a clear error.
    /// Used at every render callsite — sbagent loads prompts from disk now,
    /// so this path is required for any phase that invokes an agent.
    pub fn require_prompt_overrides_dir(&self) -> Result<&Path> {
        self.prompt_overrides_dir
            .as_deref()
            .context(
                "`prompt_overrides_dir` not set in config; required since sbagent loads prompts \
                 from disk. Typical operator value: `.sbagent/prompts/`. sbagent seeds bundled \
                 defaults into this dir on startup; the operator can edit them in place.",
            )
    }

    /// Load settings from a TOML file. Path resolution, in order of
    /// precedence (first match wins):
    ///
    /// 1. **`--config-path <path>`** (the `path` argument). Must exist. Use
    ///    this for non-standard layouts or in tests.
    /// 2. **`./config.toml`** in cwd. Backward-compat for operators whose
    ///    config currently sits next to their `sessions/` dir.
    /// 3. **`$XDG_CONFIG_HOME/sbagent/config.toml`** (Linux + macOS operators
    ///    who set `XDG_CONFIG_HOME` explicitly).
    /// 4. **`$HOME/.config/sbagent/config.toml`** (macOS default; also the
    ///    Linux fallback when `XDG_CONFIG_HOME` is unset).
    ///
    /// If none match, returns [`Settings::default`] — every field `None`.
    /// That's almost certainly not what an operator wants, but it lets
    /// non-session commands (`sbagent prompt lint`, `sbagent schema
    /// export`) run without a config in place, which is occasionally
    /// useful in CI / scratch contexts.
    ///
    /// Deserialize errors (typo'd field, wrong type) are surfaced rather
    /// than swallowed — for a deployment tool, silently dropping a config
    /// is a footgun.
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
        cfg.try_deserialize::<Settings>()
            .with_context(|| format!("parsing {}", resolved.unwrap().display()))
    }

    /// Apply the resolution order documented on [`Settings::load`] and
    /// return the first config path that exists, or `None`.
    pub fn resolve_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
        if let Some(p) = explicit {
            return Some(p.to_path_buf());
        }
        let cwd_local = Path::new("config.toml");
        if cwd_local.is_file() {
            return Some(cwd_local.to_path_buf());
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
    fn load_returns_default_when_no_path_and_no_cwd_config() {
        // Use a tmp cwd guaranteed to lack config.toml AND scrub
        // XDG_CONFIG_HOME / HOME so the new resolution order (which
        // checks both fallbacks) doesn't pick up the developer
        // machine's `~/.config/sbagent/config.toml`. SAFETY: see the
        // `resolve_config_path_precedence_order` test for the
        // env-var-in-tests rationale (single-thread per-test).
        let tmp = tempfile::tempdir().unwrap();
        let prev_cwd = std::env::current_dir().unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");
        std::env::set_current_dir(tmp.path()).unwrap();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("HOME");
        }
        let result = Settings::load(None);
        std::env::set_current_dir(prev_cwd).unwrap();
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
                .framework_root
                .is_none()
        );
        assert!(settings.codex_model.is_none());
        assert!(
            settings
                .publish_token_file
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
                source_dir = "/mnt/chainstate/mainnet"
                stacks_bench_start_at = 5_000_000
                stacks_bench_count = 200_000
                publish_token_file = "/etc/sbagent/gh_token"
                publish_pr_labels = ["needs-bench-review", "auto-generated"]
                publish_draft_prs = false
            "#,
        )
        .unwrap();
        let s = Settings::load(Some(&path)).expect("load");
        assert_eq!(s.source_dir, Some(PathBuf::from("/mnt/chainstate/mainnet")));
        assert_eq!(s.stacks_bench_start_at, Some(5_000_000));
        assert_eq!(s.stacks_bench_count, Some(200_000));
        assert_eq!(s.publish_token_file, Some(PathBuf::from("/etc/sbagent/gh_token")));
        assert_eq!(s.publish_draft_prs, Some(false));
        let labels = s
            .publish_pr_labels
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
        assert_eq!(s.effective_git_auth_username(), "x-access-token");
        assert_eq!(
            s.effective_git_auth_url_prefix()
                .unwrap(),
            "https://github.com/"
        );
    }

    #[test]
    fn effective_git_auth_normalizes_operator_prefix() {
        let s = Settings {
            git_auth_url_prefix: Some("https://gitlab.example".into()),
            git_auth_username: Some("oauth2".into()),
            ..Settings::default()
        };
        // Trailing slash appended so `starts_with(prefix)` cannot be defeated
        // by a typosquat sibling host.
        assert_eq!(
            s.effective_git_auth_url_prefix()
                .unwrap(),
            "https://gitlab.example/"
        );
        assert_eq!(s.effective_git_auth_username(), "oauth2");
    }

    #[test]
    fn effective_git_auth_preserves_empty_expert_mode() {
        let s = Settings {
            git_auth_url_prefix: Some(String::new()),
            ..Settings::default()
        };
        assert_eq!(
            s.effective_git_auth_url_prefix()
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
            schemas_dir: Some(PathBuf::from("/abs/custom")),
            prompt_overrides_dir: Some(PathBuf::from(".sbagent/prompts")),
            ..Settings::default()
        };
        assert_eq!(default_schemas_dir(&s), Some(PathBuf::from("/abs/custom")));

        // 2. Conventional sibling layout.
        let s = Settings {
            prompt_overrides_dir: Some(PathBuf::from(".sbagent/prompts")),
            ..Settings::default()
        };
        assert_eq!(default_schemas_dir(&s), Some(PathBuf::from(".sbagent/schemas")));

        // 3. Bare filename → next to it (NOT under .sbagent/).
        // This is the case Codex flagged: init.rs and layout.rs used to
        // disagree here. The shared helper makes them agree.
        let s = Settings {
            prompt_overrides_dir: Some(PathBuf::from("prompts")),
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
                git_auth_url_prefix: Some(bad.into()),
                ..Settings::default()
            };
            let err = s
                .effective_git_auth_url_prefix()
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

    /// `resolve_config_path` walks four locations in order; the
    /// explicit `--config-path` arg wins, then a cwd-local
    /// `./config.toml`, then `$XDG_CONFIG_HOME/sbagent/config.toml`,
    /// then `$HOME/.config/sbagent/config.toml`. This test exercises
    /// all four paths from a single test body so the env-var
    /// manipulation (global state) doesn't race other tests.
    #[test]
    fn resolve_config_path_precedence_order() {
        let tmp = tempfile::tempdir().unwrap();
        let explicit = tmp
            .path()
            .join("explicit.toml");
        let cwd_local_dir = tmp
            .path()
            .join("cwd-with-local");
        let cwd_empty_dir = tmp.path().join("cwd-empty");
        let xdg_root = tmp.path().join("xdg");
        let home_root = tmp.path().join("home");
        for d in [&cwd_local_dir, &cwd_empty_dir, &xdg_root, &home_root] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&explicit, "").unwrap();
        std::fs::write(cwd_local_dir.join("config.toml"), "").unwrap();
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

        // Snapshot env so we can restore.
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");
        let prev_cwd = std::env::current_dir().unwrap();

        // 1. Explicit beats everything else.
        std::env::set_current_dir(&cwd_local_dir).unwrap();
        // SAFETY: changing process-wide env vars is safe here because
        // cargo nextest runs each test in its own process; within a
        // single test body these `set_var` calls don't race other
        // tests. The cargo-test runner (without nextest) would race,
        // but that's out of scope for this project's testing setup.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &xdg_root);
            std::env::set_var("HOME", &home_root);
        }
        assert_eq!(
            Settings::resolve_config_path(Some(&explicit)),
            Some(explicit.clone()),
            "explicit --config-path must beat all fallbacks",
        );

        // 2. cwd-local beats XDG + HOME.
        let got = Settings::resolve_config_path(None);
        assert_eq!(
            got.as_deref()
                .map(|p| p.file_name().unwrap()),
            Some(std::ffi::OsStr::new("config.toml")),
            "cwd-local config.toml must beat XDG/HOME",
        );
        // (got is "config.toml" relative — fine; it resolves under cwd.)

        // 3. With cwd empty, XDG beats HOME.
        std::env::set_current_dir(&cwd_empty_dir).unwrap();
        assert_eq!(
            Settings::resolve_config_path(None),
            Some(xdg_path.clone()),
            "XDG_CONFIG_HOME/sbagent/config.toml must beat $HOME fallback",
        );

        // 4. With XDG unset, HOME fallback wins.
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        assert_eq!(
            Settings::resolve_config_path(None),
            Some(home_path.clone()),
            "$HOME/.config/sbagent/config.toml must be the last fallback",
        );

        // 5. With neither XDG nor HOME and no cwd-local: None.
        unsafe {
            std::env::remove_var("HOME");
        }
        assert_eq!(
            Settings::resolve_config_path(None),
            None,
            "with no env + no cwd-local, resolution returns None",
        );

        // Restore env.
        std::env::set_current_dir(&prev_cwd).unwrap();
        unsafe {
            if let Some(v) = prev_xdg {
                std::env::set_var("XDG_CONFIG_HOME", v);
            }
            if let Some(v) = prev_home {
                std::env::set_var("HOME", v);
            }
        }
    }
}
