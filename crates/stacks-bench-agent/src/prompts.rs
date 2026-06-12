//! Disk-first prompt rendering via MiniJinja.
//!
//! Templates always live on disk in the operator's
//! [`LayoutSettings::prompt_overrides_dir`](crate::settings::LayoutSettings::prompt_overrides_dir).
//! The tool binary carries each template as `include_str!` content and
//! seeds the operator's directory on startup with **don't-replace-if-exists**
//! semantics, so a freshly cloned operator gets a working baseline and a
//! tuned operator keeps their edits.
//!
//! Rendering happens at runtime through MiniJinja in strict mode
//! (undefined variables are an error, not silent ""). Drift between
//! operator-edited templates and the prompt struct field set is caught by
//! [`lint`] — invoked by users via `sbagent prompt lint` and by tests via
//! [`tests::bundled_templates_lint_clean`].
//!
//! # Why disk-first?
//!
//! 1. Operators can iterate on prompts without rebuilding the tool.
//! 2. Tuned templates are visible in `git diff` for the operator repo.
//! 3. Future autonomy can have agents propose prompt changes — possible only
//!    when prompts live in a known on-disk location.
//!
//! This mirrors [karpathy/autoresearch](https://github.com/karpathy/autoresearch)'s
//! `program.md` model — the iterable human/agent contract.
//!
//! # Lifecycle
//!
//! 1. `sbagent` startup → [`seed_to`] populates `layout.prompt_overrides_dir`
//!    with any missing templates from the bundled defaults.
//! 2. Phase invocation → [`render`] reads `<dir>/<FILE_NAME>` and renders with
//!    MiniJinja strict mode, populating fields from the struct's
//!    [`serde::Serialize`] output.
//! 3. `sbagent prompt lint` → [`lint`] parses every template + dry-renders
//!    against a synthetic but field-complete context to surface unknown
//!    references, type errors, and unparseable syntax before the agent ever
//!    sees the prompt.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Serialize;

/// Every renderable prompt struct implements this trait, providing the
/// filename of its template + the bundled default content. Implementations
/// are mechanical (one per struct, below).
pub trait Prompt: Serialize {
    /// Template filename inside the operator's `layout.prompt_overrides_dir`.
    const FILE_NAME: &'static str;
    /// Bundled-as-`include_str!` default content. Used by [`seed_to`] to
    /// populate the operator's dir on first run, and by [`lint`] as the
    /// reference set when comparing against operator-edited copies.
    const BUNDLED: &'static str;
}

/// Render `t`'s template by reading `<dir>/<T::FILE_NAME>` from disk and
/// rendering it with MiniJinja in strict mode (undefined references are a
/// hard error). The template MUST exist on disk — [`seed_to`] guarantees
/// this when called at startup.
pub fn render<T: Prompt>(label: &str, t: &T, dir: &Path) -> Result<String> {
    let path = dir.join(T::FILE_NAME);
    let src = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "reading {label} template at {} — has the operator's prompts dir been seeded? \
             (`sbagent` seeds it on startup if `layout.prompt_overrides_dir` is set)",
            path.display(),
        )
    })?;
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    // Preserve trailing newlines (Jinja2's default strip behavior would
    // truncate templates that intentionally end with `\n`).
    env.set_keep_trailing_newline(true);
    let tmpl = env
        .template_from_str(&src)
        .with_context(|| format!("parsing {label} template at {}", path.display()))?;
    tmpl.render(t)
        .with_context(|| format!("rendering {label} template at {}", path.display()))
}

/// Result of a [`seed_to`] call. Both vectors carry the canonical `FILE_NAME`
/// values so callers can log what changed (a fresh checkout will have a
/// long `seeded` list; a tuned operator will have a long `kept` list).
#[derive(Debug, Default)]
pub struct SeedReport {
    /// Templates written this call (file was missing).
    pub seeded: Vec<&'static str>,
    /// Templates left alone (file already existed).
    pub kept: Vec<&'static str>,
}

/// Seed `dir` with every bundled template, only writing files that don't
/// already exist. Idempotent: re-running keeps existing operator edits
/// untouched. Creates `dir` if missing.
///
/// Uses `O_CREAT|O_EXCL` so concurrent seed calls can't race-corrupt a
/// file; whichever caller wins the create gets the seed, others see
/// `AlreadyExists` and treat that as "kept."
pub fn seed_to(dir: &Path) -> Result<SeedReport> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating prompts overrides dir {}", dir.display()))?;
    let mut report = SeedReport::default();
    for (name, contents) in BUNDLED_TEMPLATES {
        let path = dir.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(contents.as_bytes())
                    .with_context(|| format!("writing seed template to {}", path.display()))?;
                report.seeded.push(name);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                report.kept.push(name);
            }
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("seeding template to {}", path.display(),)));
            }
        }
    }
    Ok(report)
}

/// Force-rewrite every bundled template to disk, **overwriting** operator
/// edits. Intended for explicit invocation only (`sbagent prompt sync
/// --force` or equivalent). Returns the list of files written.
pub fn sync_force(dir: &Path) -> Result<Vec<&'static str>> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating prompts overrides dir {}", dir.display()))?;
    let mut written = Vec::with_capacity(BUNDLED_TEMPLATES.len());
    for (name, contents) in BUNDLED_TEMPLATES {
        let path = dir.join(name);
        std::fs::write(&path, contents)
            .with_context(|| format!("force-syncing template to {}", path.display()))?;
        written.push(*name);
    }
    Ok(written)
}

/// One drift finding from comparing an on-disk template dir against
/// the embedded bundle. `sbagent check` reports these as warnings —
/// operator edits to prompts are legitimate (autoresearch's
/// `program.md` model), so drift is informational, not fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftEntry {
    /// Bundle entry doesn't exist at `<dir>/<file_name>`.
    Missing {
        /// Bundle file name.
        file_name: &'static str,
    },
    /// File exists on disk but doesn't byte-match the bundle.
    Differs {
        /// Bundle file name.
        file_name: &'static str,
    },
}

