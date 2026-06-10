//! Shared types used across the v2 artifact models.
//!
//! Anything that appears in more than one of the four artifact JSON files
//! lives here: enums (lens, bucket, risk, breakage class, delivery mode),
//! the three-axis improvement vector, the hotspot record, and the lens
//! disposition propagated from analysis through merge into summary.

use anyhow::{Context as _, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::ValidateModel;

/// Selection lens — the value-axis triage promoted a candidate on, and that
/// the analyzer must dispose of explicitly. Carries through verbatim into
/// merge output and the summary.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SelectionLens {
    /// Wall-time savings under execution-bucket spans.
    TxLatency,
    /// Clarity-budget headroom freed (deterministic Clarity cost units).
    TenureThroughput,
    /// Wall-time savings under commit-bucket spans.
    CommitTime,
}

/// Family kind discriminator on triage candidates and downstream analyses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FamilyKind {
    /// Family identified by representative transaction shapes.
    TxFamily,
    /// Family identified by representative blocks.
    BlockFamily,
    /// Family identified by a specific contract.function.
    ContractFamily,
}

/// Work-bucket classification for a target. Determined by the nearest
/// `Segment: ...` ancestor of the target span in the trace tree.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    /// Target lives under `Segment: Tx Execution` / `Transaction`.
    BlockProcessing,
    /// Target lives under one of the commit-bucket Segment anchors.
    BlockCommit,
}

/// Subjective risk level on a proposed fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Low-risk: localized, mechanical, easily reverted.
    Low,
    /// Medium-risk: moderate scope, well-tested area.
    Medium,
    /// High-risk: cross-cutting, hard-to-verify, or touches subtle invariants.
    High,
}

/// Classification of a consensus-breaking change. Required when
/// `consensus_breaking == true`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BreakageClass {
    /// Recalibrating a Clarity-VM cost weight (e.g. `costs.toml`).
    ClarityCostWeight,
    /// Changing how the Clarity VM evaluates an opcode or function.
    ClarityVmBehavior,
    /// Block production logic (mempool ordering, tx admission, fee logic).
    /// Exercised by stacks-bench.
    MiningFlow,
    /// Block acceptance / header / signature checks. NOT exercised by
    /// stacks-bench — `poc_implementable` MUST be false.
    BlockValidation,
    /// MARF on-disk storage format change.
    MarfLayout,
    /// Other consensus-relevant serialization changes.
    OnChainFormat,
}

/// Derived routing decision computed by the merge phase from
/// `consensus_breaking` + `poc_implementable`. Drives Phase 5 publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    /// Performance fix — full bench, normal PR.
    NormalPr,
    /// Deliberate consensus change shipped as a PoC PR (scoped tests, no
    /// benchmark, draft PR with safety labels).
    ConsensusPocPr,
    /// Consensus change too large for PoC mode — issue-only.
    ConsensusIssue,
}

impl DeliveryMode {
    /// Derive a `DeliveryMode` from `consensus_breaking` + `poc_implementable`
    /// per the merge schema's if/then chain. `poc_implementable` is ignored
    /// (and irrelevant) when `consensus_breaking == false`.
    pub fn derive(consensus_breaking: bool, poc_implementable: Option<bool>) -> Self {
        match (consensus_breaking, poc_implementable) {
            (false, _) => Self::NormalPr,
            (true, Some(true)) => Self::ConsensusPocPr,
            (true, _) => Self::ConsensusIssue,
        }
    }

    /// True iff this delivery mode is benchmark-eligible (only `NormalPr`).
    pub fn bench_eligible(self) -> bool {
        matches!(self, Self::NormalPr)
    }
}

/// Per-axis honest estimate of a fix's percent reduction on each value lens.
/// All three axes are required; use `0` for axes the fix doesn't move.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImprovementVector {
    /// Percent reduction in execution-bucket wall time.
    pub tx_latency: f64,
    /// Percent reduction in the binding Clarity-cost axis.
    pub tenure_throughput: f64,
    /// Percent reduction in commit-bucket wall time.
    pub commit_time: f64,
}

/// Hotspot record carried on every analyzer-emitted target and merged target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hotspot {
    /// Span name as it appears in trace queries.
    pub span: String,
    /// Exclusive wall time at the target span (microseconds).
    pub self_wall_us: i64,
    /// Inclusive wall time at the target span (self plus subtree, µs).
    pub total_wall_us: i64,
    /// Number of times the span was entered in the run.
    pub calls: i64,
    /// `file.rs:LINE` location string.
    pub location: String,
}

/// Status of a lens disposition: either an analyzer committed at least one
/// target on the lens (`Addressed`) or it confirmed the lens signal is real
/// but structurally unfixable (`NotActionable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LensDispositionStatus {
    /// At least one target in the analysis claims non-trivial impact on the
    /// lens.
    Addressed,
    /// Analyzer drilled in, confirmed the signal, found no structural handle.
    /// Requires a code-level reason in the carrying record.
    NotActionable,
}

