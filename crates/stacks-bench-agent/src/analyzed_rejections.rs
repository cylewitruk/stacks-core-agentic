//! Cross-session analyzed-rejections ledger.
//!
//! The bot's persistent memory of "this family was promoted to
//! analysis in a prior session, and the analyzer either rejected it
//! outright or accepted-with-not-actionable disposition." Triage
//! consults this ledger via `sbagent rejections probe` to avoid
//! re-promoting families that have already been investigated; the
//! analyzer fan-out (`session::analyzers`) appends in-process via
//! [`append`] after each analysis.json lands. The CLI's
//! `sbagent rejections append` exists for operator debug / manual
//! seeding and calls the same module function.
//!
//! Storage: a single `<operator>/memory/analyzed-rejections.jsonl`
//! file, one JSON record per line, append-only. A shared lockfile at
//! `<operator>/memory/.locks/memory.lock` protects all memory-state
//! mutations (single lock covers any future memory files too).
//!
//! Architecture context: this module is one half of the
//! "expansive-triage + ledger-converges" loop. Triage promotes
//! anything with a plausible structural-fix hypothesis (no fixability
//! gates); the analyzer rules things out with code context; the
//! ledger records those rulings; triage probes before promoting on
//! subsequent sessions. The system's steady-state load returns to
//! pre-relaxation levels once the ledger has absorbed the
//! plausible-but-dead-end families.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::models::{FromJson, ToJson};

/// Outcome class recorded on a ledger entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Analyzer returned `status: "rejected"` — the triage signal
    /// itself was a false positive at code level.
    Rejected,
    /// Analyzer returned `status: "accepted"` but
    /// `lens_disposition.status == "not_actionable"` — the signal
    /// was real but no structural handle exists.
    NotActionable,
}

impl Outcome {
    /// Snake-case identifier used on the CLI and in JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::NotActionable => "not_actionable",
        }
    }

    /// Parse from the CLI's string form.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "rejected" => Ok(Self::Rejected),
            "not_actionable" => Ok(Self::NotActionable),
            other => bail!("unknown outcome `{other}` (expected `rejected` or `not_actionable`)"),
        }
    }
}

/// One ledger record. Append-only on disk; mutations go through `trim`
/// or `remove` which rewrite the file under the exclusive lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// ISO-8601 UTC timestamp of when the record was appended.
    pub ts: String,
    /// Session id that produced this rejection.
    pub session_id: String,
    /// Stable kebab-case family identifier — same naming convention
    /// as triage's `candidates[i].id`. Operator-readable.
    pub family_id: String,
    /// Lens the family was promoted on.
    pub lens: String,
    /// Outcome class (see [`Outcome`]).
    pub outcome: Outcome,
    /// Family kind — `tx_family`, `block_family`, or `contract_family`.
    pub kind: String,
    /// Sorted list of suspected spans (sorted so identical spans in
    /// different input order produce the same fingerprint).
    pub suspected_spans: Vec<String>,
    /// Contract function key when `kind == "contract_family"` —
    /// `<issuer>.<contract>.<function>` or just `<issuer>.<contract>`
    /// when function is unspecified. `None` otherwise.
    pub contract_function: Option<String>,
    /// Canonical fingerprint computed from
    /// (lens, kind, sorted_spans, contract_function). Stable across
    /// sessions for the same logical family — the agent computes it
    /// from triage-time inputs to probe for prior rejections.
    pub fingerprint: String,
    /// Stacks-core commit SHA the analyzer was looking at when it
    /// produced the rejection. Lets operators reason about whether a
    /// rejection might be stale relative to current code.
    pub stacks_core_sha: Option<String>,
    /// Analyzer's reason text — copied verbatim from the analyzer's
    /// `analysis.json` `reason` (rejected) or `lens_disposition.reason`
    /// (not_actionable). Code-level prose the operator can read.
    pub reason: String,
    /// Optional absolute path to the analyzer's `analysis.json` for
    /// the full evidence chain. Operators (and future agents) can
    /// follow this for context beyond the one-line reason.
    pub evidence_path: Option<String>,
}