impl DriftEntry {
    /// File name this drift entry refers to.
    pub fn file_name(&self) -> &'static str {
        match self {
            Self::Missing { file_name } | Self::Differs { file_name } => file_name,
        }
    }
}

/// Compare every bundled template/reference doc against
/// `<dir>/<file_name>`, returning [`DriftEntry`] per mismatch. Used by
/// `sbagent check` for the warn-level prompt drift notice.
pub fn drift(dir: &Path) -> Result<Vec<DriftEntry>> {
    let mut out = Vec::new();
    for (name, contents) in BUNDLED_TEMPLATES {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                if s != *contents {
                    out.push(DriftEntry::Differs { file_name: name });
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                out.push(DriftEntry::Missing { file_name: name });
            }
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("reading on-disk template {}", path.display())));
            }
        }
    }
    Ok(out)
}

/// One issue produced by [`lint`]. Severity is implicit: presence of any
/// `LintFinding` means lint failed.
#[derive(Debug)]
pub struct LintFinding {
    /// Template filename (e.g. `"optimizer.md"`).
    pub template: &'static str,
    /// Human-readable description of what's wrong.
    pub message: String,
}

/// Validate every renderable template under `dir` by parsing it with
/// MiniJinja and dry-rendering against a field-complete synthetic context.
/// When the dry-render succeeds, also walk the rendered output for
/// `<!-- lint:example schema="<name>" -->` markers above ```json fences
/// and validate each marked body against the bundled JSON Schema. See
/// [`check_example_fences`] for the marker contract.
///
/// Returns one [`LintFinding`] per detected issue; an empty vector means
/// lint passed. Reference docs are not in this bundle — see
/// [`crate::context::lint_bundle`] for their validation.
pub fn lint(dir: &Path) -> Result<Vec<LintFinding>> {
    let mut findings = Vec::new();
    // Renderable templates: parse + dry-render against the matching struct,
    // then schema-validate any marker-tagged output examples in the rendered
    // text. Render failures skip example linting (no rendered text to walk).
    macro_rules! check {
        ($t:ty) => {{
            let path = dir.join(<$t>::FILE_NAME);
            match std::fs::read_to_string(&path) {
                Ok(src) => {
                    let mut env = minijinja::Environment::new();
                    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
                    env.set_keep_trailing_newline(true);
                    match env.template_from_str(&src) {
                        Ok(tmpl) => {
                            let synthetic = <$t>::synthetic_for_lint();
                            match tmpl.render(&synthetic) {
                                Ok(rendered) => {
                                    check_example_fences(<$t>::FILE_NAME, &rendered, &mut findings);
                                }
                                Err(e) => findings.push(LintFinding {
                                    template: <$t>::FILE_NAME,
                                    message: format!("dry-render failed: {e:#}"),
                                }),
                            }
                        }
                        Err(e) => findings.push(LintFinding {
                            template: <$t>::FILE_NAME,
                            message: format!("parse failed: {e:#}"),
                        }),
                    }
                }
                Err(e) if e.kind() == ErrorKind::NotFound => findings.push(LintFinding {
                    template: <$t>::FILE_NAME,
                    message: format!(
                        "template not on disk at {} — run `sbagent` once to seed",
                        path.display(),
                    ),
                }),
                Err(e) => findings.push(LintFinding {
                    template: <$t>::FILE_NAME,
                    message: format!("read failed: {e:#}"),
                }),
            }
        }};
    }
    check!(TriagePrompt);
    check!(AnalyzerPrompt);
    check!(MergePrompt);
    check!(OptimizerPrompt);
    check!(ResultsAnalyzerPrompt);
    check!(PrWriterPrompt);
    check!(IssueWriterPrompt);

    Ok(findings)
}

/// Walk `rendered` for output-example fences explicitly opted into
/// schema validation via an HTML-comment marker.
///
/// Marker contract:
///
/// ```text
/// <!-- lint:example schema="<short-name>" -->
/// ```json
/// { ... }
/// ```
/// ```
///
/// - The marker must appear on its own line. Optional blank lines may sit
///   between the marker and the fence opener.
/// - `<short-name>` resolves to `<short-name>.schema.json` in the bundled
///   schema table ([`crate::schemas::BUNDLED_SCHEMAS`]).
/// - Unmarked fences are skipped — opt-in, not opt-out. This avoids false
///   positives on input-data fences (`{{ var_json }}` blobs) and on inline
///   shape illustrations that don't represent a whole-doc schema.
///
/// Per-fence failure modes surface as separate [`LintFinding`] entries
/// so an operator can fix several at once:
///
/// - **unknown schema**: `lint:example schema="<x>"` names a schema that
///   doesn't exist in the bundle.
/// - **parse failed**: marker present and fence found but the JSON body doesn't
///   parse (likely a leftover MiniJinja placeholder that isn't quote-safe).
/// - **schema mismatch**: parsed body fails JSON Schema validation; the message
///   names every reported error path.
/// - **dangling marker**: marker line followed by something that isn't a
///   ```json fence within a few blank lines.
fn check_example_fences(template: &'static str, rendered: &str, findings: &mut Vec<LintFinding>) {
    use crate::schemas::BUNDLED_SCHEMAS;

    let lines: Vec<&str> = rendered.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(schema_name) = parse_example_marker(line) {
            // Marker found. Skip ahead over blank lines to the fence
            // opener.
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if j >= lines.len() || lines[j].trim() != "```json" {
                findings.push(LintFinding {
                    template,
                    message: format!(
                        "lint:example marker on line {} (schema=\"{schema_name}\") is not \
                         immediately followed by a ```json fence",
                        i + 1,
                    ),
                });
                i = j;
                continue;
            }
            // Collect fence body until the closing ``` on its own line.
            let body_start = j + 1;
            let mut body_end = body_start;
            while body_end < lines.len() && lines[body_end].trim() != "```" {
                body_end += 1;
            }
            if body_end >= lines.len() {
                findings.push(LintFinding {
                    template,
                    message: format!(
                        "lint:example fence opened on line {} (schema=\"{schema_name}\") never \
                         closes",
                        j + 1,
                    ),
                });
                i = lines.len();
                continue;
            }
            let body = lines[body_start..body_end].join("\n");

            // Resolve <name>.schema.json in the bundled set.
            let schema_filename = format!("{schema_name}.schema.json");
            let Some(schema_src) = BUNDLED_SCHEMAS
                .iter()
                .find(|(n, _)| *n == schema_filename)
                .map(|(_, src)| *src)
            else {
                findings.push(LintFinding {
                    template,
                    message: format!(
                        "lint:example schema=\"{schema_name}\" on line {} does not match any \
                         bundled schema",
                        i + 1,
                    ),
                });
                i = body_end + 1;
                continue;
            };

            // Parse + validate.
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(value) => match parse_and_compile_schema(schema_src) {
                    Ok(validator) => {
                        let errs: Vec<String> = validator
                            .iter_errors(&value)
                            .map(|e| format!("{e}"))
                            .collect();
                        if !errs.is_empty() {
                            findings.push(LintFinding {
                                template,
                                message: format!(
                                    "lint:example schema=\"{schema_name}\" (line {}) failed \
                                     validation: {}",
                                    j + 1,
                                    errs.join("; "),
                                ),
                            });
                        }
                    }
                    Err(e) => findings.push(LintFinding {
                        template,
                        message: format!(
                            "lint:example schema=\"{schema_name}\" failed to compile bundled \
                             schema: {e}",
                        ),
                    }),
                },
                Err(e) => findings.push(LintFinding {
                    template,
                    message: format!(
                        "lint:example fence on line {} (schema=\"{schema_name}\") is not \
                         parseable JSON: {e}",
                        j + 1,
                    ),
                }),
            }
            i = body_end + 1;
        } else {
            i += 1;
        }
    }
}

