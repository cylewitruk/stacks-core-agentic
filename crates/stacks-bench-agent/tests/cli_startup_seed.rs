//! Auto-seeding semantics for `CliContext::from_args`.
//!
//! Contract:
//! - Every command EXCEPT `check` triggers don't-replace-style auto-seed for
//!   schemas (and always for prompts when configured). This keeps operators on
//!   the pre-bundle layout silently bootstrapped.
//! - `check` is the deliberate exception. It must observe the operator dir
//!   verbatim — auto-seeding would mask `schemas::DriftEntry::Missing` and
//!   report OK after a delete that the operator wanted `check` to flag.

use clap::Parser;
use stacks_bench_agent::cli::{CliArgs, CliContext};

/// Build a working CliArgs by parsing argv. `-c <path>` lets us point
/// the context at a tempdir's config.toml without touching the
/// developer's actual `~/.config/sbagent/config.toml`.
fn parse(argv: &[&str]) -> CliArgs {
    CliArgs::try_parse_from(argv).expect("parse CliArgs")
}

/// Write a minimum config.toml + an empty `.sbagent/prompts/` dir so
/// Layout can resolve without a framework_root. Returns the config
/// path + resolved schemas + queries dirs (siblings of the prompts dir).
fn stage_operator_dir(
    tmp: &tempfile::TempDir,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let target = tmp.path();
    let prompts = target
        .join(".sbagent")
        .join("prompts");
    let schemas = target
        .join(".sbagent")
        .join("schemas");
    let queries = target
        .join(".sbagent")
        .join("queries");
    std::fs::create_dir_all(&prompts).unwrap();
    let config = target.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "prompt_overrides_dir = \"{}\"\nschemas_dir = \"{}\"\nqueries_dir = \"{}\"\n",
            prompts.display(),
            schemas.display(),
            queries.display(),
        ),
    )
    .unwrap();
    (config, schemas, queries)
}

/// `sbagent sync` (a non-check command) triggers schema auto-seed:
/// even before sync's body runs, the missing schemas should be
/// materialized by `CliContext::from_args`.
#[test]
fn from_args_auto_seeds_schemas_for_non_check_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, schemas, queries) = stage_operator_dir(&tmp);

    // Pre-seed: schemas + queries dirs do NOT exist yet.
    assert!(
        !schemas
            .join("candidates.schema.json")
            .exists()
    );
    assert!(
        !queries
            .join("run_summary.sql")
            .exists()
    );

    let args = parse(&["sbagent", "-c", config.to_str().unwrap(), "sync"]);
    let _ctx = CliContext::from_args(&args).expect("from_args");

    // After from_args: every bundled schema + query should be on disk.
    for name in ["candidates", "analysis", "optimization-targets", "summary"] {
        assert!(
            schemas
                .join(format!("{name}.schema.json"))
                .is_file(),
            "{name}.schema.json should have been auto-seeded for `sync`",
        );
    }
    for name in ["run_summary.sql", "top_spans_by_self_wall.sql", "README.md"] {
        assert!(queries.join(name).is_file(), "{name} should have been auto-seeded for `sync`",);
    }
}

/// `sbagent check` is the deliberate exception: from_args must NOT
/// auto-heal a missing schema. This keeps the `DriftEntry::Missing`
/// branch in `check_bundle_schema_drift` reachable, matching the
/// documented contract ("check fails on bundle drift; missing files
/// are drift").
#[test]
fn from_args_does_not_auto_seed_schemas_for_check() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, schemas, queries) = stage_operator_dir(&tmp);

    assert!(
        !schemas
            .join("candidates.schema.json")
            .exists()
    );
    assert!(
        !queries
            .join("run_summary.sql")
            .exists()
    );

    let args = parse(&["sbagent", "-c", config.to_str().unwrap(), "check"]);
    let _ctx = CliContext::from_args(&args).expect("from_args");

    for name in ["candidates", "analysis", "optimization-targets", "summary"] {
        let path = schemas.join(format!("{name}.schema.json"));
        assert!(!path.exists(), "`check` MUST NOT auto-heal a missing schema; found {path:?}",);
    }
    for name in ["run_summary.sql", "top_spans_by_self_wall.sql", "README.md"] {
        let path = queries.join(name);
        assert!(!path.exists(), "`check` MUST NOT auto-heal a missing query; found {path:?}",);
    }
}
