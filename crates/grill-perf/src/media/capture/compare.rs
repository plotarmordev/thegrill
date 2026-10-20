use super::*;
use crate::{
    envelope::{self, Direction, EnvelopeDecision, EnvelopeReason, Rational},
    evidence::{ReferenceIdentity, ReferenceIdentityStatus},
    policy::Outcome,
};

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Axis {
    SameCheckpoint,
    ChangedModel,
    Unavailable,
}

#[derive(Serialize)]
pub(in crate::media) struct Comparison {
    version: u32,
    kind: &'static str,
    claim: &'static str,
    support: &'static str,
    axis: Axis,
    workload_sha256: String,
    reference_identity: ReferenceIdentity,
    acquisitions: Vec<Inspection>,
    gates: Vec<GateResult>,
    reasons: Vec<&'static str>,
    pub(in crate::media) outcome: Outcome,
}

#[derive(Serialize)]
struct ArmSummary {
    #[serde(flatten)]
    samples: CellSummary,
    seconds_per_image: Option<f64>,
    images_per_second_end_to_end: Option<f64>,
}

#[derive(Serialize)]
struct HashEquality {
    seed: u64,
    equal: Option<bool>,
}

#[derive(Serialize)]
struct GateResult {
    gate: Gate,
    outcome: Outcome,
    reason: Option<String>,
    reference: Option<[Rational; 2]>,
    candidate: Option<[Rational; 2]>,
    adverse_bounds: Option<[f64; 2]>,
    reference_summary: ArmSummary,
    candidate_summary: ArmSummary,
    cross_arm_hash_equality: Vec<HashEquality>,
}

fn same_runner(a: &Loaded, b: &Loaded) -> bool {
    a.plan.runner_bytes == b.plan.runner_bytes
        && a.receipt.program.declared_path == b.receipt.program.declared_path
        && a.receipt.program.resolved_sha256 == b.receipt.program.resolved_sha256
        && a.receipt
            .setup_program
            .as_ref()
            .map(|program| (&program.declared_path, &program.resolved_sha256))
            == b.receipt
                .setup_program
                .as_ref()
                .map(|program| (&program.declared_path, &program.resolved_sha256))
        && a.deployment == b.deployment
}

fn admit(captures: &[Loaded], arms: &[Arm]) -> Result<()> {
    let a = &captures[0];
    for (capture, arm) in captures.iter().zip(arms) {
        if capture.inspection.status != CaptureStatus::Complete {
            return Err("comparison requires complete captures".into());
        }
        if &capture.receipt.slot.arm != arm {
            return Err("capture slot arm does not match its comparison position".into());
        }
        if capture.plan.workload_bytes != a.plan.workload_bytes {
            return Err("captures must share byte-identical workloads".into());
        }
        if capture.receipt.collector.sha256 != a.receipt.collector.sha256
            || capture.receipt.platform != a.receipt.platform
        {
            return Err("captures must use the same collector binary and platform".into());
        }
    }
    if a.plan.workload.slots.len() != captures.len() {
        return Err("every declared slot must be supplied, including b2".into());
    }
    if !same_runner(a, captures.last().expect("reference return")) {
        return Err("A and A2 runner bytes, program or setup hash, or deployment differ".into());
    }
    if same_runner(a, &captures[1]) {
        return Err("B must differ from A in runner, program, setup or deployment".into());
    }
    if captures.len() == 4 && !same_runner(&captures[1], &captures[2]) {
        return Err("B2 must match B runner, program, setup and deployment".into());
    }
    for pair in captures.windows(2) {
        if pair[0].finished_unix_ms > pair[1].receipt.started_unix_ms {
            return Err(format!(
                "chronology requires {} to finish no later than {} starts",
                pair[0].receipt.slot.id, pair[1].receipt.slot.id
            ));
        }
    }
    Ok(())
}

fn arm_summary(cell: &Cell, workload: &Workload, runs: &[&Run]) -> ArmSummary {
    let samples = summarize(cell, runs.iter().copied());
    let images = f64::from(
        cell.request
            .as_ref()
            .unwrap_or(&workload.request)
            .images_per_run,
    );
    let mean = samples.end_to_end_us.as_ref().map(|summary| summary.mean);
    ArmSummary {
        seconds_per_image: mean.map(|us| us / images / 1_000_000.0),
        images_per_second_end_to_end: mean
            .filter(|us| *us > 0.0)
            .map(|us| images * 1_000_000.0 / us),
        samples,
    }
}

fn hash_equality(reference: &[&Run], candidate: &[&Run]) -> Vec<HashEquality> {
    let a = hashes_by_seed(reference.iter().copied());
    let b = hashes_by_seed(candidate.iter().copied());
    a.keys()
        .chain(b.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|seed| HashEquality {
            seed,
            equal: a
                .get(&seed)
                .zip(b.get(&seed))
                .filter(|(a, b)| identical_hashes(a) && identical_hashes(b))
                .map(|(a, b)| a[0] == b[0]),
        })
        .collect()
}

fn envelope_failure(result: &mut GateResult, reason: EnvelopeReason) {
    result.outcome = reason.into();
    result.adverse_bounds = match reason {
        EnvelopeReason::EnvelopeStraddlesTolerance { adverse_bounds } => Some(adverse_bounds),
        EnvelopeReason::ArithmeticOverflow { adverse_bounds } => adverse_bounds,
        _ => None,
    };
    result.reason = Some(format!("{reason:?}"));
}

