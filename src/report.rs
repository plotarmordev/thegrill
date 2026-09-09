use crate::contract::*;
use crate::store::{AttemptState, Loaded};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResultKind {
    Correct,
    WrongAnswer,
    MalformedAnswer,
    Refused,
    OutputLimit,
    Timeout,
    ServiceError,
    TransportError,
    ClientLimit,
    Interrupted,
    LocalError,
    InvalidEvidence,
    OtherUngraded,
}

impl ResultKind {
    pub(crate) fn graded(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Success => Self::Correct,
            Outcome::Wrong => Self::WrongAnswer,
            Outcome::Malformed => Self::MalformedAnswer,
            Outcome::Refused => Self::Refused,
            Outcome::Unknown => Self::OtherUngraded,
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct ResultCounts {
    pub correct: u32,
    pub wrong_answer: u32,
    pub malformed_answer: u32,
    pub refused: u32,
    pub output_limit: u32,
    pub timeout: u32,
    pub service_error: u32,
    pub transport_error: u32,
    pub client_limit: u32,
    pub interrupted: u32,
    pub local_error: u32,
    pub invalid_evidence: u32,
    pub other_ungraded: u32,
}

impl ResultCounts {
    fn record(&mut self, result: ResultKind) {
        let count = match result {
            ResultKind::Correct => &mut self.correct,
            ResultKind::WrongAnswer => &mut self.wrong_answer,
            ResultKind::MalformedAnswer => &mut self.malformed_answer,
            ResultKind::Refused => &mut self.refused,
            ResultKind::OutputLimit => &mut self.output_limit,
            ResultKind::Timeout => &mut self.timeout,
            ResultKind::ServiceError => &mut self.service_error,
            ResultKind::TransportError => &mut self.transport_error,
            ResultKind::ClientLimit => &mut self.client_limit,
            ResultKind::Interrupted => &mut self.interrupted,
            ResultKind::LocalError => &mut self.local_error,
            ResultKind::InvalidEvidence => &mut self.invalid_evidence,
            ResultKind::OtherUngraded => &mut self.other_ungraded,
        };
        *count += 1;
    }
}

/// Explanation of an observation, not a new grade or evidence promotion.
pub(crate) fn attempt_result(attempt: &crate::store::LoadedAttempt) -> ResultKind {
    let terminal = match &attempt.state {
        AttemptState::Invalid(_) => return ResultKind::InvalidEvidence,
        AttemptState::Terminal(t) => t,
        _ => return ResultKind::OtherUngraded,
    };
    let stop = attempt
        .recovered_metadata
        .as_ref()
        .map(|m| m.stop)
        .or(terminal.stop);
    match attempt.outcome {
        Outcome::Success | Outcome::Wrong | Outcome::Refused => ResultKind::graded(attempt.outcome),
        Outcome::Malformed if stop == Some(Finish::Length) => ResultKind::OutputLimit,
        Outcome::Malformed => ResultKind::MalformedAnswer,
        Outcome::Unknown => match terminal.reason {
            Reason::UnsupportedResponse if stop == Some(Finish::Length) => ResultKind::OutputLimit,
            Reason::TotalDeadline | Reason::IdleDeadline => ResultKind::Timeout,
            Reason::ProviderError | Reason::HttpStatus => ResultKind::ServiceError,
            Reason::TransportFailure => ResultKind::TransportError,
            Reason::ResponseCap | Reason::ArtifactCap | Reason::FrameCap => ResultKind::ClientLimit,
            Reason::Interrupted => ResultKind::Interrupted,
            Reason::LocalIo => ResultKind::LocalError,
            _ => ResultKind::OtherUngraded,
        },
    }
}

pub(crate) struct VerifiedCase {
    pub case_id: String,
    pub input: String,
    pub outcome: Outcome,
    pub result: ResultKind,
}

/// A recomputed, receipt-matched grade view of either evidence kind.
pub(crate) struct Verified {
    pub claim: &'static str,
    pub source: String,
    pub tasks: String,
    pub target: String,
    pub protocol: String,
    pub grading: String,
    pub qualification: String,
    pub system: System,
    pub rendering_unknown: bool,
    pub cases: Vec<VerifiedCase>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Bounds {
    pub lower: i64,
    pub upper: i64,
    pub denominator: u32,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Coverage {
    pub success: u32,
    pub failure: u32,
    pub unknown: u32,
    pub delivered: u32,
    pub malformed: u32,
    pub refused: u32,
    pub bounds: Bounds,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Paired {
    pub both_success: u32,
    pub both_fail: u32,
    pub gains: u32,
    pub losses: u32,
    pub either_unknown: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct DiagnosticDelta {
    pub numerator: i64,
    pub denominator: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct Comparison<'a> {
    pub version: u32,
    pub claim: &'static str,
    pub left_evidence: &'static str,
    pub right_evidence: &'static str,
    pub effective_rendering: &'static str,
    pub left_system: &'a System,
    pub right_system: &'a System,
    pub n: u32,
    pub left: Coverage,
    pub right: Coverage,
    pub paired: Paired,
    pub delta_b_minus_a: Bounds,
    pub complete_case_delta_diagnostic: Option<DiagnosticDelta>,
    pub left_results: ResultCounts,
    pub right_results: ResultCounts,
}

impl Coverage {
    fn record(&mut self, outcome: Outcome) {
        self.bounds.denominator += 1;
        match outcome {
            Outcome::Success => self.success += 1,
            Outcome::Wrong => self.failure += 1,
            Outcome::Malformed => {
                self.failure += 1;
                self.malformed += 1;
            }
            Outcome::Refused => {
                self.failure += 1;
                self.refused += 1;
            }
            Outcome::Unknown => self.unknown += 1,
        }
    }

    fn finish(mut self) -> Self {
        self.delivered = self.bounds.denominator - self.unknown;
        self.bounds.lower = i64::from(self.success);
        self.bounds.upper = i64::from(self.success + self.unknown);
        self
    }
}

pub(crate) fn coverage(outcomes: impl ExactSizeIterator<Item = Outcome>) -> Coverage {
    let mut c = Coverage::default();
    for outcome in outcomes {
        c.record(outcome);
    }
    c.finish()
}

#[derive(Debug, Serialize)]
pub(crate) struct PairSummary {
    pub n: u32,
    pub left: Coverage,
    pub right: Coverage,
    pub paired: Paired,
    pub delta_b_minus_a: Bounds,
    pub complete_case_delta_diagnostic: Option<DiagnosticDelta>,
    pub left_results: ResultCounts,
    pub right_results: ResultCounts,
}

// One arithmetic implementation for whole-pack, family and problem-unit reports.
// All callers operate on admitted packs of at most CASE_CAP cases.
#[derive(Default)]
struct PairTally {
    left: Coverage,
    right: Coverage,
    paired: Paired,
    left_results: ResultCounts,
    right_results: ResultCounts,
}

impl PairTally {
    fn record(&mut self, left: &VerifiedCase, right: &VerifiedCase) {
        self.left.record(left.outcome);
        self.right.record(right.outcome);
        self.left_results.record(left.result);
        self.right_results.record(right.result);
        match (left.outcome, right.outcome) {
            (Outcome::Unknown, _) | (_, Outcome::Unknown) => self.paired.either_unknown += 1,
            (Outcome::Success, Outcome::Success) => self.paired.both_success += 1,
            (Outcome::Success, _) => self.paired.losses += 1,
            (_, Outcome::Success) => self.paired.gains += 1,
            _ => self.paired.both_fail += 1,
        }
    }

    fn finish(self) -> PairSummary {
        let left = self.left.finish();
        let right = self.right.finish();
        let n = left.bounds.denominator;
        let joint = n - self.paired.either_unknown;
        PairSummary {
            n,
            delta_b_minus_a: Bounds {
                lower: right.bounds.lower - left.bounds.upper,
                upper: right.bounds.upper - left.bounds.lower,
                denominator: n,
            },
            complete_case_delta_diagnostic: (joint != 0).then_some(DiagnosticDelta {
                numerator: i64::from(self.paired.gains) - i64::from(self.paired.losses),
                denominator: joint,
            }),
            left,
            right,
            paired: self.paired,
            left_results: self.left_results,
            right_results: self.right_results,
        }
    }
}

pub(crate) fn compare<'a>(a: &'a Verified, b: &'a Verified) -> Result<Comparison<'a>> {
    for (label, left, right) in [
        ("tasks", &a.tasks, &b.tasks),
        ("target", &a.target, &b.target),
        ("protocol", &a.protocol, &b.protocol),
        ("grading", &a.grading, &b.grading),
        ("qualification", &a.qualification, &b.qualification),
    ] {
        if left != right {
            return Err(format!("incompatible {label} identity"));
        }
    }
    if a.cases.len() != b.cases.len() {
        return Err("incompatible case count".into());
    }
    let mut tally = PairTally::default();
    for (a, b) in a.cases.iter().zip(&b.cases) {
        if a.case_id != b.case_id || a.input != b.input {
            return Err("incompatible ordered case input".into());
        }
        tally.record(a, b);
    }
    let summary = tally.finish();
    Ok(Comparison {
        version: 2,
        claim: if a.claim == b.claim && a.claim.starts_with("submitted") {
            "descriptive-finite-pack-submitted-artifacts-not-verified-execution"
        } else {
            "descriptive-finite-pack-client-collected-or-submitted-artifacts-not-authenticated-execution"
        },
        left_evidence: a.claim,
        right_evidence: b.claim,
        effective_rendering: if a.rendering_unknown || b.rendering_unknown {
            "unknown: declared-submission/service comparison only; not matched effective prompts or weights-only effects"
        } else {
            "matching supplied rendering declarations only; not independently verified prompts or weights"
        },
        left_system: &a.system,
        right_system: &b.system,
        n: summary.n,
        left: summary.left,
        right: summary.right,
        paired: summary.paired,
        delta_b_minus_a: summary.delta_b_minus_a,
        complete_case_delta_diagnostic: summary.complete_case_delta_diagnostic,
        left_results: summary.left_results,
        right_results: summary.right_results,
    })
}

#[derive(Serialize)]
pub(crate) struct StudyCase<'a> {
    pub case_id: &'a str,
    pub left: Outcome,
    pub right: Outcome,
    pub left_result: ResultKind,
    pub right_result: ResultKind,
}

#[derive(Serialize)]
pub(crate) struct UnitAnalysis<'a> {
    pub world: &'a str,
    pub summary: PairSummary,
    pub cases: Vec<StudyCase<'a>>,
}

#[derive(Serialize)]
pub(crate) struct FamilyAnalysis<'a> {
    pub group: &'a str,
    pub summary: PairSummary,
    pub units: Vec<UnitAnalysis<'a>>,
}

#[derive(Serialize)]
pub(crate) struct StudyAnalysis<'a> {
    pub version: u32,
    pub claim: &'static str,
    pub study: &'a crate::study::Study,
    pub comparison: Comparison<'a>,
    pub families: Vec<FamilyAnalysis<'a>>,
}

pub(crate) fn study_analysis<'a>(
    study: &'a crate::study::Study,
    pack: &'a Pack,
    left: &'a Verified,
    right: &'a Verified,
) -> Result<StudyAnalysis<'a>> {
    crate::study::bind(study, left, right)?;
    let comparison = compare(left, right)?;
    // Admission binds group/world membership to exact pack bytes. Vector ordering
    // follows the pack, never randomized hash-map traversal or sorted labels.
    let group_indices: std::collections::HashMap<_, _> = pack
        .groups
        .iter()
        .enumerate()
        .map(|(i, group)| (group.as_str(), i))
        .collect();
    let world_indices: std::collections::HashMap<_, _> = pack
        .worlds
        .iter()
        .enumerate()
        .map(|(i, world)| (world.as_str(), i))
        .collect();
    let mut family_tallies: Vec<_> = pack.groups.iter().map(|_| PairTally::default()).collect();
    let mut unit_tallies: Vec<_> = pack.worlds.iter().map(|_| PairTally::default()).collect();
    let mut unit_cases: Vec<Vec<StudyCase<'_>>> = pack.worlds.iter().map(|_| Vec::new()).collect();
    let mut unit_groups = vec![0; pack.worlds.len()];
    for ((case, a), b) in pack.cases.iter().zip(&left.cases).zip(&right.cases) {
        let group = group_indices[case.group.as_str()];
        let world = world_indices[case.world.as_str()];
        family_tallies[group].record(a, b);
        unit_tallies[world].record(a, b);
        unit_groups[world] = group;
        unit_cases[world].push(StudyCase {
            case_id: &case.id,
            left: a.outcome,
            right: b.outcome,
            left_result: a.result,
            right_result: b.result,
        });
    }
    let mut families: Vec<_> = pack
        .groups
        .iter()
        .zip(family_tallies)
        .map(|(group, tally)| FamilyAnalysis {
            group,
            summary: tally.finish(),
            units: Vec::new(),
        })
        .collect();
    for (((world, tally), cases), group) in pack
        .worlds
        .iter()
        .zip(unit_tallies)
        .zip(unit_cases)
        .zip(unit_groups)
    {
        families[group].units.push(UnitAnalysis {
            world,
            summary: tally.finish(),
            cases,
        });
    }
    Ok(StudyAnalysis {
        version: 2,
        claim: "descriptive-finite-pack-study; provenance-exposure-and-independent-units-are-declarations; no-confidence-intervals-or-causal-verdict",
        study,
        comparison,
        families,
    })
}

