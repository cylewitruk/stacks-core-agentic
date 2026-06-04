//! On-disk layout abstractions.
//!
//! Paths the tool needs at runtime:
//!
//! - [`FrameworkDir`] — read-only inputs (`prompts/`, `queries/`, `schemas/`).
//!   Equivalent to `FRAMEWORK_ROOT`. **Optional**: the tool checkout itself,
//!   needed only by commands that touch tool-developer-only artifacts (e.g.
//!   `sbagent schema export` writing back to the committed
//!   `<framework>/schemas/`, or `sbagent check`'s
//!   typed-model-vs-committed-schema drift gate). Operator deployments don't
//!   need it — schemas are bundled into the binary and seeded into
//!   [`Layout::schemas_dir`] under the operator's `.sbagent/schemas/`.
//! - [`Layout::base`] — the operator-provided stacks-core checkout used to
//!   build the `stacks-bench` binary and source per-target worktrees from.
//!   **Required**; no default. Typically a submodule at
//!   `<operator_root>/repos/stacks-core/`.
//! - [`Layout::sessions_root`] — writable sessions tree. Equivalent to
//!   `OPT_SESSIONS_ROOT`. Defaults to `<framework>/sessions`.
//! - [`Layout::stacks_bench_data_dir`] — persistent stacks-bench app-data dir
//!   (owned by the `stacks-bench` binary, not by `sbagent`). Defaults to
//!   `<framework>/data/stacks-bench`.
//! - [`Layout::bench_lock`] / [`Layout::test_lock`] — sbagent's own
//!   coordination lockfiles. Both live under a single `lock_dir` (default
//!   `<framework>/data/run/`) so sbagent's coordination state stays cleanly
//!   separated from `stacks-bench`'s data dir. The two filenames inside
//!   `lock_dir` are not separately configurable.
//!
//! Splitting them at the type level prevents accidental writes into the
//! framework checkout and makes test isolation cheap (point each writable
//! path at a tempdir).

use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::settings::Settings;
use crate::types::SessionId;

/// Read-only framework checkout root: where the tool's bundled prompts,
/// queries, and schemas live. Does NOT contain the stacks-core target —
/// that's operator-owned and exposed via [`Layout::base`].
#[derive(Debug, Clone)]
pub struct FrameworkDir(PathBuf);

impl FrameworkDir {
    /// Wrap an absolute path. Caller is responsible for ensuring the path
    /// actually points at a framework checkout; structural validation lives
    /// in [`Layout::from_settings`].
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// Root of the framework checkout (the workspace dir that owns
    /// `Cargo.toml` + `target/`). Used by session-start preflight to
    /// locate `target/release/sbagent` for the installed-binary
    /// drift check.
    pub fn root(&self) -> &Path {
        &self.0
    }

    /// `<framework>/prompts`.
    pub fn prompts_dir(&self) -> PathBuf {
        self.0.join("prompts")
    }

    /// `<framework>/queries`.
    pub fn queries_dir(&self) -> PathBuf {
        self.0.join("queries")
    }

    /// `<framework>/schemas` — committed JSON Schema files. Authoritative for
    /// drift checking against the typed models in [`crate::models`].
    pub fn schemas_dir(&self) -> PathBuf {
        self.0.join("schemas")
    }
}