/// Per-family lens-disposition entry as it appears in
/// `optimization-targets.json` and `summary.json`. Independent of the
/// targets array — every accepted analysis contributes one entry here, even
/// when it contributes zero targets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LensDispositionEntry {
    /// Identifier of the family this disposition belongs to.
    #[schemars(regex(pattern = KEBAB_PATTERN))]
    pub family_id: String,
    /// Lens triage promoted the family on.
    pub lens: SelectionLens,
    /// Disposition status.
    pub status: LensDispositionStatus,
    /// Required when `status == NotActionable`; optional but allowed when
    /// `status == Addressed` (e.g. to note an unintuitive mechanism).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Schema version sentinel for v2 artifacts. Carried by `candidates.json`,
/// `optimizer-report.json`, and `coordinator-provenance.json` — shapes
/// that did not move in the Pass 1c cutover.
pub const SCHEMA_VERSION_V2: u32 = 2;

/// Schema version sentinel for v1 artifacts. Carried by the
/// `sessions.jsonl` ledger record, the analyzed-rejections ledger, and
/// Pass 1c's `results-analysis.json`.
pub const SCHEMA_VERSION_V1: u32 = 1;

/// Schema version sentinel for v3 artifacts. Carried by `analysis.json`
/// and `optimization-targets.json` — the artifacts that gained Pass
/// 1c semantics: analyzer-emitted `verification_replay` with one or
/// more `BenchInvocation`s, and finalize/summary fields sourced from
/// `results-analysis.json`. `summary.json` rode this version
/// transitionally before bumping to v4 alongside source provenance.
pub const SCHEMA_VERSION_V3: u32 = 3;

/// Schema version sentinel for v4 artifacts. Carried by `summary.json`,
/// which gains the four source-provenance fields (`source_url`,
/// `source_branch`, `source_sha`, `source_fetched_at`) populated from
/// `source.json` at session start.
pub const SCHEMA_VERSION_V4: u32 = 4;

/// Kebab-case identifier regex applied to family ids, target ids, and
/// fix signatures. Matches `^[a-z0-9][a-z0-9-]*$` — same pattern the
/// hand-written v2 schemas enforced.
pub const KEBAB_PATTERN: &str = "^[a-z0-9][a-z0-9-]*$";

/// Stricter kebab-case regex applied to `BenchInvocation.id`. Must start
/// with a lowercase letter, end with `[a-z0-9]` (no trailing hyphen), max
/// 40 chars. The id appears in on-disk artifact paths
/// (`verify/<target>/baseline-<id>-run-K/`), so the format is locked at
/// the schema level — a malformed id fails before any bench runs.
pub const INVOCATION_ID_PATTERN: &str = "^[a-z](?:[a-z0-9-]{0,38}[a-z0-9])?$";

/// Hard ceiling on `VerificationReplay.invocations.len()`. Validation
/// rejects above this at the model layer regardless of operator config;
/// the operator-configurable cap (`analyzer.max_invocations_per_target`,
/// default 3) lives at the coordinator level and is independently
/// applied pre-bench.
pub const BENCH_INVOCATION_HARD_MAX: usize = 16;

/// 0x-prefixed 64-hex-char regex used for stacks transaction hashes and
/// stacks index block hashes. Stable, globally unique, cryptographic —
/// preferred over stacks-bench DB synthetic integer ids (which are
/// data-dir-local and change on re-index) anywhere an identifier lives in
/// a long-lived artifact (`candidates.json`, `analysis.json`,
/// `optimization-targets.json`, events).
pub const HEX_HASH_PATTERN: &str = "^0x[0-9a-fA-F]{64}$";

/// Direction component of an [`ExpectedSignal`]. The results-analyzer
/// (Phase 3.5) judges measured-vs-expected SHAPE first using this field;
/// a direction mismatch is load-bearing (analyzer's mechanism hypothesis
/// is wrong) while a magnitude mismatch is advisory.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SignalDirection {
    /// Candidate is expected to be faster / produce a positive
    /// `improvement_pct` on this invocation.
    Improves,
    /// Candidate is expected to land within ±`tolerance_pct` of the
    /// baseline — used for control / corroboration invocations
    /// (e.g. cold first-touch should be unchanged when the
    /// optimization is a warm-cache win).
    Neutral,
    /// Candidate is expected to be slower / produce a negative
    /// `improvement_pct`. Used when the analyzer explicitly trades
    /// one workload shape for another.
    Regresses,
}

