//! `sbagent source cache-id` — print the cache id derived from
//! `[source]` settings.
//!
//! Used by the Phase 4 operator migration recipe (see
//! [docs/setup.md](../../../../../docs/setup.md)) to name the bare cache
//! directory: `<workspace>/cache/<cache_id>.git`. The id is otherwise
//! determined silently inside session start, so a one-shot
//! human-readable surface is the cheapest way to support `git clone
//! --bare --local <submodule> "<workspace>/cache/$(sbagent source
//! cache-id).git"`.

use anyhow::Result;
use clap::Args;

use crate::cli::CliContext;
use crate::source::resolve_cache_id;

/// Args for `sbagent source cache-id`.
#[derive(Debug, Clone, Args)]
pub struct CacheIdArgs {}

/// Print the resolved cache id to stdout (no trailing newline beyond
/// `println!`'s — shell substitution `$(sbagent source cache-id)`
/// strips it). Reads `[source].url` + optional `[source].id` from
/// settings; fails loudly if `[source].url` is unset (pre-v3-cutover
/// operators have nothing meaningful to derive from).
pub fn run(_args: CacheIdArgs, ctx: &CliContext) -> Result<()> {
    let (url, _branch) = ctx
        .settings
        .source
        .require_url_and_branch()?;
    let id = resolve_cache_id(
        ctx.settings
            .source
            .id
            .as_deref(),
        url,
    )
    .map_err(|e| anyhow::anyhow!("resolving cache id: {e}"))?;
    println!("{id}");
    Ok(())
}