/// Inputs to [`compute_fingerprint`]. Same shape as the CLI's
/// `--lens / --kind / --spans / --contract` args.
pub struct FingerprintInputs<'a> {
    /// Lens — `tx_latency`, `tenure_throughput`, or `commit_time`.
    pub lens: &'a str,
    /// Family kind — `tx_family`, `block_family`, `contract_family`.
    pub kind: &'a str,
    /// Suspected spans (any order; canonicalized internally).
    pub suspected_spans: &'a [String],
    /// Contract function key, `None` for non-contract families.
    pub contract_function: Option<&'a str>,
}

/// Canonicalize a list of suspected spans: trim each entry, drop
/// any that are empty post-trim, sort, dedup. Centralized here so
/// the gate (`fingerprint_specific_enough`) and the fingerprint
/// computation can't drift — both must collapse `[""]`, `["  "]`,
/// and `["x", "x"]` to identical normalized forms.
fn canonical_spans(spans: &[String]) -> Vec<String> {
    let mut out: Vec<String> = spans
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Canonicalize the contract function key: `Some("")` and
/// `Some("   ")` collapse to `None` so a whitespace-only override
/// can't sneak past the specificity gate.
fn canonical_contract(contract: Option<&str>) -> Option<&str> {
    contract.filter(|c| !c.trim().is_empty())
}

/// Compute the canonical fingerprint for a family. Same canonical
/// form used in both `append` (the record's `fingerprint` field) and
/// `probe` (the query input) so equal-fingerprint matches are byte-
/// exact.
///
/// Canonical form: `lens|kind|sorted_spans.join(",")|contract_or_*`
/// — readable enough to use directly as the fingerprint without a
/// hash (no new crypto dep needed; trivially inspectable for debug).
pub fn compute_fingerprint(inputs: &FingerprintInputs<'_>) -> String {
    let spans = canonical_spans(inputs.suspected_spans);
    let contract = canonical_contract(inputs.contract_function).unwrap_or("*");
    format!(
        "{lens}|{kind}|{spans}|{contract}",
        lens = inputs.lens,
        kind = inputs.kind,
        spans = spans.join(","),
    )
}

/// Compute the canonical contract function key for the
/// `Record::contract_function` field. Returns `None` when issuer is
/// empty (= not a contract family).
pub fn canonical_contract_key(
    issuer: &str,
    contract: &str,
    function: Option<&str>,
) -> Option<String> {
    if issuer.is_empty() && contract.is_empty() {
        return None;
    }
    Some(match function {
        Some(f) if !f.is_empty() => format!("{issuer}.{contract}.{f}"),
        _ => format!("{issuer}.{contract}"),
    })
}

// ---------- File location helpers ----------

/// Absolute path to the analyzed-rejections JSONL file inside the
/// memory dir.
pub fn ledger_path(memory_dir: &Path) -> PathBuf {
    memory_dir.join("analyzed-rejections.jsonl")
}

/// Absolute path to the shared memory lockfile. Covers every memory
/// file under `memory_dir` for v1 — single lock simplifies semantics
/// and per-file contention is negligible.
pub fn lock_path(memory_dir: &Path) -> PathBuf {
    memory_dir
        .join(".locks")
        .join("memory.lock")
}

// ---------- Lockfile helpers ----------

/// Open (creating if needed) the shared memory lockfile.
fn open_lockfile(memory_dir: &Path) -> Result<File> {
    let path = lock_path(memory_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating lock dir {}", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening memory lock {}", path.display()))
}

/// Run `f` while holding an exclusive lock on the memory lockfile.
/// The lock is released when this function returns.
///
/// Closure-based to keep the guard's lifetime tied to a real local
/// owner rather than smuggling a `'static` guard out of the function
/// via transmute.
pub fn with_exclusive_lock<F, R>(memory_dir: &Path, f: F) -> Result<R>
where
    F: FnOnce() -> Result<R>,
{
    let file = open_lockfile(memory_dir)?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock
        .write()
        .context("acquiring exclusive memory lock")?;
    f()
}

