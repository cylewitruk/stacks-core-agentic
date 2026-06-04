//! End-to-end tests for `sbagent init`. Drives a real `git` against a
//! tempdir-rooted local "bot" repo so we exercise submodule add, prompt
//! seeding, .gitignore generation, and the bot-identity commit without
//! touching GitHub.

use std::path::{Path, PathBuf};

use stacks_bench_agent::cli::init::{self, InitArgs};
use stacks_bench_agent::settings::Settings;

/// Stand up a local "bot" git repo with one commit on `feat/test`,
/// reachable as a local file:// URL. Returns the URL string that
/// `base_repo_url` should point at + the branch name.
fn stage_local_remote(tmp: &Path, name: &str, branch: &str) -> (String, String) {
    let dir = tmp.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    run_git(&dir, &["init", "-q", "-b", branch]);
    run_git(&dir, &["config", "user.email", "fake@t"]);
    run_git(&dir, &["config", "user.name", "fake"]);
    run_git(&dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"fake\"\nversion=\"0.0.0\"\n")
        .unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    // `git submodule add` over a local path needs file://-prefixed URLs
    // (or the protocol.file.allow=always config). Use file:// for portability.
    let url = format!("file://{}", dir.display());
    (url, branch.to_owned())
}

fn run_git(dir: &Path, args: &[&str]) {
    stacks_bench_agent::git::run_git(dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e:#}"));
}

fn settings_for(base_dir: PathBuf, base_url: String, branch: String) -> Settings {
    Settings {
        // `init` writes to target_dir but doesn't read framework dirs
        // until later commands. Provide a placeholder so settings load
        // doesn't fail mid-test.
        dev: stacks_bench_agent::settings::DevSettings {
            framework_root: Some(base_dir.join("framework")),
        },
        stacks_core: stacks_bench_agent::settings::StacksCoreSettings {
            base: Some(PathBuf::from("repos/stacks-core")),
            base_repo_url: Some(base_url),
        },
        publish: stacks_bench_agent::settings::PublishSettings {
            base_branch: Some(branch),
            base_repo: Some("test-bot/stacks-core".into()),
            head_owner: Some("test-bot".into()),
            ..stacks_bench_agent::settings::PublishSettings::default()
        },
        layout: stacks_bench_agent::settings::LayoutSettings {
            prompt_overrides_dir: Some(PathBuf::from(".sbagent/prompts")),
            ..stacks_bench_agent::settings::LayoutSettings::default()
        },
        git: stacks_bench_agent::settings::GitSettings {
            author_name: Some("test-bot".into()),
            author_email: Some("test-bot@example".into()),
            ..stacks_bench_agent::settings::GitSettings::default()
        },
        ..Settings::default()
    }
}

/// `sbagent init` produces a usable operator dir from a config + an
/// empty target dir. Asserts on the load-bearing artifacts:
/// - submodule added at the configured `base` path, checked out at the
///   configured branch
/// - bot remote configured on the submodule (so per-target clones inherit it
///   via `recreate_checkout`'s remote replication)
/// - prompt overrides seeded (at least `optimizer.md` ends up on disk)
/// - `.gitignore` written
/// - one bot-authored commit on `main`
#[tokio::test]
async fn init_bootstraps_operator_dir_end_to_end() {
    // Configure git globally to allow `git submodule add file://...`
    // inside this test. `protocol.file.allow=user` is git's default
    // post-CVE-2022-39253; we have to relax it for the submodule add
    // call. Set it only for the test framework's tempdir to avoid
    // touching the operator's real ~/.gitconfig.
    let tmp = tempfile::tempdir().unwrap();
    // Stage a fake stacks-core remote at tmp/remote-stacks-core.
    let (base_url, branch) = stage_local_remote(tmp.path(), "remote-stacks-core", "feat/test");

    // Target dir = where the operator gets initialized.
    let target = tmp.path().join("operator");
    std::fs::create_dir_all(&target).unwrap();

    let settings = settings_for(target.clone(), base_url, branch.clone());

    // Pre-set GIT_CONFIG_COUNT-style env so `git submodule add` is
    // allowed to clone over file://. Wrap the init call in a scope
    // that sets + clears these so the test is self-contained.
    // SAFETY: see settings.rs::resolve_config_path_precedence_order
    // for the env-var-in-tests rationale.
    let prev = std::env::var_os("GIT_CONFIG_COUNT");
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }

    let result = init::run(
        InitArgs {
            dir: Some(target.clone()),
            push: false,
            push_branch: "main".to_owned(),
            seed_from: None,
        },
        &settings,
    )
    .await;

    // Restore env regardless of test outcome.
    unsafe {
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        if let Some(v) = prev {
            std::env::set_var("GIT_CONFIG_COUNT", v);
        } else {
            std::env::remove_var("GIT_CONFIG_COUNT");
        }
    }

    result.expect("init.run must succeed");

    // 1. .git/ created on target.
    assert!(target.join(".git").is_dir(), "target .git/ must exist");

    // 2. submodule present + on the configured branch.
    let sub_path = target
        .join("repos")
        .join("stacks-core");
    assert!(sub_path.exists(), "submodule path must exist: {sub_path:?}");
    let sub_branch = git_stdout(&sub_path, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(sub_branch, branch, "submodule should be checked out on configured branch");

    // 3. submodule has the `bot` remote (derived from publish_head_owner).
    let bot_url = git_stdout(&sub_path, &["remote", "get-url", "bot"]);
    assert_eq!(bot_url, "https://github.com/test-bot/stacks-core.git");

    // 4. prompts seeded.
    let optimizer_md = target
        .join(".sbagent")
        .join("prompts")
        .join("optimizer.md");
    assert!(optimizer_md.is_file(), "optimizer.md should be seeded: {optimizer_md:?}");

    // 4b. schemas seeded to the sibling `.sbagent/schemas/` dir.
    let schemas_dir = target
        .join(".sbagent")
        .join("schemas");
    for name in ["candidates", "analysis", "optimization-targets", "summary"] {
        let path = schemas_dir.join(format!("{name}.schema.json"));
        assert!(path.is_file(), "{name}.schema.json should be seeded: {path:?}");
    }

    // 4c. queries seeded alongside, also stage-tracked.
    let queries_dir = target
        .join(".sbagent")
        .join("queries");
    for name in ["run_summary.sql", "top_spans_by_self_wall.sql", "README.md"] {
        let path = queries_dir.join(name);
        assert!(path.is_file(), "{name} should be seeded: {path:?}");
    }

    // 5. .gitignore written.
    let gitignore = target.join(".gitignore");
    assert!(gitignore.is_file(), ".gitignore should be written");
    let body = std::fs::read_to_string(&gitignore).unwrap();
    assert!(
        body.contains("/config.toml"),
        ".gitignore should defensively exclude /config.toml (legacy / -c convenience copies)"
    );
    assert!(
        body.contains("repos/stacks-core/target/"),
        ".gitignore should exclude submodule target/"
    );

    // 6. one commit on main, authored as the bot.
    let head_branch = git_stdout(&target, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(head_branch, "main");
    let author = git_stdout(&target, &["log", "-1", "--pretty=%an <%ae>"]);
    assert_eq!(
        author, "test-bot <test-bot@example>",
        "commit must be authored by bot via env-vars"
    );
    let subject = git_stdout(&target, &["log", "-1", "--pretty=%s"]);
    assert_eq!(subject, "chore: initial operator state");
}

/// `init` is idempotent over re-runs on a clean tree: prompt seeding
/// reports "kept" instead of "written", `.gitignore` is preserved,
/// no extra commit lands.
#[tokio::test]
async fn init_is_idempotent_on_re_run() {
    let tmp = tempfile::tempdir().unwrap();
    let (base_url, branch) = stage_local_remote(tmp.path(), "remote-stacks-core", "feat/test");
    let target = tmp.path().join("operator");
    std::fs::create_dir_all(&target).unwrap();
    let settings = settings_for(target.clone(), base_url, branch);

    let prev = std::env::var_os("GIT_CONFIG_COUNT");
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }

    let args = InitArgs {
        dir: Some(target.clone()),
        push: false,
        push_branch: "main".to_owned(),
        seed_from: None,
    };
    init::run(args.clone(), &settings)
        .await
        .expect("first init");
    let head_after_first = git_stdout(&target, &["rev-parse", "HEAD"]);
    init::run(args, &settings)
        .await
        .expect("second init must succeed");
    let head_after_second = git_stdout(&target, &["rev-parse", "HEAD"]);

    unsafe {
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        if let Some(v) = prev {
            std::env::set_var("GIT_CONFIG_COUNT", v);
        } else {
            std::env::remove_var("GIT_CONFIG_COUNT");
        }
    }

    assert_eq!(
        head_after_first, head_after_second,
        "re-running init on a clean tree must NOT create a new commit",
    );
}

/// `seed_branch` clones a branch from a source URL and pushes it to a
/// destination URL — the underlying push mechanism `init --seed-from`
/// uses to populate a brand-new bot fork with the substrate branch
/// before `git submodule add` runs.
///
/// Tested against `file://` URLs so we exercise the bare-clone +
/// push-with-PAT-env path without a real GitHub server. The strict
/// `https://github.com/` validation on `--seed-from` lives one level up
/// in `init::run` (covered separately by
/// `init_seed_from_rejects_non_github_base_repo_url`) so we can call
/// `seed_branch` directly here without needing a bypass.
#[tokio::test]
async fn seed_branch_pushes_to_empty_dest_via_local_files() {
    let tmp = tempfile::tempdir().unwrap();
    // 1. Upstream (human's fork): has the substrate branch.
    let (upstream_url, branch) =
        stage_local_remote(tmp.path(), "upstream-stacks-core", "feat/test");
    // 2. Bot fork: bare repo, NO branches.
    let bot_dir = tmp
        .path()
        .join("bot-stacks-core.git");
    std::fs::create_dir_all(&bot_dir).unwrap();
    stacks_bench_agent::git::run_git(&bot_dir, &["init", "--bare"]).unwrap();
    let bot_url = format!("file://{}", bot_dir.display());

    // Pre-seed sanity: bot fork has no refs.
    let pre =
        stacks_bench_agent::git::run_git_output(&bot_dir, &["ls-remote", "--heads", "."]).unwrap();
    assert!(pre.is_empty(), "bot fork pre-seed must be empty");

    // `seed_branch` uses GIT_CONFIG_COUNT-style env vars internally for
    // the auth header. `file://` pushes don't actually check the
    // header, but we still need `protocol.file.allow=always` for the
    // bare clone of upstream. SAFETY: see settings.rs::
    // resolve_config_path_precedence_order test for the
    // env-vars-in-tests rationale.
    let prev = std::env::var_os("GIT_CONFIG_COUNT");
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }

    let result =
        stacks_bench_agent::cli::init::seed_branch(&upstream_url, &bot_url, &branch, "fake-pat");

    unsafe {
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        if let Some(v) = prev {
            std::env::set_var("GIT_CONFIG_COUNT", v);
        } else {
            std::env::remove_var("GIT_CONFIG_COUNT");
        }
    }

    result.expect("seed_branch must succeed against file:// dest");

    // Post: bot fork now has the substrate branch.
    let post_refs =
        stacks_bench_agent::git::run_git_output(&bot_dir, &["ls-remote", "--heads", "."]).unwrap();
    assert!(
        post_refs.contains(&format!("refs/heads/{branch}")),
        "bot fork should have refs/heads/{branch} after seed_branch; got:\n{post_refs}",
    );
}

