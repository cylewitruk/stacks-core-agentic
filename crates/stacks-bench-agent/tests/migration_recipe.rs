//! Migration recipe rehearsal.
//!
//! Walks the operator-side migration recipe from `docs/setup.md`
//! ("Migrating a pre-v3 operator dir") against a synthetic
//! pre-cutover operator directory, asserting the converged shape.
//!
//! Per step, the test uses whichever invocation best mirrors what
//! the operator's shell would do:
//!
//! - Git steps (init, submodule add, deinit/rm, commit) shell out to
//!   `std::process::Command::new("git")`.
//! - `sbagent source cache-id` (step 3) runs the compiled
//!   `CARGO_BIN_EXE_sbagent` binary.
//! - The `$EDITOR config.toml` step (step 2) is a programmatic rewrite — the
//!   test is a fixture, not an interactive operator.
//! - `rm -rf` / `rm -f` / `rmdir` steps (step 4) use `std::fs::remove_*`
//!   directly; the operator's shell would do the same syscalls with different
//!   syntax.
//!
//! If a recipe step's exact command ever needs adjusting, update
//! both this test and `docs/setup.md` so they stay in lockstep.
//!
//! Runs entirely offline: the "upstream" repo is a `git init --bare`
//! tempdir, accessed via `file://` URLs.

use std::path::{Path, PathBuf};

fn run_git(dir: &Path, args: &[&str]) {
    run_git_with_env(dir, args, &[])
}

fn run_git_with_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) {
    let mut cmd = std::process::Command::new("git");
    // Force commit/tag signing off across the whole test — the
    // operator's local `~/.gitconfig` may enable it (the
    // stacks-bench-bot project does), and the fixture has no signing
    // key. Test commits must be unconditionally bot-authored, not
    // GPG-attested.
    cmd.args(["-c", "commit.gpgsign=false", "-c", "tag.gpgSign=false"]);
    cmd.args(args)
        .current_dir(dir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("spawning git {args:?}: {e}"));
    assert!(status.success(), "git {:?} failed in {}", args, dir.display());
}

fn run_git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("spawning git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .to_owned()
}

/// Spin up a tiny "upstream" bare repo containing one branch with one
/// commit. Returns the bare-repo path; the branch is `feat/stacks-bench`.
fn make_upstream_bare(tmp_root: &Path) -> PathBuf {
    let bare = tmp_root.join("upstream.git");
    let seed = tmp_root.join("upstream-seed");
    std::fs::create_dir_all(&seed).unwrap();

    // git init the seed, write a marker, commit, then push --mirror.
    run_git(&seed, &["init", "-b", "feat/stacks-bench"]);
    run_git(&seed, &["config", "user.name", "Upstream Author"]);
    run_git(&seed, &["config", "user.email", "upstream@example.com"]);
    std::fs::write(seed.join("README.md"), "stacks-core fixture\n").unwrap();
    run_git(&seed, &["add", "README.md"]);
    run_git(&seed, &["commit", "-m", "seed"]);

    run_git(tmp_root, &["init", "--bare", "upstream.git"]);
    let bare_url = format!("file://{}", bare.display());
    run_git(&seed, &["push", &bare_url, "feat/stacks-bench"]);

    bare
}

/// Materialize a synthetic pre-cutover operator dir: `git init`, add
/// `repos/stacks-core` as a submodule pointing at `upstream_bare`,
/// commit `.gitmodules` + submodule pointer, seed `.sbagent/` from
/// the bundled binary, drop a `config.toml` carrying the legacy
/// `[stacks_core]` stanza.
///
/// Returns `(operator_dir, prompts_dir)`.
fn stage_pre_cutover_operator(tmp_root: &Path, upstream_bare: &Path) -> (PathBuf, PathBuf) {
    let operator = tmp_root.join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    run_git(&operator, &["init", "-b", "main"]);
    run_git(&operator, &["config", "user.name", "Pre-Cutover Operator"]);
    run_git(&operator, &["config", "user.email", "operator@example.com"]);
    // Allow file:// submodule transport so `git submodule add file://...`
    // doesn't fall over on git ≥ 2.38 (the security default rejects
    // file-transport submodules).
    let upstream_url = format!("file://{}", upstream_bare.display());
    let env = [("GIT_ALLOW_PROTOCOL", "file:https:ssh")];
    run_git_with_env(
        &operator,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-b",
            "feat/stacks-bench",
            &upstream_url,
            "repos/stacks-core",
        ],
        &env,
    );
    run_git(&operator, &["add", ".gitmodules", "repos/stacks-core"]);
    run_git(&operator, &["commit", "-m", "chore: add stacks-core submodule"]);

    // Seed the `.sbagent/` bundle from the binary's embedded bundle so
    // the migration recipe's final `sbagent check` would find it.
    let sbagent = operator.join(".sbagent");
    std::fs::create_dir_all(&sbagent).unwrap();
    let prompts = sbagent.join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    stacks_bench_agent::prompts::seed_to(&prompts).unwrap();
    stacks_bench_agent::schemas::seed_to(&sbagent.join("schemas")).unwrap();
    stacks_bench_agent::queries::seed_to(&sbagent.join("queries")).unwrap();
    stacks_bench_agent::context::seed_to(&sbagent.join("context")).unwrap();

    // Pre-cutover config: carries `[stacks_core]`, no `[source]`.
    // The `[layout].agent_workspace_root` is what step 3's `mkdir
    // -p <workspace>/cache` references.
    let workspace = tmp_root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let config_path = operator.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[layout]\nprompt_overrides_dir = \"{}\"\nagent_workspace_root = \
             \"{}\"\n\n[stacks_core]\nbase = \"repos/stacks-core\"\nbase_repo_url = \"{}\"\n",
            prompts.display(),
            workspace.display(),
            upstream_url,
        ),
    )
    .unwrap();

    (operator, prompts)
}