fn gate_result(gate: &Gate, captures: &[Loaded]) -> GateResult {
    let workload = &captures[0].plan.workload;
    let cell = workload
        .cells
        .iter()
        .find(|cell| cell.id == gate.cell)
        .expect("validated gate cell");
    let mut arms: [Vec<&Run>; 2] = [Vec::new(), Vec::new()];
    for capture in captures {
        let candidate = matches!(capture.receipt.slot.arm, Arm::B | Arm::B2);
        arms[usize::from(candidate)].extend(
            capture
                .inspection
                .runs
                .iter()
                .filter(|run| run.cell == gate.cell),
        );
    }
    let values = arms.each_ref().map(|runs| {
        runs.iter()
            .filter(|run| run.phase == Phase::Measured && run.eligible)
            .map(|run| Rational {
                numerator: run.process.end_to_end_us,
                denominator: 1,
            })
            .collect::<Vec<_>>()
    });
    let mut result = GateResult {
        gate: gate.clone(),
        outcome: Outcome::Inconclusive,
        reason: None,
        reference: None,
        candidate: None,
        adverse_bounds: None,
        reference_summary: arm_summary(cell, workload, &arms[0]),
        candidate_summary: arm_summary(cell, workload, &arms[1]),
        cross_arm_hash_equality: hash_equality(&arms[0], &arms[1]),
    };
    match (envelope::range(&values[0]), envelope::range(&values[1])) {
        (Ok(reference), Ok(candidate)) => {
            result.reference = reference;
            result.candidate = candidate;
        }
        (Err(reason), _) | (_, Err(reason)) => {
            envelope_failure(&mut result, reason);
            return result;
        }
    }
    if values.iter().any(|values| values.len() < 3)
        || arms
            .iter()
            .flatten()
            .any(|run| run.phase == Phase::Measured && !run.eligible)
    {
        result.reason = Some("insufficient_samples".into());
        return result;
    }
    let reference = result.reference.expect("at least three reference samples");
    let candidate = result.candidate.expect("at least three candidate samples");
    match envelope::assess(
        reference,
        candidate,
        gate.max_adverse_bps,
        gate.max_reference_spread_bps,
        Direction::LowerBetter,
    ) {
        Ok(assessment) => {
            result.outcome = match assessment.decision {
                EnvelopeDecision::Pass => Outcome::Pass,
                EnvelopeDecision::Regression => Outcome::Regression,
            };
            result.adverse_bounds = Some(assessment.adverse_bounds);
        }
        Err(reason) => envelope_failure(&mut result, reason),
    }
    result
}

fn matching_pins(a: &Runner, b: &Runner) -> bool {
    if a.pins.is_empty() || b.pins.is_empty() {
        return true;
    }
    fn pins(runner: &Runner) -> BTreeSet<&str> {
        runner.pins.iter().map(|pin| pin.sha256.as_str()).collect()
    }
    pins(a) == pins(b)
}

pub(in crate::media) fn compare(
    a: &Path,
    b: &Path,
    a2: &Path,
    b2: Option<&Path>,
) -> Result<Comparison> {
    let mut captures = vec![load(a)?, load(b)?];
    let mut arms = vec![Arm::A, Arm::B];
    if let Some(b2) = b2 {
        captures.push(load(b2)?);
        arms.push(Arm::B2);
    }
    captures.push(load(a2)?);
    arms.push(Arm::A2);
    admit(&captures, &arms)?;
    let reference_identity = evidence::deployment_identity(
        &captures[0].deployment,
        &captures.last().expect("reference return").deployment,
    );
    let axis = match (
        &captures[0].deployment.model_revision,
        &captures[1].deployment.model_revision,
    ) {
        (Some(a), Some(b))
            if a == b && matching_pins(&captures[0].plan.runner, &captures[1].plan.runner) =>
        {
            Axis::SameCheckpoint
        }
        (Some(_), Some(_)) => Axis::ChangedModel,
        _ => Axis::Unavailable,
    };
    let gates: Vec<_> = captures[0]
        .plan
        .workload
        .gates
        .iter()
        .map(|gate| gate_result(gate, &captures))
        .collect();
    let mut outcome = gates
        .iter()
        .fold(Outcome::Pass, |outcome, gate| outcome.max(gate.outcome));
    let mut reasons = Vec::new();
    if axis == Axis::ChangedModel {
        reasons.push("changed_model_axis");
    } else if axis == Axis::Unavailable {
        reasons.push("model_identity_unavailable");
    }
    if reference_identity.status == ReferenceIdentityStatus::Unavailable {
        reasons.push("reference_identity_unavailable");
    }
    if !reasons.is_empty() && matches!(outcome, Outcome::Pass | Outcome::Regression) {
        outcome = Outcome::Inconclusive;
    }
    Ok(Comparison {
        version: 1,
        kind: "media-comparison-v1",
        claim: "observed-envelope-direction-under-declared-bounds-not-output-quality",
        support: SUPPORT,
        axis,
        workload_sha256: captures[0].receipt.workload_sha256.clone(),
        reference_identity,
        acquisitions: captures
            .into_iter()
            .map(|capture| capture.inspection)
            .collect(),
        gates,
        reasons,
        outcome,
    })
}