/// `--push` requires an HTTPS origin. SSH remotes would silently
/// ignore the injected `http.extraheader` and fall back to SSH key auth;
/// init must error up-front rather than producing a confusing
/// "no credentials" git error.
#[tokio::test]
async fn init_push_rejects_ssh_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let (base_url, branch) = stage_local_remote(tmp.path(), "remote-stacks-core", "feat/test");
    let target = tmp.path().join("operator");
    std::fs::create_dir_all(&target).unwrap();
    let mut settings = settings_for(target.clone(), base_url, branch);
    let token_path = tmp.path().join("gh_token");
    std::fs::write(&token_path, "fake-pat").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    settings.publish.token_file = Some(token_path);

    let prev = std::env::var_os("GIT_CONFIG_COUNT");
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }

    // Pre-init the target with an SSH-style origin.
    stacks_bench_agent::git::run_git(&target, &["init", "-b", "main"]).unwrap();
    stacks_bench_agent::git::run_git(
        &target,
        &["remote", "add", "origin", "git@github.com:bot/operator.git"],
    )
    .unwrap();

    let err = init::run(
        InitArgs {
            dir: Some(target.clone()),
            seed_from: None,
            push: true,
            push_branch: "main".to_owned(),
        },
        &settings,
    )
    .await;

    unsafe {
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        if let Some(v) = prev {
            std::env::set_var("GIT_CONFIG_COUNT", v);
        } else {
            std::env::remove_var("GIT_CONFIG_COUNT");
        }
    }

    let err = err.expect_err("--push against SSH origin must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("HTTPS") || msg.contains("https"),
        "error message should explain the HTTPS requirement; got: {msg}",
    );
    assert!(
        msg.contains("git@github.com:bot/operator.git"),
        "error message should include the offending URL; got: {msg}",
    );
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    stacks_bench_agent::git::run_git_output(dir, args)
        .unwrap_or_else(|e| panic!("git {args:?}: {e:#}"))
}

