use crate::{evidence, model::*};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

pub const CAP: usize = 64 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    version: u32,
    method: String,
    id: String,
    collector_sha256: String,
    workload_source_sha256: String,
    min_trials: u32,
    cells: Vec<CellPolicy>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CellPolicy {
    cell: String,
    metrics: Vec<MetricPolicy>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetricPolicy {
    metric: Metric,
    max_regression_bps: u32,
    max_reference_spread_bps: u32,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
enum Metric {
    WaveLatencyUs,
    AchievedCompletionTokensPerSecond,
    DecodeTokensPerSecond,
    PrefillTokensPerSecond,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    InvalidEvidence,
    InvalidPolicy,
    PolicyHashMismatch,
    PolicySourceMismatch,
    PolicyCollectorMismatch,
    PolicyScopeMismatch,
    InsufficientDeclaredTrials,
    MissingPolicy,
    ConflictingPolicy,
    MissingReference,
    IncompatibleEvidence,
    ReferenceUnqualified,
    RoleReuse,
    DeclaredStartsOutOfOrder,
    SessionIncomplete,
    WarmupIncomplete,
    MetricUnavailable,
    OutputAmountsMismatch,
    NonpositiveReference,
    ReferenceSpreadExceeded,
    EnvelopeStraddlesTolerance,
    ArithmeticOverflow,
    EvaluatorUnavailable,
}
impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidEvidence => "invalid_evidence",
            Self::InvalidPolicy => "invalid_policy",
            Self::PolicyHashMismatch => "policy_hash_mismatch",
            Self::PolicySourceMismatch => "policy_source_mismatch",
            Self::PolicyCollectorMismatch => "policy_collector_mismatch",
            Self::PolicyScopeMismatch => "policy_scope_mismatch",
            Self::InsufficientDeclaredTrials => "insufficient_declared_trials",
            Self::MissingPolicy => "missing_policy",
            Self::ConflictingPolicy => "conflicting_policy",
            Self::MissingReference => "missing_reference",
            Self::IncompatibleEvidence => "incompatible_evidence",
            Self::ReferenceUnqualified => "reference_unqualified",
            Self::RoleReuse => "role_reuse",
            Self::DeclaredStartsOutOfOrder => "declared_starts_out_of_order",
            Self::SessionIncomplete => "session_incomplete",
            Self::WarmupIncomplete => "warmup_incomplete",
            Self::MetricUnavailable => "metric_unavailable",
            Self::OutputAmountsMismatch => "output_amounts_mismatch",
            Self::NonpositiveReference => "nonpositive_reference",
            Self::ReferenceSpreadExceeded => "reference_spread_exceeded",
            Self::EnvelopeStraddlesTolerance => "envelope_straddles_tolerance",
            Self::ArithmeticOverflow => "arithmetic_overflow",
            Self::EvaluatorUnavailable => "evaluator_unavailable",
        }
    }
}
fn sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn parse(
    bytes: &[u8],
    collector_sha256: &str,
    source_sha256: &str,
    workload: &Workload,
) -> Result<Policy, Reason> {
    if bytes.len() > CAP {
        return Err(Reason::InvalidPolicy);
    }
    let policy: Policy = serde_json::from_slice(bytes).map_err(|_| Reason::InvalidPolicy)?;
    if policy.version != 1
        || policy.method != "observed-envelope-v1"
        || !identifier(&policy.id)
        || !sha(&policy.collector_sha256)
        || !sha(&policy.workload_source_sha256)
        || !(3..=100).contains(&policy.min_trials)
    {
        return Err(Reason::InvalidPolicy);
    }
    if policy.collector_sha256 != collector_sha256 {
        return Err(Reason::PolicyCollectorMismatch);
    }
    if policy.workload_source_sha256 != source_sha256 {
        return Err(Reason::PolicySourceMismatch);
    }
    if policy.cells.len() != workload.cells.len() {
        return Err(Reason::PolicyScopeMismatch);
    }
    let mut cells = HashSet::new();
    for declared in &policy.cells {
        let cell = workload
            .cells
            .iter()
            .find(|c| c.id == declared.cell)
            .ok_or(Reason::PolicyScopeMismatch)?;
        if !cells.insert(&declared.cell)
            || declared.metrics.is_empty()
            || declared.metrics.len() > 4
        {
            return Err(Reason::PolicyScopeMismatch);
        }
        if cell.trials < policy.min_trials {
            return Err(Reason::InsufficientDeclaredTrials);
        }
        let mut metrics = HashSet::new();
        for metric in &declared.metrics {
            if !metrics.insert(metric.metric)
                || metric.max_regression_bps > 9999
                || metric.max_reference_spread_bps > 1_000_000
            {
                return Err(Reason::InvalidPolicy);
            }
        }
    }
    Ok(policy)
}