impl Deref for FrameworkDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for FrameworkDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// Resolved on-disk layout. Built once at CLI startup from [`Settings`] and
/// passed via the CLI context to every command. Mirrors the bash
/// `.env.example`: framework root + two independent writable paths
/// (`OPT_SESSIONS_ROOT` and `STACKS_BENCH_DATA_DIR`).
#[derive(Debug, Clone)]
pub struct Layout {
    /// Read-only framework checkout root. **Optional**: only commands
    /// that touch tool-developer-only artifacts (e.g. `sbagent schema
    /// export` writing back to the committed `<framework>/schemas/`,
    /// the typed-model-vs-committed-schema drift gate inside `sbagent
    /// check`) require it. Operator deployments don't — schemas live
    /// at [`Layout::schemas_dir`] (on-disk mirror of the embedded
    /// bundle). Callers that need it use
    /// [`Layout::require_framework_dir`].
    pub framework: Option<FrameworkDir>,
    /// Operator's on-disk mirror of the bundled JSON Schemas
    /// (`.sbagent/schemas/` by convention). Resolution: explicit
    /// `LayoutSettings::schemas_dir` → sibling of `prompt_overrides_dir` →
    /// `<framework>/schemas`. Always populated when `Layout` exists,
    /// so consumer code never has to bail on a missing schemas dir.
    /// The dir itself may not exist on disk yet (init/sync materialize it).
    pub schemas_dir: PathBuf,
    /// Operator's on-disk mirror of the bundled SQL queries
    /// (`.sbagent/queries/` by convention). Same resolution rules as
    /// [`Layout::schemas_dir`]. Triage + analyzer agents read SQL
    /// files from here at runtime (via the rendered prompt's
    /// `queries_dir` field), and the coordinator's
    /// [`crate::session::triage_queries::prerender`] reads `.sql`
    /// files from here for the pre-rendered orientation CSVs.
    pub queries_dir: PathBuf,
    /// Operator's on-disk mirror of the bundled context / reference
    /// docs (`.sbagent/context/` by convention). Each entry is a
    /// markdown file paired with a TOML sidecar declaring which phases
    /// may surface it. Same resolution rules as [`Layout::schemas_dir`]
    /// and [`Layout::queries_dir`]: explicit
    /// `LayoutSettings::context_overrides_dir` → sibling of
    /// `prompt_overrides_dir` → `<framework>/context` →
    /// conventional `.sbagent/context`. Phase orchestrators consult
    /// [`crate::context::paths_for_phase`] to resolve which docs apply.
    pub context_dir: PathBuf,
    /// Operator's accumulated cross-session memory dir
    /// (`<operator>/memory/` by convention). Top-level visible (NOT
    /// under `.sbagent/`) because this is accumulated bot
    /// knowledge — not bundled state. Houses the
    /// analyzed-rejections ledger today; future memory types land
    /// here too. See [`crate::analyzed_rejections`].
    pub memory_dir: PathBuf,
    /// Sessions root — `<framework>/sessions` by default. Each session lives
    /// at `<sessions_root>/<id>/results`. Equivalent to bash
    /// `$OPT_SESSIONS_ROOT`.
    pub sessions_root: PathBuf,
    /// Operator git repository root (holds `sessions/`, `repos/`, the
    /// `sessions.jsonl` ledger, and `session/<id>` archive branches).
    /// `None` here is *not* an error at startup; only `sbagent session
    /// archive` (and the `--archive` flag on `session run`) require it.
    /// Callers that need it use [`Layout::require_operator_repo_root`].
    /// Resolution: explicit [`LayoutSettings::operator_repo_root`] → parent of
    /// `sessions_root` (the conventional `<operator>/sessions/` layout)
    /// → `None`.
    pub operator_repo_root: Option<PathBuf>,
    /// Persistent stacks-bench app-data dir, owned by the `stacks-bench`
    /// binary. Defaults to `<framework>/data/stacks-bench`. SQLite db
    /// lives below `<dir>/appdata/stacks-bench.db`. Equivalent to bash
    /// `$STACKS_BENCH_DATA_DIR`.
    pub stacks_bench_data_dir: PathBuf,
    /// Bench-coordination lockfile, derived as `<lock_dir>/benchmark.lock`
    /// (default `<framework>/data/run/benchmark.lock`). Wraps every
    /// `bench run` / `bench rerun` invocation to serialize against the
    /// persistent db.
    pub bench_lock: PathBuf,
    /// Test-coordination lockfile, derived as `<lock_dir>/test.lock`
    /// (default `<framework>/data/run/test.lock`). Used by optimizer
    /// subagents to serialize `cargo nextest` runs across parallel
    /// worktrees.
    pub test_lock: PathBuf,
    /// `BASE` — the operator-provided `stacks-core` checkout that builds the
    /// `stacks-bench` binary and is the source of per-target worktrees.
    /// `None` here is *not* an error at startup (so unrelated commands like
    /// `sbagent prompt lint` can run without a fully-resolved target);
    /// callers that need it call [`Layout::require_base`].
    pub base: Option<PathBuf>,
    /// Optional `--shadow-dir-root <DIR>` passed to `stacks-bench bench
    /// run`. See [`crate::settings::StacksBenchSettings::shadow_dir`].
    /// Always absolutized at Layout construction; `None` means stacks-bench
    /// falls back to its default (the source dir's parent).
    pub stacks_bench_shadow_dir: Option<PathBuf>,
    /// Optional root for mutable agent scratch state (per-target git
    /// clones, etc.). See
    /// [`crate::settings::LayoutSettings::agent_workspace_root`]. Always
    /// absolutized at Layout construction; `None` means optimizer
    /// checkouts fall back to the legacy
    /// `<sessions_root>/<id>/worktrees/` path.
    pub agent_workspace_root: Option<PathBuf>,
}