/// `git add -A` would sweep any unrelated file present in the target
/// dir into "chore: initial operator state" — including local edits an
/// operator may have made before running `sbagent init`. init must
/// stage ONLY the paths it created: `.gitignore`, `.gitmodules`,
/// `<base>`, `<prompt_overrides_dir>`. Anything else stays untouched
/// in the working tree, free for the operator to inspect/commit
/// separately.
#[tokio::test]
async fn init_does_not_commit_pre_existing_unrelated_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (base_url, branch) = stage_local_remote(tmp.path(), "remote-stacks-core", "feat/test");
    let target = tmp.path().join("operator");
    std::fs::create_dir_all(&target).unwrap();
    // Pre-populate target with a stray file the operator might have
    // had open in their editor + a stray subdir from a previous build.
    std::fs::write(target.join("stray-file.txt"), "operator's local edit\n").unwrap();
    std::fs::create_dir_all(target.join("scratch")).unwrap();
    std::fs::write(
        target
            .join("scratch")
            .join("notes.md"),
        "TODO\n",
    )
    .unwrap();

    let settings = settings_for(target.clone(), base_url, branch);

    let prev = std::env::var_os("GIT_CONFIG_COUNT");
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }

    init::run(
        InitArgs {
            dir: Some(target.clone()),
            seed_from: None,
            push: false,
            push_branch: "main".to_owned(),
        },
        &settings,
    )
    .await
    .expect("init must succeed even with stray pre-existing files");

    unsafe {
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        if let Some(v) = prev {
            std::env::set_var("GIT_CONFIG_COUNT", v);
        } else {
            std::env::remove_var("GIT_CONFIG_COUNT");
        }
    }

    // The initial commit MUST NOT include stray-file.txt or scratch/.
    // Use `git show --stat` to list paths in the commit.
    let stat = git_stdout(&target, &["show", "--name-only", "--pretty=format:", "HEAD"]);
    let committed: Vec<&str> = stat
        .lines()
        .filter(|l| !l.is_empty())
        .collect();
    assert!(
        !committed.contains(&"stray-file.txt"),
        "stray-file.txt must NOT be in the initial commit; got: {committed:?}",
    );
    assert!(
        !committed
            .iter()
            .any(|p| p.starts_with("scratch/")),
        "scratch/ entries must NOT be in the initial commit; got: {committed:?}",
    );

    // The stray files MUST still exist on disk (we didn't delete them,
    // just didn't commit them).
    assert!(
        target
            .join("stray-file.txt")
            .is_file()
    );
    assert!(
        target
            .join("scratch")
            .join("notes.md")
            .is_file()
    );

    // `git status --porcelain` should still show them as untracked.
    let porcelain = git_stdout(&target, &["status", "--porcelain"]);
    assert!(porcelain.contains("stray-file.txt"), "stray-file.txt should remain untracked");
    assert!(porcelain.contains("scratch/"), "scratch/ should remain untracked");
}