/// Run `f` while holding a shared lock on the memory lockfile.
pub fn with_shared_lock<F, R>(memory_dir: &Path, f: F) -> Result<R>
where
    F: FnOnce() -> Result<R>,
{
    let file = open_lockfile(memory_dir)?;
    let lock = fd_lock::RwLock::new(file);
    let _guard = lock
        .read()
        .context("acquiring shared memory lock")?;
    f()
}

// ---------- Load + append ----------

/// Load every record from the ledger. Returns an empty vector when
/// the file doesn't exist (a fresh operator dir). Skips lines that
/// fail to parse with a tracing warning rather than failing the
/// entire load — a single corrupt line shouldn't block the bot.
///
/// Reads with a shared lock to defend against torn reads when an
/// append is in flight.
pub fn load_all(memory_dir: &Path) -> Result<Vec<Record>> {
    with_shared_lock(memory_dir, || load_all_locked(memory_dir))
}

/// Lock-free load. Caller must already hold the memory lock.
fn load_all_locked(memory_dir: &Path) -> Result<Vec<Record>> {
    let path = ledger_path(memory_dir);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for (idx, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("reading line {idx} of {}", path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match Record::from_json(trimmed) {
            Ok(r) => out.push(r),
            Err(e) => {
                tracing::warn!(
                    line = idx + 1,
                    error = %e,
                    "skipping malformed analyzed-rejections record",
                );
            }
        }
    }
    Ok(out)
}

/// Append a new record to the ledger. Atomic via the exclusive lock
/// (single `O_APPEND` open + write under the lock guarantees no
/// interleaved writes).
///
/// Skips records whose fingerprint would be overly broad (no
/// suspected spans AND no contract function) — those would collide
/// with every future family on the same lens/kind, poisoning probe
/// results. The skip is logged at WARN so it surfaces in the
/// orchestrator's events.
///
/// Dedup policy: if a record with the same fingerprint already
/// exists, the new record is **still appended** (history is
/// preserved per-session). Operators trim accumulated history via
/// `sbagent rejections trim --keep-last N`.
pub fn append(memory_dir: &Path, record: &Record) -> Result<()> {
    if !fingerprint_specific_enough(
        &record.suspected_spans,
        record
            .contract_function
            .as_deref(),
    ) {
        tracing::warn!(
            family_id = %record.family_id,
            lens = %record.lens,
            kind = %record.kind,
            "skipping analyzed-rejections append: fingerprint is too broad (no \
             suspected_spans and no contract_function); a recorded entry would match every \
             future family on the same lens/kind",
        );
        return Ok(());
    }
    with_exclusive_lock(memory_dir, || append_locked(memory_dir, record))
}

/// Lock-free append. Caller must already hold the exclusive lock and
/// is responsible for the fingerprint-specificity check.
fn append_locked(memory_dir: &Path, record: &Record) -> Result<()> {
    let path = ledger_path(memory_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {} for append", path.display()))?;
    let line = record
        .to_json()
        .context("serializing analyzed-rejection record")?;
    writeln!(file, "{line}").with_context(|| format!("writing to {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flushing {}", path.display()))?;
    Ok(())
}

/// Rewrite the ledger from a new set of records. Used by `trim` and
/// `remove`. Holds the exclusive lock for the rewrite; writes to a
/// temp file then renames over to keep the operation crash-safe.
pub fn rewrite(memory_dir: &Path, records: &[Record]) -> Result<()> {
    with_exclusive_lock(memory_dir, || rewrite_locked(memory_dir, records))
}

/// Lock-free rewrite. Caller must already hold the exclusive lock.
fn rewrite_locked(memory_dir: &Path, records: &[Record]) -> Result<()> {
    let path = ledger_path(memory_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp_path = path.with_extension("jsonl.tmp");
    {
        let mut tmp = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| format!("opening {} for rewrite", tmp_path.display()))?;
        for r in records {
            let line = r
                .to_json()
                .context("serializing record during rewrite")?;
            writeln!(tmp, "{line}")
                .with_context(|| format!("writing to {}", tmp_path.display()))?;
        }
        tmp.flush()
            .with_context(|| format!("flushing {}", tmp_path.display()))?;
    }
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("renaming {} → {}", tmp_path.display(), path.display()))?;
    Ok(())
}

/// Atomic load → transform → rewrite under a single exclusive lock.
/// Use this for any mutation that depends on prior state, so a
/// concurrent `append` between the load and the rewrite can't be
/// silently lost. Returns the number of records dropped (input count
/// minus output count, clamped at zero).
pub fn load_filter_and_rewrite<F>(memory_dir: &Path, f: F) -> Result<usize>
where
    F: FnOnce(Vec<Record>) -> Result<Vec<Record>>,
{
    with_exclusive_lock(memory_dir, || {
        let before = load_all_locked(memory_dir)?;
        let before_count = before.len();
        let after = f(before)?;
        let after_count = after.len();
        rewrite_locked(memory_dir, &after)?;
        Ok(before_count.saturating_sub(after_count))
    })
}

// ---------- Probe / query primitives ----------

/// Return false when the fingerprint inputs would produce an overly
/// broad fingerprint — no suspected spans AND no contract function
/// means the canonical form is `lens|kind||*`, which collides with
/// every future family on the same lens/kind. Used to gate both
/// `append` (skip-and-warn) and `probe` (return empty without
/// touching the ledger).
///
/// Mirrors the trim+filter applied by [`canonical_spans`] /
/// [`canonical_contract`] so a model-written `[""]` /
/// `[" ", "\t"]` / `Some("   ")` doesn't sneak past the gate by
/// looking non-empty while contributing nothing to identity.
pub fn fingerprint_specific_enough(spans: &[String], contract: Option<&str>) -> bool {
    let has_span = spans
        .iter()
        .any(|s| !s.trim().is_empty());
    let has_contract = canonical_contract(contract).is_some();
    has_span || has_contract
}

/// Probe the ledger for entries matching the given fingerprint
/// inputs. Returns all matching records (history preserved across
/// sessions; agent typically gets 0 or 1 matches in practice).
///
/// Returns empty without touching the ledger when the fingerprint
/// would be overly broad — see [`fingerprint_specific_enough`]. A
/// caller probing with no spans and no contract is asking "has
/// anything on this lens/kind ever been rejected?", which is never
/// the right semantic.
///
/// Match semantics: exact-fingerprint equality. Fuzzy near-matches
/// are deliberately not supported in v1 — better to miss an obvious
/// duplicate (operator can re-tune the agent's naming) than to skip
/// a legitimate promotion.
pub fn probe(memory_dir: &Path, inputs: &FingerprintInputs<'_>) -> Result<Vec<Record>> {
    if !fingerprint_specific_enough(inputs.suspected_spans, inputs.contract_function) {
        return Ok(Vec::new());
    }
    let fp = compute_fingerprint(inputs);
    let all = load_all(memory_dir)?;
    Ok(all
        .into_iter()
        .filter(|r| r.fingerprint == fp)
        .collect())
}

/// Look up all records for a given `family_id` (exact match).
pub fn by_family_id(memory_dir: &Path, family_id: &str) -> Result<Vec<Record>> {
    let all = load_all(memory_dir)?;
    Ok(all
        .into_iter()
        .filter(|r| r.family_id == family_id)
        .collect())
}

/// Look up all records for a given fingerprint (exact match).
pub fn by_fingerprint(memory_dir: &Path, fingerprint: &str) -> Result<Vec<Record>> {
    let all = load_all(memory_dir)?;
    Ok(all
        .into_iter()
        .filter(|r| r.fingerprint == fingerprint)
        .collect())
}

/// Group all records by `family_id`, sorted by latest `ts` desc
/// within each group. Used by the markdown renderer.
pub fn grouped_by_family(memory_dir: &Path) -> Result<BTreeMap<String, Vec<Record>>> {
    let all = load_all(memory_dir)?;
    let mut grouped: BTreeMap<String, Vec<Record>> = BTreeMap::new();
    for r in all {
        grouped
            .entry(r.family_id.clone())
            .or_default()
            .push(r);
    }
    for entries in grouped.values_mut() {
        entries.sort_by(|a, b| b.ts.cmp(&a.ts));
    }
    Ok(grouped)
}

// ---------- Misc ----------

/// Current UTC timestamp in the ISO-8601 form the ledger records use.
/// Format: `2026-05-17T14:23:45Z` (no fractional seconds, no offset
/// beyond `Z`).
pub fn now_utc_iso8601() -> String {
    // std::time + manual format avoids pulling chrono into the dep
    // graph for one timestamp call.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (days, time_secs) = (secs / 86_400, secs % 86_400);
    let (hours, rem) = (time_secs / 3600, time_secs % 3600);
    let (minutes, seconds) = (rem / 60, rem % 60);
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days-since-1970-01-01 to (year, month, day). Standard
/// Howard Hinnant proleptic-Gregorian algorithm.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe
        .saturating_sub(doe / 1460)
        .saturating_add(doe / 36_524)
        .saturating_sub(doe / 146_096))
        / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y_out = if m <= 2 { y + 1 } else { y };
    (y_out, m, d)
}