/// Step 2 of the recipe: rewrite `config.toml` — drop `[stacks_core]`,
/// add `[source]`. The shell recipe says `$EDITOR config.toml`; the
/// test does the same edit programmatically.
fn rewrite_config_for_post_cutover(
    operator: &Path,
    upstream_url: &str,
    prompts: &Path,
    workspace: &Path,
) {
    let config_path = operator.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[layout]\nprompt_overrides_dir = \"{}\"\nagent_workspace_root = \
             \"{}\"\n\n[source]\nurl = \"{}\"\nbranch = \"feat/stacks-bench\"\n",
            prompts.display(),
            workspace.display(),
            upstream_url,
        ),
    )
    .unwrap();
}

/// Run `sbagent source cache-id` against the operator's `config.toml`
/// and return the printed id (the value step 3 uses for the cache
/// dir name).
fn exec_source_cache_id(config_path: &Path) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sbagent"))
        .args(["-c", config_path.to_str().unwrap(), "source", "cache-id"])
        .output()
        .expect("spawn sbagent source cache-id");
    assert!(
        out.status.success(),
        "sbagent source cache-id failed:\nstderr={}\nstdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn migration_recipe_converges_pre_cutover_operator_to_post_cutover() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream_bare = make_upstream_bare(tmp.path());
    let (operator, prompts) = stage_pre_cutover_operator(tmp.path(), &upstream_bare);
    let workspace = tmp.path().join("workspace");
    let upstream_url = format!("file://{}", upstream_bare.display());

    // ── Step 1: confirm clean state on operator main + submodule. ──
    // Smoke check that the fixture is well-formed before the recipe runs.
    run_git(&operator, &["status"]);
    run_git(
        &operator
            .join("repos")
            .join("stacks-core"),
        &["status"],
    );

    // ── Step 2: rewrite config.toml. ──
    rewrite_config_for_post_cutover(&operator, &upstream_url, &prompts, &workspace);
    let config_path = operator.join("config.toml");
    let post_config = std::fs::read_to_string(&config_path).unwrap();
    assert!(post_config.contains("[source]"), "config.toml must gain [source]");
    assert!(!post_config.contains("[stacks_core]"), "config.toml must lose [stacks_core]",);

    // ── Step 3: seed bare cache from existing submodule. ──
    let cache_dir = workspace.join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let cache_id = exec_source_cache_id(&config_path);
    assert!(!cache_id.is_empty(), "cache id must be non-empty");
    let cache_path = cache_dir.join(format!("{cache_id}.git"));
    run_git(
        tmp.path(),
        &[
            "clone",
            "--bare",
            "--local",
            operator
                .join("repos")
                .join("stacks-core")
                .to_str()
                .unwrap(),
            cache_path.to_str().unwrap(),
        ],
    );
    assert!(
        cache_path
            .join("HEAD")
            .is_file(),
        "bare cache must be a real bare repo at {}",
        cache_path.display(),
    );

    // ── Step 4: remove submodule from operator dir. ──
    // `git submodule deinit -f` may fail in tests because the fixture
    // submodule's `.git/config` lookup falls back oddly; the recipe's
    // intent is "drop the submodule from operator state" so use the
    // explicit equivalents: `git rm`, `rm -rf .git/modules/...`, drop
    // `.gitmodules`.
    run_git(&operator, &["submodule", "deinit", "-f", "repos/stacks-core"]);
    run_git(&operator, &["rm", "-rf", "repos/stacks-core"]);
    let modules_dir = operator
        .join(".git")
        .join("modules")
        .join("repos")
        .join("stacks-core");
    if modules_dir.exists() {
        std::fs::remove_dir_all(&modules_dir).unwrap();
    }
    let gitmodules = operator.join(".gitmodules");
    if gitmodules.exists() {
        std::fs::remove_file(&gitmodules).unwrap();
    }
    let repos_dir = operator.join("repos");
    if repos_dir.exists() {
        // `rmdir` analog: remove only if empty (mirrors the recipe's
        // `rmdir ... || true`).
        let _ = std::fs::remove_dir(&repos_dir);
    }

    // ── Step 5: commit removal as the bot identity. ──
    run_git(
        &operator,
        &[
            "-c",
            "user.name=Stacks BenchBot",
            "-c",
            "user.email=bot@example.com",
            "commit",
            "-am",
            "migrate: drop repos/stacks-core submodule (v3 cutover)",
        ],
    );

    // ── Converged shape assertions. ──
    assert!(
        !operator
            .join("repos")
            .exists(),
        "operator/repos/ subtree must be gone post-migration",
    );
    assert!(!gitmodules.exists(), ".gitmodules must be gone post-migration");
    assert!(
        cache_path
            .join("HEAD")
            .is_file(),
        "bare cache must persist at {}",
        cache_path.display(),
    );

    let log = run_git_stdout(&operator, &["log", "--format=%an <%ae>: %s"]);
    let migrate_line = log
        .lines()
        .find(|l| l.contains("migrate: drop repos/stacks-core submodule"))
        .unwrap_or_else(|| panic!("operator-main log missing migration commit; got:\n{log}"));
    assert!(
        migrate_line.contains("Stacks BenchBot <bot@example.com>"),
        "migration commit must be authored as the bot; got line:\n{migrate_line}",
    );

    // ── Step 6 (smoke): cache-id helper still resolves to the same id ──
    // post-migration. This is the recipe's final sanity check.
    let cache_id_after = exec_source_cache_id(&config_path);
    assert_eq!(
        cache_id, cache_id_after,
        "cache id must be stable across the migration (it's derived from `[source].url`)",
    );
}