/// `--seed-from` requires `base_repo_url` to be an HTTPS GitHub URL
/// (the destination of the PAT-authenticated push). SSH or non-GitHub
/// HTTPS URLs would silently bypass the auth header — init must
/// reject them up-front, same pattern as the `--push` HTTPS check.
#[tokio::test]
async fn init_seed_from_rejects_non_github_base_repo_url() {
    let tmp = tempfile::tempdir().unwrap();
    let (upstream_url, branch) =
        stage_local_remote(tmp.path(), "upstream-stacks-core", "feat/test");
    let target = tmp.path().join("operator");
    std::fs::create_dir_all(&target).unwrap();
    let mut settings =
        settings_for(target.clone(), "git@github.com:bot/stacks-core.git".into(), branch);
    // Token (required by --seed-from's read_publish_token).
    let token_path = tmp.path().join("gh_token");
    std::fs::write(&token_path, "fake-pat").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    settings.publish.token_file = Some(token_path);

    let err = init::run(
        InitArgs {
            dir: Some(target.clone()),
            seed_from: Some(upstream_url),
            push: false,
            push_branch: "main".to_owned(),
        },
        &settings,
    )
    .await
    .expect_err("--seed-from with SSH base_repo_url must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("https://github.com/") && msg.contains("git@github.com:bot/stacks-core.git"),
        "error should explain the prefix requirement and include the offending URL; got: {msg}",
    );
}