/// Parse `<!-- lint:example schema="<name>" -->` and return `<name>`.
/// Returns `None` if the line isn't a recognised marker. Tolerant of
/// surrounding whitespace and double or single quotes around the name.
fn parse_example_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix("<!--")
        .and_then(|s| s.strip_suffix("-->"))?
        .trim();
    let after_keyword = inner
        .strip_prefix("lint:example")?
        .trim_start();
    let rest = after_keyword
        .strip_prefix("schema=")?
        .trim_start();
    let (quote, body) = if let Some(b) = rest.strip_prefix('"') {
        ('"', b)
    } else if let Some(b) = rest.strip_prefix('\'') {
        ('\'', b)
    } else {
        return None;
    };
    let end = body.find(quote)?;
    Some(&body[..end])
}

/// Compile a bundled schema source string into a [`jsonschema::Validator`].
fn parse_and_compile_schema(src: &str) -> Result<jsonschema::Validator, String> {
    let value: serde_json::Value =
        serde_json::from_str(src).map_err(|e| format!("schema parse: {e}"))?;
    jsonschema::Validator::new(&value).map_err(|e| format!("schema compile: {e}"))
}

// ---------- Bundled template registry ----------

/// `(FILE_NAME, BUNDLED)` table for every renderable prompt template
/// seeded into the operator's prompts dir. Reference docs
/// (`non-targets.md`, `bucket-anchors.md`, `stacks-domain-context.md`)
/// live in a separate bundle — see [`crate::context`] — because they're
/// human-readable reference material with sidecar metadata, not
/// substitution-bearing templates.
const BUNDLED_TEMPLATES: &[(&str, &str)] = &[
    ("triage.md", TriagePrompt::BUNDLED),
    ("analyzer.md", AnalyzerPrompt::BUNDLED),
    ("merge-analyses.md", MergePrompt::BUNDLED),
    ("optimizer.md", OptimizerPrompt::BUNDLED),
    ("results-analyzer.md", ResultsAnalyzerPrompt::BUNDLED),
    ("pr-writer.md", PrWriterPrompt::BUNDLED),
    ("issue-writer.md", IssueWriterPrompt::BUNDLED),
];

// ---------- Renderable prompt structs ----------

/// Phase 1 — triage agent context. One instance per session.
#[derive(Serialize)]
pub struct TriagePrompt {
    /// Session id, used by the agent for output paths.
    pub opt_session_id: String,
    /// Absolute path to the session results dir.
    pub opt_session_dir: String,
    /// Stacks-bench data dir (SQLite db lives below this).
    pub stacks_bench_data_dir: String,
    /// Stacks-core checkout root.
    pub base: String,
    /// Baseline run id resolved earlier in the session.
    pub baseline_run_id: String,
    /// Baseline rerun id resolved earlier in the session.
    pub baseline_rerun_id: String,
    /// Pre-computed noise floor (percent), used by the agent to filter
    /// targets that aren't worth chasing.
    pub precomputed_noise_floor_pct: String,
    /// Absolute path to non-targets.md reference doc.
    pub non_targets_path: String,
    /// Absolute path to bucket-anchors.md reference doc.
    pub bucket_anchors_path: String,
    /// Absolute path to stacks-domain-context.md reference doc. Calibration
    /// anchor for scale + magnitude + terminology decisions.
    pub domain_context_path: String,
    /// Absolute path to candidates.schema.json.
    pub candidates_schema_path: String,
    /// Framework's queries dir (SQL templates).
    pub queries_dir: String,
    /// Per-session pre-rendered triage query outputs.
    pub triage_queries_dir: String,
    /// Comma-separated axis weights
    /// (`tx_latency,tenure_throughput,commit_time`).
    pub stacks_bench_axis_weights: String,
    /// Absolute operator memory dir. The agent passes this verbatim
    /// to `sbagent rejections probe --memory-dir <...>` so the
    /// sandboxed child reads the orchestrator-resolved ledger
    /// instead of whatever it would resolve from its own cwd.
    pub memory_dir: String,
}

impl Prompt for TriagePrompt {
    const FILE_NAME: &'static str = "triage.md";
    const BUNDLED: &'static str = include_str!("../templates/triage.md");
}