impl Layout {
    /// Resolve [`Layout::base`] or return a clear error pointing the
    /// operator at the right config field. Use this at every callsite that
    /// builds a worktree, runs a cargo command, or shells out to
    /// `stacks-bench`.
    pub fn require_base(&self) -> Result<&Path> {
        self.base.as_deref().context(
            "`base` (stacks-core checkout path) not set in settings; required for this command. \
             Populate `base` in config.toml — typically `<operator-repo>/repos/stacks-core/`.",
        )
    }

    /// Resolve [`Layout::operator_repo_root`] or return a clear error.
    /// Required by `sbagent session archive` (and `session run
    /// --archive`); not required by any other command.
    pub fn require_operator_repo_root(&self) -> Result<&Path> {
        self.operator_repo_root
            .as_deref()
            .context(
                "`layout.operator_repo_root` not set in settings and could not be auto-derived \
                 (no parent of `layout.sessions_root`). The archive flow needs this — the git \
                 repo that will hold `sessions.jsonl` on `main` and `session/<id>` write-once \
                 branches. Set `layout.operator_repo_root` in config.toml to the absolute path of \
                 your operator repo.",
            )
    }

    /// Resolve [`Layout::framework`] or return a clear error. Use this
    /// only in tool-developer paths (e.g. `sbagent schema export`'s
    /// default `--out`, or the typed-model-vs-committed-schema drift
    /// gate). Operator deployments don't need a framework dir;
    /// schemas live at [`Layout::schemas_dir`].
    pub fn require_framework_dir(&self) -> Result<&FrameworkDir> {
        self.framework
            .as_ref()
            .context(
                "`dev.framework_root` not set in settings and could not be auto-derived (no \
                 ancestor dir with `prompts/` + `schemas/`). This command needs the tool's source \
                 checkout — set `dev.framework_root` in config to your local `stacks-bench-agent` \
                 clone, or run from inside it. Operator commands don't need this; schemas live in \
                 `.sbagent/schemas/` (seeded by `sbagent init` / `sbagent sync`).",
            )
    }
}