/// Per-invocation hypothesis: what the analyzer expects the measurement
/// to show. Direction is the LOAD-BEARING piece (results-analyzer judges
/// shape consistency first); magnitude is advisory (`estimate_pct`
/// optional; tolerance optional). Splitting the qualitative claim from
/// the quantitative one lets the analyzer commit to direction while
/// being honest about uncertain magnitudes ("warm should improve;
/// could be 5% or 50% depending on working-set hit rate").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpectedSignal {
    /// Selection lens the analyzer measured the candidate target against
    /// — same enum as `Candidate.selection_lens` and
    /// `LensDisposition.lens`.
    pub axis: SelectionLens,
    /// Direction component. Load-bearing.
    pub direction: SignalDirection,
    /// Optional point estimate of the magnitude as a signed percent.
    /// Convention: positive = faster (consistent with
    /// `improvement_pct`'s sign). Skipped from output when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_pct: Option<f64>,
    /// Optional non-negative tolerance band (also percent). When set
    /// alongside `estimate_pct`, the results-analyzer treats
    /// `[estimate_pct - tolerance_pct, estimate_pct + tolerance_pct]`
    /// as the expected range. With `direction == Neutral` and no
    /// `estimate_pct`, treats `[-tolerance_pct, +tolerance_pct]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance_pct: Option<f64>,
}

impl ValidateModel for ExpectedSignal {
    fn validate_model(&self) -> Result<()> {
        if let Some(t) = self.tolerance_pct
            && (!t.is_finite() || t < 0.0)
        {
            bail!("expected_signal.tolerance_pct = {t} must be finite and >= 0");
        }
        if let Some(e) = self.estimate_pct
            && !e.is_finite()
        {
            bail!("expected_signal.estimate_pct = {e} must be finite");
        }
        Ok(())
    }
}

/// Profiler mode applied to a [`BenchInvocation`]. v1 only accepts
/// `"rich"`; lean opt-in lands later when a real target needs it. The
/// flag-symmetry invariant is encoded here: baseline and candidate for
/// the same `id` MUST run with the same `profiler` value.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ProfilerMode {
    /// Full profile: no `--bench-spans-only`, no `--no-profiler-kv`.
    /// Span + profiler-kv data preserved for the verifier
    /// (Pass 2) and the results-analyzer (Pass 1c Phase 3.5).
    Rich,
}

/// Samples the analyzer wants a [`BenchInvocation`] to replay. Stacks-bench
/// treats txids and blocks as mutually-exclusive CLI inputs, so the
/// variants map 1:1 to `stacks-bench bench run` flag sets. Tagged enum
/// (`kind` discriminator) for sharp validation errors + readable JSON.
///
/// All identifier values in `Txids` / `Blocks` are 0x-prefixed
/// 64-hex-char hashes ([`HEX_HASH_PATTERN`]). Heights and synthetic ids
/// are NEVER acceptable; the analyzer picks identifiers from the
/// `tx_hash` / `stacks_block_hash` columns, not from synthetic db ids
/// that are data-dir-local.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BenchSamples {
    /// Transaction-replay invocation: `stacks-bench bench run --txid <hash>`
    /// for each entry.
    Txids {
        /// 1-16 representative transaction hashes.
        #[schemars(length(min = 1, max = 16))]
        #[schemars(inner(regex(pattern = HEX_HASH_PATTERN)))]
        txids: Vec<String>,
    },
    /// Block-replay invocation: `stacks-bench bench run --block <hash>`
    /// for each entry.
    Blocks {
        /// 1-16 representative stacks index block hashes.
        #[schemars(length(min = 1, max = 16))]
        #[schemars(inner(regex(pattern = HEX_HASH_PATTERN)))]
        blocks: Vec<String>,
    },
    /// Full-range block-window invocation: `stacks-bench bench run
    /// --start-at <N> --count <M>`. Used when the analyzer wants a
    /// canonical block range rather than a hand-picked sample.
    BlockRange {
        /// First block height to replay. >= 1.
        #[schemars(range(min = 1))]
        start_at: u64,
        /// Block count to replay. 1..=50_000 — sanity ceiling.
        #[schemars(range(min = 1, max = 50_000))]
        count: u64,
    },
}