/// Convert a `Record::stacks_core_sha` like `6ec953ee94...` to a
/// short form for display.
pub fn short_sha(sha: &str) -> &str {
    let end = sha.len().min(10);
    &sha[..end]
}

/// Validate a record's `fingerprint` field matches what
/// `compute_fingerprint` produces from its other fields. Used by
/// load-time consistency checks if a future caller wants them; not
/// invoked by default (cheap insurance against hand-edited entries
/// that drift).
#[must_use]
pub fn fingerprint_matches(record: &Record) -> bool {
    let expected = compute_fingerprint(&FingerprintInputs {
        lens: &record.lens,
        kind: &record.kind,
        suspected_spans: &record.suspected_spans,
        contract_function: record
            .contract_function
            .as_deref(),
    });
    expected == record.fingerprint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_under_span_reorder_and_dedup() {
        let a = compute_fingerprint(&FingerprintInputs {
            lens: "tx_latency",
            kind: "tx_family",
            suspected_spans: &["zeta".to_owned(), "alpha".to_owned(), "alpha".to_owned()],
            contract_function: None,
        });
        let b = compute_fingerprint(&FingerprintInputs {
            lens: "tx_latency",
            kind: "tx_family",
            suspected_spans: &["alpha".to_owned(), "zeta".to_owned()],
            contract_function: None,
        });
        assert_eq!(a, b);
        assert_eq!(a, "tx_latency|tx_family|alpha,zeta|*");
    }

    #[test]
    fn fingerprint_differs_by_lens_kind_contract() {
        let base = FingerprintInputs {
            lens: "tx_latency",
            kind: "tx_family",
            suspected_spans: &["s".to_owned()],
            contract_function: None,
        };
        let by_lens = FingerprintInputs { lens: "commit_time", ..base };
        let by_kind = FingerprintInputs { kind: "block_family", ..base };
        let by_contract = FingerprintInputs {
            contract_function: Some("SP.x.y"),
            ..base
        };
        let baseline = compute_fingerprint(&base);
        assert_ne!(baseline, compute_fingerprint(&by_lens));
        assert_ne!(baseline, compute_fingerprint(&by_kind));
        assert_ne!(baseline, compute_fingerprint(&by_contract));
    }

    #[test]
    fn append_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let fp = compute_fingerprint(&FingerprintInputs {
            lens: "tx_latency",
            kind: "tx_family",
            suspected_spans: &["span_a".to_owned(), "span_b".to_owned()],
            contract_function: None,
        });
        let record = Record {
            ts: now_utc_iso8601(),
            session_id: "20260517-000000-test".to_owned(),
            family_id: "test-family".to_owned(),
            lens: "tx_latency".to_owned(),
            outcome: Outcome::NotActionable,
            kind: "tx_family".to_owned(),
            suspected_spans: vec!["span_a".to_owned(), "span_b".to_owned()],
            contract_function: None,
            fingerprint: fp.clone(),
            stacks_core_sha: Some("6ec953ee94abc".to_owned()),
            reason: "no structural handle".to_owned(),
            evidence_path: None,
        };
        append(dir, &record).unwrap();
        append(dir, &record).unwrap();
        let loaded = load_all(dir).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(
            loaded
                .iter()
                .all(|r| r.fingerprint == fp)
        );
    }

    #[test]
    fn probe_matches_by_fingerprint_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let inputs = FingerprintInputs {
            lens: "tx_latency",
            kind: "tx_family",
            suspected_spans: &["span_a".to_owned()],
            contract_function: None,
        };
        let record = Record {
            ts: now_utc_iso8601(),
            session_id: "s".to_owned(),
            family_id: "test-family".to_owned(),
            lens: "tx_latency".to_owned(),
            outcome: Outcome::Rejected,
            kind: "tx_family".to_owned(),
            suspected_spans: vec!["span_a".to_owned()],
            contract_function: None,
            fingerprint: compute_fingerprint(&inputs),
            stacks_core_sha: None,
            reason: "r".to_owned(),
            evidence_path: None,
        };
        append(dir, &record).unwrap();

        // Exact match → 1 hit.
        let hits = probe(dir, &inputs).unwrap();
        assert_eq!(hits.len(), 1);

        // Different span set → 0 hits (fingerprint differs).
        let other = FingerprintInputs {
            suspected_spans: &["span_b".to_owned()],
            ..inputs
        };
        assert!(
            probe(dir, &other)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rewrite_replaces_file_content() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Each record carries a unique span so the broad-fingerprint
        // gate on `append` doesn't reject them — that gate is tested
        // separately in `append_skips_overly_broad_fingerprint`.
        let r = |family_id: &str| Record {
            ts: now_utc_iso8601(),
            session_id: "s".to_owned(),
            family_id: family_id.to_owned(),
            lens: "tx_latency".to_owned(),
            outcome: Outcome::Rejected,
            kind: "tx_family".to_owned(),
            suspected_spans: vec![format!("sp_{family_id}")],
            contract_function: None,
            fingerprint: format!("tx_latency|tx_family|sp_{family_id}|*"),
            stacks_core_sha: None,
            reason: "".to_owned(),
            evidence_path: None,
        };
        append(dir, &r("a")).unwrap();
        append(dir, &r("b")).unwrap();
        append(dir, &r("c")).unwrap();
        assert_eq!(load_all(dir).unwrap().len(), 3);

        let kept = vec![r("b")];
        rewrite(dir, &kept).unwrap();
        let after = load_all(dir).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].family_id, "b");
    }

    #[test]
    fn load_returns_empty_for_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_all(tmp.path()).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_filter_and_rewrite_is_atomic_load_then_rewrite() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let r = |family_id: &str, span: &str| Record {
            ts: now_utc_iso8601(),
            session_id: "s".to_owned(),
            family_id: family_id.to_owned(),
            lens: "tx_latency".to_owned(),
            outcome: Outcome::Rejected,
            kind: "tx_family".to_owned(),
            suspected_spans: vec![span.to_owned()],
            contract_function: None,
            fingerprint: compute_fingerprint(&FingerprintInputs {
                lens: "tx_latency",
                kind: "tx_family",
                suspected_spans: &[span.to_owned()],
                contract_function: None,
            }),
            stacks_core_sha: None,
            reason: "".to_owned(),
            evidence_path: None,
        };
        append(dir, &r("a", "sp_a")).unwrap();
        append(dir, &r("b", "sp_b")).unwrap();
        append(dir, &r("c", "sp_c")).unwrap();

        let dropped = load_filter_and_rewrite(dir, |recs| {
            Ok(recs
                .into_iter()
                .filter(|r| r.family_id != "b")
                .collect())
        })
        .unwrap();
        assert_eq!(dropped, 1);
        let after = load_all(dir).unwrap();
        assert_eq!(after.len(), 2);
        assert!(
            after
                .iter()
                .all(|r| r.family_id != "b")
        );
    }

    #[test]
    fn fingerprint_specific_enough_gates_empty_spans_and_no_contract() {
        assert!(!fingerprint_specific_enough(&[], None));
        assert!(!fingerprint_specific_enough(&[], Some("")));
        assert!(fingerprint_specific_enough(&[], Some("SP.foo.bar")));
        assert!(fingerprint_specific_enough(&["x".to_owned()], None));
    }

    #[test]
    fn fingerprint_specific_enough_treats_blank_strings_as_no_signal() {
        // Empty / whitespace-only span strings contribute nothing to
        // identity and must NOT pass the gate. Same for a
        // whitespace-only contract.
        assert!(!fingerprint_specific_enough(&["".to_owned()], None));
        assert!(!fingerprint_specific_enough(&["   ".to_owned()], None));
        assert!(!fingerprint_specific_enough(&["".to_owned(), "  ".to_owned()], Some("   "),));
        // A mix of blank + real → still passes; the real one carries
        // the identity.
        assert!(fingerprint_specific_enough(&["".to_owned(), "real_span".to_owned()], None,));
    }

    #[test]
    fn fingerprint_canonicalizes_blank_and_dup_spans() {
        let a = compute_fingerprint(&FingerprintInputs {
            lens: "tx_latency",
            kind: "tx_family",
            suspected_spans: &[
                "".to_owned(),
                "  alpha ".to_owned(),
                "alpha".to_owned(),
                "  ".to_owned(),
            ],
            contract_function: None,
        });
        let b = compute_fingerprint(&FingerprintInputs {
            lens: "tx_latency",
            kind: "tx_family",
            suspected_spans: &["alpha".to_owned()],
            contract_function: None,
        });
        assert_eq!(a, b);
        assert_eq!(a, "tx_latency|tx_family|alpha|*");
    }

    #[test]
    fn append_skips_overly_broad_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let broad = Record {
            ts: now_utc_iso8601(),
            session_id: "s".to_owned(),
            family_id: "broad".to_owned(),
            lens: "tx_latency".to_owned(),
            outcome: Outcome::Rejected,
            kind: "tx_family".to_owned(),
            suspected_spans: vec![],
            contract_function: None,
            fingerprint: "tx_latency|tx_family||*".to_owned(),
            stacks_core_sha: None,
            reason: "".to_owned(),
            evidence_path: None,
        };
        append(dir, &broad).unwrap();
        assert!(
            load_all(dir)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn probe_short_circuits_on_overly_broad_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Seed with a real entry so probe-vs-load divergence is obvious.
        let record = Record {
            ts: now_utc_iso8601(),
            session_id: "s".to_owned(),
            family_id: "f".to_owned(),
            lens: "tx_latency".to_owned(),
            outcome: Outcome::Rejected,
            kind: "tx_family".to_owned(),
            suspected_spans: vec!["sp".to_owned()],
            contract_function: None,
            fingerprint: compute_fingerprint(&FingerprintInputs {
                lens: "tx_latency",
                kind: "tx_family",
                suspected_spans: &["sp".to_owned()],
                contract_function: None,
            }),
            stacks_core_sha: None,
            reason: "".to_owned(),
            evidence_path: None,
        };
        append(dir, &record).unwrap();
        let hits = probe(
            dir,
            &FingerprintInputs {
                lens: "tx_latency",
                kind: "tx_family",
                suspected_spans: &[],
                contract_function: None,
            },
        )
        .unwrap();
        assert!(hits.is_empty(), "broad probe must return empty");
    }

    #[test]
    fn outcome_parse_roundtrip() {
        assert_eq!(Outcome::parse("rejected").unwrap(), Outcome::Rejected);
        assert_eq!(Outcome::parse("not_actionable").unwrap(), Outcome::NotActionable);
        assert!(Outcome::parse("garbage").is_err());
    }

    #[test]
    fn canonical_contract_key_shapes() {
        assert_eq!(canonical_contract_key("SP", "foo", Some("bar")), Some("SP.foo.bar".to_owned()),);
        assert_eq!(canonical_contract_key("SP", "foo", None), Some("SP.foo".to_owned()),);
        assert_eq!(canonical_contract_key("", "", None), None);
    }
}