impl Layout {
    /// Build a [`Layout`] from settings. Paths are resolved against the
    /// framework root in the same way the bash `.env.example` does:
    /// `OPT_SESSIONS_ROOT="${FRAMEWORK_ROOT}/sessions"` and
    /// `STACKS_BENCH_DATA_DIR="${FRAMEWORK_ROOT}/data/stacks-bench"`.
    /// Missing directories are NOT created here (each command decides when
    /// to materialize subdirectories it owns).
    pub fn from_settings(settings: &Settings) -> Result<Self> {
        // Framework root is now optional. Operator deployments don't need
        // a tool-source checkout; only `sbagent schema export`'s default
        // `--out` and the typed-model-vs-committed drift gate care.
        let framework = absolutize_opt(
            settings
                .dev
                .framework_root
                .clone()
                .or_else(default_framework_root),
        )?;
        // Anchor for writable defaults when framework isn't set: cwd.
        // sbagent never auto-populates dirs at startup, so any
        // command that actually needs these will surface its own
        // require_* error if the resolved path doesn't exist.
        let writable_anchor = framework
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        // Resolve `agent_workspace_root` up front; it informs both the
        // sessions-root default (now) and the optimizer-checkouts dir
        // (downstream). When unset, we still fall back to the legacy
        // cwd / framework-relative `sessions` path so existing
        // operators don't break — but the new-and-recommended layout
        // is `<agent_workspace_root>/sessions/<id>/`, which keeps
        // session bulk OUT of the operator git repo so the `session/<id>`
        // archive branch's tracked files don't get wiped from the
        // working tree on every branch switch.
        let agent_workspace_root = absolutize_opt(
            settings
                .layout
                .agent_workspace_root
                .clone(),
        )?;
        // Distinguish "operator set sessions_root explicitly" from
        // "we defaulted it" — only the explicit case feeds the
        // operator_repo_root parent fallback below.
        let sessions_root_explicit = settings
            .layout
            .sessions_root
            .is_some();
        let sessions_root = absolutize(
            settings
                .layout
                .sessions_root
                .clone()
                .or_else(|| {
                    agent_workspace_root
                        .as_deref()
                        .map(|w| w.join("sessions"))
                })
                .unwrap_or_else(|| writable_anchor.join("sessions")),
        )?;
        // Operator repo root: explicit setting wins; otherwise fall
        // back to `sessions_root.parent()` ONLY when sessions_root
        // was set explicitly (legacy `<operator>/sessions/` layout —
        // parent lands on the operator). When sessions_root was
        // defaulted to `<agent_workspace_root>/sessions/` (new
        // workspace layout) the parent lands on the workspace, NOT
        // the operator, and would silently target the wrong dir;
        // leave operator_repo_root as `None` so
        // `require_operator_repo_root` surfaces a clear config
        // error.
        let operator_repo_root = absolutize_opt(
            settings
                .layout
                .operator_repo_root
                .clone(),
        )?
        .or_else(|| {
            if sessions_root_explicit {
                sessions_root
                    .parent()
                    .map(Path::to_path_buf)
            } else {
                None
            }
        });
        let stacks_bench_data_dir = absolutize(
            settings
                .stacks_bench
                .data_dir
                .clone()
                .unwrap_or_else(|| {
                    writable_anchor
                        .join("data")
                        .join("stacks-bench")
                }),
        )?;
        // Lockfiles live under `<writable_anchor>/data/run/` by default so
        // sbagent's coordination state stays separate from stacks-bench's
        // own data dir. Operators override the directory via `lock_dir`;
        // the two filenames inside it are not configurable separately
        // (there's no use case for split locations).
        let lock_dir = absolutize(
            settings
                .layout
                .lock_dir
                .clone()
                .unwrap_or_else(|| {
                    writable_anchor
                        .join("data")
                        .join("run")
                }),
        )?;
        let bench_lock = lock_dir.join("benchmark.lock");
        let test_lock = lock_dir.join("test.lock");
        // `base` may legitimately be `None` here — unrelated commands
        // (e.g. `sbagent prompt lint`) don't need a stacks-core checkout.
        // Callers that DO need it use `Layout::require_base`.
        let base = absolutize_opt(
            settings
                .stacks_core
                .base
                .clone(),
        )?;
        let stacks_bench_shadow_dir = absolutize_opt(
            settings
                .stacks_bench
                .shadow_dir
                .clone(),
        )?;
        // `agent_workspace_root` is already resolved above (see the
        // sessions-root block). Re-bind for clarity at the
        // construction site below.
        // Schemas dir resolution (operator's on-disk bundle mirror):
        // explicit setting wins; otherwise derive a sibling of
        // `prompt_overrides_dir` (so `.sbagent/prompts` ↔ `.sbagent/schemas`
        // works without a config change); otherwise fall back to the
        // framework's `schemas/` dir (back-compat for operators on the
        // pre-bundle layout).
        // Shared with `sbagent init` via `settings::default_schemas_dir`
        // — both paths MUST agree byte-for-byte so an operator can't
        // commit one schemas dir at init time and then have the
        // running tool create a second one. Layout extends the shared
        // derivation with its framework-side fallback (back-compat for
        // operators still on the pre-bundle layout).
        let schemas_dir = absolutize(
            crate::settings::default_schemas_dir(settings)
                .or_else(|| {
                    framework
                        .as_deref()
                        .map(|fw| fw.join("schemas"))
                })
                .unwrap_or_else(|| PathBuf::from(".sbagent").join("schemas")),
        )?;
        let queries_dir = absolutize(
            crate::settings::default_queries_dir(settings)
                .or_else(|| {
                    framework
                        .as_deref()
                        .map(|fw| fw.join("queries"))
                })
                .unwrap_or_else(|| PathBuf::from(".sbagent").join("queries")),
        )?;
        let context_dir = absolutize(
            crate::settings::default_context_dir(settings)
                .or_else(|| {
                    framework
                        .as_deref()
                        .map(|fw| fw.join("context"))
                })
                .unwrap_or_else(|| PathBuf::from(".sbagent").join("context")),
        )?;
        // Memory dir derivation: explicit `layout.memory_dir` wins
        // verbatim. Otherwise apply the sibling-of-tunables rule used
        // by schemas/queries/context, except the natural sibling is
        // `<operator>/memory/` (top-level, not under `.sbagent/`).
        // When `prompt_overrides_dir = ".sbagent/prompts"`, the
        // sibling helper yields `.sbagent/memory`; lift to
        // `<operator>/memory` for the derived case ONLY. (Lifting an
        // operator's explicit `.sbagent/memory` choice would silently
        // override the config.)
        let memory_dir = absolutize(
            if let Some(explicit) = settings
                .layout
                .memory_dir
                .as_ref()
            {
                explicit.clone()
            } else {
                let derived = crate::settings::default_memory_dir(settings).map(|p| {
                    if p.parent()
                        .and_then(|pp| pp.file_name())
                        .is_some_and(|n| n == ".sbagent")
                    {
                        p.parent()
                            .and_then(|pp| pp.parent())
                            .map_or_else(|| PathBuf::from("memory"), |gp| gp.join("memory"))
                    } else {
                        p
                    }
                });
                derived
                    .or_else(|| {
                        framework
                            .as_deref()
                            .map(|fw| fw.join("memory"))
                    })
                    .unwrap_or_else(|| PathBuf::from("memory"))
            },
        )?;
        Ok(Self {
            framework: framework.map(FrameworkDir::new),
            schemas_dir,
            queries_dir,
            context_dir,
            memory_dir,
            sessions_root,
            stacks_bench_data_dir,
            bench_lock,
            test_lock,
            base,
            stacks_bench_shadow_dir,
            agent_workspace_root,
            operator_repo_root,
        })
    }