impl ValidateModel for BenchSamples {
    fn validate_model(&self) -> Result<()> {
        match self {
            Self::Txids { txids } => {
                if txids.is_empty() {
                    bail!("bench_samples.txids must be non-empty");
                }
                if txids.len() > 16 {
                    bail!("bench_samples.txids has {} entries; max 16", txids.len());
                }
                for (i, h) in txids.iter().enumerate() {
                    if !is_hex_hash(h) {
                        bail!(
                            "bench_samples.txids[{i}] = {:?} is not a 0x-prefixed 64-hex-char \
                             hash (analyzers must pick txids from the `tx_hash` column, not \
                             synthetic DB ids)",
                            h
                        );
                    }
                }
            }
            Self::Blocks { blocks } => {
                if blocks.is_empty() {
                    bail!("bench_samples.blocks must be non-empty");
                }
                if blocks.len() > 16 {
                    bail!("bench_samples.blocks has {} entries; max 16", blocks.len());
                }
                for (i, h) in blocks.iter().enumerate() {
                    if !is_hex_hash(h) {
                        bail!(
                            "bench_samples.blocks[{i}] = {:?} is not a 0x-prefixed 64-hex-char \
                             hash (analyzers must pick blocks from the `stacks_block_hash` \
                             column, not synthetic DB ids)",
                            h
                        );
                    }
                }
            }
            Self::BlockRange { start_at, count } => {
                if *start_at == 0 {
                    bail!("bench_samples.block_range.start_at must be >= 1");
                }
                if *count == 0 {
                    bail!("bench_samples.block_range.count must be >= 1");
                }
                if *count > 50_000 {
                    bail!("bench_samples.block_range.count = {count} exceeds the 50_000 ceiling");
                }
            }
        }
        Ok(())
    }
}

/// Returns true when `s` is a 0x-prefixed 64-hex-char hash (matches
/// [`HEX_HASH_PATTERN`]). Hand-written to avoid pulling in a regex dep
/// for one validator; the rule is simple enough to inline. Shared by
/// every callsite that consumes analyzer-emitted txid / block hashes
/// (today: [`BenchSamples`] Rust-side validation; the on-disk JSON
/// schema's regex enforces the same shape at write time).
pub fn is_hex_hash(s: &str) -> bool {
    s.len() == 66
        && s.starts_with("0x")
        && s[2..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
}

/// One analyst-defined stacks-bench invocation. The analyzer decides
/// HOW the target gets measured — Phase 1.8 runs the baseline side of
/// each invocation, Phase 3 mirrors it on the candidate side with
/// identical `(samples, warmup, repetitions, profiler)`, and the
/// post-bench results-analyzer (Phase 3.5) pairs them by `id` to judge
/// `measured_pct` against `expected_signal`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BenchInvocation {
    /// Schema-validated path key. ASCII kebab-case, max 40 chars,
    /// regex [`INVOCATION_ID_PATTERN`]. Unique within a single
    /// `VerificationReplay.invocations` array (duplicates fail
    /// validation). Used in artifact paths
    /// (`verify/<target>/<id>/bench-run.json`,
    /// `optimize/<target>/<id>/bench-run.json`) and as the pairing key
    /// across baseline ↔ candidate ↔ results-analysis.
    #[schemars(regex(pattern = INVOCATION_ID_PATTERN))]
    pub id: String,
    /// Free-text human-facing label for prompts, the summary table, and
    /// the PR body. Non-blank.
    pub label: String,
    /// One-line purpose: what this invocation tests and why. Surfaced in
    /// the PR body and the results-analysis verdict.
    pub purpose: String,
    /// What to replay — tagged enum on `kind`.
    pub samples: BenchSamples,
    /// Number of warmup repetitions discarded before measurement starts
    /// (`--warmup` on stacks-bench). 0..=200.
    #[schemars(range(min = 0, max = 200))]
    pub warmup: u32,
    /// Number of measured repetitions (`--repetitions` on stacks-bench).
    /// 1..=200.
    #[schemars(range(min = 1, max = 200))]
    pub repetitions: u32,
    /// Profiler shape. v1: `rich` only. Baseline + candidate for the
    /// same `id` MUST match — that's the flag-symmetry contract.
    pub profiler: ProfilerMode,
    /// Analyzer hypothesis for this invocation. Direction load-bearing.
    pub expected_signal: ExpectedSignal,
}

impl ValidateModel for BenchInvocation {
    fn validate_model(&self) -> Result<()> {
        if !is_valid_invocation_id(&self.id) {
            bail!(
                "bench_invocation.id = {:?} must match {} (lowercase kebab-case, max 40 chars, no \
                 trailing hyphen)",
                self.id,
                INVOCATION_ID_PATTERN
            );
        }
        if self.label.trim().is_empty() {
            bail!("bench_invocation `{}`: label must be non-blank", self.id);
        }
        if self.purpose.trim().is_empty() {
            bail!("bench_invocation `{}`: purpose must be non-blank", self.id);
        }
        self.samples
            .validate_model()
            .with_context(|| format!("bench_invocation `{}`", self.id))?;
        if self.warmup > 200 {
            bail!(
                "bench_invocation `{}`: warmup = {} is out of range [0, 200]",
                self.id,
                self.warmup
            );
        }
        if !(1..=200).contains(&self.repetitions) {
            bail!(
                "bench_invocation `{}`: repetitions = {} is out of range [1, 200]",
                self.id,
                self.repetitions
            );
        }
        self.expected_signal
            .validate_model()
            .with_context(|| format!("bench_invocation `{}`", self.id))?;
        Ok(())
    }
}