/// Phase 1.5 — analyzer agent context. One instance per family.
#[derive(Serialize)]
pub struct AnalyzerPrompt {
    /// Family id this analyzer is investigating.
    pub family_id: String,
    /// JSON-serialized single-family slice of candidates.json.
    pub family_json: String,
    /// Absolute output dir for the analyzer's analysis.json + writeups.
    pub output_dir: String,
    /// Stacks-core checkout root.
    pub base: String,
    /// Stacks-bench data dir (SQLite db lives below this).
    pub stacks_bench_data_dir: String,
    /// Framework's queries dir.
    pub queries_dir: String,
    /// Per-session pre-rendered triage query outputs.
    pub triage_queries_dir: String,
    /// Baseline run id (for drilldown queries).
    pub baseline_run_id: String,
    /// Absolute path to bucket-anchors.md.
    pub bucket_anchors_path: String,
    /// Absolute path to non-targets.md.
    pub non_targets_path: String,
    /// Absolute path to stacks-domain-context.md reference doc. Calibration
    /// anchor for scale + magnitude + terminology decisions.
    pub domain_context_path: String,
    /// Absolute path to analysis.schema.json.
    pub analysis_schema_path: String,
    /// Effective operator cap on `verification_replay.invocations[]`
    /// per target (see `analyzer.max_invocations_per_target`). Surfaced
    /// in the prompt so the agent budgets BEFORE emitting — exceeding
    /// this cap rejects the analyzer's output after the (expensive)
    /// Codex call.
    pub max_invocations_per_target: String,
}

impl Prompt for AnalyzerPrompt {
    const FILE_NAME: &'static str = "analyzer.md";
    const BUNDLED: &'static str = include_str!("../templates/analyzer.md");
}

/// Phase 1.7 — merge agent context. One instance per session.
#[derive(Serialize)]
pub struct MergePrompt {
    /// Session id.
    pub opt_session_id: String,
    /// Absolute path to the session results dir.
    pub opt_session_dir: String,
    /// Baseline run id.
    pub baseline_run_id: String,
    /// Baseline rerun id.
    pub baseline_rerun_id: String,
    /// Noise floor (percent) as a string.
    pub noise_floor_pct: String,
    /// Absolute path to bucket-anchors.md.
    pub bucket_anchors_path: String,
    /// Absolute path to optimization-targets.schema.json.
    pub optimization_targets_schema_path: String,
    /// Model id used by the merge call (passed through to the agent
    /// for traceability in its own writeup).
    pub codex_merge_model: String,
    /// Concatenated accepted analyses JSON (full corpus).
    pub accepted_analyses_json: String,
}

impl Prompt for MergePrompt {
    const FILE_NAME: &'static str = "merge-analyses.md";
    const BUNDLED: &'static str = include_str!("../templates/merge-analyses.md");
}

/// Phase 2 — optimizer subagent context. One instance per merged target.
#[derive(Serialize)]
pub struct OptimizerPrompt {
    /// Per-target git worktree (the optimizer's working dir).
    pub worktree_dir: String,
    /// Per-target output dir. Agent writes `optimizer-report.json` here;
    /// coordinator renders companion `implementation.md` / `abort.md`
    /// from that report post-validation.
    pub output_dir: String,
    /// JSON-serialized merged target.
    pub target_json: String,
    /// Delivery mode (`normal_pr` / `consensus_poc_pr` / `consensus_issue`).
    pub delivery_mode: String,
    /// Nextest filter expression for the PoC test scope (only used in
    /// `consensus_poc_pr` mode; empty otherwise).
    pub poc_test_scope_expr: String,
    /// Absolute path to non-targets.md.
    pub non_targets_path: String,
    /// Absolute path to stacks-domain-context.md reference doc. Calibration
    /// anchor for scale + magnitude + terminology decisions.
    pub domain_context_path: String,
    /// Absolute path to optimization-targets.schema.json.
    pub optimization_targets_schema_path: String,
    /// Absolute path to optimizer-report.schema.json. The agent
    /// validates its `optimizer-report.json` output against this
    /// schema before exit; mismatch is a hard validation failure that
    /// the coordinator surfaces.
    pub optimizer_report_schema_path: String,
    /// Persistent stacks-bench data dir. Retained for the coordinator-owned
    /// bench loop; the optimizer prompt itself must not run stacks-bench.
    pub stacks_bench_data_dir: String,
    /// Chainstate source path for coordinator-owned bench runs.
    pub bench_source_dir: String,
    /// Network identifier for coordinator-owned bench runs.
    pub bench_network: String,
    /// Optional warmup value for coordinator-owned bench runs.
    pub bench_warmup: String,
    /// Optional filter value for coordinator-owned bench runs.
    pub bench_filter: String,
    /// Optional shadow-dir root for coordinator-owned bench runs.
    pub bench_shadow_dir_root: String,
    /// Requested optimizer attempts. Currently coordinator-clamped to one;
    /// retained for the future coordinator-owned multi-attempt loop.
    pub optimizer_attempts: String,
    /// Wall-clock budget for future coordinator-owned optimizer loops.
    pub optimizer_budget_minutes: String,
}

impl Prompt for OptimizerPrompt {
    const FILE_NAME: &'static str = "optimizer.md";
    const BUNDLED: &'static str = include_str!("../templates/optimizer.md");
}