/// Operators on a non-GitHub forge set `git_auth_url_prefix` to their
/// forge's HTTPS root (e.g. `https://gitlab.com/`). `--push` should
/// then reject URLs that don't match THAT prefix — not GitHub's — so a
/// stray GitHub origin against a GitLab-configured operator surfaces
/// up-front rather than silently failing the auth.
#[tokio::test]
async fn init_push_rejects_url_outside_configured_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let (base_url, branch) = stage_local_remote(tmp.path(), "remote-stacks-core", "feat/test");
    let target = tmp.path().join("operator");
    std::fs::create_dir_all(&target).unwrap();
    let mut settings = settings_for(target.clone(), base_url, branch);
    settings.git.auth_url_prefix = Some("https://gitlab.com/".into());
    let token_path = tmp.path().join("gh_token");
    std::fs::write(&token_path, "fake-pat").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    settings.publish.token_file = Some(token_path);

    let prev = std::env::var_os("GIT_CONFIG_COUNT");
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }

    stacks_bench_agent::git::run_git(&target, &["init", "-b", "main"]).unwrap();
    stacks_bench_agent::git::run_git(
        &target,
        &["remote", "add", "origin", "https://github.com/bot/operator.git"],
    )
    .unwrap();

    let err = init::run(
        InitArgs {
            dir: Some(target.clone()),
            seed_from: None,
            push: true,
            push_branch: "main".to_owned(),
        },
        &settings,
    )
    .await;

    unsafe {
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        if let Some(v) = prev {
            std::env::set_var("GIT_CONFIG_COUNT", v);
        } else {
            std::env::remove_var("GIT_CONFIG_COUNT");
        }
    }

    let err = err.expect_err("origin outside configured prefix must error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("https://gitlab.com/") && msg.contains("https://github.com/bot/operator.git"),
        "error must cite configured prefix + offending URL; got: {msg}",
    );
}