/// Returns true when `id` matches [`INVOCATION_ID_PATTERN`]. Pulled out
/// so model validation, coordinator pre-flight, and tests share one
/// implementation (we avoid a runtime regex dep here — the rule is
/// simple enough to inline). `pub` so consumers outside this module
/// (results-analyzer's `PerInvocationResult` validation, future Pass 2
/// verifier output) reuse it.
pub fn is_valid_invocation_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 40 {
        return false;
    }
    let bytes = id.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    if !bytes
        .last()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Per-target replay recipe — the analyzer's verification plan rendered
/// as machine-readable inputs for stacks-bench's targeted-replay modes.
/// One target carries one [`VerificationReplay`], which carries one or
/// more [`BenchInvocation`]s — each invocation is one self-contained
/// `stacks-bench bench run` command. Phase 1.8 runs every invocation
/// against the baseline; Phase 3 runs the same invocations against the
/// candidate; the post-bench results-analyzer (Phase 3.5) pairs them
/// per-invocation and emits the headline verdict.
///
/// On `bench_eligible == true` (`delivery_mode == normal_pr`) targets
/// `verification_replay` is required. On consensus targets
/// (`consensus_poc_pr` / `consensus_issue`) it is optional and ignored
/// by the pipeline — those modes don't reach Phase 1.8/3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationReplay {
    /// One-line explanation of the overall measurement strategy. Why
    /// these specific invocations? E.g. "cold-first-touch isolates MARF
    /// node-cache misses; warm-steady measures the steady-state replay
    /// of the same block set." Surfaced in summaries.
    pub rationale: String,
    /// One or more invocations, each a self-contained `stacks-bench
    /// bench run` command. `len()` is bounded by
    /// [`BENCH_INVOCATION_HARD_MAX`] (currently 16). Invocation `id`s
    /// must be unique within this list.
    #[schemars(length(min = 1, max = 16))]
    pub invocations: Vec<BenchInvocation>,
    /// Optional list of span names the analyzer expects to move as a
    /// result of the proposed change. Free-form; used as analyzer
    /// hints to the post-bench results-analyzer (Phase 3.5). Not
    /// required for any pipeline step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspected_spans: Option<Vec<String>>,
}

impl ValidateModel for VerificationReplay {
    fn validate_model(&self) -> Result<()> {
        if self
            .rationale
            .trim()
            .is_empty()
        {
            bail!("verification_replay.rationale must be non-blank");
        }
        if self.invocations.is_empty() {
            bail!(
                "verification_replay.invocations must contain at least one entry (omit the field \
                 entirely if the target needs no targeted replay)"
            );
        }
        if self.invocations.len() > BENCH_INVOCATION_HARD_MAX {
            bail!(
                "verification_replay.invocations = {} exceeds hard max of {}",
                self.invocations.len(),
                BENCH_INVOCATION_HARD_MAX
            );
        }
        let mut seen = std::collections::HashSet::with_capacity(self.invocations.len());
        for (i, inv) in self
            .invocations
            .iter()
            .enumerate()
        {
            if !seen.insert(inv.id.as_str()) {
                bail!(
                    "verification_replay.invocations[{i}]: duplicate id {:?} (invocation ids must \
                     be unique within a target)",
                    inv.id
                );
            }
            inv.validate_model()
                .with_context(|| format!("verification_replay.invocations[{i}]"))?;
        }
        // Pass 1c invariant: every invocation on a target must share the
        // same `expected_signal.axis`. The results-analyzer (Phase 3.5)
        // commits to ONE `axis` per `results-analysis.json`; mixing
        // axes across invocations would produce a verdict denominated
        // against a lens that doesn't apply to half the data.
        // v2 may relax this (per-invocation axis with weighted
        // aggregation); v1 is single-axis.
        let first_axis = self.invocations[0]
            .expected_signal
            .axis;
        for (i, inv) in self
            .invocations
            .iter()
            .enumerate()
            .skip(1)
        {
            if inv.expected_signal.axis != first_axis {
                bail!(
                    "verification_replay.invocations[{i}].expected_signal.axis = {:?} differs \
                     from invocations[0].expected_signal.axis = {:?}; v1 requires every \
                     invocation on a target to share an axis (the results-analyzer commits to one \
                     lens per target)",
                    inv.expected_signal.axis,
                    first_axis,
                );
            }
        }
        if let Some(spans) = &self.suspected_spans {
            for (i, s) in spans.iter().enumerate() {
                if s.trim().is_empty() {
                    bail!("verification_replay.suspected_spans[{i}] must be non-blank");
                }
            }
        }
        Ok(())
    }
}