/// Phase 3.5 — results-analyzer agent context. One instance per
/// `bench_eligible` target whose Phase 3 candidate bench produced
/// run-ids.
#[derive(Serialize)]
pub struct ResultsAnalyzerPrompt {
    /// Session id.
    pub session_id: String,
    /// Target id this verdict belongs to.
    pub target_id: String,
    /// Absolute output dir (`analyze/<target>/`).
    pub output_dir: String,
    /// Stacks-core checkout root.
    pub base: String,
    /// Stacks-bench data dir (SQLite db lives below this).
    pub stacks_bench_data_dir: String,
    /// Framework's queries dir.
    pub queries_dir: String,
    /// JSON-serialized merged target (carries `verification_replay`).
    pub target_json: String,
    /// JSON-serialized optimizer report (`Implemented` variant only —
    /// targets with `Aborted` reports never reach Phase 3.5).
    pub optimizer_report_json: String,
    /// Per-target dir holding per-invocation candidate bench outputs
    /// (`<dir>/<invocation-id>/bench-run.json`).
    pub candidate_invocations_dir: String,
    /// Per-target dir holding per-invocation baseline bench outputs.
    pub baseline_invocations_dir: String,
    /// Absolute path to the candidate `InvocationRunIds` JSON.
    pub candidate_run_ids_path: String,
    /// Absolute path to the baseline `InvocationRunIds` JSON.
    pub baseline_run_ids_path: String,
    /// Absolute path to results-analysis.schema.json.
    pub results_analysis_schema_path: String,
}

impl Prompt for ResultsAnalyzerPrompt {
    const FILE_NAME: &'static str = "results-analyzer.md";
    const BUNDLED: &'static str = include_str!("../templates/results-analyzer.md");
}

/// Phase 5 — PR-writer agent context. One instance per PR-mode target.
#[derive(Serialize)]
pub struct PrWriterPrompt {
    /// Session id.
    pub opt_session_id: String,
    /// Merged target id.
    pub target_id: String,
    /// Per-target output dir (publish artifacts land here).
    pub output_dir: String,
    /// Per-target git worktree.
    pub worktree_dir: String,
    /// JSON-serialized merged target.
    pub target_json: String,
    /// JSON-serialized per-target experiment record (status + improvement).
    pub experiment_json: String,
    /// JSON-serialized Phase 3.5 verdict (`results-analysis.json`).
    /// Carries `pr_body_summary` (verbatim PR-body Result section),
    /// `caveats[]`, the per-invocation breakdown, and the
    /// `verdict` / `confidence` lattice the prompt frames around.
    /// `"{}"` when no verdict was produced (consensus_poc_pr targets,
    /// or normal_pr targets that aborted before Phase 3.5).
    pub results_analysis_json: String,
    /// Delivery mode (`normal_pr` / `consensus_poc_pr`).
    pub delivery_mode: String,
}

impl Prompt for PrWriterPrompt {
    const FILE_NAME: &'static str = "pr-writer.md";
    const BUNDLED: &'static str = include_str!("../templates/pr-writer.md");
}

/// Phase 5 — issue-writer agent context. One instance per
/// `consensus_issue` target.
#[derive(Serialize)]
pub struct IssueWriterPrompt {
    /// Session id.
    pub opt_session_id: String,
    /// Merged target id.
    pub target_id: String,
    /// Per-target output dir.
    pub output_dir: String,
    /// JSON-serialized merged target.
    pub target_json: String,
}

impl Prompt for IssueWriterPrompt {
    const FILE_NAME: &'static str = "issue-writer.md";
    const BUNDLED: &'static str = include_str!("../templates/issue-writer.md");
}

// ---------- Lint support: synthetic field-complete contexts ----------
//
// Each prompt struct has a `synthetic_for_lint` constructor returning a
// fully-populated instance with placeholder strings. `lint()` uses these
// to dry-render every bundled template, catching unknown references /
// parse errors / type mismatches.

impl TriagePrompt {
    fn synthetic_for_lint() -> Self {
        Self {
            opt_session_id: "lint".into(),
            opt_session_dir: "/tmp/lint".into(),
            stacks_bench_data_dir: "/tmp/lint/data".into(),
            base: "/tmp/lint/base".into(),
            baseline_run_id: "0".into(),
            baseline_rerun_id: "0".into(),
            precomputed_noise_floor_pct: "0".into(),
            non_targets_path: "/tmp/lint/non-targets.md".into(),
            bucket_anchors_path: "/tmp/lint/bucket-anchors.md".into(),
            domain_context_path: "/tmp/lint/stacks-domain-context.md".into(),
            candidates_schema_path: "/tmp/lint/candidates.schema.json".into(),
            queries_dir: "/tmp/lint/queries".into(),
            triage_queries_dir: "/tmp/lint/triage/queries".into(),
            stacks_bench_axis_weights: "0.4,0.4,0.2".into(),
            memory_dir: "/tmp/lint/memory".into(),
        }
    }
}

impl AnalyzerPrompt {
    fn synthetic_for_lint() -> Self {
        Self {
            family_id: "lint".into(),
            family_json: "{}".into(),
            output_dir: "/tmp/lint/out".into(),
            base: "/tmp/lint/base".into(),
            stacks_bench_data_dir: "/tmp/lint/data".into(),
            queries_dir: "/tmp/lint/queries".into(),
            triage_queries_dir: "/tmp/lint/triage/queries".into(),
            baseline_run_id: "0".into(),
            bucket_anchors_path: "/tmp/lint/bucket-anchors.md".into(),
            non_targets_path: "/tmp/lint/non-targets.md".into(),
            domain_context_path: "/tmp/lint/stacks-domain-context.md".into(),
            analysis_schema_path: "/tmp/lint/analysis.schema.json".into(),
            max_invocations_per_target: "8".into(),
        }
    }
}

impl MergePrompt {
    fn synthetic_for_lint() -> Self {
        Self {
            opt_session_id: "lint".into(),
            opt_session_dir: "/tmp/lint".into(),
            baseline_run_id: "0".into(),
            baseline_rerun_id: "0".into(),
            noise_floor_pct: "0".into(),
            bucket_anchors_path: "/tmp/lint/bucket-anchors.md".into(),
            optimization_targets_schema_path: "/tmp/lint/optimization-targets.schema.json".into(),
            codex_merge_model: "gpt-test".into(),
            accepted_analyses_json: "[]".into(),
        }
    }
}