// Declaration order is also the aggregate precedence, not a vote across gates.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "UPPERCASE")]
pub enum Outcome {
    Pass,
    Inconclusive,
    Regression,
    Error,
}
impl Outcome {
    pub fn exit(self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Error => 1,
            Self::Inconclusive => 2,
            Self::Regression => 3,
        }
    }
}
#[derive(Clone, Copy, Serialize)]
struct Rational {
    numerator: u64,
    denominator: u64,
}
impl From<(u64, u64)> for Rational {
    fn from((numerator, denominator): (u64, u64)) -> Self {
        Self {
            numerator,
            denominator,
        }
    }
}
fn order(a: Rational, b: Rational) -> std::cmp::Ordering {
    (u128::from(a.numerator) * u128::from(b.denominator))
        .cmp(&(u128::from(b.numerator) * u128::from(a.denominator)))
}
fn scaled(a: Rational, scale: u32, b: Rational) -> Result<u128, Reason> {
    u128::from(a.numerator)
        .checked_mul(u128::from(scale))
        .and_then(|n| n.checked_mul(u128::from(b.denominator)))
        .ok_or(Reason::ArithmeticOverflow)
}
fn range(values: &[Rational]) -> Option<[Rational; 2]> {
    let first = *values.first()?;
    Some(
        values
            .iter()
            .copied()
            .fold([first, first], |[low, high], v| {
                [
                    if order(v, low).is_lt() { v } else { low },
                    if order(v, high).is_gt() { v } else { high },
                ]
            }),
    )
}
#[derive(Serialize)]
struct Coverage {
    expected_waves: u32,
    observed_waves: usize,
    eligible_waves: usize,
    expected_observations: u32,
    observed_observations: usize,
    expected_warmups: u32,
    eligible_warmups: usize,
}
#[derive(Serialize)]
struct Roles<T> {
    baseline: T,
    candidate: T,
    reference: T,
}
#[derive(Serialize)]
struct RoleIdentity {
    plan_sha256: String,
    evidence_sha256: String,
    collector_sha256: String,
    workload_source_sha256: String,
    policy_sha256: Option<String>,
}
#[derive(Serialize)]
struct Gate {
    cell: String,
    metric: Metric,
    max_regression_bps: u32,
    max_reference_spread_bps: u32,
    coverage: Roles<Coverage>,
    ranges: Roles<Option<[Rational; 2]>>,
    pooled_reference_range: Option<[Rational; 2]>,
    adverse_bounds: Option<[f64; 2]>,
    decision: Outcome,
    reason_codes: Vec<Reason>,
}
#[derive(Serialize)]
pub struct Decision {
    version: u32,
    claim: &'static str,
    pub decision: Outcome,
    pub eligibility: bool,
    policy_sha256: Option<String>,
    policy_id: Option<String>,
    min_trials: Option<u32>,
    evaluator_sha256: Option<String>,
    roles: Roles<Option<RoleIdentity>>,
    gates: Vec<Gate>,
    reason_codes: Vec<Reason>,
}
fn observations(
    run: Option<&evidence::Loaded>,
    cell: &Cell,
    metric: Metric,
) -> (Coverage, Vec<Rational>) {
    let mut coverage = Coverage {
        expected_waves: cell.trials,
        observed_waves: 0,
        eligible_waves: 0,
        expected_observations: cell.trials
            * if matches!(
                metric,
                Metric::WaveLatencyUs | Metric::AchievedCompletionTokensPerSecond
            ) {
                1
            } else {
                cell.concurrency
            },
        observed_observations: 0,
        expected_warmups: cell.warmup_trials,
        eligible_warmups: 0,
    };
    let mut values = Vec::new();
    if let Some(run) = run {
        for wave in run
            .waves
            .iter()
            .flatten()
            .filter(|w| w.spec.cell == cell.id)
        {
            if wave.spec.phase == Phase::Warmup {
                coverage.eligible_warmups += usize::from(wave.eligible);
                continue;
            }
            coverage.observed_waves += 1;
            coverage.eligible_waves += usize::from(wave.eligible);
            match metric {
                Metric::WaveLatencyUs => {
                    if wave.elapsed_us > 0 {
                        values.push((wave.elapsed_us, 1).into());
                    }
                }
                Metric::AchievedCompletionTokensPerSecond => {
                    if let Some(tokens) = wave
                        .completion_tokens
                        .filter(|_| wave.eligible && wave.elapsed_us > 0)
                    {
                        values.push((tokens, wave.elapsed_us).into());
                    }
                }
                Metric::DecodeTokensPerSecond | Metric::PrefillTokensPerSecond => {
                    values.extend(
                        wave.attempts
                            .iter()
                            .filter_map(|a| {
                                if metric == Metric::DecodeTokensPerSecond {
                                    evidence::decode_sample(a)
                                } else {
                                    evidence::prefill_sample(a)
                                }
                            })
                            .map(Rational::from),
                    );
                }
            }
        }
    }
    coverage.observed_observations = values.len();
    (coverage, values)
}
fn amounts_match(a: &evidence::Loaded, b: &evidence::Loaded, cell: &str) -> bool {
    a.plan
        .waves
        .iter()
        .enumerate()
        .filter(|(_, s)| s.phase == Phase::Measured && s.cell == cell)
        .all(|(i, _)| match (&a.waves[i], &b.waves[i]) {
            (Some(a), Some(b)) => a.attempts.iter().zip(&b.attempts).all(|(a, b)| {
                a.usage.completion_tokens.is_some()
                    && a.usage.completion_tokens == b.usage.completion_tokens
            }),
            _ => false,
        })
}
fn evaluate(
    gate: &mut Gate,
    pooled: [Rational; 2],
    candidate: [Rational; 2],
) -> Result<(), Reason> {
    let [low, high] = pooled;
    let [c_low, c_high] = candidate;
    gate.pooled_reference_range = Some(pooled);
    if low.numerator == 0 {
        return Err(Reason::NonpositiveReference);
    }
    if scaled(high, 10000, low)? > scaled(low, 10000 + gate.max_reference_spread_bps, high)? {
        return Err(Reason::ReferenceSpreadExceeded);
    }
    let ratio = |a: Rational, b: Rational| {
        (a.numerator as f64 / a.denominator as f64) / (b.numerator as f64 / b.denominator as f64)
    };
    let latency = gate.metric == Metric::WaveLatencyUs;
    gate.adverse_bounds = Some(if latency {
        [ratio(c_low, high) - 1.0, ratio(c_high, low) - 1.0]
    } else {
        [1.0 - ratio(c_high, low), 1.0 - ratio(c_low, high)]
    });
    let tolerance = gate.max_regression_bps;
    let (pass, regression) = if latency {
        (
            scaled(c_high, 10000, low)? <= scaled(low, 10000 + tolerance, c_high)?,
            scaled(c_low, 10000, high)? > scaled(high, 10000 + tolerance, c_low)?,
        )
    } else {
        (
            scaled(c_low, 10000, high)? >= scaled(high, 10000 - tolerance, c_low)?,
            scaled(c_high, 10000, low)? < scaled(low, 10000 - tolerance, c_high)?,
        )
    };
    gate.decision = if pass {
        Outcome::Pass
    } else if regression {
        Outcome::Regression
    } else {
        return Err(Reason::EnvelopeStraddlesTolerance);
    };
    Ok(())
}
fn add(reasons: &mut Vec<Reason>, reason: Reason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}
pub fn decide(a: &Path, b: &Path, reference: Option<&Path>) -> Decision {
    let mut result = Decision {
        version: 1,
        claim: "observed-policy-decision-not-statistical-or-causal",
        decision: Outcome::Pass,
        eligibility: false,
        policy_sha256: None,
        policy_id: None,
        min_trials: None,
        evaluator_sha256: None,
        roles: Roles {
            baseline: None,
            candidate: None,
            reference: None,
        },
        gates: Vec::new(),
        reason_codes: Vec::new(),
    };
    match evidence::binary_digest() {
        Ok(hash) => result.evaluator_sha256 = Some(hash),
        Err(_) => {
            result.decision = Outcome::Error;
            add(&mut result.reason_codes, Reason::EvaluatorUnavailable);
        }
    }
    let mut load = |path: Option<&Path>| match path.map(evidence::load_verified) {
        Some(Ok(run)) if sha(&run.plan.collector_sha256) => Some(run),
        Some(Ok(_)) => {
            result.decision = Outcome::Error;
            add(&mut result.reason_codes, Reason::InvalidEvidence);
            None
        }
        Some(Err(error)) => {
            result.decision = Outcome::Error;
            add(&mut result.reason_codes, error.reason);
            None
        }
        None => {
            add(&mut result.reason_codes, Reason::MissingReference);
            None
        }
    };
    let left = load(Some(a));
    let right = load(Some(b));
    let repeat = load(reference);
    let runs = [left.as_ref(), right.as_ref(), repeat.as_ref()];
    let identity = |run: Option<&evidence::Loaded>| {
        run.map(|r| RoleIdentity {
            plan_sha256: r.plan_sha256.clone(),
            evidence_sha256: r.evidence_sha256.clone(),
            collector_sha256: r.plan.collector_sha256.clone(),
            workload_source_sha256: r.plan.source_sha256.clone(),
            policy_sha256: r.plan.policy_sha256.clone(),
        })
    };
    result.roles = Roles {
        baseline: identity(runs[0]),
        candidate: identity(runs[1]),
        reference: identity(runs[2]),
    };
    let mut bound = None;
    for run in runs.into_iter().flatten() {
        match run.plan.policy_sha256.as_ref() {
            None => add(&mut result.reason_codes, Reason::MissingPolicy),
            Some(hash) => {
                if bound.is_some_and(|prior| prior != hash) {
                    result.decision = Outcome::Error;
                    add(&mut result.reason_codes, Reason::ConflictingPolicy);
                }
                bound = Some(hash);
            }
        }
        if run.history.count != 1
            || run.history.open
            || run.history.last_status.as_deref() != Some("completed")
        {
            add(&mut result.reason_codes, Reason::SessionIncomplete);
        }
        if let Some(left) = &left
            && !evidence::compatible(&left.plan, &run.plan)
        {
            result.decision = Outcome::Error;
            add(&mut result.reason_codes, Reason::IncompatibleEvidence);
        }
    }
    for i in 0..runs.len() {
        for j in i + 1..runs.len() {
            if let (Some(a), Some(b)) = (runs[i], runs[j]) {
                if a.plan_sha256 == b.plan_sha256 || a.lineage_sha256 == b.lineage_sha256 {
                    add(&mut result.reason_codes, Reason::RoleReuse);
                }
                if a.plan.started_unix_ms >= b.plan.started_unix_ms {
                    add(&mut result.reason_codes, Reason::DeclaredStartsOutOfOrder);
                }
            }
        }
    }
    if let (Some(a), Some(a2)) = (runs[0], runs[2])
        && evidence::reference_identity(&a.plan, &a2.plan).status
            != evidence::ReferenceIdentityStatus::DeclaredMatch
    {
        add(&mut result.reason_codes, Reason::ReferenceUnqualified);
    }
    let qualified = result.reason_codes.is_empty();
    let invalid = result.decision == Outcome::Error;
    if !qualified {
        result.decision = result.decision.max(Outcome::Inconclusive);
    }
    if let Some(a) = &left {
        result.policy_sha256 = a.plan.policy_sha256.clone();
        if let Some(policy) = &a.policy {
            result.policy_id = Some(policy.id.clone());
            result.min_trials = Some(policy.min_trials);
            for declared in &policy.cells {
                // Policy admission already proved exact cell coverage.
                let cell = a
                    .plan
                    .workload
                    .cells
                    .iter()
                    .find(|c| c.id == declared.cell)
                    .unwrap();
                for metric in &declared.metrics {
                    let [(ac, av), (bc, bv), (rc, rv)] =
                        runs.map(|r| observations(r, cell, metric.metric));
                    let mut gate = Gate {
                        cell: cell.id.clone(),
                        metric: metric.metric,
                        max_regression_bps: metric.max_regression_bps,
                        max_reference_spread_bps: metric.max_reference_spread_bps,
                        coverage: Roles {
                            baseline: ac,
                            candidate: bc,
                            reference: rc,
                        },
                        ranges: Roles {
                            baseline: range(&av),
                            candidate: range(&bv),
                            reference: range(&rv),
                        },
                        pooled_reference_range: None,
                        adverse_bounds: None,
                        decision: Outcome::Inconclusive,
                        reason_codes: result.reason_codes.clone(),
                    };
                    for coverage in [
                        &gate.coverage.baseline,
                        &gate.coverage.candidate,
                        &gate.coverage.reference,
                    ] {
                        if coverage.expected_warmups == 0
                            || coverage.eligible_warmups != coverage.expected_warmups as usize
                        {
                            add(&mut gate.reason_codes, Reason::WarmupIncomplete);
                        }
                        if coverage.eligible_waves != coverage.expected_waves as usize
                            || coverage.observed_observations
                                != coverage.expected_observations as usize
                        {
                            add(&mut gate.reason_codes, Reason::MetricUnavailable);
                        }
                    }
                    if let (Some(b), Some(r)) = (runs[1], runs[2])
                        && evidence::compatible(&a.plan, &b.plan)
                        && evidence::compatible(&a.plan, &r.plan)
                        && (!amounts_match(a, b, &cell.id) || !amounts_match(a, r, &cell.id))
                    {
                        add(&mut gate.reason_codes, Reason::OutputAmountsMismatch);
                    }
                    if gate.reason_codes.is_empty()
                        && let (Some(ar), Some(rr), Some(br)) = (
                            gate.ranges.baseline,
                            gate.ranges.reference,
                            gate.ranges.candidate,
                        )
                    {
                        let pooled = range(&[ar[0], ar[1], rr[0], rr[1]]).unwrap();
                        if let Err(reason) = evaluate(&mut gate, pooled, br) {
                            gate.decision = if reason == Reason::ArithmeticOverflow {
                                Outcome::Error
                            } else {
                                Outcome::Inconclusive
                            };
                            add(&mut gate.reason_codes, reason);
                        }
                    } else if invalid {
                        gate.decision = Outcome::Error;
                    }
                    result.decision = result.decision.max(gate.decision);
                    result.gates.push(gate);
                }
            }
        }
    }
    result.eligibility = qualified
        && result.gates.iter().all(|g| {
            g.reason_codes.iter().all(|r| {
                matches!(
                    r,
                    Reason::ReferenceSpreadExceeded
                        | Reason::EnvelopeStraddlesTolerance
                        | Reason::NonpositiveReference
                )
            })
        });
    result
}