/// One row in [`InvocationRunIds`] — pairs an invocation `id` (from the
/// target's [`VerificationReplay`]) with the stacks-bench `benchmark_run`
/// row that materialized when that invocation ran. Used symmetrically by
/// Phase 1.8 (baseline side, written to
/// `verify/<target>/baseline-run-ids.json`) and Phase 3 (candidate side,
/// `optimize/<target>/candidate-run-ids.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvocationRunId {
    /// Pairing key — matches the corresponding
    /// [`BenchInvocation::id`] on the target.
    #[schemars(regex(pattern = INVOCATION_ID_PATTERN))]
    pub invocation_id: String,
    /// stacks-bench DB run id (`benchmark_run.id`).
    pub run_id: i64,
}

/// Typed model for `verify/<target>/baseline-run-ids.json` and
/// `optimize/<target>/candidate-run-ids.json`. Validates that each
/// `invocation_id` is unique within the array; loaders apply the
/// additional context check that the ids align with the target's
/// [`VerificationReplay`] entries.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvocationRunIds {
    /// One row per [`BenchInvocation`] that ran. Order preserves the
    /// analyzer's invocation order (the writer pushes entries in
    /// iteration order).
    pub entries: Vec<InvocationRunId>,
}

impl InvocationRunIds {
    /// Flat run-id vec in `entries[]` order. Used by surfaces that
    /// still consume `Vec<i64>` (e.g. [`Experiment::run_ids`]) until
    /// the per-invocation pairing lands on the summary model.
    pub fn run_ids(&self) -> Vec<i64> {
        self.entries
            .iter()
            .map(|e| e.run_id)
            .collect()
    }
}

impl ValidateModel for InvocationRunIds {
    fn validate_model(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::with_capacity(self.entries.len());
        for (i, e) in self
            .entries
            .iter()
            .enumerate()
        {
            if !is_valid_invocation_id(&e.invocation_id) {
                bail!(
                    "invocation_run_ids.entries[{i}]: invocation_id = {:?} does not match {}",
                    e.invocation_id,
                    INVOCATION_ID_PATTERN
                );
            }
            if e.run_id <= 0 {
                bail!(
                    "invocation_run_ids.entries[{i}]: run_id = {} must be > 0 (stacks-bench DB \
                     primary keys are 1-indexed)",
                    e.run_id
                );
            }
            if !seen.insert(e.invocation_id.as_str()) {
                bail!(
                    "invocation_run_ids.entries[{i}]: duplicate invocation_id {:?}",
                    e.invocation_id
                );
            }
        }
        Ok(())
    }
}

/// Phantom type that serializes / deserializes as the integer literal `2`
/// and emits `{"const": 2}` in the JSON Schema. Carrying this on each
/// top-level artifact struct forces the wire to declare its version
/// explicitly; an artifact written with the wrong literal fails to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersionV2;

impl SchemaVersionV2 {
    /// Return the underlying integer constant (always `2`).
    pub const fn get(self) -> u32 {
        SCHEMA_VERSION_V2
    }
}

impl Default for SchemaVersionV2 {
    fn default() -> Self {
        Self
    }
}

impl Serialize for SchemaVersionV2 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(SCHEMA_VERSION_V2)
    }
}

impl<'de> Deserialize<'de> for SchemaVersionV2 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = u32::deserialize(d)?;
        if v == SCHEMA_VERSION_V2 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "expected schema_version={SCHEMA_VERSION_V2}, got {v}"
            )))
        }
    }
}

impl JsonSchema for SchemaVersionV2 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("SchemaVersionV2")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "const": SCHEMA_VERSION_V2
        })
    }
}

/// V1 counterpart to [`SchemaVersionV2`]. Serializes as the integer
/// literal `1`; emits `{"const": 1}` in JSON Schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersionV1;

impl SchemaVersionV1 {
    /// Return the underlying integer constant (always `1`).
    pub const fn get(self) -> u32 {
        SCHEMA_VERSION_V1
    }
}

impl Default for SchemaVersionV1 {
    fn default() -> Self {
        Self
    }
}

impl Serialize for SchemaVersionV1 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(SCHEMA_VERSION_V1)
    }
}

impl<'de> Deserialize<'de> for SchemaVersionV1 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = u32::deserialize(d)?;
        if v == SCHEMA_VERSION_V1 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "expected schema_version={SCHEMA_VERSION_V1}, got {v}"
            )))
        }
    }
}

impl JsonSchema for SchemaVersionV1 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("SchemaVersionV1")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "const": SCHEMA_VERSION_V1
        })
    }
}

/// Phantom type that serializes / deserializes as the integer literal `3`
/// and emits `{"const": 3}` in the JSON Schema. Defined for Pass 1c's
/// upcoming bump of the v2 artifact set; not yet carried on any artifact
/// (the cutover commit flips usage sites). Mirrors [`SchemaVersionV2`]
/// and [`SchemaVersionV1`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersionV3;

impl SchemaVersionV3 {
    /// Return the underlying integer constant (always `3`).
    pub const fn get(self) -> u32 {
        SCHEMA_VERSION_V3
    }
}