impl OptimizerPrompt {
    fn synthetic_for_lint() -> Self {
        Self {
            worktree_dir: "/tmp/lint/worktree".into(),
            output_dir: "/tmp/lint/out".into(),
            target_json: "{}".into(),
            delivery_mode: "normal_pr".into(),
            poc_test_scope_expr: String::new(),
            non_targets_path: "/tmp/lint/non-targets.md".into(),
            domain_context_path: "/tmp/lint/stacks-domain-context.md".into(),
            optimization_targets_schema_path: "/tmp/lint/optimization-targets.schema.json".into(),
            optimizer_report_schema_path: "/tmp/lint/optimizer-report.schema.json".into(),
            stacks_bench_data_dir: "/tmp/lint/data".into(),
            bench_source_dir: "/tmp/lint/chainstate".into(),
            bench_network: "mainnet".into(),
            bench_warmup: "5000".into(),
            bench_filter: "contract-call".into(),
            bench_shadow_dir_root: "/tmp/lint/shadows".into(),
            optimizer_attempts: "5".into(),
            optimizer_budget_minutes: "60".into(),
        }
    }
}

impl ResultsAnalyzerPrompt {
    fn synthetic_for_lint() -> Self {
        Self {
            session_id: "lint".into(),
            target_id: "t".into(),
            output_dir: "/tmp/lint/out".into(),
            base: "/tmp/lint/base".into(),
            stacks_bench_data_dir: "/tmp/lint/data".into(),
            queries_dir: "/tmp/lint/queries".into(),
            target_json: "{}".into(),
            optimizer_report_json: "{}".into(),
            candidate_invocations_dir: "/tmp/lint/candidate".into(),
            baseline_invocations_dir: "/tmp/lint/baseline".into(),
            candidate_run_ids_path: "/tmp/lint/candidate-run-ids.json".into(),
            baseline_run_ids_path: "/tmp/lint/baseline-run-ids.json".into(),
            results_analysis_schema_path: "/tmp/lint/results-analysis.schema.json".into(),
        }
    }
}

impl PrWriterPrompt {
    fn synthetic_for_lint() -> Self {
        Self {
            opt_session_id: "lint".into(),
            target_id: "t".into(),
            output_dir: "/tmp/lint/out".into(),
            worktree_dir: "/tmp/lint/worktree".into(),
            target_json: "{}".into(),
            experiment_json: "{}".into(),
            results_analysis_json: "{}".into(),
            delivery_mode: "normal_pr".into(),
        }
    }
}

impl IssueWriterPrompt {
    fn synthetic_for_lint() -> Self {
        Self {
            opt_session_id: "lint".into(),
            target_id: "t".into(),
            output_dir: "/tmp/lint/out".into(),
            target_json: "{}".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeding into an empty dir writes every bundled template + reference
    /// doc; subsequent calls leave existing files alone.
    #[test]
    fn seed_writes_missing_and_keeps_existing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        let first = seed_to(dir).expect("first seed");
        assert_eq!(first.seeded.len(), BUNDLED_TEMPLATES.len());
        assert!(first.kept.is_empty());

        // Hand-edit one file to simulate an operator tune.
        let optimizer_path = dir.join("optimizer.md");
        std::fs::write(&optimizer_path, "OPERATOR EDIT\n").expect("write edit");

        let second = seed_to(dir).expect("second seed");
        assert!(second.seeded.is_empty());
        assert_eq!(second.kept.len(), BUNDLED_TEMPLATES.len());
        // Operator edit survived.
        assert_eq!(std::fs::read_to_string(&optimizer_path).unwrap(), "OPERATOR EDIT\n");
    }

    /// `sync_force` overwrites operator edits — deliberately. Used only by
    /// explicit operator action.
    #[test]
    fn sync_force_overwrites_operator_edits() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_to(dir).expect("seed");
        let optimizer_path = dir.join("optimizer.md");
        std::fs::write(&optimizer_path, "OPERATOR EDIT\n").expect("write edit");

        sync_force(dir).expect("sync");
        let after = std::fs::read_to_string(&optimizer_path).unwrap();
        assert!(after.contains("# Goal")); // bundled content
        assert!(!after.contains("OPERATOR EDIT"));
    }

    /// Rendering reads from disk + renders via MiniJinja strict.
    #[test]
    fn render_reads_from_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        std::fs::write(
            dir.join("issue-writer.md"),
            "issue for {{ target_id }} (session {{ opt_session_id }})\n",
        )
        .expect("write override");

