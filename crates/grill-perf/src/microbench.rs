//! Kernel and collective microbenchmark evidence (#54).
//!
//! HTTP output speed cannot prove that an isolated kernel or collective
//! changed, so this module keeps kernel/fabric observations in their own
//! closed artifact family. Three provenance states are distinct and never
//! silently upgraded:
//!
//! * `declared` — an operator statement with no measurement bytes. A declared
//!   artifact carries no samples and is rejected as measurement evidence.
//! * `imported` — bytes this collector parsed but did not produce. Importing
//!   always records `imported`, even when the payload claims native execution,
//!   and the payload's own claim is retained separately for audit.
//! * `native_observed` — bytes produced by a program this collector launched,
//!   or by the in-process CPU reference below. This records *observation*,
//!   never authenticated execution: the collector cannot attest what a child
//!   process actually did, and every runtime pin it retains is reported as an
//!   unauthenticated observation rather than as provenance.
//!
//! Only one adapter executes inside this binary: the deterministic CPU
//! `sum_u64` reference over the vector `0..bound`. It proves operation
//! recording, inspection and exact comparison, and it is the only adapter that
//! may be reported as native-observed under the current CPU-only
//! authorization. The EXL3 E3 grouped expert adapter and the NCCL all-reduce
//! adapter are explicitly launched programs; their device execution is a
//! separately authorized window and their artifacts stay unexercised until
//! then.
//!
//! Grounding for the two shipped device adapters, pinned before
//! implementation (hashes recorded in this file, in the plans under
//! `crates/grill-perf/examples/` and in `docs/performance/KERNEL-FABRIC.md`):
//!
//! * `MiaAI-Lab/GLM-5.3-Flash-EXL3-2x-DGX-Sparks` at
//!   `f906ee990596486e10ddbe381efa6f0e496f77e3`:
//!   `tests/bench_e3_microbench.py` (operation and synthetic-routing source
//!   anchor) and `tests/test_exl3_overlay.py` (frozen parity tolerances).
//! * `NVIDIA/nccl-tests` `doc/PERFORMANCE.md` for the all-reduce byte
//!   definitions used by the reported bandwidth labels.
//!
//! Algorithmic payload bytes are `numel * sizeof(dtype)`. The factor
//! `2*(world-1)/world` is the nccl-tests *bus bandwidth* normalization for
//! all-reduce; it is labelled separately and is not measured physical link
//! traffic. A kernel or collective result never sets a serving-speed claim.
//!
//! Clocks and samples. Every artifact carries exactly one clock identity, kind,
//! unit, declared resolution and synchronization contract, and the plan must
//! declare the identical contract. A retained sample is the timer's own
//! decimal representation (`raw`) plus the exact rational nanosecond duration
//! converted from it, including scientific notation and fractional
//! nanoseconds. The declared clock resolution states the advertised timer
//! class; it never rounds, quantizes or gates a retained value, and a
//! sub-resolution digit is preserved rather than rejected. Every decision is
//! exact rational arithmetic — there is no floating-point decision path.
//!
//! Study shape. A native domain comparison needs at least three complete
//! measured acquisitions in EACH of the A, B and A2 roles, prospectively
//! declared in a study document before any acquisition is recorded. Missing,
//! failed or unusable acquisitions are never replaced, filtered or dropped:
//! they withhold the comparison. Within one acquisition the statistic is the
//! exact mean of the measured samples, or for rank scope the exact mean of the
//! complete per-repetition maxima; roles are then compared by pooling the A/A2
//! acquisition statistics into a reference envelope against the B envelope.

use crate::envelope::{self, Direction, EnvelopeDecision, EnvelopeReason, Rational};
use crate::evidence;
use crate::metrics;
use crate::model::{FILE_CAP, OrNull, Result};
use crate::policy::Outcome;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const KIND: &str = "microbench-artifact-v1";
pub const PLAN_KIND: &str = "microbench-plan-v1";
pub const STUDY_KIND: &str = "microbench-study-v1";
pub const RECEIPT_KIND: &str = "microbench-receipt-v1";
pub const VERSION: u32 = 1;

/// Bounded operator program output; a larger artifact is rejected.
pub const OUTPUT_CAP: usize = 1024 * 1024;
/// Bounded retained stderr for every operator program.
pub const STDERR_CAP: usize = 64 * 1024;
/// Polling cadence and post-signal drain/reap allowance, not real-time guarantees.
#[cfg(target_os = "linux")]
const PROGRAM_POLL: Duration = Duration::from_millis(2);
#[cfg(target_os = "linux")]
const PROGRAM_CLEANUP: Duration = Duration::from_millis(100);
/// Bounded significant digits in a retained decimal value.
pub const DECIMAL_DIGITS: usize = 32;
/// Bounded decimal exponent magnitude in a retained scientific value.
pub const DECIMAL_EXPONENT: u32 = 30;
/// Largest accepted single duration (~13 days) keeps exact arithmetic bounded.
pub const MAX_DURATION_NS: u64 = 1 << 50;
/// Largest accepted rank count for a declared collective.
pub const MAX_RANKS: u32 = 64;
/// Fewest complete measured acquisitions a role may declare.
pub const MIN_ROLE_ACQUISITIONS: u32 = 3;
/// Most acquisitions a single role may declare.
pub const MAX_ROLE_ACQUISITIONS: u32 = 64;
/// Most observations a producer may retain.
pub const MAX_OBSERVATIONS: usize = 32;

/// `tests/bench_e3_microbench.py` at f906ee990596486e10ddbe381efa6f0e496f77e3.
pub const E3_OPERATION_SOURCE_SHA256: &str =
    "f57e4fa6726b0110c44ce56b0b0968ba425e566ace6d89f2db3872aae13889ec";
/// `tests/test_exl3_overlay.py` at that revision: `_err_stats`,
/// `_assert_e3_within` and the frozen `E3_TOL_*` constants.
pub const E3_PARITY_SOURCE_SHA256: &str =
    "8777254d47cdbb89186ba3bd1ac2437cbebc7987e0791d36c68e7bccb3df2b35";
/// `NVIDIA/nccl-tests` `doc/PERFORMANCE.md`, the byte-definition source.
pub const NCCL_BYTE_SOURCE_SHA256: &str =
    "2242493053a6f9d5db1c6542a8ce7740be919778645125b94eeae440efd31edf";

pub const E3_ROUTING_ID: &str = "synthetic-zipf-permuted";
pub const E3_ACTIVATION_ID: &str = "randn-fp16-activation-seed-3";
pub const E3_WEIGHTS_ID: &str = "random-k4-trellis-shared-gate-up-suh";
pub const E3_REFERENCE_ID: &str = "apply-exl3-python-loop";
pub const SUM_REFERENCE_ID: &str = "n*(n-1)/2";
pub const REDUCE_INPUT_ID: &str = "input[i]=(i%251)+rank";
pub const REDUCE_REFERENCE_ID: &str = "world*(i%251)+world*(world-1)/2";

/// Frozen E3 tolerances from the pinned parity source: factor 1.5,
/// absolute-relative floor 1e-3, nRMSE floor 1e-4, coarse bound
/// `max(0.15, 0.08 * max(1.0, ref_max))`.
pub const E3_TOLERANCE: Tolerance = Tolerance::E3Rel {
    factor_milli: 1500,
    abs_rel_micro: 1000,
    nrmse_abs_micro: 100,
    coarse_abs_milli: 150,
    coarse_factor_milli: 80,
};

fn push(list: &mut Vec<Reason>, reason: Reason) {
    if !list.contains(&reason) {
        list.push(reason);
    }
}

/// Closed adapter set. No registry, discovery or script hook: a plan names one
/// adapter and, for the two external adapters, one explicitly declared program.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterId {
    #[serde(rename = "cpu-sum-u64-reference")]
    #[value(name = "cpu-sum-u64-reference")]
    CpuSumU64Reference,
    #[serde(rename = "exl3-e3-grouped")]
    #[value(name = "exl3-e3-grouped")]
    Exl3E3Grouped,
    #[serde(rename = "nccl-allreduce-sum")]
    #[value(name = "nccl-allreduce-sum")]
    NcclAllreduceSum,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Cpu,
    Device,
    Ranks,
}