impl Default for SchemaVersionV3 {
    fn default() -> Self {
        Self
    }
}

impl Serialize for SchemaVersionV3 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(SCHEMA_VERSION_V3)
    }
}

impl<'de> Deserialize<'de> for SchemaVersionV3 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = u32::deserialize(d)?;
        if v == SCHEMA_VERSION_V3 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "expected schema_version={SCHEMA_VERSION_V3}, got {v}"
            )))
        }
    }
}

impl JsonSchema for SchemaVersionV3 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("SchemaVersionV3")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "const": SCHEMA_VERSION_V3
        })
    }
}

/// Phantom type that serializes / deserializes as the integer literal `4`
/// and emits `{"const": 4}` in the JSON Schema. Carried by `summary.json`
/// post-v3 iteration (adds source-provenance fields). Mirrors
/// [`SchemaVersionV3`] / [`SchemaVersionV2`] / [`SchemaVersionV1`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersionV4;

impl SchemaVersionV4 {
    /// Return the underlying integer constant (always `4`).
    pub const fn get(self) -> u32 {
        SCHEMA_VERSION_V4
    }
}

impl Default for SchemaVersionV4 {
    fn default() -> Self {
        Self
    }
}

impl Serialize for SchemaVersionV4 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(SCHEMA_VERSION_V4)
    }
}

impl<'de> Deserialize<'de> for SchemaVersionV4 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = u32::deserialize(d)?;
        if v == SCHEMA_VERSION_V4 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "expected schema_version={SCHEMA_VERSION_V4}, got {v}"
            )))
        }
    }
}

