//! Pre-render the run-id-scoped triage SQL queries so the triage and
//! analyzer agents don't have to spawn `sqlite3` themselves for the
//! standard orientation / candidate-ranking set.
//!
//! Each entry in [`PRERENDERED`] is a `<framework>/queries/<name>.sql`
//! that takes only `:run_id` (and optionally `:limit`). The agent's
//! drilldown queries — anything parameterized by `:span_id`,
//! `:stacks_tx_hash`, `:stacks_block_hash`, or `:contract_name` — are
//! deliberately omitted; the agent must pick those params after
//! reading the pre-rendered orientation set.
//!
//! Output layout: `<results>/triage/queries/<name>.csv` for the
//! captured stdout, `<name>.stderr.log` alongside when the query
//! errored. Per-query failures are recorded but don't fail the phase
//! — a missing CSV is more useful signal for the agent than an
//! aborted session.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

/// (file stem, optional `:limit` value). `None` means the query takes no
/// `:limit`. See `<framework>/queries/README.md` for invariants.
const PRERENDERED: &[(&str, Option<u32>)] = &[
    // Orientation set — single-row or small-N output, no `:limit`.
    ("run_summary", None),
    ("tx_type_distribution", None),
    ("block_timing_breakdown", None),
    ("baseline_empty_block_breakdown", None),
    ("span_recurrence", None),
    // Candidate-ranking set — `:limit` matches the README defaults.
    ("top_spans_by_self_wall", Some(50)),
    ("top_spans_by_call_count", Some(50)),
    ("top_contract_calls", Some(25)),
    ("top_clarity_consumers_by_contract", Some(25)),
    ("top_txs_by_duration", Some(25)),
];

/// Pre-render every query in [`PRERENDERED`] to `<output_dir>/<name>.csv`.
/// Creates `output_dir` if needed. Per-query failures are logged but
/// don't abort.
///
/// `queries_dir` is `<framework>/queries`. `db_path` is the persistent
/// stacks-bench SQLite db (`<data_dir>/appdata/stacks-bench.db`).
pub fn prerender(queries_dir: &Path, output_dir: &Path, db_path: &Path, run_id: i64) -> Result<()> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("creating {}", output_dir.display()))?;

    for (name, limit) in PRERENDERED {
        let sql_path = queries_dir.join(format!("{name}.sql"));
        if !sql_path.is_file() {
            tracing::warn!(query = %name, "triage query missing on disk; skipping");
            continue;
        }
        let out_csv = output_dir.join(format!("{name}.csv"));
        let out_err = output_dir.join(format!("{name}.stderr.log"));
        if let Err(e) = run_one(&sql_path, &out_csv, &out_err, db_path, run_id, *limit) {
            tracing::warn!(query = %name, error = ?e, "triage query failed; agent will see empty CSV");
        }
    }
    Ok(())
}

/// Invoke `sqlite3 -header -csv` for one query. Stdout → `out_csv`,
/// stderr → `out_err`. Returns `Err` on spawn failure or non-zero exit;
/// caller decides whether to continue.
fn run_one(
    sql_path: &Path,
    out_csv: &Path,
    out_err: &Path,
    db_path: &Path,
    run_id: i64,
    limit: Option<u32>,
) -> Result<()> {
    let run_id_param = format!(".parameter set :run_id {run_id}");
    let read_cmd = format!(".read {}", sql_path.display());
    let mut cmd = Command::new("sqlite3");
    cmd.arg("-header")
        .arg("-csv")
        .arg(db_path)
        .arg("-cmd")
        .arg(&run_id_param);
    if let Some(l) = limit {
        cmd.arg("-cmd")
            .arg(format!(".parameter set :limit {l}"));
    }
    cmd.arg("-cmd").arg(&read_cmd);

    let stdout = std::fs::File::create(out_csv)
        .with_context(|| format!("opening {} for write", out_csv.display()))?;
    let stderr = std::fs::File::create(out_err)
        .with_context(|| format!("opening {} for write", out_err.display()))?;
    cmd.stdout(stdout)
        .stderr(stderr);

    let status = cmd
        .status()
        .with_context(|| format!("spawning sqlite3 for {}", sql_path.display()))?;
    if !status.success() {
        anyhow::bail!(
            "sqlite3 for {} exited {} (see {})",
            sql_path.display(),
            status,
            out_err.display()
        );
    }
    // Successful runs leave an empty stderr file; cleaner to drop it so
    // the agent doesn't see a misleading 0-byte log next to the CSV.
    if out_err
        .metadata()
        .map(|m| m.len() == 0)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(out_err);
    }
    Ok(())
}

/// Names of the CSV files this module produces. Useful for `triage clean`
/// and tests that need to enumerate the artifact set.
pub fn output_file_names() -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(PRERENDERED.len() * 2);
    for (name, _) in PRERENDERED {
        out.push(PathBuf::from(format!("{name}.csv")));
        out.push(PathBuf::from(format!("{name}.stderr.log")));
    }
    out
}
