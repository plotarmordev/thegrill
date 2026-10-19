use crate::{
    envelope::{self, Direction, EnvelopeDecision, EnvelopeReason, Rational, range},
    evidence,
    model::*,
};
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
    #[serde(default, deserialize_with = "Policy::deserialize_telemetry")]
    required_telemetry: Option<Vec<crate::metrics::v2::Requirement>>,
    cells: Vec<CellPolicy>,
    #[serde(default, deserialize_with = "present")]
    whole_conversation: Option<Thresholds>,
    #[serde(default, deserialize_with = "present")]
    tail: Option<Vec<TailPolicy>>,
}
impl Policy {
    fn deserialize_telemetry<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<crate::metrics::v2::Requirement>>, D::Error> {
        Vec::deserialize(deserializer).map(Some)
    }
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
    #[serde(default, deserialize_with = "MetricPolicy::deserialize_lane")]
    lane: Option<String>,
    max_regression_bps: u32,
    max_reference_spread_bps: u32,
}
impl MetricPolicy {
    fn deserialize_lane<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        String::deserialize(deserializer).map(Some)
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Thresholds {
    max_regression_bps: u32,
    max_reference_spread_bps: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TailTarget {
    Completion { cell: String },
    WholeConversation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TailPolicy {
    target: TailTarget,
    percentile: crate::acquisition::Percentile,
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
    FirstGeneratedTextUs,
    FirstAnswerTextUs,
    CompletionLatencyUs,
    WorstLaneFirstAnswerUs,
    FirstAnswerMaxMinRatio,
    FirstToolDeltaUs,
    FirstValidatedToolCallUs,
    WholeConversationUs,
}
impl Metric {
    fn legacy(self) -> bool {
        matches!(
            self,
            Self::WaveLatencyUs
                | Self::AchievedCompletionTokensPerSecond
                | Self::DecodeTokensPerSecond
                | Self::PrefillTokensPerSecond
        )
    }

    fn aggregate(self) -> bool {
        matches!(
            self,
            Self::WorstLaneFirstAnswerUs | Self::FirstAnswerMaxMinRatio
        )
    }

    fn per_wave(self) -> bool {
        self.aggregate()
            || matches!(
                self,
                Self::WaveLatencyUs | Self::AchievedCompletionTokensPerSecond
            )
    }

    fn direction(self) -> Direction {
        match self {
            Self::WaveLatencyUs
            | Self::FirstGeneratedTextUs
            | Self::FirstAnswerTextUs
            | Self::CompletionLatencyUs
            | Self::WorstLaneFirstAnswerUs
            | Self::FirstAnswerMaxMinRatio
            | Self::FirstToolDeltaUs
            | Self::FirstValidatedToolCallUs
            | Self::WholeConversationUs => Direction::LowerBetter,
            Self::AchievedCompletionTokensPerSecond
            | Self::DecodeTokensPerSecond
            | Self::PrefillTokensPerSecond => Direction::HigherBetter,
        }
    }
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
    RequiredTelemetryUnavailable,
    RequiredTelemetryRefuted,
    InvalidRequiredTelemetry,
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
            Self::RequiredTelemetryUnavailable => "required_telemetry_unavailable",
            Self::RequiredTelemetryRefuted => "required_telemetry_refuted",
            Self::InvalidRequiredTelemetry => "invalid_required_telemetry",
        }
    }
}
#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
pub fn parse(
    bytes: &[u8],
    collector_sha256: &str,
    source_sha256: &str,
    workload: &Workload,
    telemetry: Option<&crate::metrics::Protocol>,
) -> Result<Policy, Reason> {
    // Attached serving resources are explicitly review-only. A throughput
    // policy cannot turn an unqualified resource/capacity claim into PASS.
    if workload.resources.is_some() {
        return Err(Reason::PolicyScopeMismatch);
    }
    if bytes.len() > CAP {
        return Err(Reason::InvalidPolicy);
    }
    let policy: Policy = serde_json::from_slice(bytes).map_err(|_| Reason::InvalidPolicy)?;
    if !matches!(
        (policy.version, policy.method.as_str()),
        (1, "observed-envelope-v1") | (2, "observed-envelope-v2") | (3, "observed-envelope-v3")
    ) || !identifier(&policy.id)
        || !evidence::is_digest(&policy.collector_sha256)
        || !evidence::is_digest(&policy.workload_source_sha256)
        || !(3..=if policy.version == 3 { 1000 } else { 100 }).contains(&policy.min_trials)
    {
        return Err(Reason::InvalidPolicy);
    }
    if policy.collector_sha256 != collector_sha256 {
        return Err(Reason::PolicyCollectorMismatch);
    }
    if policy.workload_source_sha256 != source_sha256 {
        return Err(Reason::PolicySourceMismatch);
    }
    if (policy.version == 1 && workload.version >= 4)
        || (policy.version == 2 && !matches!(workload.version, 1 | 3 | 4))
        || (policy.version == 3 && workload.version != 6)
    {
        return Err(Reason::PolicyScopeMismatch);
    }
    if let Some(requirements) = &policy.required_telemetry {
        if !matches!(policy.version, 2 | 3) || requirements.is_empty() || requirements.len() > 256 {
            return Err(Reason::InvalidPolicy);
        }
        let Some(crate::metrics::Protocol::V2(config)) = telemetry else {
            return Err(Reason::PolicyScopeMismatch);
        };
        let mut identities = HashSet::new();
        for requirement in requirements {
            if !identities.insert((&requirement.name, &requirement.labels))
                || crate::metrics::v2::validate_requirement(requirement).is_err()
            {
                return Err(Reason::InvalidRequiredTelemetry);
            }
            if requirement.predicate == crate::metrics::v2::Predicate::ZeroCounterDelta
                && config.isolation != crate::metrics::v2::ISOLATION_EXCLUSIVE
            {
                return Err(Reason::PolicyScopeMismatch);
            }
        }
    }
    if policy.version != 3 && (policy.whole_conversation.is_some() || policy.tail.is_some()) {
        return Err(Reason::InvalidPolicy);
    }
    let expected_cells = if workload.version == 4 {
        workload.schedule.as_ref().map_or(0, Vec::len)
    } else {
        workload
            .cells
            .iter()
            .filter(|c| {
                workload
                    .acquisition
                    .as_ref()
                    .is_none_or(|p| p.measured(&c.case))
            })
            .count()
    };
    if expected_cells == 0 || policy.cells.len() != expected_cells {
        return Err(Reason::PolicyScopeMismatch);
    }
    let mut cells = HashSet::new();
    for declared in &policy.cells {
        let cell = population(workload, &declared.cell).ok_or(Reason::PolicyScopeMismatch)?;
        if workload
            .acquisition
            .as_ref()
            .is_some_and(|p| !p.measured(&cell.case))
        {
            return Err(Reason::PolicyScopeMismatch);
        }
        if !cells.insert(&declared.cell)
            || declared.metrics.is_empty()
            || declared.metrics.len() > if policy.version == 1 { 4 } else { 256 }
        {
            return Err(Reason::PolicyScopeMismatch);
        }
        if cell.trials < policy.min_trials || (policy.version == 3 && cell.warmup_trials == 0) {
            return Err(Reason::InsufficientDeclaredTrials);
        }
        let mut metrics = HashSet::new();
        for metric in &declared.metrics {
            if !metrics.insert((metric.metric, metric.lane.as_deref()))
                || metric.max_regression_bps > 9999
                || metric.max_reference_spread_bps > 1_000_000
                || (policy.version == 1 && !metric.metric.legacy())
                || metric.metric == Metric::WholeConversationUs
                || (policy.version != 3
                    && matches!(
                        metric.metric,
                        Metric::FirstToolDeltaUs | Metric::FirstValidatedToolCallUs
                    ))
                || (workload.version != 4 && metric.lane.is_some())
                || (metric.metric.aggregate() && cell.concurrency < 2)
            {
                return Err(Reason::InvalidPolicy);
            }
            if workload.version == 4 {
                let scenario = workload
                    .schedule
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|s| s.id == declared.cell)
                    .unwrap();
                if metric.metric.legacy()
                    || (metric.metric.aggregate()
                        && (metric.lane.is_some()
                            || scenario.kind != crate::schedule::Kind::Overlap))
                    || (!metric.metric.aggregate()
                        && !scenario
                            .lanes
                            .iter()
                            .any(|l| Some(l.id.as_str()) == metric.lane.as_deref()))
                {
                    return Err(Reason::PolicyScopeMismatch);
                }
            }
            if matches!(
                metric.metric,
                Metric::FirstToolDeltaUs | Metric::FirstValidatedToolCallUs
            ) && !workload
                .cases
                .iter()
                .find(|c| c.id == cell.case)
                .and_then(|c| c.step.as_ref())
                .is_some_and(|s| matches!(s.expect, crate::sequence::Expected::Tool { .. }))
            {
                return Err(Reason::PolicyScopeMismatch);
            }
        }
    }
    if let Some(whole) = &policy.whole_conversation
        && (!crate::acquisition::conversation(workload)
            || !thresholds_valid(whole.max_regression_bps, whole.max_reference_spread_bps))
    {
        return Err(Reason::PolicyScopeMismatch);
    }
    if let Some(tails) = &policy.tail {
        if tails.is_empty() || tails.len() > 256 {
            return Err(Reason::InvalidPolicy);
        }
        let mut seen = HashSet::new();
        for tail in tails {
            if !seen.insert((&tail.target, tail.percentile))
                || !thresholds_valid(tail.max_regression_bps, tail.max_reference_spread_bps)
            {
                return Err(Reason::InvalidPolicy);
            }
            match &tail.target {
                TailTarget::Completion { cell } => {
                    if crate::acquisition::conversation(workload)
                        || population(workload, cell).is_none()
                    {
                        return Err(Reason::PolicyScopeMismatch);
                    }
                }
                TailTarget::WholeConversation => {
                    if !crate::acquisition::conversation(workload) {
                        return Err(Reason::PolicyScopeMismatch);
                    }
                }
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

impl From<EnvelopeReason> for Outcome {
    fn from(reason: EnvelopeReason) -> Self {
        match reason {
            EnvelopeReason::InvalidRational
            | EnvelopeReason::InvalidBounds
            | EnvelopeReason::ArithmeticOverflow { .. } => Self::Error,
            _ => Self::Inconclusive,
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    lane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<TailTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    percentile: Option<crate::acquisition::Percentile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_unit: Option<&'static str>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    whole_conversation: Option<Gate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tail: Option<Vec<Gate>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required_telemetry: Option<Roles<Option<crate::metrics::v2::Assessment>>>,
    reason_codes: Vec<Reason>,
}
fn observations(
    run: Option<&evidence::Loaded>,
    cell: &Population,
    metric: Metric,
    lane: Option<&str>,
) -> (Coverage, Vec<Rational>) {
    let mut coverage = Coverage {
        expected_waves: cell.trials,
        observed_waves: 0,
        eligible_waves: 0,
        expected_observations: cell.trials
            * if metric.per_wave() || lane.is_some() {
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
                Metric::FirstGeneratedTextUs
                | Metric::FirstAnswerTextUs
                | Metric::CompletionLatencyUs
                | Metric::FirstToolDeltaUs
                | Metric::FirstValidatedToolCallUs => {
                    values.extend(
                        wave.attempts
                            .iter()
                            .filter(|a| {
                                lane.is_none_or(|id| {
                                    wave.spec
                                        .lanes
                                        .as_ref()
                                        .and_then(|lanes| lanes.get(a.lane as usize))
                                        .is_some_and(|l| l.id == id)
                                })
                            })
                            .filter_map(|a| latency_sample(a, metric)),
                    );
                }
                Metric::WholeConversationUs => (),
                Metric::WorstLaneFirstAnswerUs | Metric::FirstAnswerMaxMinRatio => {
                    if let Some(sample) = fairness_sample(wave, metric) {
                        values.push(sample);
                    }
                }
            }
        }
    }
    coverage.observed_observations = values.len();
    (coverage, values)
}

fn latency_sample(attempt: &Attempt, metric: Metric) -> Option<Rational> {
    if !attempt.dispatched
        || attempt.status != Status::Complete
        || !attempt.eligibility_errors.is_empty()
    {
        return None;
    }
    let value = match metric {
        Metric::FirstGeneratedTextUs => attempt.timing.first_generated_text_us?,
        Metric::FirstAnswerTextUs => attempt.timing.first_answer_text_us?,
        Metric::CompletionLatencyUs => attempt.timing.settle_us,
        Metric::FirstToolDeltaUs => attempt.timing.first_tool_delta_us?,
        Metric::FirstValidatedToolCallUs => attempt.timing.first_validated_tool_call_us?,
        _ => return None,
    };
    (value > 0).then(|| (value, 1).into())
}

fn fairness_sample(wave: &Wave, metric: Metric) -> Option<Rational> {
    if !wave.eligible
        || wave.spec.concurrency < 2
        || wave.attempts.len() != wave.spec.concurrency as usize
    {
        return None;
    }
    let mut low = u64::MAX;
    let mut high = 0;
    for (lane, attempt) in wave.attempts.iter().enumerate() {
        // Full admitted-lane coverage, never a ratio over surviving peers.
        if attempt.lane as usize != lane {
            return None;
        }
        let value = latency_sample(attempt, Metric::FirstAnswerTextUs)?.numerator;
        low = low.min(value);
        high = high.max(value);
    }
    match metric {
        Metric::WorstLaneFirstAnswerUs => Some((high, 1).into()),
        Metric::FirstAnswerMaxMinRatio => Some((high, low).into()),
        _ => None,
    }
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
fn envelope_reason(reason: EnvelopeReason) -> Reason {
    match reason {
        EnvelopeReason::InvalidRational | EnvelopeReason::InvalidBounds => Reason::InvalidEvidence,
        EnvelopeReason::ArithmeticOverflow { .. } => Reason::ArithmeticOverflow,
        EnvelopeReason::NonpositiveReference => Reason::NonpositiveReference,
        EnvelopeReason::ReferenceSpreadExceeded => Reason::ReferenceSpreadExceeded,
        EnvelopeReason::EnvelopeStraddlesTolerance { .. } => Reason::EnvelopeStraddlesTolerance,
    }
}

fn evaluate(
    gate: &mut Gate,
    pooled: [Rational; 2],
    candidate: [Rational; 2],
) -> Result<(), Reason> {
    gate.pooled_reference_range = Some(pooled);
    match envelope::assess(
        pooled,
        candidate,
        gate.max_regression_bps,
        gate.max_reference_spread_bps,
        gate.metric.direction(),
    ) {
        Ok(assessment) => {
            gate.adverse_bounds = Some(assessment.adverse_bounds);
            gate.decision = match assessment.decision {
                EnvelopeDecision::Pass => Outcome::Pass,
                EnvelopeDecision::Regression => Outcome::Regression,
            };
            Ok(())
        }
        Err(reason) => {
            gate.adverse_bounds = match reason {
                EnvelopeReason::ArithmeticOverflow { adverse_bounds } => adverse_bounds,
                EnvelopeReason::EnvelopeStraddlesTolerance { adverse_bounds } => {
                    Some(adverse_bounds)
                }
                _ => None,
            };
            Err(envelope_reason(reason))
        }
    }
}
fn add(reasons: &mut Vec<Reason>, reason: Reason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}
#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
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
        whole_conversation: None,
        tail: None,
        required_telemetry: None,
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
        Some(Ok(run)) if evidence::is_digest(&run.plan.collector_sha256) => Some(run),
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
            result.version = policy.version;
            result.policy_id = Some(policy.id.clone());
            result.min_trials = Some(policy.min_trials);
            for declared in &policy.cells {
                // Policy admission already proved exact cell coverage.
                let cell =
                    population(&a.plan.workload, &declared.cell).expect("admitted gate population");
                for metric in &declared.metrics {
                    let [(ac, av), (bc, bv), (rc, rv)] =
                        runs.map(|r| observations(r, &cell, metric.metric, metric.lane.as_deref()));
                    let sampled_ranges = [&av, &bv, &rv].map(|values| range(values));
                    let mut gate = Gate {
                        cell: cell.id.clone(),
                        metric: metric.metric,
                        lane: metric.lane.clone(),
                        target: None,
                        percentile: None,
                        sample_unit: (policy.version >= 2).then_some(if metric.lane.is_some() {
                            "named-lane-per-scenario-repetition"
                        } else if a
                            .plan
                            .workload
                            .acquisition
                            .as_ref()
                            .is_some_and(crate::acquisition::Protocol::conversation)
                        {
                            "same-step-per-complete-declared-acquisition"
                        } else if metric.metric.per_wave() {
                            "wave-repetition"
                        } else {
                            "lane-observations-within-wave-repetitions"
                        }),
                        max_regression_bps: metric.max_regression_bps,
                        max_reference_spread_bps: metric.max_reference_spread_bps,
                        coverage: Roles {
                            baseline: ac,
                            candidate: bc,
                            reference: rc,
                        },
                        ranges: Roles {
                            baseline: sampled_ranges[0].unwrap_or(None),
                            candidate: sampled_ranges[1].unwrap_or(None),
                            reference: sampled_ranges[2].unwrap_or(None),
                        },
                        pooled_reference_range: None,
                        adverse_bounds: None,
                        decision: Outcome::Inconclusive,
                        reason_codes: result.reason_codes.clone(),
                    };
                    for reason in sampled_ranges.into_iter().filter_map(|result| result.err()) {
                        gate.decision = Outcome::Error;
                        add(&mut gate.reason_codes, envelope_reason(reason));
                    }
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
                    if a.plan.workload.version == 4
                        && runs
                            .into_iter()
                            .flatten()
                            .any(|run| !mixed_qualified(run, &cell.id))
                    {
                        add(&mut gate.reason_codes, Reason::MetricUnavailable);
                    }
                    if a.plan
                        .workload
                        .acquisition
                        .as_ref()
                        .is_some_and(crate::acquisition::Protocol::conversation)
                        && (runs
                            .into_iter()
                            .flatten()
                            .any(|run| !conversation_qualified(run))
                            || runs[1].is_some_and(|run| !complete_amounts_match(a, run))
                            || runs[2].is_some_and(|run| !complete_amounts_match(a, run)))
                    {
                        add(&mut gate.reason_codes, Reason::MetricUnavailable);
                    }
                    if gate.reason_codes.is_empty()
                        && let (Some(ar), Some(rr), Some(br)) = (
                            gate.ranges.baseline,
                            gate.ranges.reference,
                            gate.ranges.candidate,
                        )
                    {
                        let assessment = range(&[ar[0], ar[1], rr[0], rr[1]])
                            .map_err(envelope_reason)
                            .and_then(|pooled| pooled.ok_or(Reason::InvalidEvidence))
                            .and_then(|pooled| evaluate(&mut gate, pooled, br));
                        if let Err(reason) = assessment {
                            gate.decision = if matches!(
                                reason,
                                Reason::ArithmeticOverflow | Reason::InvalidEvidence
                            ) {
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
    if let Some(policy) = left.as_ref().and_then(|r| r.policy.as_ref()) {
        if let Some(thresholds) = &policy.whole_conversation {
            let gate = acquisition_gate(
                runs,
                TailTarget::WholeConversation,
                None,
                thresholds,
                &result.reason_codes,
            );
            result.decision = result.decision.max(gate.decision);
            result.whole_conversation = Some(gate);
        }
        if let Some(tails) = &policy.tail {
            let gates: Vec<_> = tails
                .iter()
                .map(|tail| {
                    acquisition_gate(
                        runs,
                        tail.target.clone(),
                        Some(tail.percentile),
                        &Thresholds {
                            max_regression_bps: tail.max_regression_bps,
                            max_reference_spread_bps: tail.max_reference_spread_bps,
                        },
                        &result.reason_codes,
                    )
                })
                .collect();
            for gate in &gates {
                result.decision = result.decision.max(gate.decision);
            }
            result.tail = Some(gates);
        }
    }
    let mut telemetry_eligible = true;
    if let Some(requirements) = left
        .as_ref()
        .and_then(|run| run.policy.as_ref())
        .and_then(|policy| policy.required_telemetry.as_ref())
    {
        use crate::metrics::v2::AssessmentOutcome;
        let assessments = runs.map(|run| {
            run.and_then(|run| run.metrics.as_ref())
                .map(|summary| crate::metrics::v2::assess(requirements, summary))
        });
        for (role, assessment) in assessments.iter().enumerate() {
            match assessment.as_ref().map(|assessment| assessment.outcome) {
                Some(AssessmentOutcome::Satisfied) => (),
                Some(AssessmentOutcome::Refuted) if role == 1 => {
                    result.decision = result.decision.max(Outcome::Regression);
                    add(&mut result.reason_codes, Reason::RequiredTelemetryRefuted);
                }
                Some(AssessmentOutcome::Refuted) => {
                    result.decision = result.decision.max(Outcome::Inconclusive);
                    add(&mut result.reason_codes, Reason::ReferenceUnqualified);
                    telemetry_eligible = false;
                }
                Some(AssessmentOutcome::Error) => {
                    result.decision = Outcome::Error;
                    add(&mut result.reason_codes, Reason::InvalidRequiredTelemetry);
                    telemetry_eligible = false;
                }
                Some(AssessmentOutcome::Unavailable) | None => {
                    result.decision = result.decision.max(Outcome::Inconclusive);
                    add(
                        &mut result.reason_codes,
                        Reason::RequiredTelemetryUnavailable,
                    );
                    telemetry_eligible = false;
                }
            }
        }
        let [baseline, candidate, reference] = assessments;
        result.required_telemetry = Some(Roles {
            baseline,
            candidate,
            reference,
        });
    }
    result.eligibility = qualified
        && telemetry_eligible
        && !result.gates.is_empty()
        && result
            .gates
            .iter()
            .chain(result.whole_conversation.iter())
            .chain(result.tail.iter().flatten())
            .all(|g| {
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

#[cfg(test)]
#[path = "../tests/support/policy_calibration.rs"]
mod policy_calibration;

#[cfg(test)]
#[path = "../tests/support/policy_observations.rs"]
mod policy_observations;

// Gate population is not a synthetic serving Cell or request: scheduled lanes
// continue to bind their own resolved settings and exact named controls.
struct Population {
    id: String,
    case: String,
    trials: u32,
    warmup_trials: u32,
    concurrency: u32,
}
fn population(workload: &Workload, id: &str) -> Option<Population> {
    if workload.version == 4 {
        let scenario = workload.schedule.as_ref()?.iter().find(|s| s.id == id)?;
        return Some(Population {
            id: scenario.id.clone(),
            case: String::new(),
            trials: scenario.trials,
            warmup_trials: scenario.warmup_trials,
            concurrency: scenario.lanes.len() as u32,
        });
    }
    let cell = workload.cells.iter().find(|c| c.id == id)?;
    let (trials, warmup_trials) = workload
        .acquisition
        .as_ref()
        .map_or((cell.trials, cell.warmup_trials), |p| p.counts(cell));
    Some(Population {
        id: cell.id.clone(),
        case: cell.case.clone(),
        trials,
        warmup_trials,
        concurrency: cell.concurrency,
    })
}
fn thresholds_valid(adverse: u32, spread: u32) -> bool {
    adverse <= 9999 && spread <= 1_000_000
}
fn conversation_qualified(run: &evidence::Loaded) -> bool {
    [Phase::Warmup, Phase::Measured]
        .into_iter()
        .all(|phase| crate::acquisition::whole_samples(&run.plan, &run.waves, phase).is_ok())
}
fn complete_amounts_match(a: &evidence::Loaded, b: &evidence::Loaded) -> bool {
    a.plan.waves.len() == b.plan.waves.len()
        && a.waves.iter().zip(&b.waves).all(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => {
                a.eligible
                    && b.eligible
                    && a.spec == b.spec
                    && a.attempts.len() == b.attempts.len()
                    && a.attempts.iter().zip(&b.attempts).all(|(a, b)| {
                        a.usage.completion_tokens.is_some()
                            && a.usage.completion_tokens == b.usage.completion_tokens
                    })
            }
            _ => false,
        })
}
fn mixed_qualified(run: &evidence::Loaded, id: &str) -> bool {
    let Some(scenario) = run
        .plan
        .workload
        .schedule
        .as_ref()
        .and_then(|s| s.iter().find(|s| s.id == id))
    else {
        return false;
    };
    let waves: Vec<_> = run
        .waves
        .iter()
        .flatten()
        .filter(|w| w.spec.cell == id)
        .collect();
    if waves.len() != (scenario.trials + scenario.warmup_trials) as usize
        || waves.iter().any(|w| !w.eligible)
    {
        return false;
    }
    if scenario.kind == crate::schedule::Kind::Solo {
        return true;
    }
    for wave in waves {
        let Some(observation) = &wave.schedule else {
            return false;
        };
        if observation.fatal.is_some() || !required_overlap(scenario, observation) {
            return false;
        }
        for (index, lane) in scenario.lanes.iter().enumerate() {
            let Some(control) = &lane.control else {
                return false;
            };
            let Some(solo) = run.waves.iter().flatten().find(|w| {
                w.spec.cell == control.scenario
                    && w.spec.phase == wave.spec.phase
                    && w.spec.trial == wave.spec.trial
            }) else {
                return false;
            };
            let Some(position) = solo
                .spec
                .lanes
                .as_ref()
                .and_then(|ls| ls.iter().position(|l| l.id == control.lane))
            else {
                return false;
            };
            let Some(a) = wave.attempts.get(index) else {
                return false;
            };
            let Some(b) = solo.attempts.get(position) else {
                return false;
            };
            if !solo.eligible
                || a.usage.completion_tokens.is_none()
                || a.usage.completion_tokens != b.usage.completion_tokens
            {
                return false;
            }
        }
    }
    true
}
fn required_overlap(
    scenario: &crate::schedule::Scenario,
    observed: &crate::schedule::Observation,
) -> bool {
    // Each triggered edge must exhibit its declared source decode / target
    // prefill intersection. Fixed-offset lanes need an actual mixed interval
    // involving that lane. In-flight settlement alone never establishes this.
    scenario.lanes.iter().all(|lane| match &lane.arrival {
        crate::schedule::Arrival::AfterFirstGenerated { lane: source, .. } => {
            observed.overlap.iter().any(|o| {
                o.request_inflight
                    && ((&o.left == source && o.right == lane.id && o.left_decode_right_prefill)
                        || (&o.right == source && o.left == lane.id && o.right_decode_left_prefill))
            })
        }
        crate::schedule::Arrival::FixedOffset { .. } => observed.overlap.iter().any(|o| {
            o.request_inflight
                && (o.left == lane.id || o.right == lane.id)
                && (o.left_decode_right_prefill || o.right_decode_left_prefill)
        }),
    })
}
fn completion_population(run: &evidence::Loaded, cell: &str, phase: Phase) -> Option<Vec<u64>> {
    let expected = population(&run.plan.workload, cell)?;
    let expected = if phase == Phase::Warmup {
        expected.warmup_trials
    } else {
        expected.trials
    };
    let mut values = Vec::with_capacity(expected as usize);
    for wave in run
        .waves
        .iter()
        .flatten()
        .filter(|w| w.spec.cell == cell && w.spec.phase == phase)
    {
        values.push(completion_wave(wave)?);
    }
    (values.len() == expected as usize).then_some(values)
}
fn acquisition_gate(
    runs: [Option<&evidence::Loaded>; 3],
    target: TailTarget,
    percentile: Option<crate::acquisition::Percentile>,
    thresholds: &Thresholds,
    inherited: &[Reason],
) -> Gate {
    let workload = runs[0].map(|r| &r.plan.workload);
    let (trials, warmups) = match (&target, workload) {
        (TailTarget::WholeConversation, Some(w)) => w
            .cells
            .first()
            .and_then(|c| w.acquisition.as_ref().map(|p| p.counts(c)))
            .unwrap_or((0, 0)),
        (TailTarget::Completion { cell }, Some(w)) => population(w, cell)
            .map(|p| (p.trials, p.warmup_trials))
            .unwrap_or((0, 0)),
        _ => (0, 0),
    };
    let observations = runs.map(|run| {
        let sample = |phase| {
            run.and_then(|r| match &target {
                TailTarget::WholeConversation => {
                    crate::acquisition::whole_samples(&r.plan, &r.waves, phase).ok()
                }
                TailTarget::Completion { cell } => completion_population(r, cell, phase),
            })
        };
        let measured = sample(Phase::Measured);
        let (observed_waves, eligible_waves) =
            run.map_or((0, 0), |r| population_counts(r, &target, Phase::Measured));
        let (_, eligible_warmups) =
            run.map_or((0, 0), |r| population_counts(r, &target, Phase::Warmup));
        let mut coverage = Coverage {
            expected_waves: trials,
            observed_waves: 0,
            eligible_waves: 0,
            expected_observations: trials,
            observed_observations: 0,
            expected_warmups: warmups,
            eligible_warmups,
        };
        // Retain actual complete and observed counts even when partial membership
        // withholds the statistic; no survivor-only point estimate is emitted.
        coverage.observed_waves = observed_waves;
        coverage.eligible_waves = eligible_waves;
        coverage.observed_observations = eligible_waves;
        let values = measured.and_then(|mut values| {
            coverage.eligible_waves = values.len();
            coverage.observed_observations = values.len();
            match percentile {
                Some(p) => p
                    .nearest_rank(&mut values)
                    .map(|v| vec![Rational::from((v, 1))]),
                None => Some(values.into_iter().map(|v| Rational::from((v, 1))).collect()),
            }
        });
        (coverage, values.and_then(|v| range(&v).ok().flatten()))
    });
    let [(ac, ar), (bc, br), (rc, rr)] = observations;
    let mut gate = Gate {
        cell: match &target {
            TailTarget::Completion { cell } => cell.clone(),
            TailTarget::WholeConversation => "whole_conversation".into(),
        },
        metric: if target == TailTarget::WholeConversation {
            Metric::WholeConversationUs
        } else {
            Metric::CompletionLatencyUs
        },
        lane: None,
        target: Some(target.clone()),
        percentile,
        sample_unit: Some(if percentile.is_some() {
            "finite-empirical-nearest-rank-per-complete-acquisition-not-production-percentile-or-confidence"
        } else {
            "whole-conversation-wall-time-including-required-controls"
        }),
        max_regression_bps: thresholds.max_regression_bps,
        max_reference_spread_bps: thresholds.max_reference_spread_bps,
        coverage: Roles {
            baseline: ac,
            candidate: bc,
            reference: rc,
        },
        ranges: Roles {
            baseline: ar,
            candidate: br,
            reference: rr,
        },
        pooled_reference_range: None,
        adverse_bounds: None,
        decision: Outcome::Inconclusive,
        reason_codes: inherited.to_vec(),
    };
    for coverage in [
        &gate.coverage.baseline,
        &gate.coverage.candidate,
        &gate.coverage.reference,
    ] {
        if warmups == 0 || coverage.eligible_warmups != warmups as usize {
            add(&mut gate.reason_codes, Reason::WarmupIncomplete);
        }
        if trials == 0
            || coverage.eligible_waves != trials as usize
            || percentile.is_some_and(|p| coverage.observed_observations < p.floor())
        {
            add(&mut gate.reason_codes, Reason::MetricUnavailable);
        }
    }
    if let (Some(a), Some(b), Some(r)) = (runs[0], runs[1], runs[2]) {
        if evidence::compatible(&a.plan, &b.plan)
            && evidence::compatible(&a.plan, &r.plan)
            && (!complete_amounts_match(a, b) || !complete_amounts_match(a, r))
        {
            add(&mut gate.reason_codes, Reason::OutputAmountsMismatch);
        }
    } else {
        add(&mut gate.reason_codes, Reason::MissingReference);
    }
    if gate.reason_codes.is_empty()
        && let (Some(ar), Some(rr), Some(br)) = (ar, rr, br)
    {
        let assessed = range(&[ar[0], ar[1], rr[0], rr[1]])
            .map_err(envelope_reason)
            .and_then(|r| r.ok_or(Reason::InvalidEvidence))
            .and_then(|r| evaluate(&mut gate, r, br));
        if let Err(reason) = assessed {
            gate.decision =
                if matches!(reason, Reason::ArithmeticOverflow | Reason::InvalidEvidence) {
                    Outcome::Error
                } else {
                    Outcome::Inconclusive
                };
            add(&mut gate.reason_codes, reason);
        }
    }
    gate
}

#[cfg(test)]
#[path = "../tests/support/acquisition_policy.rs"]
mod acquisition_policy;

fn completion_wave(wave: &Wave) -> Option<u64> {
    if !wave.eligible || wave.attempts.len() != wave.spec.concurrency as usize {
        return None;
    }
    let mut worst = 0;
    for (lane, a) in wave.attempts.iter().enumerate() {
        if a.lane as usize != lane {
            return None;
        }
        worst = worst.max(latency_sample(a, Metric::CompletionLatencyUs)?.numerator);
    }
    (worst > 0).then_some(worst)
}
fn population_counts(run: &evidence::Loaded, target: &TailTarget, phase: Phase) -> (usize, usize) {
    match target {
        TailTarget::Completion { cell } => {
            let mut counts = (0, 0);
            for wave in run
                .waves
                .iter()
                .flatten()
                .filter(|w| w.spec.phase == phase && w.spec.cell == *cell)
            {
                counts.0 += 1;
                counts.1 += usize::from(completion_wave(wave).is_some());
            }
            counts
        }
        TailTarget::WholeConversation => {
            let Some(crate::acquisition::Protocol::Conversation {
                repetitions,
                warmup_repetitions,
                ..
            }) = &run.plan.workload.acquisition
            else {
                return (0, 0);
            };
            let count = if phase == Phase::Warmup {
                *warmup_repetitions
            } else {
                *repetitions
            };
            let mut counts = (0, 0);
            for index in 0..count {
                let identity = crate::acquisition::AcquisitionIdentity { phase, index };
                counts.0 += usize::from(
                    run.waves
                        .iter()
                        .flatten()
                        .any(|w| w.spec.acquisition == Some(identity)),
                );
                counts.1 += usize::from(
                    crate::acquisition::whole_sample(&run.plan, &run.waves, identity).is_ok(),
                );
            }
            counts
        }
    }
}