impl AdapterId {
    pub fn name(self) -> &'static str {
        match self {
            Self::CpuSumU64Reference => "cpu-sum-u64-reference",
            Self::Exl3E3Grouped => "exl3-e3-grouped",
            Self::NcclAllreduceSum => "nccl-allreduce-sum",
        }
    }

    pub fn scope(self) -> Scope {
        match self {
            Self::CpuSumU64Reference => Scope::Cpu,
            Self::Exl3E3Grouped => Scope::Device,
            Self::NcclAllreduceSum => Scope::Ranks,
        }
    }

    /// Device and collective adapters are always operator programs; the CPU
    /// reference is executed in-process.
    fn external(self) -> bool {
        !matches!(self, Self::CpuSumU64Reference)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Declared,
    Imported,
    NativeObserved,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClockKind {
    ProcessMonotonic,
    DeviceEvent,
    HostMonotonic,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    Nanoseconds,
    Milliseconds,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Synchronization {
    /// Reading a monotonic clock immediately before and after the operation.
    ProcessClockReadBeforeAfter,
    /// CUDA events recorded around the operation with a device synchronize
    /// before reading each elapsed time.
    DeviceEventTimingAfterSynchronize,
    /// A completed collective barrier immediately before and after the timed
    /// collective, so the recorded duration spans the synchronized operation.
    CollectiveBarrierBeforeAfter,
}

impl Units {
    fn nanoseconds(self) -> u64 {
        match self {
            Self::Nanoseconds => 1,
            Self::Milliseconds => 1_000_000,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Nanoseconds => "nanoseconds",
            Self::Milliseconds => "milliseconds",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Dtype {
    U64,
    Int64,
    Float16,
}

impl Dtype {
    fn bytes(self) -> u64 {
        match self {
            Self::U64 | Self::Int64 => 8,
            Self::Float16 => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    E2Kernel,
    E3Grouped,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReduceOp {
    Sum,
}

/// Closed set of runtime observations a producer may retain. A name outside
/// this set is rejected at parse time, and unknown names cannot smuggle a
/// provenance claim into the artifact.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationName {
    TorchVersion,
    NcclVersion,
    Exl3ModuleSha256,
    Device,
    Experts,
    Topk,
    Tokens,
    Hidden,
    Intermediate,
    Cap,
    SkewMilli,
    RoutingSeed,
    LayerSeed,
    ParityRoutingSeed,
    ActivationSeed,
    FallbackTier,
    World,
    Numel,
    Dtype,
    Op,
    SourcesVerified,
    SourcesDeclared,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub name: ObservationName,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    pub id: String,
    pub kind: ClockKind,
    pub units: Units,
    /// Declared advertised timer resolution. It states the timer's class and
    /// never rounds, quantizes or gates a retained sample.
    pub resolution_ns: u64,
    pub synchronization: Synchronization,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub path: String,
    pub sha256: String,
}

/// The declared, fixable operation identity. Only the frozen cells below are
/// admitted: no shape, cap, skew or tier search exists.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Operation {
    SumU64 {
        bound: u32,
        dtype: Dtype,
    },
    Exl3Experts {
        hidden: u32,
        intermediate: u32,
        experts: u32,
        topk: u32,
        tokens: u32,
        layer_seed: u64,
        routing_seed: u64,
        parity_routing_seed: u64,
        activation_seed: u64,
        skew_milli: u32,
        tier: Tier,
        cap: u32,
        dtype: Dtype,
        routing: String,
        activation: String,
        weights: String,
    },
    AllReduce {
        op: ReduceOp,
        dtype: Dtype,
        numel: u32,
        world: u32,
        ranks: Vec<u32>,
        input: String,
        reference: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Tolerance {
    ExactU64,
    ExactInt64,
    E3Rel {
        factor_milli: u32,
        abs_rel_micro: u32,
        nrmse_abs_micro: u32,
        coarse_abs_milli: u32,
        coarse_factor_milli: u32,
    },
}

impl Tolerance {
    fn factor_milli(self) -> u64 {
        match self {
            Self::E3Rel { factor_milli, .. } => u64::from(factor_milli),
            _ => 0,
        }
    }

    fn abs_rel_micro(self) -> u64 {
        match self {
            Self::E3Rel { abs_rel_micro, .. } => u64::from(abs_rel_micro),
            _ => 0,
        }
    }

    fn nrmse_abs_micro(self) -> u64 {
        match self {
            Self::E3Rel {
                nrmse_abs_micro, ..
            } => u64::from(nrmse_abs_micro),
            _ => 0,
        }
    }

    fn coarse_abs_milli(self) -> u64 {
        match self {
            Self::E3Rel {
                coarse_abs_milli, ..
            } => u64::from(coarse_abs_milli),
            _ => 0,
        }
    }

    fn coarse_factor_milli(self) -> u64 {
        match self {
            Self::E3Rel {
                coarse_factor_milli,
                ..
            } => u64::from(coarse_factor_milli),
            _ => 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CorrectnessContract {
    pub reference: String,
    pub tolerance: Tolerance,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Allowance {
    pub work_units: u64,
    pub memory_bytes: u64,
    pub deadline_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    pub adverse_bps: u32,
    pub spread_bps: u32,
}

/// Distinct ordered acquisition identity. Roles are compared only when their
/// declared starts are strictly ordered A < B < A2 and their ids differ.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Acquisition {
    pub id: String,
    pub started_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub kind: String,
    pub version: u32,
    pub adapter: AdapterId,
    /// SHA256 of the expected observed kernel implementation descriptor.
    pub revision: String,
    pub program_sha256: Option<String>,
    pub sources: Vec<Source>,
    pub operation: Operation,
    pub clock: Clock,
    pub warmups: u32,
    pub iterations: u32,
    pub allowance: Allowance,
    pub correctness: CorrectnessContract,
    pub thresholds: Option<Thresholds>,
    pub acquisition: Acquisition,
}

/// The declared candidate change axis of one study.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevisionAxis {
    pub baseline: String,
    pub candidate: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Roles<T> {
    pub baseline: T,
    pub candidate: T,
    pub reference: T,
}

/// Prospective declaration of a complete A/B/A2 group study. It is written
/// before any acquisition runs: every declared slot must be present and
/// complete, and nothing outside the declaration is admitted.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Study {
    pub kind: String,
    pub version: u32,
    pub adapter: AdapterId,
    pub revision_axis: RevisionAxis,
    pub thresholds: Thresholds,
    /// Minimum complete measured acquisitions required in every role; never
    /// below [`MIN_ROLE_ACQUISITIONS`].
    pub minimum_acquisitions: u32,
    /// Ordered acquisition-slot ids per role. Each id is one acquisition
    /// directory named after the plan's `acquisition.id`.
    pub roles: Roles<Vec<String>>,
    pub started_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RankRef {
    pub index: u32,
    pub device: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub scope: Scope,
    pub world: Option<u32>,
    pub ranks: Vec<RankRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub warmups: u32,
    pub iterations: u32,
    pub deadline_ms: u64,
    pub work_units: u64,
    pub memory_bytes: u64,
    pub completed: bool,
    pub timed_out: bool,
    /// The last observed EXL3 fallback tier, when the adapter reports one. It
    /// must equal the declared tier; a silent lower-tier fallback withholds
    /// the measurement.
    pub observed_fallback: Option<Tier>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub index: u32,
    pub rank: Option<u32>,
    pub repetition: Option<u32>,
    /// The timer's own decimal representation, preserved verbatim, including
    /// scientific notation and any sub-resolution digits.
    pub raw: String,
    /// Exact rational nanosecond duration converted from `raw`. A fractional
    /// nanosecond is retained as a rational, never rounded.
    pub duration: Rational,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ParityStats {
    pub maxabs: String,
    pub per_token_max: String,
    pub per_token_p99: String,
    pub nrmse: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Pass,
    Fail,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Correctness {
    ExactSum {
        reference: String,
        computed: String,
        bound: String,
        passed: bool,
    },
    ExactReduction {
        reference: String,
        mismatches: u64,
        tolerance_elems: u64,
        finite: bool,
        passed: bool,
    },
    E3Parity {
        reference: String,
        finite: bool,
        ref_max: String,
        e2: ParityStats,
        e3: Box<ParityStats>,
        tolerance: Tolerance,
        passed: bool,
        outcome: CheckOutcome,
        detail: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    DeadlineExceeded,
    AllowanceExceeded,
    TierFallback,
    RankMissing,
    IncompleteSamples,
    CorrectnessFailed,
    NonFinite,
    AdapterError,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub kind: FailureKind,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub kind: String,
    pub version: u32,
    pub adapter: AdapterId,
    pub revision: String,
    pub acquisition: Acquisition,
    pub operation: Operation,
    /// Assigned by this collector; never taken from the payload.
    pub provenance: Provenance,
    /// Whatever the payload claimed about itself, retained for audit.
    pub submitted_provenance: Option<Provenance>,
    pub program_sha256: Option<String>,
    /// Observed runtime identity of the executing stack, never a declared
    /// implementation pin echoed back.
    pub kernel_revision: Option<String>,
    /// Sources whose bytes the producer actually read, with the hashes it
    /// computed from them.
    pub sources: Vec<Source>,
    /// Structured, bounded runtime facts. Observations are unauthenticated.
    pub observations: Vec<Observation>,
    pub topology: Topology,
    pub clock: Clock,
    pub execution: Execution,
    pub samples: Vec<Sample>,
    pub correctness: Correctness,
    pub failures: Vec<Failure>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Captured,
    Imported,
    Failed,
}

/// Hashes and lengths describe exactly the retained prefixes, not discarded bytes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProgramOutput {
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub kind: String,
    pub version: u32,
    pub adapter: AdapterId,
    pub provenance: Provenance,
    pub status: ReceiptStatus,
    pub plan_sha256: String,
    pub artifact_sha256: Option<String>,
    pub collector_sha256: String,
    pub program: Option<String>,
    pub program_sha256: Option<String>,
    pub source_sha256: Option<String>,
    pub program_output: Option<ProgramOutput>,
    pub started_unix_ms: u64,
    pub observation_unix_ms: u64,
    pub duration_ms: u64,
    pub command: Vec<String>,
    pub failure: Option<String>,
    pub claim: String,
}

/// Domain-owned rejection codes. Precedence and exits are the existing
/// `policy::Outcome` order: ERROR(1) invalid or corrupt, INCONCLUSIVE(2)
/// retained but unavailable, REGRESSION(3), PASS(0).
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Reason {
    InvalidPlan,
    InvalidStudy,
    InvalidArtifact,
    MissingArtifact,
    MissingAcquisition,
    UnexpectedAcquisition,
    RoleMinimum,
    WarmupCountMismatch,
    CaptureFailed,
    AdapterMismatch,
    IdentityDrift,
    ClockMismatch,
    IterationCountMismatch,
    SampleGridIncomplete,
    SamplePrecision,
    SampleOverflow,
    DurationOutOfRange,
    NonFinite,
    CorrectnessFailed,
    RetainedFailure,
    DeadlineExceeded,
    AllowanceExceeded,
    TierFallback,
    RoleReuse,
    ObservedStartsOutOfOrder,
    ProvenanceMismatch,
    DeclaredStartsOutOfOrder,
    ObservationMissing,
    ObservationDrift,
    CollectorMismatch,
    InvalidRational,
    InvalidBounds,
    ArithmeticOverflow,
    NonpositiveReference,
    ReferenceSpreadExceeded,
    EnvelopeStraddlesTolerance,
}

fn envelope_reason(reason: EnvelopeReason) -> Reason {
    match reason {
        EnvelopeReason::InvalidRational => Reason::InvalidRational,
        EnvelopeReason::InvalidBounds => Reason::InvalidBounds,
        EnvelopeReason::ArithmeticOverflow { .. } => Reason::ArithmeticOverflow,
        EnvelopeReason::NonpositiveReference => Reason::NonpositiveReference,
        EnvelopeReason::ReferenceSpreadExceeded => Reason::ReferenceSpreadExceeded,
        EnvelopeReason::EnvelopeStraddlesTolerance { .. } => Reason::EnvelopeStraddlesTolerance,
    }
}

/// Failure to read a retained duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurationError {
    /// Not a bounded decimal or scientific decimal.
    Malformed,
    /// The exact conversion does not fit the bounded rational representation.
    Overflow,
    /// The value is zero, which is not a usable duration for this cell.
    Zero,
}

/// Bounded exact decimal-to-rational conversion. Accepts `digits`, `digits.digits`
/// and a bounded `e`/`E` exponent, both plain and scientific, with neither sign
/// nor separator. Rejects `nan`, `inf`, signs, empty integer parts, more than
/// [`DECIMAL_DIGITS`] digits and exponents beyond [`DECIMAL_EXPONENT`].
pub fn exact_decimal(text: &str) -> Option<Rational> {
    let (numerator, denominator) = decimal_parts(text).ok()?;
    reduce(numerator, denominator)
}

fn decimal_parts(text: &str) -> std::result::Result<(u128, u128), DurationError> {
    if text.is_empty() || text.len() > 64 {
        return Err(DurationError::Malformed);
    }
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => {
            let (sign, digits) = match exponent.strip_prefix('-') {
                Some(digits) => (-1i64, digits),
                None => (1i64, exponent.strip_prefix('+').unwrap_or(exponent)),
            };
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(DurationError::Malformed);
            }
            let magnitude: u32 = digits.parse().map_err(|_| DurationError::Malformed)?;
            if magnitude > DECIMAL_EXPONENT {
                return Err(DurationError::Malformed);
            }
            (mantissa, sign * i64::from(magnitude))
        }
        None => (text, 0),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (mantissa, ""),
    };
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || whole.len() + fraction.len() > DECIMAL_DIGITS
    {
        return Err(DurationError::Malformed);
    }
    let digits = whole
        .bytes()
        .chain(fraction.bytes())
        .try_fold(0u128, |value, byte| {
            value.checked_mul(10)?.checked_add(u128::from(byte - b'0'))
        })
        .ok_or(DurationError::Overflow)?;
    let denominator = 10u128.pow(fraction.len() as u32);
    let divisor = gcd(digits, denominator);
    let mut numerator = digits / divisor;
    let mut denominator = denominator / divisor;
    let factor = 10u128.pow(exponent.unsigned_abs() as u32);
    // Cancel before multiplication so representable scientific decimals do not
    // overflow merely because their unreduced intermediate fraction is large.
    if exponent >= 0 {
        let divisor = gcd(factor, denominator);
        numerator = numerator
            .checked_mul(factor / divisor)
            .ok_or(DurationError::Overflow)?;
        denominator /= divisor;
    } else {
        let divisor = gcd(numerator, factor);
        numerator /= divisor;
        denominator = denominator
            .checked_mul(factor / divisor)
            .ok_or(DurationError::Overflow)?;
    }
    Ok((numerator, denominator))
}

/// Exact nanosecond conversion of a retained decimal in the declared units.
/// Fractional nanoseconds are retained as an exact rational; nothing is
/// rounded and no value is rejected for being finer than the declared
/// resolution.
pub fn duration_ns(text: &str, units: Units) -> std::result::Result<Rational, DurationError> {
    let (numerator, denominator) = decimal_parts(text)?;
    if numerator == 0 {
        return Err(DurationError::Zero);
    }
    let factor = u128::from(units.nanoseconds());
    let divisor = gcd(factor, denominator);
    let scaled = numerator
        .checked_mul(factor / divisor)
        .ok_or(DurationError::Overflow)?;
    let duration = reduce(scaled, denominator / divisor).ok_or(DurationError::Overflow)?;
    if order(
        duration,
        Rational {
            numerator: MAX_DURATION_NS,
            denominator: 1,
        },
    )
    .is_gt()
    {
        return Err(DurationError::Overflow);
    }
    Ok(duration)
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    a
}

fn reduce(numerator: u128, denominator: u128) -> Option<Rational> {
    if denominator == 0 {
        return None;
    }
    let divisor = gcd(numerator, denominator).max(1);
    Some(Rational {
        numerator: u64::try_from(numerator / divisor).ok()?,
        denominator: u64::try_from(denominator / divisor).ok()?,
    })
}

fn multiply(a: Rational, b: Rational) -> Option<Rational> {
    reduce(
        u128::from(a.numerator) * u128::from(b.numerator),
        u128::from(a.denominator) * u128::from(b.denominator),
    )
}

fn add(a: Rational, b: Rational) -> Option<Rational> {
    reduce(
        (u128::from(a.numerator) * u128::from(b.denominator))
            .checked_add(u128::from(b.numerator) * u128::from(a.denominator))?,
        u128::from(a.denominator) * u128::from(b.denominator),
    )
}

fn order(a: Rational, b: Rational) -> std::cmp::Ordering {
    (u128::from(a.numerator) * u128::from(b.denominator))
        .cmp(&(u128::from(b.numerator) * u128::from(a.denominator)))
}

fn larger(a: Rational, b: Rational) -> Rational {
    if order(a, b).is_lt() { b } else { a }
}

fn display(value: Rational) -> f64 {
    value.numerator as f64 / value.denominator as f64
}

fn mean(values: &[Rational]) -> Option<Rational> {
    if values.is_empty() || values.iter().any(|value| value.denominator == 0) {
        return None;
    }
    let mut total = Rational {
        numerator: 0,
        denominator: 1,
    };
    for value in values {
        total = add(total, *value)?;
    }
    reduce(
        u128::from(total.numerator),
        u128::from(total.denominator) * values.len() as u128,
    )
}

fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 256
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn printable(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}

/// The single admitted cell per adapter. Pinning this in code, not only in a
/// document, is what makes "no cap/skew/tier search" a checked property.
pub fn frozen_operation(adapter: AdapterId) -> Operation {
    match adapter {
        AdapterId::CpuSumU64Reference => Operation::SumU64 {
            bound: 4096,
            dtype: Dtype::U64,
        },
        AdapterId::Exl3E3Grouped => Operation::Exl3Experts {
            hidden: 4096,
            intermediate: 1024,
            experts: 288,
            topk: 8,
            tokens: 1024,
            layer_seed: 0,
            routing_seed: 1,
            parity_routing_seed: 3,
            activation_seed: 3,
            skew_milli: 1000,
            tier: Tier::E3Grouped,
            cap: 32,
            dtype: Dtype::Float16,
            routing: E3_ROUTING_ID.into(),
            activation: E3_ACTIVATION_ID.into(),
            weights: E3_WEIGHTS_ID.into(),
        },
        AdapterId::NcclAllreduceSum => Operation::AllReduce {
            op: ReduceOp::Sum,
            dtype: Dtype::Int64,
            numel: 262_144,
            world: 2,
            ranks: vec![0, 1],
            input: REDUCE_INPUT_ID.into(),
            reference: REDUCE_REFERENCE_ID.into(),
        },
    }
}

/// Admit only the frozen operation identity. The collective's world and rank
/// list are operator-declared and shape-checked rather than hard-pinned: a
/// fabric run may legitimately use a different participant count, and the
/// observed world is cross-checked against the declaration everywhere.
pub fn operation_admitted(adapter: AdapterId, operation: &Operation) -> bool {
    match (adapter, operation) {
        (
            AdapterId::NcclAllreduceSum,
            Operation::AllReduce {
                op,
                dtype,
                numel,
                world,
                ranks,
                input,
                reference,
            },
        ) => {
            *op == ReduceOp::Sum
                && *dtype == Dtype::Int64
                && *numel == 262_144
                && *world >= 2
                && *world <= MAX_RANKS
                && ranks.len() == *world as usize
                && ranks
                    .iter()
                    .enumerate()
                    .all(|(index, rank)| *rank == index as u32)
                && input == REDUCE_INPUT_ID
                && reference == REDUCE_REFERENCE_ID
        }
        _ => *operation == frozen_operation(adapter),
    }
}

pub fn required_clock(adapter: AdapterId) -> Clock {
    match adapter {
        AdapterId::CpuSumU64Reference => Clock {
            id: "process-monotonic-ns".into(),
            kind: ClockKind::ProcessMonotonic,
            units: Units::Nanoseconds,
            resolution_ns: 1,
            synchronization: Synchronization::ProcessClockReadBeforeAfter,
        },
        AdapterId::Exl3E3Grouped => Clock {
            id: "cuda-event-ms".into(),
            kind: ClockKind::DeviceEvent,
            units: Units::Milliseconds,
            // Declared microsecond-class device event timer. The adapter
            // retains the timer's own decimal expansion, so a sub-microsecond
            // digit is preserved rather than rounded to this class.
            resolution_ns: 1000,
            synchronization: Synchronization::DeviceEventTimingAfterSynchronize,
        },
        AdapterId::NcclAllreduceSum => Clock {
            id: "rank-monotonic-ns".into(),
            kind: ClockKind::HostMonotonic,
            units: Units::Nanoseconds,
            resolution_ns: 1,
            synchronization: Synchronization::CollectiveBarrierBeforeAfter,
        },
    }
}

pub fn frozen_correctness(adapter: AdapterId) -> CorrectnessContract {
    match adapter {
        AdapterId::CpuSumU64Reference => CorrectnessContract {
            reference: SUM_REFERENCE_ID.into(),
            tolerance: Tolerance::ExactU64,
        },
        AdapterId::Exl3E3Grouped => CorrectnessContract {
            reference: E3_REFERENCE_ID.into(),
            tolerance: E3_TOLERANCE,
        },
        AdapterId::NcclAllreduceSum => CorrectnessContract {
            reference: REDUCE_REFERENCE_ID.into(),
            tolerance: Tolerance::ExactInt64,
        },
    }
}

pub fn frozen_warmups(adapter: AdapterId) -> u32 {
    match adapter {
        AdapterId::CpuSumU64Reference | AdapterId::Exl3E3Grouped => 2,
        AdapterId::NcclAllreduceSum => 1,
    }
}

pub fn frozen_iterations(adapter: AdapterId) -> u32 {
    match adapter {
        AdapterId::CpuSumU64Reference | AdapterId::Exl3E3Grouped | AdapterId::NcclAllreduceSum => 5,
    }
}

/// Passes of the pinned `apply_exl3_experts` entry point every E3 acquisition
/// executes before timing starts: once as the E2 kernel reference and twice as
/// the E3 candidate and its repeat. The pinned Python reference loop is a
/// different implementation and is not counted as grouped-expert work.
pub const E3_PARITY_PASSES: u32 = 3;

/// Sound minimum `(work_units, memory_bytes)` for the frozen operation of one
/// adapter, derived from the declared operation and the pinned per-adapter
/// warmup and iteration counts, and from nothing else. Both producers mirror
/// this derivation and refuse a declared allowance below it before they import
/// a device runtime, while `check_artifact` refuses retained accounting below
/// it: an under-budget cell is rejected instead of being admitted and failing
/// only after the work has already run.
///
/// Units are fixed per adapter and are never converted between adapters:
///
/// * `work_units` counts one elementary operation of the measured cell: one
///   `u64` add per element per repetition for the CPU reference, one reduced
///   `int64` element per participating rank per repetition for the collective
///   (the `world` factor is why one rank-scoped number already covers every
///   rank), and, for the E3 cell, one multiply-accumulate counted as two over
///   the projection shape `tokens * topk * 3 * 2 * hidden * intermediate`.
///   Required warmups and the parity passes the cell must execute are included.
/// * `memory_bytes` counts PyTorch tensor payloads only — never device, driver,
///   NCCL or CUDA-context memory, and never a reserved or physical bound: the
///   `u64` vector, the five live collective payloads plus the boolean payload
///   of the exactness comparison, or the fp16 activation payload the timed E3
///   kernel cannot run without. The retained E3 observation stays the larger
///   process-local allocator peak, which is itself only PyTorch-visible memory.
pub fn minimum_allowance(adapter: AdapterId, operation: &Operation) -> Option<(u64, u64)> {
    let repetitions =
        u64::from(frozen_warmups(adapter)).checked_add(u64::from(frozen_iterations(adapter)))?;
    match (adapter, operation) {
        (AdapterId::CpuSumU64Reference, Operation::SumU64 { bound, .. }) => Some((
            u64::from(*bound).checked_mul(repetitions)?,
            u64::from(*bound).checked_mul(8)?,
        )),
        (
            AdapterId::NcclAllreduceSum,
            Operation::AllReduce {
                dtype,
                numel,
                world,
                ..
            },
        ) => {
            // Five live per-rank element payloads plus the one boolean element
            // payload the exactness comparison allocates; scalar or
            // library-internal temporaries below one element payload, and all
            // non-PyTorch memory, are outside this accounting.
            let payloads = dtype.bytes().checked_mul(5)?.checked_add(1)?;
            Some((
                u64::from(*world)
                    .checked_mul(u64::from(*numel))?
                    .checked_mul(repetitions)?,
                u64::from(*numel).checked_mul(payloads)?,
            ))
        }
        (
            AdapterId::Exl3E3Grouped,
            Operation::Exl3Experts {
                hidden,
                intermediate,
                topk,
                tokens,
                dtype,
                ..
            },
        ) => {
            let per_pass = u64::from(*tokens)
                .checked_mul(u64::from(*topk))?
                .checked_mul(3)?
                .checked_mul(2)?
                .checked_mul(u64::from(*hidden))?
                .checked_mul(u64::from(*intermediate))?;
            let passes = repetitions.checked_add(u64::from(E3_PARITY_PASSES))?;
            Some((
                per_pass.checked_mul(passes)?,
                u64::from(*tokens)
                    .checked_mul(u64::from(*hidden))?
                    .checked_mul(dtype.bytes())?,
            ))
        }
        _ => None,
    }
}

/// Required observation names, and the pinned values Grill re-checks against
/// the frozen operation. A producer that reports a number it did not use is
/// caught here; a producer that reports nothing required is rejected.
fn observations_admitted(
    adapter: AdapterId,
    operation: &Operation,
    observations: &[Observation],
) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if observations.len() > MAX_OBSERVATIONS {
        push(&mut reasons, Reason::InvalidArtifact);
        return reasons;
    }
    let mut seen = std::collections::BTreeSet::new();
    for observation in observations {
        if !seen.insert(observation.name) {
            push(&mut reasons, Reason::InvalidArtifact);
        }
        if !printable(&observation.value, 256) {
            push(&mut reasons, Reason::InvalidArtifact);
        }
    }
    let value = |name: ObservationName| {
        observations
            .iter()
            .find(|observation| observation.name == name)
            .map(|observation| observation.value.as_str())
    };
    let pinned =
        |name: ObservationName, expected: &str, reasons: &mut Vec<Reason>| match value(name) {
            Some(observed) if observed == expected => (),
            Some(_) => push(reasons, Reason::ObservationDrift),
            None => push(reasons, Reason::ObservationMissing),
        };
    let numeric =
        |name: ObservationName, expected: u64, reasons: &mut Vec<Reason>| match value(name)
            .and_then(|observed| observed.parse::<u64>().ok())
        {
            Some(observed) if observed == expected => (),
            Some(_) => push(reasons, Reason::ObservationDrift),
            None => push(reasons, Reason::ObservationMissing),
        };
    let present = |name: ObservationName, reasons: &mut Vec<Reason>| match value(name) {
        Some(observed) if printable(observed, 256) => (),
        _ => push(reasons, Reason::ObservationMissing),
    };
    match (adapter, operation) {
        (AdapterId::CpuSumU64Reference, _) => {
            if !observations.is_empty() {
                push(&mut reasons, Reason::InvalidArtifact);
            }
        }
        (AdapterId::Exl3E3Grouped, Operation::Exl3Experts { .. }) => {
            present(ObservationName::TorchVersion, &mut reasons);
            present(ObservationName::NcclVersion, &mut reasons);
            present(ObservationName::Device, &mut reasons);
            match value(ObservationName::Exl3ModuleSha256) {
                Some(observed) if evidence::is_digest(observed) => (),
                Some(_) => push(&mut reasons, Reason::ObservationDrift),
                None => push(&mut reasons, Reason::ObservationMissing),
            }
            numeric(ObservationName::Experts, 288, &mut reasons);
            numeric(ObservationName::Hidden, 4096, &mut reasons);
            numeric(ObservationName::Intermediate, 1024, &mut reasons);
            numeric(ObservationName::Tokens, 1024, &mut reasons);
            numeric(ObservationName::Topk, 8, &mut reasons);
            numeric(ObservationName::Cap, 32, &mut reasons);
            numeric(ObservationName::SkewMilli, 1000, &mut reasons);
            numeric(ObservationName::LayerSeed, 0, &mut reasons);
            numeric(ObservationName::RoutingSeed, 1, &mut reasons);
            numeric(ObservationName::ParityRoutingSeed, 3, &mut reasons);
            numeric(ObservationName::ActivationSeed, 3, &mut reasons);
            present(ObservationName::FallbackTier, &mut reasons);
            let verified =
                observed_count(observations, &mut reasons, ObservationName::SourcesVerified);
            let declared =
                observed_count(observations, &mut reasons, ObservationName::SourcesDeclared);
            if let (Some(verified), Some(declared)) = (verified, declared) {
                // Every pinned source of this adapter lives in the pinned
                // checkout, so it must be verified from bytes read on the
                // producing host rather than echoed as a declaration.
                if verified == 0 {
                    push(&mut reasons, Reason::ObservationMissing);
                }
                if declared != 0 {
                    push(&mut reasons, Reason::ObservationDrift);
                }
            }
        }
        (AdapterId::NcclAllreduceSum, Operation::AllReduce { world, .. }) => {
            present(ObservationName::TorchVersion, &mut reasons);
            present(ObservationName::NcclVersion, &mut reasons);
            present(ObservationName::Device, &mut reasons);
            numeric(ObservationName::World, u64::from(*world), &mut reasons);
            numeric(ObservationName::Numel, 262_144, &mut reasons);
            pinned(ObservationName::Dtype, "int64", &mut reasons);
            pinned(ObservationName::Op, "sum", &mut reasons);
            match observed_count(observations, &mut reasons, ObservationName::SourcesVerified) {
                // The collective adapter can always verify its own script; a
                // reference document it cannot resolve stays declared.
                Some(0) | None => push(&mut reasons, Reason::ObservationMissing),
                Some(_) => (),
            }
            observed_count(observations, &mut reasons, ObservationName::SourcesDeclared);
        }
        _ => push(&mut reasons, Reason::IdentityDrift),
    }
    reasons
}

/// Read a required counting observation. A missing or unparsable count is a
/// missing observation, never a zero.
fn observed_count(
    observations: &[Observation],
    reasons: &mut Vec<Reason>,
    name: ObservationName,
) -> Option<u64> {
    match observed(observations, name).and_then(|value| value.parse::<u64>().ok()) {
        Some(count) => Some(count),
        None => {
            push(reasons, Reason::ObservationMissing);
            None
        }
    }
}

/// Read one retained observation by name.
pub fn observed(observations: &[Observation], name: ObservationName) -> Option<&str> {
    observations
        .iter()
        .find(|observation| observation.name == name)
        .map(|observation| observation.value.as_str())
}

/// Exact payload bytes for the operation: `numel * sizeof(dtype)`. This is the
/// nccl-tests *algorithm* denominator, not the bus normalization.
pub fn algorithm_payload_bytes(operation: &Operation) -> Option<u64> {
    match operation {
        Operation::AllReduce { dtype, numel, .. } => u64::from(*numel).checked_mul(dtype.bytes()),
        _ => None,
    }
}

/// The nccl-tests all-reduce bus normalization `2*(world-1)/world`, retained
/// as an exact rational and labelled as a normalization, never as measured
/// physical link traffic.
pub fn bus_normalization(world: u32) -> Option<Rational> {
    if world < 2 {
        return None;
    }
    reduce(u128::from(2 * (world - 1)), u128::from(world))
}

/// Descriptive derivation for a rank-scoped operation. Both bandwidth values
/// are exact rationals derived from the acquisition statistic; the floating
/// display exists only for readability.
#[derive(Serialize)]
pub struct Derived {
    pub definition: &'static str,
    pub definition_source_sha256: &'static str,
    pub payload_bytes: u64,
    pub clock_id: String,
    pub units: &'static str,
    pub duration: Rational,
    pub duration_display_ns: f64,
    pub algorithm_bandwidth_bps: Rational,
    pub algorithm_bandwidth_display: f64,
    pub bus_normalization: Rational,
    pub bus_normalization_display: f64,
    pub bus_bandwidth_bps: Rational,
    pub bus_bandwidth_display: f64,
    pub bus_scope: &'static str,
}

fn derived(operation: &Operation, clock: &Clock, duration: Rational) -> Option<Derived> {
    let payload = algorithm_payload_bytes(operation)?;
    let world = match operation {
        Operation::AllReduce { world, .. } => *world,
        _ => return None,
    };
    if duration.numerator == 0 {
        return None;
    }
    let normalization = bus_normalization(world)?;
    // Exact bytes per second: payload * 1e9 / duration.
    let algorithm = reduce(
        u128::from(payload)
            .checked_mul(1_000_000_000)?
            .checked_mul(u128::from(duration.denominator))?,
        u128::from(duration.numerator),
    )?;
    let bus = multiply(algorithm, normalization)?;
    Some(Derived {
        definition: "algorithm bandwidth = payload_bytes / duration; payload_bytes = numel * sizeof(dtype)",
        definition_source_sha256: NCCL_BYTE_SOURCE_SHA256,
        payload_bytes: payload,
        clock_id: clock.id.clone(),
        units: clock.units.name(),
        duration,
        duration_display_ns: display(duration),
        algorithm_bandwidth_bps: algorithm,
        algorithm_bandwidth_display: display(algorithm),
        bus_normalization: normalization,
        bus_normalization_display: display(normalization),
        bus_bandwidth_bps: bus,
        bus_bandwidth_display: display(bus),
        bus_scope: "nccl-tests all-reduce bus-bandwidth normalization; not measured physical link traffic",
    })
}

struct Findings {
    invalid: Vec<Reason>,
    unavailable: Vec<Reason>,
}

impl Findings {
    fn new() -> Self {
        Self {
            invalid: Vec::new(),
            unavailable: Vec::new(),
        }
    }

    fn invalidate(&mut self, reason: Reason) {
        push(&mut self.invalid, reason);
    }

    fn withhold(&mut self, reason: Reason) {
        push(&mut self.unavailable, reason);
    }

    fn clean(&self) -> bool {
        self.invalid.is_empty() && self.unavailable.is_empty()
    }
}

/// Validate the prospective declaration on its own. The frozen cell is checked
/// here so a plan cannot drift into an unapproved shape, cap, tier or clock,
/// and the declared allowance must already cover the reference requirement.
pub fn check_plan(plan: &Plan) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if plan.kind != PLAN_KIND || plan.version != VERSION {
        push(&mut reasons, Reason::InvalidPlan);
        return reasons;
    }
    if !evidence::is_digest(&plan.revision) {
        push(&mut reasons, Reason::InvalidPlan);
    }
    if !identifier(&plan.acquisition.id) || plan.acquisition.started_unix_ms == 0 {
        push(&mut reasons, Reason::InvalidPlan);
    }
    if !operation_admitted(plan.adapter, &plan.operation) {
        push(&mut reasons, Reason::IdentityDrift);
    }
    if plan.clock != required_clock(plan.adapter) {
        push(&mut reasons, Reason::ClockMismatch);
    }
    if plan.correctness != frozen_correctness(plan.adapter) {
        push(&mut reasons, Reason::IdentityDrift);
    }
    if plan.warmups != frozen_warmups(plan.adapter) {
        push(&mut reasons, Reason::WarmupCountMismatch);
    }
    if plan.iterations != frozen_iterations(plan.adapter) {
        push(&mut reasons, Reason::IterationCountMismatch);
    }
    if plan.allowance.work_units == 0
        || plan.allowance.memory_bytes == 0
        || plan.allowance.deadline_ms == 0
        || plan.allowance.deadline_ms > 3_600_000
    {
        push(&mut reasons, Reason::InvalidPlan);
    }
    if let Some(thresholds) = plan.thresholds
        && (thresholds.adverse_bps > 9999 || thresholds.spread_bps > 1_000_000)
    {
        push(&mut reasons, Reason::InvalidBounds);
    }
    for source in &plan.sources {
        if source.path.is_empty()
            || source.path.len() > 4096
            || !evidence::is_digest(&source.sha256)
        {
            push(&mut reasons, Reason::InvalidPlan);
        }
    }
    // A positive allowance is not enough for any adapter: the declaration must
    // already cover the frozen cell's known minimum work and memory, or a
    // device plan is admitted on a token budget and fails only after the work
    // has run.
    match minimum_allowance(plan.adapter, &plan.operation) {
        Some((work, memory))
            if work <= plan.allowance.work_units && memory <= plan.allowance.memory_bytes => {}
        _ => push(&mut reasons, Reason::AllowanceExceeded),
    }
    match plan.adapter {
        AdapterId::CpuSumU64Reference => {
            if plan.program_sha256.is_some() || !plan.sources.is_empty() {
                push(&mut reasons, Reason::InvalidPlan);
            }
        }
        AdapterId::Exl3E3Grouped => {
            if !plan
                .program_sha256
                .as_deref()
                .is_some_and(evidence::is_digest)
                || !plan
                    .sources
                    .iter()
                    .any(|source| source.sha256 == E3_OPERATION_SOURCE_SHA256)
                || !plan
                    .sources
                    .iter()
                    .any(|source| source.sha256 == E3_PARITY_SOURCE_SHA256)
            {
                push(&mut reasons, Reason::IdentityDrift);
            }
        }
        AdapterId::NcclAllreduceSum => {
            if !plan
                .program_sha256
                .as_deref()
                .is_some_and(evidence::is_digest)
                || !plan
                    .sources
                    .iter()
                    .any(|source| source.sha256 == NCCL_BYTE_SOURCE_SHA256)
            {
                push(&mut reasons, Reason::IdentityDrift);
            }
            if let Operation::AllReduce { world, ranks, .. } = &plan.operation
                && (*world < 2
                    || *world > MAX_RANKS
                    || ranks.len() != *world as usize
                    || ranks.iter().enumerate().any(|(i, r)| *r != i as u32))
            {
                push(&mut reasons, Reason::IdentityDrift);
            }
        }
    }
    reasons
}

/// Validate the prospective A/B/A2 study declaration. Nothing here consults
/// measured bytes: the declaration must already be complete and bounded.
pub fn check_study(study: &Study) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if study.kind != STUDY_KIND || study.version != VERSION {
        push(&mut reasons, Reason::InvalidStudy);
        return reasons;
    }
    if study.started_unix_ms == 0
        || !evidence::is_digest(&study.revision_axis.baseline)
        || !evidence::is_digest(&study.revision_axis.candidate)
    {
        push(&mut reasons, Reason::InvalidStudy);
    }
    if study.thresholds.adverse_bps > 9999 || study.thresholds.spread_bps > 1_000_000 {
        push(&mut reasons, Reason::InvalidBounds);
    }
    if study.minimum_acquisitions < MIN_ROLE_ACQUISITIONS
        || study.minimum_acquisitions > MAX_ROLE_ACQUISITIONS
    {
        push(&mut reasons, Reason::RoleMinimum);
    }
    let mut all: Vec<&String> = Vec::new();
    for slots in [
        &study.roles.baseline,
        &study.roles.candidate,
        &study.roles.reference,
    ] {
        if slots.len() < MIN_ROLE_ACQUISITIONS as usize
            || slots.len() > MAX_ROLE_ACQUISITIONS as usize
            || slots.len() < study.minimum_acquisitions as usize
        {
            push(&mut reasons, Reason::RoleMinimum);
        }
        for slot in slots {
            if !identifier(slot) {
                push(&mut reasons, Reason::InvalidStudy);
            }
            all.push(slot);
        }
    }
    let mut sorted = all.clone();
    sorted.sort();
    sorted.dedup();
    if sorted.len() != all.len() {
        push(&mut reasons, Reason::RoleReuse);
    }
    reasons
}

/// Structural then plan-relative validation. `plan` is absent only for
/// standalone inspection of a retained artifact, where plan-dependent checks
/// cannot run and are reported as such.
fn check_artifact(plan: Option<&Plan>, artifact: &Artifact) -> Findings {
    let mut findings = Findings::new();
    if artifact.kind != KIND || artifact.version != VERSION {
        findings.invalidate(Reason::InvalidArtifact);
        return findings;
    }
    if artifact.provenance == Provenance::Declared {
        findings.invalidate(Reason::InvalidArtifact);
    }
    if !evidence::is_digest(&artifact.revision)
        || !identifier(&artifact.acquisition.id)
        || artifact.acquisition.started_unix_ms == 0
        || artifact
            .program_sha256
            .as_deref()
            .is_some_and(|hash| !evidence::is_digest(hash))
    {
        findings.invalidate(Reason::InvalidArtifact);
    }
    if artifact.adapter.scope() != artifact.topology.scope {
        findings.invalidate(Reason::IdentityDrift);
    }
    if !operation_admitted(artifact.adapter, &artifact.operation) {
        findings.invalidate(Reason::IdentityDrift);
    }
    if artifact.clock != required_clock(artifact.adapter) {
        findings.invalidate(Reason::ClockMismatch);
    }
    if !artifact.adapter.external()
        && (artifact.program_sha256.is_some()
            || !artifact.sources.is_empty()
            || !artifact.observations.is_empty())
    {
        findings.invalidate(Reason::InvalidArtifact);
    }
    if artifact.adapter.external() && artifact.program_sha256.is_none() {
        findings.invalidate(Reason::IdentityDrift);
    }
    if artifact.kernel_revision.as_deref().is_none_or(|revision| {
        revision.is_empty()
            || revision.len() > 1024
            || !revision.is_ascii()
            || revision.bytes().any(|byte| byte.is_ascii_control())
            || evidence::digest(revision.as_bytes()) != artifact.revision
    }) {
        findings.invalidate(Reason::IdentityDrift);
    }
    for reason in observations_admitted(
        artifact.adapter,
        &artifact.operation,
        &artifact.observations,
    ) {
        findings.invalidate(reason);
    }
    check_sources(None, &artifact.sources, &mut findings);
    if artifact.execution.warmups != frozen_warmups(artifact.adapter) {
        findings.invalidate(Reason::WarmupCountMismatch);
    }
    if artifact.execution.iterations != frozen_iterations(artifact.adapter) {
        findings.invalidate(Reason::IterationCountMismatch);
    }
    // Retained accounting cannot be understated: it must at least cover the
    // frozen cell's known minimum, or the artifact claims to have run the
    // operation with less work or fewer live payloads than the operation can
    // occupy. The reported figures are re-checked against the allowance below.
    match minimum_allowance(artifact.adapter, &artifact.operation) {
        Some((work, memory))
            if artifact.execution.work_units >= work
                && artifact.execution.memory_bytes >= memory => {}
        _ => findings.invalidate(Reason::AllowanceExceeded),
    }
    if artifact.execution.timed_out {
        findings.withhold(Reason::DeadlineExceeded);
    }
    if !artifact.execution.completed {
        findings.withhold(Reason::SampleGridIncomplete);
    }
    if artifact.adapter == AdapterId::Exl3E3Grouped {
        let raw_fallback = artifact
            .observations
            .iter()
            .find(|observation| observation.name == ObservationName::FallbackTier)
            .map(|observation| observation.value.as_str());
        let normalized = match raw_fallback {
            Some("grouped") => Some(Tier::E3Grouped),
            Some("kernel") => Some(Tier::E2Kernel),
            _ => None,
        };
        if artifact.execution.observed_fallback != normalized {
            findings.invalidate(Reason::ObservationDrift);
        }
        let tier = match &artifact.operation {
            Operation::Exl3Experts { tier, .. } => Some(*tier),
            _ => None,
        };
        if tier.is_none() || artifact.execution.observed_fallback != tier {
            findings.withhold(Reason::TierFallback);
        }
    }
    check_grid(artifact, &mut findings);
    check_correctness(artifact, &mut findings);
    if artifact.failures.iter().any(|failure| {
        matches!(
            failure.kind,
            FailureKind::CorrectnessFailed | FailureKind::NonFinite
        )
    }) {
        findings.invalidate(Reason::CorrectnessFailed);
    } else if !artifact.failures.is_empty() {
        findings.withhold(Reason::RetainedFailure);
    }
    if let Some(plan) = plan {
        if plan.adapter != artifact.adapter {
            findings.invalidate(Reason::AdapterMismatch);
        }
        if plan.revision != artifact.revision
            || plan.acquisition != artifact.acquisition
            || plan.operation != artifact.operation
            || plan.clock != artifact.clock
            || plan.warmups != artifact.execution.warmups
            || plan.iterations != artifact.execution.iterations
            || plan.program_sha256 != artifact.program_sha256
        {
            findings.invalidate(Reason::IdentityDrift);
        }
        check_sources(Some(&plan.sources), &artifact.sources, &mut findings);
        if artifact.execution.work_units > plan.allowance.work_units
            || artifact.execution.memory_bytes > plan.allowance.memory_bytes
        {
            findings.withhold(Reason::AllowanceExceeded);
        }
        if artifact.execution.deadline_ms > plan.allowance.deadline_ms {
            findings.invalidate(Reason::InvalidArtifact);
        }
        match &plan.operation {
            Operation::AllReduce { world, ranks, .. } => {
                if artifact.topology.world != Some(*world)
                    || artifact.topology.ranks.len() != ranks.len()
                {
                    findings.invalidate(Reason::IdentityDrift);
                }
            }
            // The CPU reference bound is verified from the operation itself in
            // `check_correctness`, so it holds with or without a retained plan.
            Operation::SumU64 { .. } | Operation::Exl3Experts { .. } => (),
        }
    }
    findings
}

/// Observed sources must carry a usable path and hash; with a plan present the
/// observed hash set must equal the pinned hash set exactly, so a producer
/// cannot drop, add or substitute a pinned source. Paths are the paths the
/// producer actually read and are not required to match the declared hints.
fn check_sources(pinned: Option<&[Source]>, observed: &[Source], findings: &mut Findings) {
    let mut seen = std::collections::BTreeSet::new();
    for source in observed {
        if !printable(&source.path, 4096) || !evidence::is_digest(&source.sha256) {
            findings.invalidate(Reason::InvalidArtifact);
        }
        if !seen.insert(source.sha256.clone()) {
            findings.invalidate(Reason::InvalidArtifact);
        }
    }
    if let Some(pinned) = pinned {
        let mut expected: Vec<&str> = pinned.iter().map(|source| source.sha256.as_str()).collect();
        let mut found: Vec<&str> = observed
            .iter()
            .map(|source| source.sha256.as_str())
            .collect();
        expected.sort_unstable();
        found.sort_unstable();
        if expected != found {
            findings.invalidate(Reason::IdentityDrift);
        }
    }
}

fn check_grid(artifact: &Artifact, findings: &mut Findings) {
    let units = artifact.clock.units;
    let world = artifact.topology.world.unwrap_or(0);
    let mut seen = std::collections::BTreeSet::new();
    for (position, sample) in artifact.samples.iter().enumerate() {
        if sample.duration.denominator == 0 {
            findings.invalidate(Reason::SamplePrecision);
            continue;
        }
        match duration_ns(&sample.raw, units) {
            Ok(expected) => {
                if order(expected, sample.duration).is_ne() {
                    findings.invalidate(Reason::SamplePrecision);
                }
            }
            Err(DurationError::Malformed) => findings.invalidate(Reason::SamplePrecision),
            Err(DurationError::Overflow) => findings.invalidate(Reason::SampleOverflow),
            Err(DurationError::Zero) => findings.invalidate(Reason::DurationOutOfRange),
        }
        let key = match artifact.topology.scope {
            Scope::Ranks => match (sample.repetition, sample.rank) {
                (Some(repetition), Some(rank)) => {
                    if rank >= world || repetition >= artifact.execution.iterations {
                        findings.invalidate(Reason::SampleGridIncomplete);
                        continue;
                    }
                    (repetition, rank)
                }
                _ => {
                    findings.invalidate(Reason::SampleGridIncomplete);
                    continue;
                }
            },
            Scope::Cpu | Scope::Device => {
                if sample.rank.is_some() || sample.repetition.is_some() {
                    findings.invalidate(Reason::SampleGridIncomplete);
                    continue;
                }
                (sample.index, 0)
            }
        };
        if !seen.insert(key) || sample.index != position as u32 {
            findings.invalidate(Reason::SampleGridIncomplete);
        }
    }
    let expected = match artifact.topology.scope {
        Scope::Ranks => {
            if !(2..=MAX_RANKS).contains(&world) || artifact.topology.ranks.len() != world as usize
            {
                findings.invalidate(Reason::SampleGridIncomplete);
                return;
            }
            u64::from(world) * u64::from(artifact.execution.iterations)
        }
        Scope::Cpu | Scope::Device => u64::from(artifact.execution.iterations),
    };
    if artifact.samples.len() as u64 != expected {
        findings.invalidate(Reason::SampleGridIncomplete);
    }
}

fn check_correctness(artifact: &Artifact, findings: &mut Findings) {
    match &artifact.correctness {
        Correctness::ExactSum {
            reference,
            computed,
            bound,
            passed,
        } => {
            if reference != SUM_REFERENCE_ID {
                findings.invalidate(Reason::IdentityDrift);
            }
            let (Some(computed), Some(bound)) = (exact_decimal(computed), exact_decimal(bound))
            else {
                findings.invalidate(Reason::NonFinite);
                return;
            };
            if order(computed, bound).is_ne() || !*passed {
                findings.invalidate(Reason::CorrectnessFailed);
            }
            // The declared reference must be the independent closed form for
            // the declared bound, so neither value can drift independently.
            if let Operation::SumU64 {
                bound: declared, ..
            } = &artifact.operation
                && order(
                    bound,
                    Rational {
                        numerator: reference_sum(*declared),
                        denominator: 1,
                    },
                )
                .is_ne()
            {
                findings.invalidate(Reason::CorrectnessFailed);
            }
        }
        Correctness::ExactReduction {
            reference,
            mismatches,
            tolerance_elems,
            finite,
            passed,
        } => {
            if reference != REDUCE_REFERENCE_ID || *tolerance_elems != 0 {
                findings.invalidate(Reason::IdentityDrift);
            }
            if !*finite {
                findings.invalidate(Reason::NonFinite);
            }
            if *mismatches != 0 || !*passed {
                findings.invalidate(Reason::CorrectnessFailed);
            }
        }
        Correctness::E3Parity {
            reference,
            finite,
            ref_max,
            e2,
            e3,
            tolerance,
            passed,
            outcome,
            ..
        } => {
            if reference != E3_REFERENCE_ID {
                findings.invalidate(Reason::IdentityDrift);
            }
            if *tolerance != E3_TOLERANCE {
                findings.invalidate(Reason::IdentityDrift);
            }
            if !*finite {
                findings.invalidate(Reason::NonFinite);
            }
            let (Some(ref_max), Some(e2), Some(e3)) =
                (parity_scalar(ref_max), stats(e2), stats(e3))
            else {
                findings.invalidate(Reason::NonFinite);
                return;
            };
            match e3_bounds(*tolerance, ref_max, &e2) {
                Some(bounds) => {
                    let violated = e3[0] > bounds[0]
                        || e3[1] > bounds[1]
                        || e3[2] > bounds[2]
                        || e3[3] > bounds[3]
                        || e3[0] >= bounds[4];
                    if violated || *outcome != CheckOutcome::Pass || !*passed {
                        findings.invalidate(Reason::CorrectnessFailed);
                    }
                }
                None => findings.invalidate(Reason::ArithmeticOverflow),
            }
        }
    }
}

fn parity_scalar(text: &str) -> Option<f64> {
    if text.len() > 64 || !text.as_bytes().first()?.is_ascii_digit() {
        return None;
    }
    let value = text.parse::<f64>().ok()?;
    // Refuse nonfinite values and nonzero decimal text that underflows to zero.
    let underflow = value == 0.0
        && text
            .bytes()
            .take_while(|byte| !matches!(byte, b'e' | b'E'))
            .any(|byte| matches!(byte, b'1'..=b'9'));
    (value.is_finite() && !underflow).then_some(value)
}

fn stats(value: &ParityStats) -> Option<[f64; 4]> {
    Some([
        parity_scalar(&value.maxabs)?,
        parity_scalar(&value.per_token_max)?,
        parity_scalar(&value.per_token_p99)?,
        parity_scalar(&value.nrmse)?,
    ])
}

/// Reproduce the pinned Python helper's binary64 operations, in the same order.
/// The retained strings are repr(float) statistics, not rational timer samples.
fn e3_bounds(tolerance: Tolerance, ref_max: f64, e2: &[f64; 4]) -> Option<[f64; 5]> {
    let floor = (tolerance.abs_rel_micro() as f64 / 1_000_000.0) * ref_max;
    let factor = tolerance.factor_milli() as f64 / 1000.0;
    let mut bounds = [0.0; 5];
    for index in 0..3 {
        bounds[index] = factor * e2[index] + floor;
    }
    bounds[3] = factor * e2[3] + tolerance.nrmse_abs_micro() as f64 / 1_000_000.0;
    bounds[4] = (tolerance.coarse_abs_milli() as f64 / 1000.0)
        .max((tolerance.coarse_factor_milli() as f64 / 1000.0) * ref_max.max(1.0));
    bounds
        .iter()
        .all(|value| value.is_finite())
        .then_some(bounds)
}

fn reference_sum(bound: u32) -> u64 {
    let n = u64::from(bound);
    n * (n - 1) / 2
}

/// One measured acquisition yields one exact statistic: the arithmetic mean of
/// its measured samples, or for rank scope the mean of the complete
/// per-repetition maxima. Never a best case, percentile or survivor subset.
pub fn statistic(artifact: &Artifact) -> Option<Rational> {
    let values = match artifact.topology.scope {
        Scope::Ranks => {
            let world = artifact.topology.world?;
            if world < 2
                || artifact.samples.len() as u64
                    != u64::from(world) * u64::from(artifact.execution.iterations)
            {
                return None;
            }
            let mut per_repetition = Vec::new();
            for repetition in 0..artifact.execution.iterations {
                let mut worst: Option<Rational> = None;
                let mut seen = 0u32;
                for sample in artifact
                    .samples
                    .iter()
                    .filter(|sample| sample.repetition == Some(repetition))
                {
                    if sample.duration.denominator == 0 {
                        return None;
                    }
                    seen += 1;
                    worst = Some(match worst {
                        Some(current) => larger(current, sample.duration),
                        None => sample.duration,
                    });
                }
                if seen != world {
                    return None;
                }
                per_repetition.push(worst?);
            }
            per_repetition
        }
        Scope::Cpu | Scope::Device => {
            if artifact.samples.len() != artifact.execution.iterations as usize {
                return None;
            }
            artifact
                .samples
                .iter()
                .map(|sample| sample.duration)
                .collect()
        }
    };
    mean(&values)
}

fn exerciser(provenance: Provenance, adapter: AdapterId) -> &'static str {
    match (provenance, adapter.scope()) {
        (Provenance::Declared, _) => "operator-declared",
        (Provenance::Imported, _) => "imported-bytes",
        (Provenance::NativeObserved, Scope::Cpu) => "native-cpu-reference",
        (Provenance::NativeObserved, Scope::Device) => "native-device-program-observation",
        (Provenance::NativeObserved, Scope::Ranks) => "native-collective-program-observation",
    }
}

fn scope_sentence(provenance: Provenance, adapter: AdapterId) -> String {
    match (provenance, adapter.scope()) {
        (Provenance::Imported, _) => format!(
            "Imported comparison only for {}. Bytes this collector did not produce cannot establish that {} was exercised, and any provenance claim inside the payload is retained but never upgraded. Device or fabric qualification requires a separately authorized native capture.",
            adapter.name(),
            adapter.name()
        ),
        (Provenance::Declared, _) => format!(
            "Operator declaration only for {}; no measurement bytes were produced or parsed.",
            adapter.name()
        ),
        (Provenance::NativeObserved, Scope::Cpu) => format!(
            "Native CPU protocol exercise for {}. This proves native operation execution, sample recording, inspection and exact comparison; it does not demonstrate GPU, device or collective support, and it never sets a serving-speed claim.",
            adapter.name()
        ),
        (Provenance::NativeObserved, Scope::Device) => format!(
            "This collector launched the {} program and retained its synchronized device-event samples on the declared clock. Runtime pins are unauthenticated observations, not authenticated execution, and a kernel result never sets a serving-speed claim; serving impact needs a separately linked serving acquisition.",
            adapter.name()
        ),
        (Provenance::NativeObserved, Scope::Ranks) => format!(
            "This collector launched the {} program and retained complete per-rank samples on the declared clock. One completed world repetition is one sample; ranks are never pooled, and rank durations from the same repetition may only be maximized. Runtime pins are unauthenticated observations, not authenticated execution, and a collective result never sets a serving-speed claim.",
            adapter.name()
        ),
    }
}

// ---------------------------------------------------------------------------
// Native CPU reference adapter
// ---------------------------------------------------------------------------

fn reduce_sum(data: &[u64]) -> u64 {
    data.iter()
        .fold(0u64, |total, value| total.wrapping_add(*value))
}

/// Deterministic native CPU reference: `sum_u64` over `0..bound`, with the
/// exact closed-form reference `n*(n-1)/2` computed independently. The reduced
/// value passes through `black_box` on both sides so the timed reduction
/// cannot be optimized away.
fn cpu_reference(plan: &Plan, bound: u32, collector_sha256: &str) -> Result<Artifact> {
    let kernel_revision = format!("cpu-sum-u64-reference-v1;collector:{collector_sha256}");
    let revision = evidence::digest(kernel_revision.as_bytes());
    if plan.revision != revision {
        return Err("plan revision does not identify this native CPU implementation".into());
    }
    let data: Vec<u64> = (0..u64::from(bound)).collect();
    let memory_bytes = (data.len() as u64)
        .checked_mul(8)
        .ok_or("CPU reference memory accounting overflow")?;
    let work_units = u64::from(bound)
        .checked_mul(u64::from(plan.warmups) + u64::from(plan.iterations))
        .ok_or("CPU reference work accounting overflow")?;
    if memory_bytes > plan.allowance.memory_bytes || work_units > plan.allowance.work_units {
        return Err(format!(
            "declared allowance is below the reference requirement: work {work_units}/{}, memory {memory_bytes}/{}",
            plan.allowance.work_units, plan.allowance.memory_bytes
        ));
    }
    let deadline = Duration::from_millis(plan.allowance.deadline_ms);
    let started = Instant::now();
    let mut warm = 0u64;
    for _ in 0..plan.warmups {
        warm = warm.wrapping_add(std::hint::black_box(reduce_sum(std::hint::black_box(
            &data,
        ))));
    }
    std::hint::black_box(warm);
    let mut samples = Vec::new();
    let mut failures = Vec::new();
    let mut timed_out = false;
    for index in 0..plan.iterations {
        if started.elapsed() >= deadline {
            timed_out = true;
            failures.push(Failure {
                kind: FailureKind::DeadlineExceeded,
                detail: format!(
                    "reference deadline of {} ms elapsed before iteration {index}",
                    plan.allowance.deadline_ms
                ),
            });
            break;
        }
        let origin = Instant::now();
        let observed = std::hint::black_box(reduce_sum(std::hint::black_box(&data)));
        let elapsed = origin.elapsed();
        if observed != reference_sum(bound) {
            failures.push(Failure {
                kind: FailureKind::CorrectnessFailed,
                detail: format!(
                    "iteration {index} reduced {observed} instead of {}",
                    reference_sum(bound)
                ),
            });
        }
        let nanos = u64::try_from(elapsed.as_nanos())
            .map_err(|_| "reference duration does not fit nanoseconds".to_string())?;
        samples.push(Sample {
            index,
            rank: None,
            repetition: None,
            raw: nanos.to_string(),
            duration: Rational {
                numerator: nanos,
                denominator: 1,
            },
        });
    }
    Ok(Artifact {
        kind: KIND.into(),
        version: VERSION,
        adapter: plan.adapter,
        revision,
        acquisition: plan.acquisition.clone(),
        operation: plan.operation.clone(),
        provenance: Provenance::NativeObserved,
        submitted_provenance: None,
        program_sha256: None,
        kernel_revision: Some(kernel_revision),
        sources: Vec::new(),
        observations: Vec::new(),
        topology: Topology {
            scope: Scope::Cpu,
            world: None,
            ranks: Vec::new(),
        },
        clock: plan.clock.clone(),
        execution: Execution {
            warmups: plan.warmups,
            iterations: plan.iterations,
            deadline_ms: plan.allowance.deadline_ms,
            work_units,
            memory_bytes,
            completed: !timed_out && samples.len() == plan.iterations as usize,
            timed_out,
            observed_fallback: None,
        },
        samples,
        correctness: Correctness::ExactSum {
            reference: SUM_REFERENCE_ID.into(),
            computed: reference_sum(bound).to_string(),
            bound: reference_sum(bound).to_string(),
            passed: true,
        },
        failures,
    })
}

// ---------------------------------------------------------------------------
// Explicit operator programs
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ProgramRun {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
    #[cfg(target_os = "linux")]
    status: Option<i32>,
    failure: Option<String>,
    elapsed_ms: u64,
}

impl ProgramRun {
    fn fail(&mut self, detail: impl AsRef<str>) {
        if let Some(failure) = &mut self.failure {
            failure.push_str("; ");
            failure.push_str(detail.as_ref());
        } else {
            self.failure = Some(detail.as_ref().to_string());
        }
    }

    fn output(&self) -> ProgramOutput {
        ProgramOutput {
            stdout_sha256: evidence::digest(&self.stdout),
            stderr_sha256: evidence::digest(&self.stderr),
            stdout_bytes: self.stdout.len() as u64,
            stderr_bytes: self.stderr.len() as u64,
            stdout_truncated: self.stdout_truncated,
            stderr_truncated: self.stderr_truncated,
        }
    }
}

/// Scoped to the synchronous external-capture command, including publication.
/// Existing serving collectors own their separate process-wide signal latch.
#[cfg(target_os = "linux")]
mod program_signals {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ACTIVE: Mutex<()> = Mutex::new(());
    static CANCELLED: AtomicBool = AtomicBool::new(false);

    extern "C" fn cancel(_: libc::c_int) {
        CANCELLED.store(true, Ordering::SeqCst);
    }

    pub fn cancelled() -> bool {
        CANCELLED.load(Ordering::SeqCst)
    }

    pub struct Guard {
        previous: Vec<(libc::c_int, libc::sigaction)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Guard {
        pub fn install() -> crate::model::Result<Self> {
            let lock = ACTIVE
                .try_lock()
                .map_err(|_| "external capture already active")?;
            CANCELLED.store(false, Ordering::SeqCst);
            let mut guard = Self {
                previous: Vec::new(),
                _lock: lock,
            };
            // Auto-reaping or a competing SIGCHLD handler can release the PID/PGID.
            // SAFETY: sigaction is plain C data; all-zero is a valid value.
            let mut chld: libc::sigaction = unsafe { std::mem::zeroed() };
            // SAFETY: a null new action only queries into the owned out-parameter.
            if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut chld) } != 0
                || chld.sa_sigaction != libc::SIG_DFL
                || chld.sa_flags & libc::SA_NOCLDWAIT != 0
            {
                return Err("external capture requires a waitable child leader".into());
            }
            for signal in [libc::SIGINT, libc::SIGTERM] {
                // SAFETY: sigaction is plain C data; all-zero is a valid value.
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                action.sa_sigaction = cancel as *const () as libc::sighandler_t;
                // SAFETY: initializes the mask of the owned action.
                unsafe { libc::sigemptyset(&mut action.sa_mask) };
                // SAFETY: sigaction is plain C data; all-zero is a valid value.
                let mut previous = unsafe { std::mem::zeroed() };
                // SAFETY: `cancel` only performs an atomic store, so it is
                // async-signal-safe; both pointers are live locals.
                if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
                    return Err(format!(
                        "install capture signal handler: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                guard.previous.push((signal, previous));
            }
            Ok(guard)
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            for (signal, previous) in self.previous.iter().rev() {
                // SAFETY: restores the action saved when this signal was installed.
                unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn nonblocking(stream: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let fd = stream.as_raw_fd();
    // SAFETY: fcntl only operates on the fd borrowed from `stream` for this call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: as above; the new flags extend the flags just read.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// At most 16 reads per stream per tick, so a busy writer cannot starve the timer.
#[cfg(target_os = "linux")]
fn drain(
    stream: &mut impl std::io::Read,
    bytes: &mut Vec<u8>,
    cap: usize,
    truncated: &mut bool,
    done: &mut bool,
) -> std::io::Result<()> {
    if *done {
        return Ok(());
    }
    let mut buffer = [0_u8; 8192];
    for _ in 0..16 {
        match stream.read(&mut buffer) {
            Ok(0) => {
                *done = true;
                break;
            }
            Ok(count) => {
                let retained = count.min(cap - bytes.len());
                bytes.extend_from_slice(&buffer[..retained]);
                *truncated |= retained < count;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => break,
            Err(error) => {
                *truncated = true;
                *done = true;
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Observe exit without reaping: the owned leader reserves its PID and PGID.
#[cfg(target_os = "linux")]
fn leader_exited(pid: u32) -> std::io::Result<bool> {
    // SAFETY: siginfo_t is plain C data; all-zero is a valid value.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: WNOWAIT observes without reaping; `info` is an owned out-parameter.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: waitid succeeded with WNOHANG, so si_pid is set (zero when no change).
    Ok(unsafe { info.si_pid() } != 0)
}

/// Launch only PROGRAM [extra..] --plan PLAN. No shell interpretation.
/// No wait/reader joins: cleanup has its own finite drain/reap allowance.
#[cfg(target_os = "linux")]
fn run_program(
    program: &Path,
    extra: &[String],
    plan_path: &Path,
    deadline: Duration,
) -> ProgramRun {
    use std::os::unix::process::CommandExt;
    let started = Instant::now();
    let mut run = ProgramRun::default();
    if program_signals::cancelled() {
        run.fail("collector cancelled before program launch");
        return run;
    }
    let mut child = match Command::new(program)
        .args(extra)
        .arg("--plan")
        .arg(plan_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            run.fail(format!("spawn {}: {error}", program.display()));
            run.elapsed_ms = started.elapsed().as_millis() as u64;
            return run;
        }
    };
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out_done = false;
    let mut err_done = false;
    if let Err(error) = nonblocking(&stdout) {
        run.fail(format!("stdout nonblocking setup: {error}"));
        run.stdout_truncated = true;
        out_done = true; // Never read a descriptor whose nonblocking setup failed.
    }
    if let Err(error) = nonblocking(&stderr) {
        run.fail(format!("stderr nonblocking setup: {error}"));
        run.stderr_truncated = true;
        err_done = true;
    }
    let mut cleanup_started = None;
    let mut reaped = false;
    let mut cancellation_recorded = false;
    loop {
        if let Err(error) = drain(
            &mut stdout,
            &mut run.stdout,
            OUTPUT_CAP,
            &mut run.stdout_truncated,
            &mut out_done,
        ) {
            run.fail(format!("stdout read failed: {error}"));
        }
        if let Err(error) = drain(
            &mut stderr,
            &mut run.stderr,
            STDERR_CAP,
            &mut run.stderr_truncated,
            &mut err_done,
        ) {
            run.fail(format!("stderr read failed: {error}"));
        }
        if program_signals::cancelled() && !cancellation_recorded {
            run.fail("collector cancelled by SIGINT or SIGTERM");
            cancellation_recorded = true;
        }
        if cleanup_started.is_none() {
            if run.stdout_truncated || run.stderr_truncated {
                run.fail("program stdout/stderr capture truncated (cap or read/setup failure)");
            }
            if started.elapsed() >= deadline {
                run.fail(format!(
                    "program exceeded the declared {} ms deadline",
                    deadline.as_millis()
                ));
            }
            let exited = match leader_exited(pid) {
                Ok(exited) => exited,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => false,
                Err(error) => {
                    // In particular ECHILD means identity is no longer reserved.
                    // Never signal a numeric PID/PGID after that loss of ownership.
                    run.fail(format!(
                        "observe owned leader failed; group not signalled: {error}"
                    ));
                    break;
                }
            };
            if exited || run.failure.is_some() {
                // This is the ONLY group signal. WNOWAIT above kept the leader
                // unreaped even on normal exit; no try_wait occurs before it.
                // SAFETY: kill takes plain integers; the unreaped leader keeps
                // its PGID reserved, so the signal cannot reach a reused group.
                if unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) } != 0 {
                    run.fail(format!(
                        "owned group cleanup signal failed: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                cleanup_started = Some(Instant::now());
            }
        }
        if let Some(cleanup) = cleanup_started {
            // No further group signals are permitted after this reap.
            if !reaped {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        run.status = status.code();
                        reaped = true;
                    }
                    Ok(None) => (),
                    Err(error) => {
                        run.fail(format!("owned leader reap failed: {error}"));
                        break;
                    }
                }
            }
            if reaped && out_done && err_done {
                break;
            }
            if cleanup.elapsed() >= PROGRAM_CLEANUP {
                if !reaped {
                    run.fail("owned leader not reaped within 100 ms cleanup allowance");
                }
                break;
            }
        }
        std::thread::sleep(PROGRAM_POLL);
    }
    if !out_done {
        run.stdout_truncated = true;
        run.fail("stdout drainage incomplete at cleanup boundary");
    }
    if !err_done {
        run.stderr_truncated = true;
        run.fail("stderr drainage incomplete at cleanup boundary");
    }
    if run.stdout_truncated || run.stderr_truncated {
        run.fail("raw output is truncated; retained prefixes only");
    }
    if run.status != Some(0) {
        run.fail(format!("program exited with status {:?}", run.status));
    }
    run.elapsed_ms = started.elapsed().as_millis() as u64;
    run
}

#[cfg(not(target_os = "linux"))]
fn run_program(_: &Path, _: &[String], _: &Path, _: Duration) -> ProgramRun {
    ProgramRun {
        failure: Some(
            "native external capture requires Linux owned-process-group containment".into(),
        ),
        ..ProgramRun::default()
    }
}

/// Hash an explicitly declared program file. Symlinks are resolved first
/// because interpreters and launchers are commonly symlinked; the digest is of
/// the resolved bytes.
pub(crate) fn hash_program(path: &Path) -> Result<String> {
    let resolved = std::fs::canonicalize(path)
        .map_err(|e| format!("resolve program {}: {e}", path.display()))?;
    evidence::hash_file(&resolved, FILE_CAP)
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct CaptureReport {
    pub claim: String,
    pub status: &'static str,
    pub adapter: AdapterId,
    pub exerciser: &'static str,
    pub provenance: Provenance,
    pub plan_sha256: String,
    pub artifact_sha256: Option<String>,
    pub out: String,
    pub program: Option<String>,
    pub program_sha256: Option<String>,
    pub samples: usize,
    pub statistic: Option<Rational>,
    pub invalid: Vec<Reason>,
    pub unavailable: Vec<Reason>,
    pub failure: Option<String>,
    pub scope: String,
}

impl CaptureReport {
    pub fn exit(&self) -> u8 {
        if !self.invalid.is_empty() || self.failure.is_some() {
            Outcome::Error.exit()
        } else if !self.unavailable.is_empty() {
            Outcome::Inconclusive.exit()
        } else {
            Outcome::Pass.exit()
        }
    }

    fn provenance_name(&self) -> &'static str {
        match self.provenance {
            Provenance::Declared => "declared",
            Provenance::Imported => "imported",
            Provenance::NativeObserved => "native_observed",
        }
    }
}

#[derive(Serialize)]
pub struct Inspection {
    pub claim: &'static str,
    pub path: String,
    pub kind: String,
    pub version: u32,
    pub adapter: AdapterId,
    pub revision: String,
    pub exerciser: &'static str,
    pub provenance: Provenance,
    pub submitted_provenance: Option<Provenance>,
    pub operation: Operation,
    pub plan_sha256: Option<String>,
    pub plan_verified: bool,
    /// Directory acquisition checks ran; a standalone file is structural only.
    pub acquisition_checked: bool,
    pub artifact_sha256: String,
    /// Collector that produced this receipt, when one was retained.
    pub collector_sha256: Option<String>,
    pub program_sha256: Option<String>,
    pub kernel_revision: Option<String>,
    pub sources: Vec<Source>,
    pub observations: Vec<Observation>,
    pub topology: Topology,
    pub clock: Clock,
    pub execution: Execution,
    pub samples: usize,
    pub statistic: Option<Rational>,
    pub statistic_definition: &'static str,
    pub correctness: Correctness,
    pub failures: Vec<Failure>,
    pub derived: Option<Derived>,
    pub receipt: Option<Receipt>,
    pub invalid: Vec<Reason>,
    pub unavailable: Vec<Reason>,
    pub scope: String,
}

impl Inspection {
    pub fn exit(&self) -> u8 {
        if !self.invalid.is_empty() {
            Outcome::Error.exit()
        } else if !self.unavailable.is_empty() {
            Outcome::Inconclusive.exit()
        } else {
            Outcome::Pass.exit()
        }
    }

    fn provenance_name(&self) -> &'static str {
        match self.provenance {
            Provenance::Declared => "declared",
            Provenance::Imported => "imported",
            Provenance::NativeObserved => "native_observed",
        }
    }
}

/// One acquisition inside a role, as retained evidence.
#[derive(Serialize)]
pub struct AcquisitionView {
    pub slot: String,
    pub plan_sha256: Option<String>,
    pub artifact_sha256: Option<String>,
    pub provenance: Option<Provenance>,
    pub revision: Option<String>,
    pub samples: usize,
    pub statistic: Option<Rational>,
    pub statistic_display_ns: Option<f64>,
    pub invalid: Vec<Reason>,
    pub unavailable: Vec<Reason>,
    pub observed_interval_unix_ms: Option<[u64; 2]>,
    #[serde(skip)]
    plan: Option<Plan>,
    #[serde(skip)]
    collector_sha256: Option<String>,
}

impl AcquisitionView {
    fn missing(slot: &str, reason: Reason) -> Self {
        Self {
            slot: slot.to_string(),
            plan_sha256: None,
            artifact_sha256: None,
            provenance: None,
            revision: None,
            samples: 0,
            statistic: None,
            statistic_display_ns: None,
            invalid: Vec::new(),
            unavailable: vec![reason],
            observed_interval_unix_ms: None,
            plan: None,
            collector_sha256: None,
        }
    }

    fn complete(&self) -> bool {
        self.invalid.is_empty() && self.unavailable.is_empty() && self.statistic.is_some()
    }
}

#[derive(Serialize)]
pub struct RoleGroup {
    pub revision: Option<String>,
    pub declared: usize,
    pub present: usize,
    pub complete: usize,
    pub eligible: bool,
    pub range: Option<[Rational; 2]>,
    pub acquisitions: Vec<AcquisitionView>,
}

#[derive(Serialize)]
pub struct StudySummary {
    pub adapter: AdapterId,
    pub minimum_acquisitions: u32,
    pub started_unix_ms: u64,
    pub declared_acquisitions: u32,
    pub present_acquisitions: u32,
    pub complete_acquisitions: u32,
}

#[derive(Serialize)]
pub struct Decision {
    pub version: u32,
    pub claim: &'static str,
    pub scope: String,
    pub statistic: &'static str,
    pub statistic_definition: &'static str,
    pub direction: &'static str,
    pub axis: &'static str,
    pub axis_baseline: Option<String>,
    pub axis_candidate: Option<String>,
    pub decision: Outcome,
    pub adverse_bounds: Option<[f64; 2]>,
    pub thresholds: Option<Thresholds>,
    pub study_sha256: Option<String>,
    pub study: Option<StudySummary>,
    pub roles: Roles<RoleGroup>,
    /// Pooled A/A2 reference envelope.
    pub reference_range: Option<[Rational; 2]>,
    pub candidate_range: Option<[Rational; 2]>,
    pub collector_sha256: Option<String>,
    pub evaluator_sha256: Option<String>,
    pub reason_codes: Vec<Reason>,
}

impl Decision {
    pub fn exit(&self) -> u8 {
        self.decision.exit()
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct CaptureOptions {
    /// Closed adapter identifier.
    #[arg(long, value_enum)]
    pub adapter: AdapterId,
    /// Prospective declaration validated against the produced artifact.
    #[arg(long)]
    pub plan: PathBuf,
    /// Fresh output directory; existing evidence is never overwritten.
    #[arg(long)]
    pub out: PathBuf,
    /// Explicit adapter program for the two external adapters.
    #[arg(long)]
    pub program: Option<PathBuf>,
    /// Required for device and collective adapters: names the separately
    /// authorized device window. The CPU reference never needs it.
    #[arg(long)]
    pub authorize_device_window: bool,
    /// Arguments passed through to the program after `--`.
    #[arg(last = true)]
    pub args: Vec<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct ImportOptions {
    /// Retained artifact bytes from a producer this collector did not launch.
    #[arg(long)]
    pub artifact: PathBuf,
    #[arg(long)]
    pub plan: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct InspectOptions {
    /// Artifact file, or a directory holding `artifact.json`/`plan.json`.
    pub path: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct CompareOptions {
    /// Prospective A/B/A2 study declaration.
    #[arg(long)]
    pub study: PathBuf,
    /// Role directory holding one acquisition directory per declared slot.
    #[arg(long)]
    pub baseline: PathBuf,
    #[arg(long)]
    pub candidate: PathBuf,
    #[arg(long)]
    pub reference: PathBuf,
    #[arg(long)]
    pub json: bool,
}

fn parse_plan(bytes: &[u8]) -> Result<Plan> {
    let plan: Plan =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid microbench plan: {e}"))?;
    let reasons = check_plan(&plan);
    if !reasons.is_empty() {
        return Err(format!(
            "microbench plan rejected: {reasons:?}; the frozen cell, clock, correctness contract and pins are not negotiable in this slice"
        ));
    }
    Ok(plan)
}

fn parse_artifact(bytes: &[u8]) -> Result<Artifact> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid microbench artifact: {e}"))
}

/// Publish `plan.json`, optional `artifact.json` and `receipt.json` into a
/// directory created by the caller. A failed producer still retains its plan
/// and receipt.
fn publish(
    out: &Path,
    plan_bytes: &[u8],
    artifact: Option<&Artifact>,
    receipt: &Receipt,
) -> Result<(String, Option<String>)> {
    let plan_path = out.join("plan.json");
    if std::fs::symlink_metadata(&plan_path).is_err() {
        evidence::write(&plan_path, plan_bytes)?;
    }
    let artifact_sha = match artifact {
        Some(artifact) => {
            let bytes = evidence::json(&out.join("artifact.json"), artifact)?;
            Some(evidence::digest(&bytes))
        }
        None => None,
    };
    let mut receipt = receipt.clone();
    receipt.artifact_sha256 = artifact_sha.clone();
    evidence::publish(out, "receipt.json", &receipt)?;
    evidence::sync(out)?;
    Ok((evidence::digest(plan_bytes), artifact_sha))
}

/// Fields every receipt shares; the producer-specific fields are filled in at
/// the call site so no constructor grows an argument list.
struct ReceiptSeed {
    adapter: AdapterId,
    provenance: Provenance,
    status: ReceiptStatus,
    claim: &'static str,
    plan_sha256: String,
    collector_sha256: String,
    started_unix_ms: u64,
    observation_unix_ms: u64,
    duration_ms: u64,
}

impl ReceiptSeed {
    fn build(self) -> Receipt {
        Receipt {
            kind: RECEIPT_KIND.into(),
            version: VERSION,
            adapter: self.adapter,
            provenance: self.provenance,
            status: self.status,
            plan_sha256: self.plan_sha256,
            artifact_sha256: None,
            collector_sha256: self.collector_sha256,
            program: None,
            program_sha256: None,
            source_sha256: None,
            program_output: None,
            started_unix_ms: self.started_unix_ms,
            observation_unix_ms: self.observation_unix_ms,
            duration_ms: self.duration_ms,
            command: Vec::new(),
            failure: None,
            claim: self.claim.into(),
        }
    }
}

#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
pub fn capture(options: &CaptureOptions) -> Result<CaptureReport> {
    let plan_bytes = evidence::read(&options.plan, FILE_CAP)?;
    let plan = parse_plan(&plan_bytes)?;
    if plan.adapter != options.adapter {
        return Err(format!(
            "plan declares adapter {} but --adapter is {}",
            plan.adapter.name(),
            options.adapter.name()
        ));
    }
    let collector_sha256 = evidence::binary_digest()?;
    let plan_sha256 = evidence::digest(&plan_bytes);
    let started_unix_ms = metrics::unix_ms_checked()?;
    if plan.acquisition.started_unix_ms > started_unix_ms {
        return Err(
            "plan declares an acquisition start in the future; declare a plan before running it"
                .into(),
        );
    }
    if plan.adapter.external() && !options.authorize_device_window {
        return Err(
            "device and collective capture is outside the CPU-only authorization; pass --authorize-device-window only inside a separately approved window".into(),
        );
    }
    let mut command = vec![format!(
        "grill-perf microbench capture --adapter {}",
        plan.adapter.name()
    )];
    evidence::fresh(&options.out)?;
    #[cfg(target_os = "linux")]
    let signal_guard = plan
        .adapter
        .external()
        .then(program_signals::Guard::install);
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let (artifact, mut receipt, mut failure) = if plan.adapter.external() {
        let program = options.program.as_deref();
        let mut program_sha256 = None;
        let setup = (|| -> Result<()> {
            #[cfg(target_os = "linux")]
            if let Some(Err(error)) = &signal_guard {
                return Err(error.clone());
            }
            let program = program.ok_or(
                "this adapter requires an explicitly declared --program; there is no discovery",
            )?;
            let digest = hash_program(program)?;
            program_sha256 = Some(digest.clone());
            if plan.program_sha256.as_deref() != Some(digest.as_str()) {
                return Err("the declared program does not match plan.program_sha256; re-pin the plan instead of running a different producer".into());
            }
            Ok(())
        })();
        if let Some(program) = program {
            command.push(format!("program={}", program.display()));
        }
        command.extend(options.args.iter().cloned());
        let plan_path = options.out.join("plan.json");
        evidence::write(&plan_path, &plan_bytes)?;
        let mut run = match setup {
            Ok(()) => run_program(
                program.expect("admitted program"),
                &options.args,
                &plan_path,
                Duration::from_millis(plan.allowance.deadline_ms),
            ),
            Err(error) => ProgramRun {
                failure: Some(error),
                ..ProgramRun::default()
            },
        };
        // Raw bytes are authoritative even when JSON parsing, spawn or setup fails.
        evidence::write(&options.out.join("stdout.bin"), &run.stdout)?;
        evidence::write(&options.out.join("stderr.bin"), &run.stderr)?;
        #[cfg(target_os = "linux")]
        if program_signals::cancelled() && run.failure.is_none() {
            run.fail("collector cancelled by SIGINT or SIGTERM");
        }
        let mut artifact = if run.failure.is_none() {
            match parse_artifact(&run.stdout) {
                Ok(artifact) => Some(artifact),
                Err(error) => {
                    run.fail(error);
                    None
                }
            }
        } else {
            None
        };
        if let Some(payload) = artifact.as_mut() {
            native_provenance(payload);
        }
        let mut receipt = ReceiptSeed {
            adapter: plan.adapter,
            provenance: Provenance::NativeObserved,
            status: if run.failure.is_some() {
                ReceiptStatus::Failed
            } else {
                ReceiptStatus::Captured
            },
            claim: "observed-producer-not-authenticated-execution",
            plan_sha256: plan_sha256.clone(),
            collector_sha256: collector_sha256.clone(),
            started_unix_ms,
            observation_unix_ms: metrics::unix_ms_checked()?,
            duration_ms: run.elapsed_ms,
        }
        .build();
        let output = run.output();
        receipt.program = program.map(|path| path.display().to_string());
        receipt.program_sha256 = program_sha256;
        receipt.source_sha256 = Some(output.stdout_sha256.clone());
        receipt.program_output = Some(output);
        receipt.command = command;
        receipt.failure = run.failure.clone();
        (artifact, receipt, run.failure)
    } else {
        if options.program.is_some() {
            return Err(
                "the in-process CPU reference adapter rejects --program; an operator program needs an external adapter id"
                    .into(),
            );
        }
        let bound = match plan.operation {
            Operation::SumU64 { bound, .. } => bound,
            _ => return Err("the CPU reference requires the sum-u64 operation".into()),
        };
        let artifact = cpu_reference(&plan, bound, &collector_sha256)?;
        let failed = artifact.failures.iter().any(|failure| {
            matches!(
                failure.kind,
                FailureKind::CorrectnessFailed
                    | FailureKind::NonFinite
                    | FailureKind::DeadlineExceeded
            )
        });
        let mut receipt = ReceiptSeed {
            adapter: plan.adapter,
            provenance: Provenance::NativeObserved,
            status: if failed {
                ReceiptStatus::Failed
            } else {
                ReceiptStatus::Captured
            },
            claim: "native-in-process-cpu-reference",
            plan_sha256: plan_sha256.clone(),
            collector_sha256: collector_sha256.clone(),
            started_unix_ms,
            observation_unix_ms: metrics::unix_ms_checked()?,
            duration_ms: 0,
        }
        .build();
        receipt.command = command;
        receipt.failure = failed.then(|| "the native reference retained a failure".to_string());
        let failure = receipt.failure.clone();
        (Some(artifact), receipt, failure)
    };
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut findings = match &artifact {
        Some(artifact) => check_artifact(Some(&plan), artifact),
        None => {
            let mut findings = Findings::new();
            findings.withhold(Reason::CaptureFailed);
            findings
        }
    };
    receipt.failure = receipt.failure.clone().or_else(|| failure.clone());
    #[cfg(target_os = "linux")]
    if plan.adapter.external() && program_signals::cancelled() {
        failure.get_or_insert_with(|| "collector cancelled by SIGINT or SIGTERM".into());
        receipt.status = ReceiptStatus::Failed;
        receipt.failure = failure.clone();
        findings.withhold(Reason::CaptureFailed);
    }
    let (plan_sha256, artifact_sha256) =
        publish(&options.out, &plan_bytes, artifact.as_ref(), &receipt)?;
    Ok(CaptureReport {
        claim: receipt.claim.clone(),
        status: if artifact.is_some() && failure.is_none() {
            "captured"
        } else {
            "failed"
        },
        adapter: plan.adapter,
        exerciser: exerciser(receipt.provenance, plan.adapter),
        provenance: receipt.provenance,
        plan_sha256,
        artifact_sha256,
        out: options.out.display().to_string(),
        program: receipt.program.clone(),
        program_sha256: receipt.program_sha256.clone(),
        samples: artifact
            .as_ref()
            .map_or(0, |artifact| artifact.samples.len()),
        statistic: artifact.as_ref().and_then(statistic),
        invalid: findings.invalid,
        unavailable: findings.unavailable,
        failure,
        scope: scope_sentence(receipt.provenance, plan.adapter),
    })
}

pub fn import(options: &ImportOptions) -> Result<CaptureReport> {
    let plan_bytes = evidence::read(&options.plan, FILE_CAP)?;
    let plan = parse_plan(&plan_bytes)?;
    let artifact_bytes = evidence::read(&options.artifact, FILE_CAP)?;
    let mut artifact = parse_artifact(&artifact_bytes)?;
    let collector_sha256 = evidence::binary_digest()?;
    let observation_unix_ms = metrics::unix_ms_checked()?;
    if plan.acquisition.started_unix_ms > observation_unix_ms {
        return Err("plan declares an acquisition start in the future".into());
    }
    // Importing never upgrades provenance, whatever the payload claims.
    if artifact.provenance == Provenance::NativeObserved {
        artifact.submitted_provenance = Some(Provenance::NativeObserved);
    }
    artifact.provenance = Provenance::Imported;
    let findings = check_artifact(Some(&plan), &artifact);
    let plan_sha256 = evidence::digest(&plan_bytes);
    let mut receipt = ReceiptSeed {
        adapter: plan.adapter,
        provenance: Provenance::Imported,
        status: ReceiptStatus::Imported,
        claim: "imported-bytes-not-authenticated-execution",
        plan_sha256,
        collector_sha256,
        started_unix_ms: plan.acquisition.started_unix_ms,
        observation_unix_ms,
        duration_ms: 0,
    }
    .build();
    receipt.program_sha256 = artifact.program_sha256.clone();
    receipt.source_sha256 = Some(evidence::digest(&artifact_bytes));
    receipt.command = vec!["grill-perf microbench import".into()];
    evidence::fresh(&options.out)?;
    let (plan_sha256, artifact_sha256) =
        publish(&options.out, &plan_bytes, Some(&artifact), &receipt)?;
    Ok(CaptureReport {
        claim: "imported-bytes-not-authenticated-execution".into(),
        status: "imported",
        adapter: plan.adapter,
        exerciser: exerciser(Provenance::Imported, plan.adapter),
        provenance: Provenance::Imported,
        plan_sha256,
        artifact_sha256,
        out: options.out.display().to_string(),
        program: None,
        program_sha256: artifact.program_sha256.clone(),
        samples: artifact.samples.len(),
        statistic: statistic(&artifact),
        invalid: findings.invalid,
        unavailable: findings.unavailable,
        failure: None,
        scope: scope_sentence(Provenance::Imported, plan.adapter),
    })
}

struct Loaded {
    plan: Plan,
    plan_sha256: String,
    artifact: Artifact,
    artifact_sha256: String,
    receipt: Option<Receipt>,
    findings: Findings,
}

enum LoadError {
    /// The evidence is absent or unreadable: unavailable, not corrupt.
    Unavailable(Reason),
    /// The evidence is present but invalid: never silently accepted.
    Invalid(Reason),
}

/// The only collector normalization applied to a native producer payload.
fn native_provenance(artifact: &mut Artifact) {
    if artifact.provenance != Provenance::NativeObserved {
        artifact.submitted_provenance = Some(artifact.provenance);
        artifact.provenance = Provenance::NativeObserved;
    }
}

fn read_receipt(path: &Path) -> std::result::Result<Option<Receipt>, LoadError> {
    let path = path.join("receipt.json");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(LoadError::Invalid(Reason::InvalidArtifact)),
        Ok(_) => {
            let bytes = evidence::read(&path, FILE_CAP)
                .map_err(|_| LoadError::Invalid(Reason::InvalidArtifact))?;
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| LoadError::Invalid(Reason::InvalidArtifact))
        }
    }
}

/// Verify every present raw-output manifest, including failed acquisitions.
/// Missing manifests are admitted by the archival reader, not by native eligibility.
fn check_program_output(
    path: &Path,
    receipt: &Receipt,
) -> std::result::Result<Option<Vec<u8>>, LoadError> {
    let Some(output) = &receipt.program_output else {
        return Ok(None);
    };
    if !evidence::is_digest(&output.stdout_sha256)
        || !evidence::is_digest(&output.stderr_sha256)
        || output.stdout_bytes > OUTPUT_CAP as u64
        || output.stderr_bytes > STDERR_CAP as u64
        || receipt.provenance != Provenance::NativeObserved
        || !receipt.adapter.external()
        || receipt.source_sha256.as_deref() != Some(output.stdout_sha256.as_str())
    {
        return Err(LoadError::Invalid(Reason::IdentityDrift));
    }
    let stdout = evidence::read(&path.join("stdout.bin"), OUTPUT_CAP)
        .map_err(|_| LoadError::Invalid(Reason::InvalidArtifact))?;
    let stderr = evidence::read(&path.join("stderr.bin"), STDERR_CAP)
        .map_err(|_| LoadError::Invalid(Reason::InvalidArtifact))?;
    if stdout.len() as u64 != output.stdout_bytes
        || stderr.len() as u64 != output.stderr_bytes
        || evidence::digest(&stdout) != output.stdout_sha256
        || evidence::digest(&stderr) != output.stderr_sha256
    {
        return Err(LoadError::Invalid(Reason::IdentityDrift));
    }
    Ok(Some(stdout))
}

fn load_dir(path: &Path) -> std::result::Result<Loaded, LoadError> {
    evidence::directory(path).map_err(|_| LoadError::Unavailable(Reason::MissingArtifact))?;
    let plan_bytes = evidence::read(&path.join("plan.json"), FILE_CAP)
        .map_err(|_| LoadError::Unavailable(Reason::MissingArtifact))?;
    let plan = parse_plan(&plan_bytes).map_err(|_| LoadError::Invalid(Reason::InvalidPlan))?;
    let receipt = read_receipt(path)?;
    let raw_stdout = receipt
        .as_ref()
        .map(|receipt| check_program_output(path, receipt))
        .transpose()?
        .flatten();
    let artifact_bytes = evidence::read(&path.join("artifact.json"), FILE_CAP)
        .map_err(|_| LoadError::Unavailable(Reason::MissingArtifact))?;
    let artifact =
        parse_artifact(&artifact_bytes).map_err(|_| LoadError::Invalid(Reason::InvalidArtifact))?;
    let plan_sha256 = evidence::digest(&plan_bytes);
    let artifact_sha256 = evidence::digest(&artifact_bytes);
    let mut findings = check_artifact(Some(&plan), &artifact);
    match &receipt {
        None => findings.withhold(Reason::MissingArtifact),
        Some(receipt) => {
            if receipt.kind != RECEIPT_KIND
                || receipt.version != VERSION
                || receipt.plan_sha256 != plan_sha256
                || receipt.artifact_sha256.as_deref() != Some(artifact_sha256.as_str())
                || receipt.adapter != artifact.adapter
                || receipt.provenance != artifact.provenance
                || receipt.program_sha256 != artifact.program_sha256
                || !evidence::is_digest(&receipt.collector_sha256)
            {
                findings.invalidate(Reason::IdentityDrift);
            }
            let expected_status = match receipt.provenance {
                Provenance::NativeObserved => ReceiptStatus::Captured,
                Provenance::Imported => ReceiptStatus::Imported,
                Provenance::Declared => ReceiptStatus::Failed,
            };
            if receipt.failure.is_some() || receipt.status != expected_status {
                findings.withhold(Reason::CaptureFailed);
            }
            if receipt.provenance == Provenance::NativeObserved {
                if receipt.started_unix_ms < plan.acquisition.started_unix_ms
                    || receipt.observation_unix_ms < receipt.started_unix_ms
                {
                    findings.invalidate(Reason::ObservedStartsOutOfOrder);
                }
                if !artifact.adapter.external() {
                    let kernel = format!(
                        "cpu-sum-u64-reference-v1;collector:{}",
                        receipt.collector_sha256
                    );
                    if artifact.kernel_revision.as_deref() != Some(kernel.as_str()) {
                        findings.invalidate(Reason::IdentityDrift);
                    }
                }
            }
            if artifact.provenance == Provenance::NativeObserved && artifact.adapter.external() {
                match (&receipt.program_output, raw_stdout) {
                    (Some(output), Some(stdout)) => {
                        if output.stdout_truncated || output.stderr_truncated {
                            findings.withhold(Reason::CaptureFailed);
                        } else {
                            match parse_artifact(&stdout) {
                                Ok(mut raw) => {
                                    native_provenance(&mut raw);
                                    if raw != artifact {
                                        findings.invalidate(Reason::IdentityDrift);
                                    }
                                }
                                Err(_) => findings.invalidate(Reason::InvalidArtifact),
                            }
                        }
                    }
                    _ => findings.withhold(Reason::MissingArtifact),
                }
            }
        }
    }
    Ok(Loaded {
        plan,
        plan_sha256,
        artifact,
        artifact_sha256,
        receipt,
        findings,
    })
}

pub fn inspect(options: &InspectOptions) -> Result<Inspection> {
    if options.path.is_dir() {
        return match load_dir(&options.path) {
            Ok(loaded) => {
                let plan_sha256 = loaded.plan_sha256.clone();
                Ok(inspection(options, plan_sha256, loaded))
            }
            Err(LoadError::Unavailable(reason)) => Err(format!(
                "{}: {reason:?}; the directory must hold plan.json and artifact.json",
                options.path.display()
            )),
            Err(LoadError::Invalid(reason)) => {
                Err(format!("{}: {reason:?}", options.path.display()))
            }
        };
    }
    let bytes = evidence::read(&options.path, FILE_CAP)?;
    let artifact_sha256 = evidence::digest(&bytes);
    let artifact = parse_artifact(&bytes)?;
    let sibling = options.path.parent().map(|parent| parent.join("plan.json"));
    let (plan, plan_sha256) = match sibling
        .as_deref()
        .and_then(|path| evidence::read(path, FILE_CAP).ok())
        .and_then(|bytes| {
            parse_plan(&bytes)
                .ok()
                .map(|plan| (plan, evidence::digest(&bytes)))
        }) {
        Some((plan, sha)) => (Some(plan), Some(sha)),
        None => (None, None),
    };
    let findings = check_artifact(plan.as_ref(), &artifact);
    let parent = options.path.parent().unwrap_or(Path::new("."));
    let receipt = read_receipt(parent).map_err(|_| "invalid sibling microbench receipt")?;
    if let Some(receipt) = &receipt {
        check_program_output(parent, receipt).map_err(|_| "invalid retained program output")?;
    }
    Ok(inspection_of(
        options.path.display().to_string(),
        plan_sha256,
        artifact_sha256,
        artifact,
        receipt,
        findings,
        false,
    ))
}

fn inspection(options: &InspectOptions, plan_sha256: String, loaded: Loaded) -> Inspection {
    inspection_of(
        options.path.display().to_string(),
        Some(plan_sha256),
        loaded.artifact_sha256,
        loaded.artifact,
        loaded.receipt,
        loaded.findings,
        true,
    )
}

fn inspection_of(
    path: String,
    plan_sha256: Option<String>,
    artifact_sha256: String,
    artifact: Artifact,
    receipt: Option<Receipt>,
    findings: Findings,
    acquisition_checked: bool,
) -> Inspection {
    let statistic = statistic(&artifact);
    let derived = statistic.and_then(|value| derived(&artifact.operation, &artifact.clock, value));
    let collector_sha256 = receipt
        .as_ref()
        .map(|receipt| receipt.collector_sha256.clone());
    Inspection {
        claim: "microbench-artifact-inspection-not-qualification",
        path,
        kind: artifact.kind.clone(),
        version: artifact.version,
        adapter: artifact.adapter,
        revision: artifact.revision.clone(),
        exerciser: exerciser(artifact.provenance, artifact.adapter),
        provenance: artifact.provenance,
        submitted_provenance: artifact.submitted_provenance,
        operation: artifact.operation.clone(),
        plan_verified: plan_sha256.is_some(),
        acquisition_checked,
        plan_sha256,
        artifact_sha256,
        collector_sha256,
        program_sha256: artifact.program_sha256.clone(),
        kernel_revision: artifact.kernel_revision.clone(),
        sources: artifact.sources.clone(),
        observations: artifact.observations.clone(),
        topology: artifact.topology.clone(),
        clock: artifact.clock.clone(),
        execution: artifact.execution.clone(),
        samples: artifact.samples.len(),
        statistic,
        statistic_definition: "exact arithmetic mean of the measured samples in this acquisition (rank scope: mean of complete per-repetition maxima)",
        correctness: artifact.correctness.clone(),
        failures: artifact.failures.clone(),
        derived,
        receipt,
        invalid: findings.invalid,
        unavailable: findings.unavailable,
        scope: scope_sentence(artifact.provenance, artifact.adapter),
    }
}

fn load_view(slot: &str, path: &Path) -> AcquisitionView {
    match load_dir(path) {
        Ok(loaded) => {
            let observed_interval = loaded
                .receipt
                .as_ref()
                .filter(|receipt| receipt.provenance == Provenance::NativeObserved)
                .map(|receipt| [receipt.started_unix_ms, receipt.observation_unix_ms]);
            let statistic = loaded
                .findings
                .clean()
                .then(|| statistic(&loaded.artifact))
                .flatten();
            AcquisitionView {
                slot: slot.to_string(),
                plan_sha256: Some(loaded.plan_sha256),
                artifact_sha256: Some(loaded.artifact_sha256),
                provenance: Some(loaded.artifact.provenance),
                revision: Some(loaded.artifact.revision.clone()),
                samples: loaded.artifact.samples.len(),
                statistic,
                statistic_display_ns: statistic.map(display),
                invalid: loaded.findings.invalid,
                unavailable: loaded.findings.unavailable,
                observed_interval_unix_ms: observed_interval,
                plan: Some(loaded.plan),
                collector_sha256: loaded
                    .receipt
                    .as_ref()
                    .map(|receipt| receipt.collector_sha256.clone()),
            }
        }
        Err(LoadError::Invalid(reason)) => {
            let mut view = AcquisitionView::missing(slot, Reason::MissingArtifact);
            view.unavailable.clear();
            view.invalid = vec![reason];
            view
        }
        Err(LoadError::Unavailable(_)) => {
            AcquisitionView::missing(slot, Reason::MissingAcquisition)
        }
    }
}

/// Enumerate a role directory and load exactly the declared slots. Anything
/// present but not declared is rejected: the prospective declaration is the
/// complete membership of the role, so an unexpected acquisition can never be
/// substituted for a declared one.
fn load_role_group(dir: &Path, slots: &[String], revision: Option<String>) -> RoleGroup {
    let mut group = RoleGroup {
        revision,
        declared: slots.len(),
        present: 0,
        complete: 0,
        eligible: false,
        range: None,
        acquisitions: Vec::new(),
    };
    let entries = match role_entries(dir) {
        Ok(entries) => entries,
        Err(reason) => {
            group.acquisitions = vec![AcquisitionView::missing("<role>", reason)];
            return group;
        }
    };
    let declared: std::collections::BTreeSet<&String> = slots.iter().collect();
    for entry in &entries {
        if !declared.contains(entry) {
            let mut view = AcquisitionView::missing(entry, Reason::UnexpectedAcquisition);
            view.unavailable.clear();
            view.invalid = vec![Reason::UnexpectedAcquisition];
            group.acquisitions.push(view);
        }
    }
    for slot in slots {
        let view = load_view(slot, &dir.join(slot));
        group.present += usize::from(view.artifact_sha256.is_some());
        group.complete += usize::from(view.complete());
        group.acquisitions.push(view);
    }
    group.eligible = group.complete == slots.len() && slots.len() >= MIN_ROLE_ACQUISITIONS as usize;
    if group.eligible {
        let statistics: Vec<Rational> = group
            .acquisitions
            .iter()
            .filter_map(|view| view.statistic)
            .collect();
        if statistics.len() == slots.len()
            && let Ok(Some(range)) = envelope::range(&statistics)
        {
            group.range = Some(range);
        } else {
            group.eligible = false;
        }
    }
    group
}

fn role_entries(dir: &Path) -> std::result::Result<Vec<String>, Reason> {
    evidence::directory(dir).map_err(|_| Reason::MissingArtifact)?;
    let listing = std::fs::read_dir(dir).map_err(|_| Reason::MissingArtifact)?;
    let mut entries = Vec::new();
    for entry in listing {
        if entries.len() > MAX_ROLE_ACQUISITIONS as usize + 1 {
            return Err(Reason::UnexpectedAcquisition);
        }
        let entry = entry.map_err(|_| Reason::MissingArtifact)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Reason::UnexpectedAcquisition)?;
        if !entry
            .file_type()
            .map_err(|_| Reason::MissingArtifact)?
            .is_dir()
        {
            return Err(Reason::UnexpectedAcquisition);
        }
        entries.push(name);
    }
    Ok(entries)
}

/// Every pin except the declared candidate revision axis must match across all
/// acquisitions of a study.
fn pins_match(a: &Plan, b: &Plan) -> bool {
    a.kind == b.kind
        && a.version == b.version
        && a.adapter == b.adapter
        && a.program_sha256 == b.program_sha256
        && a.sources == b.sources
        && a.operation == b.operation
        && a.clock == b.clock
        && a.warmups == b.warmups
        && a.iterations == b.iterations
        && a.allowance == b.allowance
        && a.correctness == b.correctness
        && a.thresholds == b.thresholds
}

/// Complete A/B/A2 group comparison. A missing, extra or unusable acquisition
/// withholds the verdict; there is no survivor-only or one-per-role path.
#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
pub fn compare(options: &CompareOptions) -> Decision {
    let study_bytes = evidence::read(&options.study, FILE_CAP).ok();
    let study: Option<Study> = study_bytes
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<Study>(bytes).ok());
    let study_sha256 = study_bytes.as_deref().map(evidence::digest);
    let evaluator_sha256 = evidence::binary_digest().ok();
    let mut reasons = Vec::new();
    let mut decision = Outcome::Pass;
    let Some(study) = study else {
        return finish(Verdict {
            decision: Outcome::Error,
            reason_codes: vec![Reason::InvalidStudy],
            study_sha256,
            study: None,
            groups: empty_groups(),
            reference_range: None,
            candidate_range: None,
            adverse_bounds: None,
            thresholds: None,
            collector_sha256: None,
            evaluator_sha256,
            adapter: None,
        });
    };
    for reason in check_study(&study) {
        decision = Outcome::Error;
        push(&mut reasons, reason);
    }
    let groups = Roles {
        baseline: load_role_group(
            &options.baseline,
            &study.roles.baseline,
            Some(study.revision_axis.baseline.clone()),
        ),
        candidate: load_role_group(
            &options.candidate,
            &study.roles.candidate,
            Some(study.revision_axis.candidate.clone()),
        ),
        reference: load_role_group(
            &options.reference,
            &study.roles.reference,
            Some(study.revision_axis.baseline.clone()),
        ),
    };
    let summary = StudySummary {
        adapter: study.adapter,
        minimum_acquisitions: study.minimum_acquisitions,
        started_unix_ms: study.started_unix_ms,
        declared_acquisitions: (study.roles.baseline.len()
            + study.roles.candidate.len()
            + study.roles.reference.len()) as u32,
        present_acquisitions: (groups.baseline.present
            + groups.candidate.present
            + groups.reference.present) as u32,
        complete_acquisitions: (groups.baseline.complete
            + groups.candidate.complete
            + groups.reference.complete) as u32,
    };

    // Native ordering is observed receipt start-to-finish ordering. Imported
    // evidence has only its declared chronology and never gains native scope.
    let mut collector: Option<String> = None;
    let mut provenance = None;
    let mut seen_slots = std::collections::BTreeSet::new();
    let mut seen_artifacts = std::collections::BTreeSet::new();
    let mut role_intervals: [Vec<[u64; 2]>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut reference_plan: Option<&Plan> = None;
    for (index, group) in [&groups.baseline, &groups.candidate, &groups.reference]
        .into_iter()
        .enumerate()
    {
        for view in &group.acquisitions {
            for reason in &view.invalid {
                decision = Outcome::Error;
                push(&mut reasons, *reason);
            }
            for reason in &view.unavailable {
                decision = decision.max(Outcome::Inconclusive);
                push(&mut reasons, *reason);
            }
            if !seen_slots.insert(view.slot.clone()) {
                decision = Outcome::Error;
                push(&mut reasons, Reason::RoleReuse);
            }
            if let Some(artifact_sha256) = &view.artifact_sha256
                && !seen_artifacts.insert(artifact_sha256.clone())
            {
                decision = Outcome::Error;
                push(&mut reasons, Reason::RoleReuse);
            }
            match (&collector, &view.collector_sha256) {
                (Some(previous), Some(observed)) if previous != observed => {
                    decision = Outcome::Error;
                    push(&mut reasons, Reason::CollectorMismatch);
                }
                (None, Some(observed)) => collector = Some(observed.clone()),
                _ => (),
            }
            match (provenance, view.provenance) {
                (Some(previous), Some(observed)) if previous != observed => {
                    decision = Outcome::Error;
                    push(&mut reasons, Reason::ProvenanceMismatch);
                }
                (None, Some(observed)) => provenance = Some(observed),
                _ => (),
            }
            let Some(plan) = &view.plan else {
                continue;
            };
            if plan.acquisition.id != view.slot {
                decision = Outcome::Error;
                push(&mut reasons, Reason::InvalidStudy);
            }
            if plan.adapter != study.adapter {
                decision = Outcome::Error;
                push(&mut reasons, Reason::AdapterMismatch);
            }
            let expected_revision = if index == 1 {
                &study.revision_axis.candidate
            } else {
                &study.revision_axis.baseline
            };
            if &plan.revision != expected_revision {
                decision = Outcome::Error;
                push(&mut reasons, Reason::ObservationDrift);
            }
            if plan.acquisition.started_unix_ms < study.started_unix_ms {
                decision = Outcome::Error;
                push(&mut reasons, Reason::DeclaredStartsOutOfOrder);
            }
            // The study re-declares the thresholds; a plan that declares
            // different ones is a conflicting prospective declaration.
            if plan.thresholds != Some(study.thresholds) {
                decision = Outcome::Error;
                push(&mut reasons, Reason::InvalidBounds);
            }
            if let Some(interval) = view.observed_interval_unix_ms {
                role_intervals[index].push(interval);
            } else if view.provenance != Some(Provenance::NativeObserved) {
                role_intervals[index].push([plan.acquisition.started_unix_ms; 2]);
            }
            match reference_plan {
                Some(reference) if !pins_match(reference, plan) => {
                    decision = Outcome::Error;
                    push(&mut reasons, Reason::IdentityDrift);
                }
                None => reference_plan = Some(plan),
                _ => (),
            }
        }
    }
    let order_reason = if provenance == Some(Provenance::NativeObserved) {
        Reason::ObservedStartsOutOfOrder
    } else {
        Reason::DeclaredStartsOutOfOrder
    };
    for intervals in &role_intervals {
        if intervals.windows(2).any(|pair| pair[0][1] >= pair[1][0]) {
            decision = Outcome::Error;
            push(&mut reasons, order_reason);
        }
    }
    let overlaps = |left: &[[u64; 2]], right: &[[u64; 2]]| {
        left.iter()
            .map(|interval| interval[1])
            .max()
            .zip(right.iter().map(|interval| interval[0]).min())
            .is_some_and(|(last, first)| last >= first)
    };
    if overlaps(&role_intervals[0], &role_intervals[1])
        || overlaps(&role_intervals[1], &role_intervals[2])
    {
        decision = Outcome::Error;
        push(&mut reasons, order_reason);
    }

    // The envelope is computed only from a complete, eligible membership.
    let mut reference_range = None;
    let mut candidate_range = None;
    let mut adverse_bounds = None;
    if groups.baseline.eligible && groups.candidate.eligible && groups.reference.eligible {
        let mut pooled: Vec<Rational> = Vec::new();
        for group in [&groups.baseline, &groups.reference] {
            pooled.extend(group.acquisitions.iter().filter_map(|view| view.statistic));
        }
        let candidate_values: Vec<Rational> = groups
            .candidate
            .acquisitions
            .iter()
            .filter_map(|view| view.statistic)
            .collect();
        match (
            envelope::range(&pooled).and_then(|range| range.ok_or(EnvelopeReason::InvalidRational)),
            envelope::range(&candidate_values)
                .and_then(|range| range.ok_or(EnvelopeReason::InvalidRational)),
        ) {
            (Ok(pooled_range), Ok(candidate_bounds)) => {
                reference_range = Some(pooled_range);
                candidate_range = Some(candidate_bounds);
                match envelope::assess(
                    pooled_range,
                    candidate_bounds,
                    study.thresholds.adverse_bps,
                    study.thresholds.spread_bps,
                    Direction::LowerBetter,
                ) {
                    Ok(assessment) => {
                        adverse_bounds = Some(assessment.adverse_bounds);
                        decision = decision.max(match assessment.decision {
                            EnvelopeDecision::Pass => Outcome::Pass,
                            EnvelopeDecision::Regression => Outcome::Regression,
                        });
                    }
                    Err(reason) => {
                        adverse_bounds = match reason {
                            EnvelopeReason::ArithmeticOverflow { adverse_bounds } => adverse_bounds,
                            EnvelopeReason::EnvelopeStraddlesTolerance { adverse_bounds } => {
                                Some(adverse_bounds)
                            }
                            _ => None,
                        };
                        let mapped = envelope_reason(reason);
                        decision = decision.max(match mapped {
                            Reason::ArithmeticOverflow | Reason::InvalidRational => Outcome::Error,
                            _ => Outcome::Inconclusive,
                        });
                        push(&mut reasons, mapped);
                    }
                }
            }
            (Err(reason), _) | (_, Err(reason)) => {
                decision = Outcome::Error;
                push(&mut reasons, envelope_reason(reason));
            }
        }
    } else {
        if decision == Outcome::Pass {
            decision = Outcome::Inconclusive;
        }
        push(&mut reasons, Reason::MissingAcquisition);
    }

    finish(Verdict {
        decision,
        reason_codes: reasons,
        study_sha256,
        study: Some(summary),
        groups,
        reference_range,
        candidate_range,
        adverse_bounds,
        thresholds: Some(study.thresholds),
        collector_sha256: collector,
        evaluator_sha256,
        adapter: Some(study.adapter),
    })
}

fn empty_groups() -> Roles<RoleGroup> {
    let empty = || RoleGroup {
        revision: None,
        declared: 0,
        present: 0,
        complete: 0,
        eligible: false,
        range: None,
        acquisitions: Vec::new(),
    };
    Roles {
        baseline: empty(),
        candidate: empty(),
        reference: empty(),
    }
}

/// Everything the terminal decision needs, assembled in one place.
struct Verdict {
    decision: Outcome,
    reason_codes: Vec<Reason>,
    study_sha256: Option<String>,
    study: Option<StudySummary>,
    groups: Roles<RoleGroup>,
    reference_range: Option<[Rational; 2]>,
    candidate_range: Option<[Rational; 2]>,
    adverse_bounds: Option<[f64; 2]>,
    thresholds: Option<Thresholds>,
    collector_sha256: Option<String>,
    evaluator_sha256: Option<String>,
    adapter: Option<AdapterId>,
}

fn finish(verdict: Verdict) -> Decision {
    let Verdict {
        decision,
        reason_codes,
        study_sha256,
        study,
        groups,
        reference_range,
        candidate_range,
        adverse_bounds,
        thresholds,
        collector_sha256,
        evaluator_sha256,
        adapter,
    } = verdict;
    let provenance = groups
        .baseline
        .acquisitions
        .iter()
        .find_map(|view| view.provenance);
    let scope = if reason_codes.contains(&Reason::ProvenanceMismatch) {
        "Mixed-provenance evidence: no uniform native or imported comparison scope exists.".into()
    } else {
        match (provenance, adapter) {
        (Some(Provenance::NativeObserved), Some(adapter)) => {
            scope_sentence(Provenance::NativeObserved, adapter)
        }
        (Some(Provenance::Imported), Some(adapter)) => scope_sentence(Provenance::Imported, adapter),
        (Some(Provenance::Declared), Some(adapter)) => scope_sentence(Provenance::Declared, adapter),
        _ => "Unavailable evidence: the study declaration or the baseline acquisitions could not be loaded, so no comparison scope exists.".into(),
    }
    };
    let minimum = study.as_ref().map_or(MIN_ROLE_ACQUISITIONS, |summary| {
        summary.minimum_acquisitions
    });
    Decision {
        version: VERSION,
        claim: "observed-microbench-group-comparison-not-serving-speed",
        scope: format!(
            "{scope} At least {minimum} complete measured acquisitions are required in every A/B/A2 role; missing, extra or unusable acquisitions withhold the verdict. This is a measurement-layer observation only; kernel or collective results never establish serving-speed impact, capacity, adoption or live qualification."
        ),
        statistic: "mean-duration-ns",
        statistic_definition: "exact arithmetic mean within one acquisition (rank scope: mean of complete per-repetition maxima); roles compare acquisition statistics, and the reference envelope pools the A and A2 roles",
        direction: "lower_better",
        axis: if groups.baseline.revision.is_some()
            && groups.baseline.revision == groups.candidate.revision
        {
            "same-implementation-control"
        } else {
            "observed-kernel-revision"
        },
        axis_baseline: groups.baseline.revision.clone(),
        axis_candidate: groups.candidate.revision.clone(),
        decision,
        adverse_bounds,
        thresholds,
        study_sha256,
        study,
        roles: groups,
        reference_range,
        candidate_range,
        collector_sha256,
        evaluator_sha256,
        reason_codes,
    }
}

pub fn human_capture(report: &CaptureReport) -> String {
    let mut text = String::new();
    text.push_str(&format!(
        "{}: {} via {} ({})\n",
        report.status,
        report.adapter.name(),
        report.exerciser,
        report.provenance_name()
    ));
    text.push_str(&format!("  evidence: {}\n", report.out));
    text.push_str(&format!("  samples: {}", report.samples));
    if let Some(statistic) = report.statistic {
        text.push_str(&format!(
            "; mean {}/{} ns",
            statistic.numerator, statistic.denominator
        ));
    }
    text.push('\n');
    for reason in &report.invalid {
        text.push_str(&format!("  invalid: {reason:?}\n"));
    }
    for reason in &report.unavailable {
        text.push_str(&format!("  unavailable: {reason:?}\n"));
    }
    if let Some(failure) = &report.failure {
        text.push_str(&format!("  failure: {failure}\n"));
    }
    text.push_str(&format!("  scope: {}\n", report.scope));
    text
}

pub fn human_inspection(inspection: &Inspection) -> String {
    let mut text = String::new();
    text.push_str(&format!(
        "{} version {} adapter {}, provenance {}, plan verified {}\n",
        inspection.kind,
        inspection.version,
        inspection.adapter.name(),
        inspection.provenance_name(),
        inspection.plan_verified
    ));
    text.push_str(&format!(
        "  revision {}; samples {}; exerciser {}\n",
        inspection.revision, inspection.samples, inspection.exerciser
    ));
    if let Some(statistic) = inspection.statistic {
        text.push_str(&format!(
            "  mean duration {}/{} ns on clock {} ({})\n",
            statistic.numerator,
            statistic.denominator,
            inspection.clock.id,
            inspection.clock.units.name()
        ));
    }
    for observation in &inspection.observations {
        text.push_str(&format!(
            "  observed {:?} = {}\n",
            observation.name, observation.value
        ));
    }
    if let Some(derived) = &inspection.derived {
        text.push_str(&format!(
            "  {}: algorithm bandwidth {:.3} B/s\n",
            derived.definition, derived.algorithm_bandwidth_display
        ));
        text.push_str(&format!(
            "  bus normalization {}/{} = {:.3} ({})\n",
            derived.bus_normalization.numerator,
            derived.bus_normalization.denominator,
            derived.bus_normalization_display,
            derived.bus_scope
        ));
    }
    for reason in &inspection.invalid {
        text.push_str(&format!("  invalid: {reason:?}\n"));
    }
    for reason in &inspection.unavailable {
        text.push_str(&format!("  unavailable: {reason:?}\n"));
    }
    for failure in &inspection.failures {
        text.push_str(&format!(
            "  retained failure {:?}: {}\n",
            failure.kind, failure.detail
        ));
    }
    text.push_str(&format!("  scope: {}\n", inspection.scope));
    text
}

pub fn human_decision(decision: &Decision) -> String {
    let mut text = String::new();
    text.push_str(&format!(
        "microbench group comparison ({}): {:?}\n",
        decision.statistic, decision.decision
    ));
    if let Some(study) = &decision.study {
        text.push_str(&format!(
            "  study {}: {} declared acquisitions, {} present, {} complete; minimum {} per role\n",
            study.adapter.name(),
            study.declared_acquisitions,
            study.present_acquisitions,
            study.complete_acquisitions,
            study.minimum_acquisitions
        ));
    }
    for (name, group) in [
        ("baseline", &decision.roles.baseline),
        ("candidate", &decision.roles.candidate),
        ("reference", &decision.roles.reference),
    ] {
        text.push_str(&format!(
            "  {name}: revision {}, {}/{} complete, eligible {}\n",
            OrNull(group.revision.as_deref()),
            group.complete,
            group.declared,
            group.eligible
        ));
        for view in &group.acquisitions {
            match view.statistic {
                Some(statistic) => text.push_str(&format!(
                    "    {}: {}/{} ns\n",
                    view.slot, statistic.numerator, statistic.denominator
                )),
                None => text.push_str(&format!("    {}: unavailable\n", view.slot)),
            }
            for reason in view.invalid.iter().chain(&view.unavailable) {
                text.push_str(&format!("      {reason:?}\n"));
            }
        }
    }
    if let Some(bounds) = decision.adverse_bounds {
        text.push_str(&format!(
            "  adverse bounds: {:+.4} .. {:+.4}\n",
            bounds[0], bounds[1]
        ));
    }
    for reason in &decision.reason_codes {
        text.push_str(&format!("  reason: {reason:?}\n"));
    }
    text.push_str(&format!("  scope: {}\n", decision.scope));
    text
}

#[cfg(test)]
#[path = "../tests/support/microbench_model.rs"]
mod microbench_model;