impl JsonSchema for SchemaVersionV4 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("SchemaVersionV4")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "const": SCHEMA_VERSION_V4
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_hash(b: u8) -> String {
        format!("0x{}", std::iter::repeat_n(format!("{:02x}", b), 32).collect::<String>())
    }

    fn invocation(id: &str) -> BenchInvocation {
        BenchInvocation {
            id: id.to_owned(),
            label: format!("label for {id}"),
            purpose: format!("purpose for {id}"),
            samples: BenchSamples::Txids { txids: vec![hex_hash(0xab)] },
            warmup: 5,
            repetitions: 10,
            profiler: ProfilerMode::Rich,
            expected_signal: ExpectedSignal {
                axis: SelectionLens::TxLatency,
                direction: SignalDirection::Improves,
                estimate_pct: Some(4.5),
                tolerance_pct: Some(3.0),
            },
        }
    }

    #[test]
    fn invocation_rejects_uppercase_id() {
        let mut inv = invocation("warm-steady");
        inv.id = "Warm-Steady".to_owned();
        let e = inv
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains(INVOCATION_ID_PATTERN) || e.contains("kebab-case"), "{e}");
    }

    #[test]
    fn invocation_rejects_trailing_hyphen() {
        let mut inv = invocation("warm-steady");
        inv.id = "warm-".to_owned();
        let e = inv
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains(INVOCATION_ID_PATTERN) || e.contains("kebab-case"), "{e}");
    }

    #[test]
    fn invocation_rejects_id_over_40_chars() {
        let mut inv = invocation("warm-steady");
        inv.id = std::iter::repeat_n('a', 41).collect();
        let e = inv
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains(INVOCATION_ID_PATTERN) || e.contains("kebab-case"), "{e}");
    }

    #[test]
    fn invocation_rejects_repetitions_zero() {
        let mut inv = invocation("warm-steady");
        inv.repetitions = 0;
        let e = inv
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("repetitions"), "{e}");
    }

    #[test]
    fn invocation_rejects_repetitions_over_200() {
        let mut inv = invocation("warm-steady");
        inv.repetitions = 201;
        let e = inv
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("repetitions"), "{e}");
    }

    #[test]
    fn invocation_rejects_warmup_over_200() {
        let mut inv = invocation("warm-steady");
        inv.warmup = 201;
        let e = inv
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("warmup"), "{e}");
    }

    #[test]
    fn invocation_accepts_warmup_zero() {
        let mut inv = invocation("warm-steady");
        inv.warmup = 0;
        inv.validate_model()
            .expect("warmup = 0 is valid");
    }

    #[test]
    fn bench_samples_txids_rejects_empty() {
        let s = BenchSamples::Txids { txids: vec![] };
        let e = s
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("non-empty"), "{e}");
    }

    #[test]
    fn bench_samples_block_range_rejects_zero_start_at() {
        let s = BenchSamples::BlockRange { start_at: 0, count: 100 };
        let e = s
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("start_at"), "{e}");
    }

    #[test]
    fn bench_samples_block_range_rejects_excess_count() {
        let s = BenchSamples::BlockRange { start_at: 1, count: 50_001 };
        let e = s
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("ceiling"), "{e}");
    }

    #[test]
    fn expected_signal_rejects_negative_tolerance() {
        let es = ExpectedSignal {
            axis: SelectionLens::TxLatency,
            direction: SignalDirection::Improves,
            estimate_pct: Some(4.5),
            tolerance_pct: Some(-1.0),
        };
        let e = es
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("tolerance_pct"), "{e}");
    }

    #[test]
    fn expected_signal_rejects_nan_estimate() {
        let es = ExpectedSignal {
            axis: SelectionLens::TxLatency,
            direction: SignalDirection::Improves,
            estimate_pct: Some(f64::NAN),
            tolerance_pct: None,
        };
        let e = es
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("estimate_pct"), "{e}");
    }

    #[test]
    fn invocation_run_ids_rejects_duplicate() {
        let ids = InvocationRunIds {
            entries: vec![
                InvocationRunId {
                    invocation_id: "warm-steady".into(),
                    run_id: 100,
                },
                InvocationRunId {
                    invocation_id: "warm-steady".into(),
                    run_id: 101,
                },
            ],
        };
        let e = ids
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("duplicate"), "{e}");
    }

    #[test]
    fn invocation_run_ids_rejects_invalid_id_format() {
        let ids = InvocationRunIds {
            entries: vec![InvocationRunId {
                invocation_id: "Warm-Steady".into(),
                run_id: 100,
            }],
        };
        let e = ids
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains(INVOCATION_ID_PATTERN), "{e}");
    }

    #[test]
    fn invocation_run_ids_rejects_nonpositive_run_id() {
        for bad in [0, -1, i64::MIN] {
            let ids = InvocationRunIds {
                entries: vec![InvocationRunId {
                    invocation_id: "warm-steady".into(),
                    run_id: bad,
                }],
            };
            let e = ids
                .validate_model()
                .unwrap_err()
                .to_string();
            assert!(e.contains("must be > 0"), "run_id={bad} got: {e}");
        }
    }

    #[test]
    fn bench_samples_txids_rejects_malformed_hash() {
        let cases = [
            ("lol", "non-hex garbage"),
            ("0xLOL", "non-hex chars"),
            ("0xabcd", "too short"),
            ("0x", "empty after prefix"),
            ("abcdef0000000000000000000000000000000000000000000000000000000000", "missing 0x"),
            // 65 chars after 0x, wrong length.
            ("0xabcdef0000000000000000000000000000000000000000000000000000000000ab", "too long"),
        ];
        for (bad, label) in cases {
            let s = BenchSamples::Txids { txids: vec![bad.to_owned()] };
            let e = s
                .validate_model()
                .unwrap_err()
                .to_string();
            assert!(e.contains("64-hex-char"), "{label}: got: {e}");
        }
    }

    #[test]
    fn bench_samples_blocks_rejects_malformed_hash() {
        let s = BenchSamples::Blocks {
            blocks: vec!["not-a-hash".to_owned()],
        };
        let e = s
            .validate_model()
            .unwrap_err()
            .to_string();
        assert!(e.contains("64-hex-char"), "{e}");
    }

    #[test]
    fn bench_samples_txids_accepts_valid_hashes() {
        let s = BenchSamples::Txids {
            txids: vec![hex_hash(0xab), hex_hash(0xcd)],
        };
        s.validate_model()
            .expect("valid hashes accepted");
    }

    #[test]
    fn is_hex_hash_helper_canonical_cases() {
        assert!(is_hex_hash(&hex_hash(0x00)));
        assert!(is_hex_hash(&hex_hash(0xff)));
        // Uppercase hex chars accepted (HEX_HASH_PATTERN uses [0-9a-fA-F]).
        assert!(is_hex_hash(&format!("0x{}", "A".repeat(64))));
        assert!(!is_hex_hash(""));
        assert!(!is_hex_hash("0x"));
        assert!(!is_hex_hash("not-a-hash"));
        // 65 hex chars after 0x = wrong total length (67 vs required 66).
        assert!(!is_hex_hash(&format!("0x{}", "a".repeat(65))));
    }

    #[test]
    fn invocation_id_helper_rejects_empty_and_oversized() {
        assert!(!is_valid_invocation_id(""));
        assert!(!is_valid_invocation_id(&"a".repeat(41)));
        assert!(is_valid_invocation_id(&"a".repeat(40)));
        assert!(is_valid_invocation_id("a"));
        assert!(!is_valid_invocation_id("1a")); // must start with a letter
        assert!(is_valid_invocation_id("a1"));
        assert!(!is_valid_invocation_id("a-")); // no trailing hyphen
        assert!(is_valid_invocation_id("a-b"));
        assert!(!is_valid_invocation_id("a_b")); // underscore not allowed
        assert!(!is_valid_invocation_id("a b")); // space not allowed
    }
}