    /// Path to the persistent stacks-bench SQLite db.
    pub fn stacks_bench_db_path(&self) -> PathBuf {
        self.stacks_bench_data_dir
            .join("appdata")
            .join("stacks-bench.db")
    }

    /// `<sessions_root>/<id>/results` — the canonical session results dir.
    pub fn session_results_dir(&self, id: &SessionId) -> PathBuf {
        self.sessions_root
            .join(id.as_str())
            .join("results")
    }

    /// `<sessions_root>/<id>/worktrees` — legacy per-target checkout
    /// parent dir, kept only for backward compatibility when
    /// `agent_workspace_root` is unset. New code should call
    /// [`Layout::session_optimizer_checkouts_dir`] instead, which
    /// routes through the workspace root when configured.
    pub fn session_worktrees_dir(&self, id: &SessionId) -> PathBuf {
        self.sessions_root
            .join(id.as_str())
            .join("worktrees")
    }

    /// Parent dir holding the per-target optimizer checkouts for one
    /// session. Each target's clone lands at
    /// `<this>/<target_id>/`. Resolution:
    ///
    /// - If `agent_workspace_root` is set:
    ///   `<agent_workspace_root>/optimizers/<session_id>/`. Routes
    ///   heavy/mutable state outside the operator repo (avoids embedded-repo
    ///   warnings, disk pollution, accidental commits).
    /// - If `agent_workspace_root` is unset (legacy):
    ///   `<sessions_root>/<session_id>/worktrees/`. Backward-compatible default
    ///   so existing operators see no surprise on upgrade.
    ///
    /// Every callsite that builds a per-target checkout path
    /// (`optimizers::run_one`, `optimize clean`, session-end prune,
    /// Phase 3 bench, Phase 5 publish) goes through this method —
    /// no ad-hoc joining of the worktree path elsewhere.
    pub fn session_optimizer_checkouts_dir(&self, id: &SessionId) -> PathBuf {
        if let Some(root) = self
            .agent_workspace_root
            .as_deref()
        {
            root.join("optimizers")
                .join(id.as_str())
        } else {
            self.session_worktrees_dir(id)
        }
    }
}