        let prompt = IssueWriterPrompt {
            opt_session_id: "20260512".into(),
            target_id: "marf-prefetch".into(),
            output_dir: "/o".into(),
            target_json: "{}".into(),
        };
        let out = render("issue-writer", &prompt, dir).expect("render");
        assert_eq!(out, "issue for marf-prefetch (session 20260512)\n");
    }

    /// Unknown variable references error clearly under MiniJinja strict mode.
    #[test]
    fn render_unknown_field_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        std::fs::write(dir.join("issue-writer.md"), "uses {{ this_field_does_not_exist }}\n")
            .expect("write override");

        let prompt = IssueWriterPrompt {
            opt_session_id: "s".into(),
            target_id: "t".into(),
            output_dir: "/o".into(),
            target_json: "{}".into(),
        };
        let err = render("issue-writer", &prompt, dir).expect_err("must error");
        let msg = format!("{err:#}");
        // MiniJinja strict mode surfaces "undefined value" with source
        // line info; the field name itself appears in the chained error
        // details via `{:#}` formatting.
        assert!(msg.contains("undefined"), "{msg}");
    }

    /// Render errors clearly when the template file is missing — defense
    /// in depth in case startup seeding was skipped.
    #[test]
    fn render_missing_template_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let prompt = IssueWriterPrompt {
            opt_session_id: "s".into(),
            target_id: "t".into(),
            output_dir: "/o".into(),
            target_json: "{}".into(),
        };
        let err = render("issue-writer", &prompt, tmp.path()).expect_err("must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("issue-writer.md"), "{msg}");
    }

    /// Layer 1B v2 pass-b.1: the optimizer prompt MUST tell the agent
    /// to not touch `.git/` and not run `stacks-bench`. Those operations
    /// are coordinator-owned (sandbox blocks them from inside codex).
    /// The agent's job in b.1 is single-shot: edit + fmt + clippy +
    /// nextest, then declare outcome via marker file. Anything that
    /// would invite the agent back into git/bench territory needs to be
    /// caught here.
    #[test]
    fn optimizer_prompt_forbids_git_and_bench_operations() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_to(tmp.path()).expect("seed");
        let p = OptimizerPrompt::synthetic_for_lint();
        let rendered = render("optimizer", &p, tmp.path()).expect("render");

        // Hard "don't do this" markers. The agent's prompt must be
        // explicit about not touching `.git/` or running stacks-bench,
        // and must explain that the coordinator commits afterward.
        for required in [
            "must not touch `.git/`",
            "coordinator commits after you exit",
            "coordinator (outside the sandbox) owns commits",
        ] {
            assert!(
                rendered.contains(required),
                "rendered prompt missing required guardrail `{required}`:\n{rendered}",
            );
        }

        // The agent's marker file is the contract; no `git commit`
        // instructions should remain anywhere in the template.
        assert!(
            !rendered.contains("git commit "),
            "rendered prompt must not contain `git commit ` instructions in b.1:\n{rendered}",
        );
        assert!(
            !rendered.contains("git reset --hard"),
            "rendered prompt must not contain `git reset --hard` in b.1:\n{rendered}",
        );
        // No `stacks-bench bench run` invocations — bench is coordinator-side.
        assert!(
            !rendered.contains("stacks-bench bench run") && !rendered.contains("--shadow-dir-root"),
            "rendered prompt must not contain `stacks-bench bench run` / `--shadow-dir-root` in \
             b.1:\n{rendered}",
        );
    }

    #[test]
    fn results_analyzer_prompt_uses_db_evidence_hierarchy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_to(tmp.path()).expect("seed");
        let mut p = ResultsAnalyzerPrompt::synthetic_for_lint();
        p.target_json = serde_json::json!({
            "id": "marf-read-cache-rollback-wrapper",
            "evidence_queries": [{
                "purpose": "Confirm MARF read span dominates warm replay.",
                "sql_path": "queries/span_run_drift.sql",
                "params": { "run_id": "100", "span_name": "marf::read" },
                "output_path": "analysis/fam/queries/marf-read-drift.csv",
                "key_observation": "baseline p95 self_wall_us = 1240000us",
                "supports_invocations": ["warm-steady"]
            }],
            "verification_replay": {
                "rationale": "warm replay",
                "invocations": [{
                    "id": "warm-steady",
                    "label": "warm",
                    "expected_signal": { "axis": "tx_latency", "direction": "improves" }
                }]
            }
        })
        .to_string();

        let rendered = render("results-analyzer", &p, tmp.path()).expect("render");
        for expected in [
            "The DB is the primary mechanism evidence",
            "`bench-run.json` as the run",
            "is the envelope and coarse directional context",
            "evidence_queries[]",
            "queries/span_run_drift.sql",
            "compare_spans_between_runs.sql",
        ] {
            assert!(rendered.contains(expected), "missing `{expected}`:\n{rendered}");
        }
        assert!(
            !rendered.contains("bench-run.json` is your primary evidence"),
            "stale primary-evidence wording survived:\n{rendered}"
        );
    }

    /// v8 Phase 2 contract: the PR-writer prompt pins per-invocation table
    /// vocabulary (boolean → `yes`/`no`, no `true`/`false` drift) AND
    /// frames `mixed` verdicts as shippable-with-caveats — NOT a clean
    /// accept, NOT a near-rejection.
    #[test]
    fn pr_writer_prompt_pins_vocabulary_and_mixed_verdict_framing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_to(tmp.path()).expect("seed");
        let p = PrWriterPrompt::synthetic_for_lint();
        let rendered = render("pr-writer", &p, tmp.path()).expect("render");

        // Per-invocation table vocab: pin the canonical yes/no rendering
        // for the matches_expected_signal boolean so the smoke's
        // yes/no/true drift can't recur.
        for expected in [
            "Matches expected signal",
            "the literal string `yes` (true)",
            "or `no` (false)",
            "never `true`/`false`",
        ] {
            assert!(rendered.contains(expected), "missing vocab substring `{expected}`");
        }

        // Mixed verdict framing: explicit "shippable with caveats", not a
        // clean accept and not a near-rejection.
        for expected in [
            "verdict = \"mixed\"",
            "shippable with caveats",
            "ship with awareness",
            "verdict is mixed",
        ] {
            assert!(rendered.contains(expected), "missing mixed-framing substring `{expected}`");
        }

        // Anti-pattern guards: the smoke's three drift cases.
        assert!(
            !rendered.contains("`true` or `false`"),
            "vocab drift: PR writer should pin yes/no, not pass through booleans"
        );
    }

    /// v8 Phase 1 contract: the results-analyzer prompt carries the smoke-
    /// session calibration anchor for the canonical mixed/medium case AND
    /// states the estimate-gap policy (overshoots stay `high`, gaps go to
    /// `caveats` not auto-demotion).
    #[test]
    fn results_analyzer_prompt_carries_v8_calibration_anchor_and_estimate_gap_policy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_to(tmp.path()).expect("seed");
        let p = ResultsAnalyzerPrompt::synthetic_for_lint();
        let rendered = render("results-analyzer", &p, tmp.path()).expect("render");

        // MARF calibration anchor — the smoke session's mixed/medium case.
        for expected in [
            "Calibration anchor",
            "MARF deferred-seal",
            "20260611-172955",
            "matches_expected_signal: false",
        ] {
            assert!(rendered.contains(expected), "missing anchor substring `{expected}`");
        }

        // Estimate-gap policy: overshoots stay `high`, gaps go to caveats.
        for expected in [
            "Estimate gaps are caveats, not confidence demotions",
            "do not demote to `medium` solely because",
            "Overshooting is",
            "a clean win",
        ] {
            assert!(rendered.contains(expected), "missing policy substring `{expected}`");
        }
    }

    /// Every bundled template parses + renders cleanly under MiniJinja
    /// strict mode against its struct's field set. Replaces the previous
    /// Askama compile-time drift check; same coverage, now at test time.
    #[test]
    fn bundled_templates_lint_clean() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        seed_to(dir).expect("seed");
        let findings = lint(dir).expect("lint");
        assert!(findings.is_empty(), "bundled templates failed lint: {:#?}", findings,);
    }

    /// Lint flags missing files cleanly.
    #[test]
    fn lint_reports_missing_template() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let findings = lint(tmp.path()).expect("lint");
        assert_eq!(findings.len(), BUNDLED_TEMPLATES.len());
        for f in &findings {
            assert!(
                f.message
                    .contains("not on disk"),
                "{:?}",
                f
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────
    // Schema-example lint (Phase 2 of v2-cleanup-and-workspace-hygiene)
    // ─────────────────────────────────────────────────────────────────

    /// Marker parser accepts double + single quotes and surrounding
    /// whitespace; rejects unrelated comments.
    #[test]
    fn parse_example_marker_recognises_supported_shapes() {
        assert_eq!(
            super::parse_example_marker(r#"<!-- lint:example schema="analysis" -->"#),
            Some("analysis"),
        );
        assert_eq!(
            super::parse_example_marker(r#"  <!-- lint:example schema='candidates' -->  "#),
            Some("candidates"),
        );
        assert_eq!(super::parse_example_marker("<!-- some other comment -->"), None);
        assert_eq!(super::parse_example_marker("not a comment at all"), None);
        // Missing closing quote → fail-closed.
        assert_eq!(super::parse_example_marker(r#"<!-- lint:example schema="oops -->"#), None,);
    }

    /// Marker on a fence whose body validates → no finding.
    #[test]
    fn check_example_fences_accepts_valid_body() {
        let rendered = "\
intro prose

<!-- lint:example schema=\"candidates\" -->

```json
{
  \"schema_version\": 2,
  \"session_id\": \"20260101-000000\",
  \"baseline_run_id\": 0,
  \"baseline_rerun_id\": 1,
  \"noise_floor_pct\": 0.0,
  \"candidates\": [],
  \"rejected_families\": [],
  \"lens_coverage\": {
    \"tx_latency\": 0,
    \"tenure_throughput\": 0,
    \"commit_time\": 0,
    \"weights_applied\": \"1,1,1\"
  }
}
```
";
        let mut findings = Vec::new();
        super::check_example_fences("triage.md", rendered, &mut findings);
        assert!(findings.is_empty(), "expected clean lint: {findings:#?}");
    }

    /// Marker on a fence whose body fails schema validation → one
    /// `failed validation` finding naming both the template and schema.
    #[test]
    fn check_example_fences_surfaces_schema_mismatch() {
        // Missing required field `session_id`.
        let rendered = "\
<!-- lint:example schema=\"candidates\" -->

```json
{
  \"schema_version\": 2,
  \"baseline_run_id\": 0,
  \"baseline_rerun_id\": 1,
  \"noise_floor_pct\": 0.0,
  \"candidates\": [],
  \"rejected_families\": [],
  \"lens_coverage\": {
    \"tx_latency\": 0,
    \"tenure_throughput\": 0,
    \"commit_time\": 0,
    \"weights_applied\": \"1,1,1\"
  }
}
```
";
        let mut findings = Vec::new();
        super::check_example_fences("triage.md", rendered, &mut findings);
        assert_eq!(findings.len(), 1, "expected one finding: {findings:#?}");
        let msg = &findings[0].message;
        assert!(msg.contains("candidates"), "schema name in message: {msg}");
        assert!(msg.contains("failed validation"), "verdict word in message: {msg}");
        assert_eq!(findings[0].template, "triage.md");
    }

    /// Unknown schema name → fail-loud finding rather than silent skip.
    #[test]
    fn check_example_fences_surfaces_unknown_schema() {
        let rendered = "\
<!-- lint:example schema=\"nonexistent\" -->

```json
{}
```
";
        let mut findings = Vec::new();
        super::check_example_fences("anywhere.md", rendered, &mut findings);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0]
                .message
                .contains("does not match any bundled schema"),
            "expected unknown-schema message: {}",
            findings[0].message,
        );
    }

    /// Marker followed by something that isn't a json fence → dangling
    /// marker finding (catches operator typos like `<!-- ... -->` then
    /// ` ```text ` ).
    #[test]
    fn check_example_fences_surfaces_dangling_marker() {
        let rendered = "\
<!-- lint:example schema=\"candidates\" -->

```text
not a json fence
```
";
        let mut findings = Vec::new();
        super::check_example_fences("anywhere.md", rendered, &mut findings);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0]
                .message
                .contains("not immediately followed by a ```json fence"),
            "expected dangling-marker message: {}",
            findings[0].message,
        );
    }

    /// Unmarked fences are skipped entirely — this is the contract that
    /// keeps input-data fences (`{{ var_json }}`) and inline shape
    /// illustrations from showering false positives.
    #[test]
    fn check_example_fences_skips_unmarked_fences() {
        let rendered = "\
no marker above the fence below — totally unrelated to lint:example

```json
{ \"this\": \"would never validate against any schema\" }
```
";
        let mut findings = Vec::new();
        super::check_example_fences("anywhere.md", rendered, &mut findings);
        assert!(findings.is_empty(), "unmarked fences must be skipped: {findings:#?}");
    }

    /// Marker on a fence whose body isn't parseable JSON → fail-loud
    /// (catches a partial MiniJinja interpolation that left a stray
    /// `{{ }}`).
    #[test]
    fn check_example_fences_surfaces_unparseable_json() {
        let rendered = "\
<!-- lint:example schema=\"candidates\" -->

```json
{ this is not JSON
```
";
        let mut findings = Vec::new();
        super::check_example_fences("anywhere.md", rendered, &mut findings);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0]
                .message
                .contains("not parseable JSON"),
            "expected JSON parse error: {}",
            findings[0].message,
        );
    }
}