// ----- Read-only run inspection -----

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InspectedState {
    NotStarted,
    Unresolved,
    Collected,
    Refused,
    Uncommitted,
    Invalid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MetadataSource {
    Terminal,
    VerifiedRawBody,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub(crate) struct InspectedAttempt<'a> {
    pub attempt: u32,
    pub case_id: &'a str,
    pub state: InspectedState,
    pub reason: Option<Reason>,
    pub detail: Option<&'a str>,
    pub stop: Option<Finish>,
    pub dispatched: Option<bool>,
    pub http_status: Option<u16>,
    pub body_bytes: Option<u64>,
    pub body_complete: Option<bool>,
    pub settle_ms: Option<u64>,
    pub usage: Option<&'a Usage>,
    pub usage_status: &'static str,
    pub metadata_source: MetadataSource,
    pub result: ResultKind,
    pub outcome: Outcome,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct InspectedCounts {
    pub not_started: u32,
    pub unresolved: u32,
    pub collected: u32,
    pub refused: u32,
    pub uncommitted: u32,
    pub invalid: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ViewStatus {
    Verified,
    Absent,
    Mismatch,
    Historical,
    Ungradeable,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct UsageFieldCoverage {
    pub present: u32,
    pub unknown: u32,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct UsageCoverage {
    pub snapshots: u32,
    pub prompt_tokens: UsageFieldCoverage,
    pub completion_tokens: UsageFieldCoverage,
    pub total_tokens: UsageFieldCoverage,
}

impl UsageFieldCoverage {
    fn observe(&mut self, value: Option<u64>) {
        if value.is_some() {
            self.present += 1;
        } else {
            self.unknown += 1;
        }
    }
}

impl UsageCoverage {
    fn observe(&mut self, usage: Option<&Usage>) {
        self.snapshots += u32::from(usage.is_some());
        self.prompt_tokens
            .observe(usage.and_then(|u| u.prompt_tokens));
        self.completion_tokens
            .observe(usage.and_then(|u| u.completion_tokens));
        self.total_tokens
            .observe(usage.and_then(|u| u.total_tokens));
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Inspection<'a> {
    pub version: u32,
    pub kind: &'static str,
    pub claim: &'static str,
    pub label: &'a str,
    pub cases: u32,
    pub system: &'a System,
    pub stream: bool,
    pub profile: &'a Profile,
    pub reasoning_effort: Option<Effort>,
    pub include_usage: Option<bool>,
    pub token_cap: &'a TokenCap,
    pub collection: &'a Collection,
    pub auth_env: Option<&'a str>,
    pub started_unix_ms: u64,
    pub counts: InspectedCounts,
    pub attempts: Vec<InspectedAttempt<'a>>,
    pub view: ViewStatus,
    pub coverage: Option<Coverage>,
    pub usage_status: &'static str,
    pub usage_semantics: &'static str,
    pub usage_coverage: UsageCoverage,
    pub result_counts: ResultCounts,
    pub lifecycle: Option<crate::lifecycle::Status>,
}

pub(crate) fn inspect<'a>(loaded: &'a Loaded, view: Result<Option<RunView>>) -> Inspection<'a> {
    let mut counts = InspectedCounts::default();
    let mut usage_coverage = UsageCoverage::default();
    let mut result_counts = ResultCounts::default();
    let attempts = loaded
        .pack
        .cases
        .iter()
        .zip(&loaded.attempts)
        .map(|(case, a)| {
            let (state, terminal) = match &a.state {
                AttemptState::NotStarted => (InspectedState::NotStarted, None),
                AttemptState::Unresolved => (InspectedState::Unresolved, None),
                AttemptState::Invalid(_) => (InspectedState::Invalid, None),
                AttemptState::Terminal(t) => (
                    match t.reason {
                        Reason::Committed => InspectedState::Collected,
                        Reason::Refused => InspectedState::Refused,
                        _ => InspectedState::Uncommitted,
                    },
                    Some(t),
                ),
            };
            match state {
                InspectedState::NotStarted => counts.not_started += 1,
                InspectedState::Unresolved => counts.unresolved += 1,
                InspectedState::Collected => counts.collected += 1,
                InspectedState::Refused => counts.refused += 1,
                InspectedState::Uncommitted => counts.uncommitted += 1,
                InspectedState::Invalid => counts.invalid += 1,
            }
            let recovered = terminal.and(a.recovered_metadata.as_ref());
            let stop = recovered
                .map(|m| m.stop)
                .or_else(|| terminal.and_then(|t| t.stop));
            let usage = match recovered {
                Some(metadata) => metadata.usage.as_ref(),
                None => terminal.and_then(|t| t.usage.as_ref()),
            };
            let result = attempt_result(a);
            result_counts.record(result);
            usage_coverage.observe(usage);
            InspectedAttempt {
                attempt: a.attempt,
                case_id: &case.id,
                state,
                reason: terminal.map(|t| t.reason),
                detail: match &a.state {
                    AttemptState::Invalid(what) => Some(*what),
                    AttemptState::Terminal(t) => Some(t.detail.as_str()),
                    _ => None,
                },
                stop,
                dispatched: terminal.map(|t| t.dispatched),
                http_status: terminal.and_then(|t| t.http.as_ref().map(|h| h.status)),
                body_bytes: terminal.map(|t| t.body.bytes),
                body_complete: terminal.map(|t| t.body.complete),
                settle_ms: terminal.map(|t| t.timing.settle_ms),
                usage,
                metadata_source: if recovered.is_some() {
                    MetadataSource::VerifiedRawBody
                } else if stop.is_some() || usage.is_some() {
                    MetadataSource::Terminal
                } else {
                    MetadataSource::Unavailable
                },
                result,
                usage_status: if usage.is_some() {
                    "unvalidated-provider-snapshot"
                } else {
                    "unknown"
                },
                outcome: a.outcome,
            }
        })
        .collect();
    let (view, coverage) = match view {
        Ok(Some(v)) => (
            ViewStatus::Verified,
            Some(self::coverage(v.cases.iter().map(|c| c.outcome))),
        ),
        Ok(None) => (ViewStatus::Absent, None),
        Err(_) if counts.invalid > 0 => (ViewStatus::Ungradeable, None),
        Err(_) => (ViewStatus::Mismatch, None),
    };
    Inspection {
        version: 5,
        kind: match loaded.kind {
            crate::store::Kind::Run => "run",
            _ => "run-grade-view",
        },
        claim: "client-collected-artifacts-not-authenticated-execution",
        label: &loaded.pack.label,
        cases: loaded.plan.cases,
        system: &loaded.plan.system,
        stream: loaded.plan.protocol.stream,
        profile: &loaded.plan.protocol.profile,
        reasoning_effort: loaded.plan.protocol.reasoning_effort,
        include_usage: loaded.plan.protocol.include_usage,
        token_cap: &loaded.plan.protocol.token_cap,
        collection: &loaded.plan.protocol.collection,
        auth_env: loaded.plan.transport.auth_env.as_deref(),
        started_unix_ms: loaded.plan.provenance.started_unix_ms,
        counts,
        attempts,
        view,
        coverage,
        usage_status: "unvalidated-provider-snapshot",
        usage_semantics: "last-provider-snapshot-not-complete-or-validated-billing; verified-raw-recovery-for-complete-v1-absent-content; field-coverage-denominator-is-all-cases",
        usage_coverage,
        result_counts,
        lifecycle: None,
    }
}

// ASCII output only for untrusted text. escape_default covers ANSI/OSC, C0/C1,
// bidi controls and non-ASCII; never emit attacker-provided terminal control bytes.
pub(crate) fn escape(text: &str) -> String {
    text.chars().flat_map(char::escape_default).collect()
}

pub(crate) fn error_preview(text: &str) -> String {
    const CAP: usize = 1024;
    const TRUNCATED: &str = "...[truncated]";
    let mut output = String::with_capacity(text.len().min(CAP));
    for c in text.chars() {
        let escaped = c.escape_default();
        if output.len() + escaped.len() > CAP - TRUNCATED.len() {
            output.push_str(TRUNCATED);
            break;
        }
        output.extend(escaped);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pack, store};

    fn fixture(outcomes: &[Outcome]) -> Verified {
        let pb = include_bytes!("../examples/synthetic-pack.json");
        let p = pack::admit(pb).unwrap();
        let sb = include_bytes!("../examples/submission-a.json");
        let s = pack::submission(sb, &p).unwrap();
        let view = store::compute(&p, pb, &s, sb).unwrap();
        assert_eq!(view.cases.len(), outcomes.len());
        let i = view.identities;
        Verified {
            claim: "submitted-artifacts-not-verified-execution",
            source: i.pack.source,
            tasks: i.pack.tasks,
            target: i.pack.target,
            protocol: i.protocol,
            grading: i.pack.grading,
            qualification: i.pack.qualification,
            system: s.system,
            rendering_unknown: true,
            cases: view
                .cases
                .into_iter()
                .zip(outcomes)
                .map(|(c, o)| VerifiedCase {
                    case_id: c.case_id,
                    input: c.input,
                    outcome: *o,
                    result: ResultKind::graded(*o),
                })
                .collect(),
        }
    }

    #[test]
    fn fixed_denominator_and_stochastic_same_system() {
        use Outcome::*;
        let a = fixture(&[Success, Wrong, Unknown, Malformed]);
        let b = fixture(&[Wrong, Success, Success, Unknown]);
        let c = compare(&a, &b).unwrap();
        assert_eq!(
            (c.delta_b_minus_a.lower, c.delta_b_minus_a.upper, c.n),
            (0, 2, 4)
        );
        assert_eq!(
            (c.paired.gains, c.paired.losses, c.paired.either_unknown),
            (1, 1, 2)
        );
        let unknown = fixture(&[Unknown; 4]);
        let c = compare(&unknown, &unknown).unwrap();
        assert_eq!((c.delta_b_minus_a.lower, c.delta_b_minus_a.upper), (-4, 4));
        assert!(c.complete_case_delta_diagnostic.is_none());
        let fail = fixture(&[Wrong; 4]);
        let success = fixture(&[Success; 4]);
        let c = compare(&fail, &success).unwrap();
        assert_eq!((c.delta_b_minus_a.lower, c.delta_b_minus_a.upper), (4, 4));
        let c = compare(&success, &success).unwrap();
        assert_eq!((c.delta_b_minus_a.lower, c.delta_b_minus_a.upper), (0, 0));
        let refused = fixture(&[Refused, Refused, Success, Unknown]);
        let c = compare(&success, &refused).unwrap();
        assert_eq!(
            (c.right.failure, c.right.refused, c.right.delivered),
            (2, 2, 3)
        );
        assert_eq!((c.delta_b_minus_a.lower, c.delta_b_minus_a.upper), (-3, -2));
    }
}