/// Make `p` absolute. Relative paths are resolved against the current
/// working directory at Layout-construction time (i.e. wherever the user
/// ran `sbagent`). Does NOT require the path to exist — `git worktree
/// add` and the optimizer materialize their destination dirs.
///
/// Why this matters: relative paths in `config.toml` (e.g. `base =
/// "repos/stacks-core"`, `sessions_root = "sessions"`) get passed
/// downstream both to `git -C <base> worktree add <wt>` (which resolves
/// <wt> *under base*) and to `codex exec --cd <wt>` (which resolves it
/// against codex's CWD). Absolutizing once at the Layout boundary makes
/// every consumer agree.
fn absolutize(p: PathBuf) -> Result<PathBuf> {
    if p.is_absolute() {
        return Ok(p);
    }
    let cwd = std::env::current_dir().context("getting current dir to absolutize layout path")?;
    Ok(cwd.join(p))
}

/// `absolutize` lifted over `Option`. `Some(rel)` → `Some(absolute)`;
/// `None` → `None`. The 8+ `.clone().map(absolutize).transpose()?`
/// chains in `from_settings` collapse to one call each.
fn absolutize_opt(p: Option<PathBuf>) -> Result<Option<PathBuf>> {
    p.map(absolutize).transpose()
}

/// Best-effort default for the framework root: walk up from the current cwd
/// looking for a directory containing `prompts/` and `schemas/`. Returns
/// `None` when nothing matches; callers either use the optional value
/// directly or surface a `require_framework_dir` error.
fn default_framework_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut cursor = cwd.as_path();
    loop {
        if cursor
            .join("prompts")
            .is_dir()
            && cursor
                .join("schemas")
                .is_dir()
        {
            return Some(cursor.to_path_buf());
        }
        cursor = cursor.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{DevSettings, LayoutSettings, Settings};

    /// When `agent_workspace_root` is unset, optimizer checkouts fall
    /// back to the legacy `<sessions_root>/<id>/worktrees/` path so
    /// existing operators upgrading from earlier sbagent versions see
    /// no behavioral change.
    #[test]
    fn session_optimizer_checkouts_dir_falls_back_to_legacy_when_unset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let framework = tmp.path().join("framework");
        std::fs::create_dir_all(framework.join("prompts")).unwrap();
        std::fs::create_dir_all(framework.join("schemas")).unwrap();
        let sessions = tmp.path().join("ses");
        let settings = Settings {
            dev: DevSettings {
                framework_root: Some(framework.clone()),
            },
            layout: LayoutSettings {
                sessions_root: Some(sessions.clone()),
                agent_workspace_root: None,
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        let id: SessionId = "20260507-104400"
            .to_owned()
            .try_into()
            .unwrap();
        assert_eq!(
            layout.session_optimizer_checkouts_dir(&id),
            sessions
                .join("20260507-104400")
                .join("worktrees"),
        );
    }

    /// When `agent_workspace_root` is set, optimizer checkouts move
    /// OUT of the operator repo and into
    /// `<root>/optimizers/<session_id>/`. The `optimizers/` subdir
    /// reserves room for future per-phase scratch dirs (analyzers,
    /// merge, publish) without rearranging the operator's setup.
    #[test]
    fn session_optimizer_checkouts_dir_uses_workspace_root_when_set() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let framework = tmp.path().join("framework");
        std::fs::create_dir_all(framework.join("prompts")).unwrap();
        std::fs::create_dir_all(framework.join("schemas")).unwrap();
        let sessions = tmp.path().join("ses");
        let workspace = tmp.path().join("ws");
        let settings = Settings {
            dev: DevSettings {
                framework_root: Some(framework),
            },
            layout: LayoutSettings {
                sessions_root: Some(sessions.clone()),
                agent_workspace_root: Some(workspace.clone()),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        let id: SessionId = "20260507-104400"
            .to_owned()
            .try_into()
            .unwrap();
        let path = layout.session_optimizer_checkouts_dir(&id);
        assert_eq!(
            path,
            workspace
                .join("optimizers")
                .join("20260507-104400")
        );
        // Sanity: NOT under sessions_root.
        assert!(
            !path.starts_with(&sessions),
            "checkouts path must NOT be under sessions_root when workspace_root is set; got \
             {path:?}",
        );
    }

    /// Layout's `schemas_dir` resolution MUST agree with
    /// `settings::default_schemas_dir` for every input — both paths
    /// drive operator-side behavior (`sbagent init` stages via the
    /// helper, `Layout` reads via the helper) and divergence here
    /// produces a phantom second schemas dir at runtime. Covers the
    /// bare-filename case Codex flagged.
    #[test]
    fn layout_schemas_dir_agrees_with_shared_helper_for_bare_prompts_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&cwd).unwrap();

        let settings = Settings {
            layout: LayoutSettings {
                prompt_overrides_dir: Some(PathBuf::from("prompts")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let helper = crate::settings::default_schemas_dir(&settings).expect("Some");
        let layout = Layout::from_settings(&settings).expect("layout");

        std::env::set_current_dir(prev_cwd).unwrap();

        // Helper returns relative; Layout absolutizes against cwd.
        // The resolved paths must be the same after absolutization.
        assert_eq!(helper, PathBuf::from("schemas"));
        // Use canonicalize-or-fallback because /tmp on macOS resolves
        // through /private/tmp; the comparison should be by canonical form.
        let expected = std::fs::canonicalize(&cwd)
            .unwrap_or(cwd.clone())
            .join("schemas");
        assert_eq!(layout.schemas_dir, expected);
    }

    /// `agent_workspace_root` is absolutized at Layout construction
    /// (consistent with the other relative-path settings) so callers
    /// downstream never have to worry about CWD-relative joins.
    #[test]
    fn agent_workspace_root_is_absolutized() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let framework = tmp.path().join("framework");
        std::fs::create_dir_all(framework.join("prompts")).unwrap();
        std::fs::create_dir_all(framework.join("schemas")).unwrap();
        let settings = Settings {
            dev: DevSettings {
                framework_root: Some(framework),
            },
            layout: LayoutSettings {
                sessions_root: Some(tmp.path().join("ses")),
                // Relative path — must come out absolute.
                agent_workspace_root: Some(PathBuf::from("relative/ws")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        let root = layout
            .agent_workspace_root
            .as_deref()
            .expect("set");
        assert!(root.is_absolute(), "agent_workspace_root must be absolutized, got {root:?}");
        assert!(root.ends_with("relative/ws"), "preserves trailing segments, got {root:?}");
    }

    /// Explicit `operator_repo_root` wins over the sessions-parent
    /// fallback. Required because split layouts (e.g. operator repo
    /// containing `data/` and `sessions/` at the same level as the repo
    /// root) need an unambiguous source of truth.
    #[test]
    fn operator_repo_root_uses_explicit_setting_when_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let framework = tmp.path().join("framework");
        std::fs::create_dir_all(framework.join("prompts")).unwrap();
        std::fs::create_dir_all(framework.join("schemas")).unwrap();
        let sessions = tmp.path().join("ses");
        let explicit = tmp.path().join("ops-repo");
        let settings = Settings {
            dev: DevSettings {
                framework_root: Some(framework),
            },
            layout: LayoutSettings {
                sessions_root: Some(sessions.clone()),
                operator_repo_root: Some(explicit.clone()),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        let resolved = layout
            .operator_repo_root
            .as_deref()
            .expect("set");
        assert_eq!(resolved, explicit);
        // Did NOT fall back to sessions_root.parent().
        assert_ne!(resolved, sessions.parent().unwrap());
    }

    /// The sessions-parent fallback for `operator_repo_root` fires
    /// ONLY when `sessions_root` was set explicitly (legacy layout
    /// — parent lands on the operator). When `sessions_root` was
    /// defaulted to `<agent_workspace_root>/sessions/`, the parent
    /// would land on the workspace (not the operator) and silently
    /// target the wrong repo. We require an explicit
    /// `operator_repo_root` in that case.
    #[test]
    fn operator_repo_root_does_not_derive_from_defaulted_workspace_sessions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let framework = tmp.path().join("framework");
        std::fs::create_dir_all(framework.join("prompts")).unwrap();
        std::fs::create_dir_all(framework.join("schemas")).unwrap();
        let workspace = tmp.path().join("ws");
        let settings = Settings {
            dev: DevSettings {
                framework_root: Some(framework),
            },
            layout: LayoutSettings {
                sessions_root: None, // defaulted from agent_workspace_root
                agent_workspace_root: Some(workspace),
                operator_repo_root: None,
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        assert!(
            layout
                .operator_repo_root
                .is_none(),
            "with defaulted sessions_root, operator_repo_root must be None (workspace path is NOT \
             the operator); got {:?}",
            layout.operator_repo_root,
        );
    }

    /// When unset, `operator_repo_root` falls back to the parent of
    /// `sessions_root` — the conventional `<operator>/sessions/`
    /// layout.
    #[test]
    fn operator_repo_root_falls_back_to_sessions_parent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let framework = tmp.path().join("framework");
        std::fs::create_dir_all(framework.join("prompts")).unwrap();
        std::fs::create_dir_all(framework.join("schemas")).unwrap();
        let operator = tmp.path().join("ops-repo");
        let sessions = operator.join("sessions");
        let settings = Settings {
            dev: DevSettings {
                framework_root: Some(framework),
            },
            layout: LayoutSettings {
                sessions_root: Some(sessions),
                operator_repo_root: None,
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        assert_eq!(
            layout
                .operator_repo_root
                .as_deref(),
            Some(operator.as_path())
        );
    }

    /// `require_operator_repo_root` errors clearly when unresolved.
    #[test]
    fn require_operator_repo_root_surfaces_clear_error() {
        let layout = Layout {
            framework: None,
            schemas_dir: PathBuf::from("/x"),
            queries_dir: PathBuf::from("/x"),
            context_dir: PathBuf::from("/x"),
            memory_dir: PathBuf::from("/x"),
            sessions_root: PathBuf::from("/x"),
            stacks_bench_data_dir: PathBuf::from("/x"),
            bench_lock: PathBuf::from("/x"),
            test_lock: PathBuf::from("/x"),
            base: None,
            stacks_bench_shadow_dir: None,
            agent_workspace_root: None,
            operator_repo_root: None,
        };
        let err = layout
            .require_operator_repo_root()
            .unwrap_err()
            .to_string();
        assert!(err.contains("operator_repo_root"), "got: {err}");
    }

    /// Explicit `layout.memory_dir = ".sbagent/memory"` must NOT be
    /// silently rewritten to `<operator>/memory`. The `.sbagent` lift
    /// rule applies to derived defaults only — an operator setting
    /// the field explicitly opts into the literal path.
    #[test]
    fn explicit_memory_dir_under_sbagent_is_honored_verbatim() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let framework = tmp.path().join("framework");
        std::fs::create_dir_all(framework.join("prompts")).unwrap();
        std::fs::create_dir_all(framework.join("schemas")).unwrap();
        let cwd = tmp.path();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(cwd).unwrap();

        let settings = Settings {
            dev: DevSettings {
                framework_root: Some(framework),
            },
            layout: LayoutSettings {
                prompt_overrides_dir: Some(PathBuf::from(".sbagent/prompts")),
                memory_dir: Some(PathBuf::from(".sbagent/memory")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        std::env::set_current_dir(prev_cwd).unwrap();

        // Layout absolutizes against cwd; canonicalize because /tmp on
        // macOS resolves through /private/tmp.
        let expected = std::fs::canonicalize(cwd)
            .unwrap_or_else(|_| cwd.to_path_buf())
            .join(".sbagent")
            .join("memory");
        assert_eq!(layout.memory_dir, expected);
    }

    /// When `memory_dir` is unset and the derived path lands under
    /// `.sbagent/memory`, the lift rule kicks in so memory ends up at
    /// `<operator>/memory` (the conventional location for accumulated
    /// bot knowledge — sibling to `.sbagent/`, not inside it).
    #[test]
    fn derived_memory_dir_lifts_out_of_sbagent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let framework = tmp.path().join("framework");
        std::fs::create_dir_all(framework.join("prompts")).unwrap();
        std::fs::create_dir_all(framework.join("schemas")).unwrap();
        let cwd = tmp.path();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(cwd).unwrap();

        let settings = Settings {
            dev: DevSettings {
                framework_root: Some(framework),
            },
            layout: LayoutSettings {
                prompt_overrides_dir: Some(PathBuf::from(".sbagent/prompts")),
                ..LayoutSettings::default()
            },
            ..Settings::default()
        };
        let layout = Layout::from_settings(&settings).expect("layout");
        std::env::set_current_dir(prev_cwd).unwrap();

        let expected = std::fs::canonicalize(cwd)
            .unwrap_or_else(|_| cwd.to_path_buf())
            .join("memory");
        assert_eq!(layout.memory_dir, expected);
    }
}