/// Trailing-slash normalization: `git_auth_url_prefix = "https://gitlab.com"`
/// (no trailing slash) must NOT accept `https://gitlab.com.evil.example/`.
/// The normalization happens inside [`Settings::effective_git_auth_url_prefix`]
/// so the prefix used for validation is `https://gitlab.com/`, which
/// the typosquat URL doesn't start with.
#[tokio::test]
async fn init_push_normalizes_prefix_against_typosquat() {
    let tmp = tempfile::tempdir().unwrap();
    let (base_url, branch) = stage_local_remote(tmp.path(), "remote-stacks-core", "feat/test");
    let target = tmp.path().join("operator");
    std::fs::create_dir_all(&target).unwrap();
    let mut settings = settings_for(target.clone(), base_url, branch);
    // Operator forgot the trailing slash — normalization MUST handle it.
    settings.git.auth_url_prefix = Some("https://gitlab.com".into());
    let token_path = tmp.path().join("gh_token");
    std::fs::write(&token_path, "fake-pat").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    settings.publish.token_file = Some(token_path);

    let prev = std::env::var_os("GIT_CONFIG_COUNT");
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }

    stacks_bench_agent::git::run_git(&target, &["init", "-b", "main"]).unwrap();
    stacks_bench_agent::git::run_git(
        &target,
        &["remote", "add", "origin", "https://gitlab.com.evil.example/bot/repo.git"],
    )
    .unwrap();

    let err = init::run(
        InitArgs {
            dir: Some(target.clone()),
            seed_from: None,
            push: true,
            push_branch: "main".to_owned(),
        },
        &settings,
    )
    .await;

    unsafe {
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        if let Some(v) = prev {
            std::env::set_var("GIT_CONFIG_COUNT", v);
        } else {
            std::env::remove_var("GIT_CONFIG_COUNT");
        }
    }

    let err = err.expect_err("typosquat origin must be rejected after prefix normalization");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("https://gitlab.com/")
            && msg.contains("https://gitlab.com.evil.example/bot/repo.git"),
        "error must cite normalized prefix + offending typosquat URL; got: {msg}",
    );
}

/// Empty `git_auth_url_prefix` is expert mode: any HTTPS URL is
/// accepted (the operator has audited the remotes git will contact and
/// is comfortable attaching the token to all of them). SSH/file://
/// URLs are STILL rejected — git won't apply `http.extraheader` to
/// them, so failing fast is still better than a confusing "no
/// credentials" error.
#[tokio::test]
async fn init_push_expert_mode_rejects_ssh_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let (base_url, branch) = stage_local_remote(tmp.path(), "remote-stacks-core", "feat/test");
    let target = tmp.path().join("operator");
    std::fs::create_dir_all(&target).unwrap();
    let mut settings = settings_for(target.clone(), base_url, branch);
    // Expert mode: drop the prefix qualifier from the git config key.
    settings.git.auth_url_prefix = Some(String::new());
    let token_path = tmp.path().join("gh_token");
    std::fs::write(&token_path, "fake-pat").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    settings.publish.token_file = Some(token_path);

    let prev = std::env::var_os("GIT_CONFIG_COUNT");
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }

    stacks_bench_agent::git::run_git(&target, &["init", "-b", "main"]).unwrap();
    stacks_bench_agent::git::run_git(
        &target,
        &["remote", "add", "origin", "git@gitlab.example:bot/operator.git"],
    )
    .unwrap();

    let err = init::run(
        InitArgs {
            dir: Some(target.clone()),
            seed_from: None,
            push: true,
            push_branch: "main".to_owned(),
        },
        &settings,
    )
    .await;

    unsafe {
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        if let Some(v) = prev {
            std::env::set_var("GIT_CONFIG_COUNT", v);
        } else {
            std::env::remove_var("GIT_CONFIG_COUNT");
        }
    }

    let err = err.expect_err("expert-mode --push against SSH origin must still error");
    let msg = format!("{err:#}");
    assert!(
        (msg.contains("HTTPS") || msg.contains("https"))
            && msg.contains("git@gitlab.example:bot/operator.git"),
        "error must require HTTPS + cite offending URL even in expert mode; got: {msg}",
    );
}
