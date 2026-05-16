//! Per-invocation overrides for the block range `stacks-bench` replays.
//!
//! These four knobs control which slice of chain history Phase 0
//! (baseline + rerun) and Phase 3 (per-target experiment benches)
//! operate on. They're identical between phases — candidate selection
//! during triage / analysis must talk about the same blocks the
//! experiments end up measuring.
//!
//! Resolution precedence (highest wins): CLI flag → env var → config
//! setting → required-error (for `start_at` / `count`) or `None` (for
//! `warmup` / `filter`). The CLI-or-env surface exists specifically
//! for autonomous sampling — a scheduled CI run varies the range per
//! invocation (e.g. matrix over the canonical Nakamoto ranges) without
//! mutating `config.toml`. Hand-driven smoke tests benefit from the
//! same shape: an operator can run `sbagent session run --start-at
//! 5000000 --count 10000 --warmup 1000` without editing config.

use anyhow::{Context as _, Result};
use clap::Args;

use crate::settings::Settings;

/// Embeddable arg group. Both `BaselineRunArgs` and `RunSessionArgs`
/// flatten this in via `#[clap(flatten)]` so the same surface is
/// available on `session baseline run` (Phase 0 only) and `session
/// run` (Phase 0 + Phase 3, both consume the same resolved range).
#[derive(Debug, Args, Clone, Default)]
pub struct BenchRangeArgs {
    /// First block height to replay. Defaults to
    /// `settings.stacks_bench_start_at`. CLI / env override exists so
    /// the closed-loop can sample different ranges per session without
    /// mutating `config.toml`.
    #[clap(long, env = "SBAGENT_BENCH_START_AT")]
    pub start_at: Option<u64>,

    /// Number of blocks to replay AFTER `warmup`. Defaults to
    /// `settings.stacks_bench_count`. The measured count is exactly
    /// this — `warmup` blocks are advanced through to settle caches
    /// + JIT but don't count toward the sample size.
    #[clap(long, env = "SBAGENT_BENCH_COUNT")]
    pub count: Option<u64>,

    /// Pre-window blocks to advance through before measurement starts.
    /// Defaults to `settings.stacks_bench_warmup` (no warmup when
    /// unset). Useful when the run is small enough that cold caches
    /// dominate the first few hundred blocks.
    #[clap(long, env = "SBAGENT_BENCH_WARMUP")]
    pub warmup: Option<u64>,

    /// Filter expression passed to `stacks-bench bench run --filter`
    /// (e.g. `contract-call`). Defaults to `settings.stacks_bench_filter`
    /// (no filter when unset). Useful when the chainstate carries
    /// non-canonical forks or when sampling a specific tx subset.
    #[clap(long, env = "SBAGENT_BENCH_FILTER")]
    pub filter: Option<String>,
}

/// Resolved range, ready to pass into the bench harness. Mirrors the
/// fields on `BenchRangeArgs` but with `start_at` / `count` made
/// required (the resolver bails when neither CLI nor config sets them).
#[derive(Debug, Clone)]
pub struct ResolvedBenchRange {
    /// Resolved start block height.
    pub start_at: u64,
    /// Resolved block count (post-warmup).
    pub count: u64,
    /// Optional warmup; resolver carries `None` through verbatim.
    pub warmup: Option<u64>,
    /// Optional filter expression; resolver carries `None` through verbatim.
    pub filter: Option<String>,
}

impl BenchRangeArgs {
    /// Resolve to a [`ResolvedBenchRange`], applying
    /// `CLI → env → config → error` precedence. Error messages name
    /// all three surfaces so the operator can pick whichever is most
    /// convenient.
    pub fn resolve(&self, settings: &Settings) -> Result<ResolvedBenchRange> {
        let start_at = self
            .start_at
            .or(settings.stacks_bench_start_at)
            .context(
                "start block height missing: pass `--start-at <N>`, set `SBAGENT_BENCH_START_AT`, \
                 or populate `stacks_bench_start_at` in config",
            )?;
        let count = self
            .count
            .or(settings.stacks_bench_count)
            .context(
                "block count missing: pass `--count <N>`, set `SBAGENT_BENCH_COUNT`, or populate \
                 `stacks_bench_count` in config",
            )?;
        let warmup = self
            .warmup
            .or(settings.stacks_bench_warmup);
        let filter = self
            .filter
            .clone()
            .or_else(|| {
                settings
                    .stacks_bench_filter
                    .clone()
            });
        Ok(ResolvedBenchRange {
            start_at,
            count,
            warmup,
            filter,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(
        start_at: Option<u64>,
        count: Option<u64>,
        warmup: Option<u64>,
        filter: Option<&str>,
    ) -> Settings {
        Settings {
            stacks_bench_start_at: start_at,
            stacks_bench_count: count,
            stacks_bench_warmup: warmup,
            stacks_bench_filter: filter.map(str::to_owned),
            ..Settings::default()
        }
    }

    /// Config-only path: both required fields set, optional fields
    /// pass through verbatim.
    #[test]
    fn resolves_from_settings_when_no_cli_overrides() {
        let args = BenchRangeArgs::default();
        let settings = settings_with(Some(5_000_000), Some(10_000), Some(1_000), Some("cc"));
        let resolved = args
            .resolve(&settings)
            .unwrap();
        assert_eq!(resolved.start_at, 5_000_000);
        assert_eq!(resolved.count, 10_000);
        assert_eq!(resolved.warmup, Some(1_000));
        assert_eq!(resolved.filter.as_deref(), Some("cc"));
    }

    /// CLI/env values win over config — the autonomous-sampling
    /// payoff. Both required (start_at, count) and optional (warmup,
    /// filter) overrides cover the override path.
    #[test]
    fn cli_overrides_settings_for_every_field() {
        let args = BenchRangeArgs {
            start_at: Some(7_300_000),
            count: Some(50_000),
            warmup: Some(2_500),
            filter: Some("perf-only".into()),
        };
        let settings = settings_with(Some(5_000_000), Some(10_000), Some(1_000), Some("cc"));
        let resolved = args
            .resolve(&settings)
            .unwrap();
        assert_eq!(resolved.start_at, 7_300_000);
        assert_eq!(resolved.count, 50_000);
        assert_eq!(resolved.warmup, Some(2_500));
        assert_eq!(resolved.filter.as_deref(), Some("perf-only"));
    }

    /// Required field missing from BOTH CLI and config → actionable
    /// error citing all three surfaces.
    #[test]
    fn missing_start_at_errors_with_all_three_surfaces() {
        let args = BenchRangeArgs {
            count: Some(10_000),
            ..Default::default()
        };
        let settings = settings_with(None, None, None, None);
        let err = args
            .resolve(&settings)
            .expect_err("missing start_at must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("--start-at"), "{msg}");
        assert!(msg.contains("SBAGENT_BENCH_START_AT"), "{msg}");
        assert!(msg.contains("stacks_bench_start_at"), "{msg}");
    }

    /// Optional fields stay `None` when neither CLI nor config sets
    /// them. (Just stating the contract — a regression here would
    /// turn into a stray empty `--warmup ''` arg downstream.)
    #[test]
    fn optionals_pass_none_through_when_unset_everywhere() {
        let args = BenchRangeArgs {
            start_at: Some(5_000_000),
            count: Some(10_000),
            ..Default::default()
        };
        let settings = settings_with(None, None, None, None);
        let resolved = args
            .resolve(&settings)
            .unwrap();
        assert_eq!(resolved.warmup, None);
        assert_eq!(resolved.filter, None);
    }
}
