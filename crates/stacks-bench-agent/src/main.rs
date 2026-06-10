//! Binary entry point for `sbagent`.
//!
//! Keep this file deliberately thin: argument parsing + global init + dispatch
//! into [`stacks_bench_agent::cli::dispatch`]. Anything testable belongs in
//! the library, not here.

use anyhow::Result;
use clap::Parser as _;
use stacks_bench_agent::cli::{CliArgs, dispatch};

#[tokio::main]
async fn main() -> Result<()> {
    // `try_parse().unwrap_or_else(|e| e.exit())` is the canonical
    // pattern: clap's own `Error::exit` writes `--help` / `--version`
    // output to stdout with status 0 (the contract any CLI tool
    // should honor), and writes real parse errors to stderr with
    // status 2. Propagating the error via `?` would dump help text
    // into anyhow's `Error: <...>` formatting and exit non-zero,
    // which breaks every operator + integration-test `--help`
    // invocation.
    let args = CliArgs::try_parse().unwrap_or_else(|e| e.exit());
    init_tracing();
    dispatch(args).await
}

/// Initialize a tracing subscriber that respects `RUST_LOG`. Defaults
/// to `info` so operators get useful output without setting an env
/// var. Logs route to **stderr**, leaving stdout reserved for
/// machine-readable program output (e.g. `sbagent source cache-id`
/// for shell substitution).
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
