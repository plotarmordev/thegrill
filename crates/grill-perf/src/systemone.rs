//! Finite System One decision-protocol captures.
//!
//! A System One request answers declared questions over one state, with optional
//! images, in a single forward pass and reports zero generated tokens. This is a
//! separate versioned domain path beside the chat-completions collector: it
//! retains per-request latency and per-burst request/decision rates, never
//! generated-token throughput. A response that reports generated tokens, omits a
//! declared answer, or returns a probability outside `[0, 1]` is a retained
//! failure, not a measurement.

use crate::{
    envelope::{self, Direction, EnvelopeDecision, EnvelopeReason, Rational},
    evidence,
    model::{Deployment, Phase, REQUEST_CAP, Result, Status},
    policy::Outcome,
    wire,
};
use clap::{Args, Subcommand};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::Semaphore, task::JoinSet};

pub const PROTOCOL: &str = "systemone-v1";
pub const PATH: &str = "/v1/systemone";
pub const PLAN_KIND: &str = "systemone-plan-v1";
pub const CAPTURE_KIND: &str = "systemone-capture-v1";
pub const COMPARISON_KIND: &str = "systemone-comparison-v1";
/// The package version is a released number, but this protocol path is not part
/// of the released support; every receipt states that distinction.
pub const SUPPORT: &str = "local-unreleased-extension-not-released-v0.6-support";
pub const CLAIM_CAPTURE: &str =
    "client-observed-system-one-decision-latency-and-burst-rates-not-attested-execution";
pub const CLAIM_COMPARISON: &str =
    "observed-envelope-direction-under-declared-bounds-not-a-certified-serving-change";

const CAP: usize = 4 * 1024 * 1024;
const CORPUS_CAP: usize = 256 * 1024 * 1024;
const INSTRUCTION_CAP: usize = 16 * 1024;
const IMAGE_CAP: usize = 4 * 1024 * 1024;
const CRITERIA_CAP: usize = 64;
const MAX_REQUESTS: usize = 256;
const MAX_QUESTIONS: usize = 64;
const MAX_IMAGES: usize = 16;
const MAX_CELLS: usize = 64;
const MAX_SLOTS: usize = 8;
const MAX_GATES: usize = 16;
const MAX_WAVES: u64 = 1024;
const MAX_ATTEMPTS: u64 = 10_000;
const MAX_CONCURRENCY: u32 = 64;
const MIN_ARM_SAMPLES: usize = 3;
const BURST_CAP: u32 = 1000;
const WARMUP_CAP: u32 = 20;

// -------------------------------------------------------------------------
// Declared plan
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub kind: String,
    pub name: String,
    pub protocol: String,
    pub requests: Vec<RequestDecl>,
    pub cells: Vec<Cell>,
    pub limits: Limits,
    pub slots: Vec<Slot>,
    pub gates: Vec<Gate>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RequestDecl {
    pub id: String,
    #[serde(default)]
    pub state: Option<serde_json::Value>,
    pub questions: BTreeMap<String, Question>,
    #[serde(default)]
    pub images: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QuestionKind {
    Noul,
    Choice,
    Score,
}

impl QuestionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Question {
    #[serde(rename = "type")]
    pub kind: QuestionKind,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub request_timeout_ms: u32,
    pub burst_timeout_ms: u32,
    pub response_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub id: String,
    pub requests: Vec<String>,
    pub concurrency: u32,
    pub warmup_bursts: u32,
    pub measured_bursts: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    A,
    B,
    A2,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub id: String,
    pub arm: Arm,
    pub warmup: bool,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    LatencyUs,
    RequestsPerSecond,
    DecisionsPerSecond,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub cell: String,
    pub metric: Metric,
    pub max_adverse_bps: u32,
    pub max_reference_spread_bps: u32,
}

fn arm_index(arm: Arm) -> usize {
    match arm {
        Arm::A => 0,
        Arm::B => 1,
        Arm::A2 => 2,
    }
}

fn arm_name(arm: Arm) -> &'static str {
    match arm {
        Arm::A => "a",
        Arm::B => "b",
        Arm::A2 => "a2",
    }
}

fn planned_bursts(plan: &Plan) -> u32 {
    plan.cells
        .iter()
        .map(|cell| cell.warmup_bursts + cell.measured_bursts)
        .sum()
}

impl Question {
    fn validate(&self, source: &str, id: &str) -> Result<()> {
        if self.instructions.is_empty()
            || self.instructions.len() > INSTRUCTION_CAP
            || self.instructions.chars().any(char::is_control)
        {
            return Err(format!(
                "request {source} question {id}: instructions must be nonempty control-free text within {INSTRUCTION_CAP} bytes"
            ));
        }
        let present = match (self.kind, &self.criteria) {
            (QuestionKind::Noul, None) => true,
            (QuestionKind::Noul, Some(value)) => value.as_object().is_some_and(|object| {
                object.len() == 2 && object.values().all(serde_json::Value::is_string)
            }),
            (QuestionKind::Choice, Some(value)) => value.as_object().is_some_and(|object| {
                !object.is_empty()
                    && object.len() <= CRITERIA_CAP
                    && object.iter().all(|(name, description)| {
                        !name.is_empty()
                            && name.len() <= 128
                            && !name.chars().any(char::is_control)
                            && (description.is_string() || description.is_null())
                    })
            }),
            (QuestionKind::Score, Some(value)) => value.as_array().is_some_and(|list| {
                (2..=10).contains(&list.len()) && list.iter().all(|entry| entry.is_string())
            }),
            (QuestionKind::Choice | QuestionKind::Score, None) => false,
        };
        if !present {
            return Err(format!(
                "request {source} question {id}: {} criteria do not match the declared type",
                self.kind.as_str()
            ));
        }
        Ok(())
    }
}

fn render_body(request: &RequestDecl) -> Result<String> {
    let mut questions = serde_json::Map::new();
    for (id, question) in &request.questions {
        let mut object = serde_json::Map::new();
        object.insert(
            "type".into(),
            serde_json::Value::String(question.kind.as_str().into()),
        );
        object.insert(
            "instructions".into(),
            serde_json::Value::String(question.instructions.clone()),
        );
        if let Some(criteria) = &question.criteria {
            object.insert("criteria".into(), criteria.clone());
        }
        questions.insert(id.clone(), serde_json::Value::Object(object));
    }
    let mut body = serde_json::Map::new();
    body.insert(
        "state".into(),
        request.state.clone().unwrap_or(serde_json::Value::Null),
    );
    body.insert("questions".into(), serde_json::Value::Object(questions));
    if !request.images.is_empty() {
        body.insert(
            "images".into(),
            serde_json::Value::Array(
                request
                    .images
                    .iter()
                    .map(|image| serde_json::Value::String(image.clone()))
                    .collect(),
            ),
        );
    }
    let body = serde_json::to_string(&serde_json::Value::Object(body))
        .map_err(|e| format!("render request {}: {e}", request.id))?;
    if body.len() > REQUEST_CAP {
        return Err(format!(
            "request {} renders {} bytes; the encoded request bound is {REQUEST_CAP}",
            request.id,
            body.len()
        ));
    }
    Ok(body)
}

impl Plan {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.kind != PLAN_KIND {
            return Err(format!(
                "expected version 1 and kind {PLAN_KIND}, found version {} and kind {}",
                self.version, self.kind
            ));
        }
        if self.protocol != PROTOCOL {
            return Err(format!("plan protocol must be {PROTOCOL}"));
        }
        if !crate::model::identifier(&self.name) {
            return Err("plan name must be a short ASCII identifier".into());
        }
        self.validate_corpus()?;
        let cells = self.validate_cells()?;
        self.validate_limits()?;
        self.validate_slots()?;
        self.validate_gates(&cells)
    }

    fn validate_corpus(&self) -> Result<()> {
        if self.requests.is_empty() || self.requests.len() > MAX_REQUESTS {
            return Err(format!("plan needs 1..={MAX_REQUESTS} declared requests"));
        }
        let mut ids = BTreeSet::new();
        let mut corpus_bytes = 0usize;
        for request in &self.requests {
            if !crate::model::identifier(&request.id) || !ids.insert(request.id.as_str()) {
                return Err("request IDs must be unique short ASCII identifiers".into());
            }
            if request.questions.is_empty() || request.questions.len() > MAX_QUESTIONS {
                return Err(format!(
                    "request {} needs 1..={MAX_QUESTIONS} questions",
                    request.id
                ));
            }
            if request.state.is_none() && request.images.is_empty() {
                return Err(format!(
                    "request {} needs a state or at least one image",
                    request.id
                ));
            }
            if request.images.len() > MAX_IMAGES {
                return Err(format!(
                    "request {} declares more than {MAX_IMAGES} images",
                    request.id
                ));
            }
            for image in &request.images {
                if !image.starts_with("data:") || image.len() > IMAGE_CAP {
                    return Err(format!(
                        "request {} images must be data URLs within {IMAGE_CAP} bytes",
                        request.id
                    ));
                }
            }
            for (id, question) in &request.questions {
                if !crate::model::identifier(id) {
                    return Err(format!(
                        "request {} question IDs must be short ASCII identifiers",
                        request.id
                    ));
                }
                question.validate(&request.id, id)?;
            }
            corpus_bytes += render_body(request)?.len();
        }
        if corpus_bytes > CORPUS_CAP {
            return Err(format!(
                "declared corpus renders {corpus_bytes} bytes; the bound is {CORPUS_CAP}"
            ));
        }
        Ok(())
    }

    fn validate_cells(&self) -> Result<BTreeSet<String>> {
        if self.cells.is_empty() || self.cells.len() > MAX_CELLS {
            return Err(format!("plan needs 1..={MAX_CELLS} cells"));
        }
        let corpus: BTreeSet<&str> = self.requests.iter().map(|r| r.id.as_str()).collect();
        let mut ids = BTreeSet::new();
        let mut waves = 0u64;
        let mut attempts = 0u64;
        for cell in &self.cells {
            if !crate::model::identifier(&cell.id) || !ids.insert(cell.id.clone()) {
                return Err("cell IDs must be unique short ASCII identifiers".into());
            }
            if cell.requests.is_empty() || cell.requests.len() > MAX_REQUESTS {
                return Err(format!(
                    "cell {} needs 1..={MAX_REQUESTS} requests",
                    cell.id
                ));
            }
            let mut members = BTreeSet::new();
            for id in &cell.requests {
                if !members.insert(id.as_str()) {
                    return Err(format!("cell {} repeats a request", cell.id));
                }
                if !corpus.contains(id.as_str()) {
                    return Err(format!("cell {} references an undeclared request", cell.id));
                }
            }
            if !(1..=MAX_CONCURRENCY).contains(&cell.concurrency)
                || cell.concurrency as usize > cell.requests.len()
            {
                return Err(format!(
                    "cell {} concurrency must be 1..={MAX_CONCURRENCY} and no larger than its burst",
                    cell.id
                ));
            }
            if cell.warmup_bursts > WARMUP_CAP
                || cell.measured_bursts == 0
                || cell.measured_bursts > BURST_CAP
            {
                return Err(format!(
                    "cell {} needs 0..={WARMUP_CAP} warmup and 1..={BURST_CAP} measured bursts",
                    cell.id
                ));
            }
            let bursts = u64::from(cell.warmup_bursts) + u64::from(cell.measured_bursts);
            waves = waves.checked_add(bursts).ok_or("wave budget overflow")?;
            attempts = attempts
                .checked_add(bursts * cell.requests.len() as u64)
                .ok_or("attempt budget overflow")?;
        }
        if waves > MAX_WAVES || attempts > MAX_ATTEMPTS {
            return Err(format!(
                "plan exceeds the {MAX_WAVES}-burst or {MAX_ATTEMPTS}-request budget"
            ));
        }
        Ok(ids)
    }

    fn validate_limits(&self) -> Result<()> {
        let limits = &self.limits;
        if !(1..=600_000).contains(&limits.request_timeout_ms) {
            return Err("limits.request_timeout_ms must be in 1..=600000".into());
        }
        let burst = limits.burst_timeout_ms;
        if burst < limits.request_timeout_ms || burst > 3_600_000 {
            return Err(
                "limits.burst_timeout_ms must be at least request_timeout_ms and at most 3600000"
                    .into(),
            );
        }
        if !(1024..=8 * 1024 * 1024).contains(&limits.response_bytes) {
            return Err("limits.response_bytes must be in 1024..=8388608".into());
        }
        Ok(())
    }

    fn validate_slots(&self) -> Result<()> {
        if self.slots.is_empty() || self.slots.len() > MAX_SLOTS {
            return Err(format!("plan needs 1..={MAX_SLOTS} slots"));
        }
        let mut ids = BTreeSet::new();
        let mut measured = [0u32; 3];
        let mut warmups = [0u32; 3];
        for (index, slot) in self.slots.iter().enumerate() {
            if !crate::model::identifier(&slot.id) || !ids.insert(slot.id.as_str()) {
                return Err("slot IDs must be unique short ASCII identifiers".into());
            }
            if slot.index as usize != index {
                return Err("slot index must equal its position in the declared plan".into());
            }
            if slot.warmup {
                warmups[arm_index(slot.arm)] += 1;
            } else {
                measured[arm_index(slot.arm)] += 1;
            }
        }
        if measured != [1, 1, 1] || warmups.iter().any(|count| *count > 1) {
            return Err(
                "plan needs exactly one measured slot per arm (a, b, a2) and at most one warmup slot per arm"
                    .into(),
            );
        }
        Ok(())
    }

    fn validate_gates(&self, cells: &BTreeSet<String>) -> Result<()> {
        if self.gates.is_empty() || self.gates.len() > MAX_GATES {
            return Err(format!("plan needs 1..={MAX_GATES} gates"));
        }
        for gate in &self.gates {
            if !cells.contains(&gate.cell) {
                return Err(format!("gate names undeclared cell {}", gate.cell));
            }
            if gate.max_adverse_bps > 9999 || gate.max_reference_spread_bps > 1_000_000 {
                return Err(
                    "gate bounds exceed the observed-envelope contract (9999 / 1000000 bps)".into(),
                );
            }
        }
        Ok(())
    }
}

fn parse_plan(source: &[u8]) -> Result<Plan> {
    let plan: Plan =
        serde_json::from_slice(source).map_err(|e| format!("invalid System One plan: {e}"))?;
    plan.validate()?;
    Ok(plan)
}

// -------------------------------------------------------------------------
// Response contract
// -------------------------------------------------------------------------

struct Answers {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    decisions: u32,
}

fn probability(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_f64)
        .is_some_and(|p| p.is_finite() && (0.0..=1.0).contains(&p))
}

fn validate_distribution(
    probabilities: &serde_json::Map<String, serde_json::Value>,
    expected: &BTreeSet<String>,
    id: &str,
    errors: &mut Vec<String>,
) -> bool {
    if probabilities.len() != expected.len()
        || probabilities.keys().any(|key| !expected.contains(key))
    {
        errors.push(format!("probability_keys_mismatch:{id}"));
        return false;
    }
    if probabilities
        .values()
        .any(|value| !probability(Some(value)))
    {
        errors.push(format!("invalid_probability:{id}"));
        return false;
    }
    true
}

fn validate_answer(
    question: &Question,
    answer: &serde_json::Value,
    id: &str,
    errors: &mut Vec<String>,
) -> bool {
    let Some(object) = answer.as_object() else {
        errors.push(format!("invalid_answer:{id}"));
        return false;
    };
    if let Some(kind) = object.get("type").and_then(serde_json::Value::as_str)
        && kind != question.kind.as_str()
    {
        errors.push(format!("answer_type_mismatch:{id}"));
        return false;
    }
    match question.kind {
        QuestionKind::Noul => {
            if !probability(object.get("noul")) {
                errors.push(format!("invalid_probability:{id}"));
                return false;
            }
            true
        }
        QuestionKind::Choice => {
            let Some(criteria) = question
                .criteria
                .as_ref()
                .and_then(serde_json::Value::as_object)
            else {
                errors.push(format!("invalid_criteria:{id}"));
                return false;
            };
            let Some(picked) = object.get("choice").and_then(serde_json::Value::as_str) else {
                errors.push(format!("missing_choice:{id}"));
                return false;
            };
            if !criteria.contains_key(picked) {
                errors.push(format!("unknown_choice:{id}"));
                return false;
            }
            if !probability(object.get("confidence")) {
                errors.push(format!("invalid_probability:{id}"));
                return false;
            }
            let Some(probabilities) = object
                .get("probabilities")
                .and_then(serde_json::Value::as_object)
            else {
                errors.push(format!("missing_probabilities:{id}"));
                return false;
            };
            let expected: BTreeSet<String> = criteria.keys().cloned().collect();
            validate_distribution(probabilities, &expected, id, errors)
        }
        QuestionKind::Score => {
            let Some(levels) = question
                .criteria
                .as_ref()
                .and_then(serde_json::Value::as_array)
            else {
                errors.push(format!("invalid_criteria:{id}"));
                return false;
            };
            let level = object
                .get("score")
                .and_then(serde_json::Value::as_f64)
                .filter(|value| value.is_finite());
            let Some(level) = level else {
                errors.push(format!("invalid_score:{id}"));
                return false;
            };
            if level < 0.0 || level > levels.len().saturating_sub(1) as f64 {
                errors.push(format!("invalid_score:{id}"));
                return false;
            }
            if !probability(object.get("confidence")) {
                errors.push(format!("invalid_probability:{id}"));
                return false;
            }
            let Some(probabilities) = object
                .get("probabilities")
                .and_then(serde_json::Value::as_object)
            else {
                errors.push(format!("missing_probabilities:{id}"));
                return false;
            };
            let expected: BTreeSet<String> = (0..levels.len()).map(|i| i.to_string()).collect();
            validate_distribution(probabilities, &expected, id, errors)
        }
    }
}

fn validate_response(
    request: &RequestDecl,
    body: &[u8],
) -> std::result::Result<Answers, (Status, Vec<String>)> {
    let mut errors = Vec::new();
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Err((Status::Malformed, vec!["malformed_response".into()]));
    };
    let Some(root) = value.as_object() else {
        return Err((Status::Malformed, vec!["malformed_response".into()]));
    };
    let usage = root.get("usage").and_then(serde_json::Value::as_object);
    let output_tokens = match usage {
        None => {
            errors.push("missing_usage".into());
            None
        }
        Some(usage) => match usage.get("output_tokens") {
            None => {
                errors.push("missing_usage_output_tokens".into());
                None
            }
            Some(token) => match token.as_u64() {
                None => {
                    errors.push("invalid_usage_output_tokens".into());
                    None
                }
                Some(tokens) if tokens != 0 => {
                    errors.push("output_tokens_nonzero".into());
                    Some(tokens)
                }
                Some(_) => Some(0),
            },
        },
    };
    let input_tokens = match usage.and_then(|usage| usage.get("input_tokens")) {
        None => None,
        Some(value) => match value.as_u64() {
            Some(tokens) => Some(tokens),
            None => {
                errors.push("invalid_usage_input_tokens".into());
                None
            }
        },
    };
    let Some(answers) = root.get("answers").and_then(serde_json::Value::as_object) else {
        errors.push("missing_answers".into());
        return Err((Status::Unsupported, errors));
    };
    for id in answers.keys() {
        if !request.questions.contains_key(id.as_str()) {
            errors.push(format!("unknown_answer:{id}"));
        }
    }
    let mut decisions = 0u32;
    for (id, question) in &request.questions {
        match answers.get(id) {
            None => errors.push(format!("missing_answer:{id}")),
            Some(answer) => {
                if validate_answer(question, answer, id, &mut errors) {
                    decisions += 1;
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(Answers {
            input_tokens,
            output_tokens,
            decisions,
        })
    } else {
        Err((Status::Unsupported, errors))
    }
}

fn json_media(response: &reqwest::Response) -> bool {
    let mut types = response.headers().get_all("content-type").iter();
    types.next().is_some_and(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
    }) && types.next().is_none()
}

// -------------------------------------------------------------------------
// Retained evidence
// -------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub ordinal: u32,
    pub request: String,
    pub lane: u32,
    pub dispatched: bool,
    pub status: Status,
    pub detail: String,
    pub http_status: Option<u16>,
    pub latency_us: u64,
    pub dispatch_offset_us: u64,
    pub request_bytes: usize,
    pub request_sha256: String,
    pub response_bytes: usize,
    pub response_sha256: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub decisions: u32,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Wave {
    pub index: u32,
    pub plan_sha256: String,
    pub phase: Phase,
    pub cell: String,
    pub trial: u32,
    pub attempts: Vec<Attempt>,
    pub makespan_us: u64,
    pub dispatch_spread_us: u64,
    pub requests: u32,
    pub decisions: u32,
    pub complete: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CorpusEntry {
    pub id: String,
    pub bytes: usize,
    pub sha256: String,
    pub questions: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Collector {
    pub version: String,
    pub source: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    pub id: String,
    pub kind: String,
    pub units: String,
    pub synchronization: String,
}

impl Clock {
    fn local() -> Self {
        Self {
            id: "std-instant-monotonic".into(),
            kind: "monotonic".into(),
            units: "microseconds".into(),
            synchronization: "single-process-origin".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub version: u32,
    pub kind: String,
    pub claim: String,
    pub support: String,
    pub protocol: String,
    pub path: String,
    pub slot: Slot,
    pub plan_sha256: String,
    pub plan_waves: u32,
    pub deployment: Deployment,
    pub deployment_sha256: String,
    pub endpoint: String,
    pub endpoint_sha256: String,
    pub collector: Collector,
    pub clock: Clock,
    pub limits: Limits,
    pub corpus: Vec<CorpusEntry>,
    pub started_unix_ms: u64,
}

fn request_dir(root: &Path, ordinal: u32) -> PathBuf {
    root.join(format!("request-{ordinal:06}"))
}

fn publish_wave(
    root: &Path,
    corpus: &BTreeMap<String, String>,
    wave: &Wave,
    responses: &[Vec<u8>],
) -> Result<()> {
    if responses.len() != wave.attempts.len() {
        return Err("burst response bytes do not match its attempts".into());
    }
    for (lane, attempt) in wave.attempts.iter().enumerate() {
        let dir = request_dir(root, attempt.ordinal);
        evidence::fresh(&dir)?;
        let body = corpus
            .get(&attempt.request)
            .ok_or("attempt names an undeclared request")?;
        evidence::write(&dir.join("body.bin"), body.as_bytes())?;
        evidence::write(&dir.join("response.bin"), &responses[lane])?;
        evidence::sync(&dir)?;
    }
    evidence::publish(root, &format!("wave-{:06}.json", wave.index), wave)
}

// -------------------------------------------------------------------------
// Capture
// -------------------------------------------------------------------------

#[derive(Args)]
pub struct CaptureArgs {
    /// Prospective declared plan; `--slot` selects the arm this capture realizes.
    #[arg(long)]
    pub plan: PathBuf,
    /// Declared plan slot index this capture realizes.
    #[arg(long)]
    pub slot: usize,
    /// Full endpoint URL whose path ends with the System One protocol path.
    #[arg(long)]
    pub endpoint: String,
    /// Operator deployment declaration; a closed JSON object with the four documented fields.
    #[arg(long)]
    pub deployment: PathBuf,
    /// Environment-variable *name* holding a bearer credential; never the value.
    #[arg(long)]
    pub auth_env: Option<String>,
    /// Permit plain HTTP to a literal loopback address.
    #[arg(long)]
    pub local_http: bool,
    /// Fresh output directory; existing evidence is never overwritten.
    #[arg(long)]
    pub out: PathBuf,
}

struct Session<'a> {
    plan: &'a Plan,
    plan_sha256: String,
    corpus: BTreeMap<String, String>,
    url: reqwest::Url,
    auth: Option<HeaderValue>,
    local_http: bool,
    out: &'a Path,
}

struct Burst<'a> {
    client: &'a reqwest::Client,
    url: &'a reqwest::Url,
    auth: Option<&'a HeaderValue>,
    plan: &'a Plan,
    cell: &'a Cell,
    corpus: &'a BTreeMap<String, String>,
}

struct Raw {
    lane: u32,
    dispatch_offset_us: u64,
    latency_us: u64,
    http_status: Option<u16>,
    body: Vec<u8>,
    failure: Option<(Status, String)>,
}

struct BurstResult {
    attempts: Vec<Attempt>,
    responses: Vec<Vec<u8>>,
    makespan_us: u64,
    dispatch_spread_us: u64,
}

async fn exchange(
    client: reqwest::Client,
    request: reqwest::Request,
    cap: usize,
    http_status: &mut Option<u16>,
    failure: &mut Option<(Status, String)>,
    body: &mut Vec<u8>,
) {
    let mut response = match client.execute(request).await {
        Ok(response) => response,
        Err(error) => {
            *failure = Some((Status::TransportError, transport_detail(&error)));
            return;
        }
    };
    let code = response.status().as_u16();
    *http_status = Some(code);
    if code != 200 {
        *failure = Some((Status::HttpError, format!("HTTP status {code}")));
        return;
    }
    if !json_media(&response) {
        *failure = Some((
            Status::Unsupported,
            "response is not a single application/json media type".into(),
        ));
        return;
    }
    loop {
        match response.chunk().await {
            Err(error) => {
                *failure = Some((Status::TransportError, transport_detail(&error)));
                return;
            }
            Ok(None) => return,
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > cap {
                    let retained = cap - body.len();
                    body.extend_from_slice(&chunk[..retained]);
                    *failure = Some((
                        Status::ResponseLimit,
                        format!("response exceeded the declared {cap}-byte limit"),
                    ));
                    return;
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
}

fn transport_detail(error: &reqwest::Error) -> String {
    if error.is_connect() {
        "connection failed".into()
    } else if error.is_timeout() {
        "transport timeout".into()
    } else if error.is_body() {
        "body transfer failed".into()
    } else {
        "transport request failed".into()
    }
}

fn offset_us(origin: Instant) -> u64 {
    origin.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

async fn dispatch_burst(burst: &Burst<'_>, origin: Instant) -> Result<Vec<Option<Raw>>> {
    let request_deadline = Duration::from_millis(u64::from(burst.plan.limits.request_timeout_ms));
    let burst_deadline = tokio::time::Instant::from_std(
        origin + Duration::from_millis(u64::from(burst.plan.limits.burst_timeout_ms)),
    );
    let semaphore = Arc::new(Semaphore::new(burst.cell.concurrency as usize));
    let mut tasks = JoinSet::new();
    let mut raws: Vec<Option<Raw>> = (0..burst.cell.requests.len()).map(|_| None).collect();
    let mut next = 0usize;
    let mut stopped = false;
    while next < burst.cell.requests.len() && !stopped {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(burst_deadline) => { stopped = true; }
            permit = semaphore.clone().acquire_owned() => {
                let permit = permit.map_err(|_| "burst dispatch semaphore closed")?;
                let id = &burst.cell.requests[next];
                let body = burst
                    .corpus
                    .get(id)
                    .ok_or("cell references an unknown request")?
                    .clone();
                let request = wire::request(burst.client, burst.url, burst.auth, body, false)?;
                let client = burst.client.clone();
                let cap = burst.plan.limits.response_bytes;
                let lane = next as u32;
                tasks.spawn(async move {
                    let _permit = permit;
                    let mut http_status = None;
                    let mut failure = None;
                    let mut body = Vec::new();
                    let start = Instant::now();
                    let dispatch_offset_us = offset_us(origin);
                    let outcome = tokio::time::timeout(
                        request_deadline,
                        exchange(client, request, cap, &mut http_status, &mut failure, &mut body),
                    )
                    .await;
                    Raw {
                        lane,
                        dispatch_offset_us,
                        latency_us: wire::us(start),
                        http_status,
                        body,
                        failure: match outcome {
                            Err(_) => Some((Status::TotalTimeout, "deadline expired".into())),
                            Ok(()) => failure,
                        },
                    }
                });
                next += 1;
            }
            joined = tasks.join_next(), if !tasks.is_empty() => {
                let Some(joined) = joined else { break; };
                let raw = joined
                    .map_err(|_| "burst task failed; no success claimed for this burst")?;
                let lane = raw.lane as usize;
                raws[lane] = Some(raw);
            }
        }
    }
    while !tasks.is_empty() {
        match tasks.join_next().await {
            Some(Ok(raw)) => {
                let lane = raw.lane as usize;
                raws[lane] = Some(raw);
            }
            Some(Err(_)) => {
                return Err("burst task failed; no success claimed for this burst".into());
            }
            None => break,
        }
    }
    Ok(raws)
}

fn assemble(
    burst: &Burst<'_>,
    raws: Vec<Option<Raw>>,
    base_ordinal: u32,
    makespan_us: u64,
) -> Result<BurstResult> {
    let mut attempts = Vec::with_capacity(burst.cell.requests.len());
    let mut responses = Vec::with_capacity(burst.cell.requests.len());
    for (lane, raw) in raws.into_iter().enumerate() {
        let id = &burst.cell.requests[lane];
        let decl = burst
            .plan
            .requests
            .iter()
            .find(|request| &request.id == id)
            .ok_or("cell references an unknown request")?;
        let body = &burst.corpus[id];
        let mut attempt = Attempt {
            ordinal: base_ordinal + lane as u32,
            request: decl.id.clone(),
            lane: lane as u32,
            dispatched: raw.is_some(),
            status: Status::TotalTimeout,
            detail: "burst deadline before dispatch".into(),
            http_status: None,
            latency_us: 0,
            dispatch_offset_us: 0,
            request_bytes: body.len(),
            request_sha256: evidence::digest(body.as_bytes()),
            response_bytes: 0,
            response_sha256: evidence::digest(&[]),
            input_tokens: None,
            output_tokens: None,
            decisions: 0,
            errors: Vec::new(),
        };
        let mut response = Vec::new();
        if let Some(raw) = raw {
            attempt.http_status = raw.http_status;
            attempt.latency_us = raw.latency_us;
            attempt.dispatch_offset_us = raw.dispatch_offset_us;
            attempt.response_bytes = raw.body.len();
            attempt.response_sha256 = evidence::digest(&raw.body);
            response = raw.body;
            match raw.failure {
                Some((status, detail)) => {
                    attempt.status = status;
                    attempt.detail = detail;
                }
                None => match validate_response(decl, &response) {
                    Ok(answers) => {
                        attempt.status = Status::Complete;
                        attempt.detail = "complete System One decision response".into();
                        attempt.input_tokens = answers.input_tokens;
                        attempt.output_tokens = answers.output_tokens;
                        attempt.decisions = answers.decisions;
                    }
                    Err((status, errors)) => {
                        attempt.status = status;
                        attempt.detail = "response violated the System One contract".into();
                        attempt.errors = errors;
                    }
                },
            }
        }
        attempts.push(attempt);
        responses.push(response);
    }
    let spread = match (
        attempts
            .iter()
            .map(|attempt| attempt.dispatch_offset_us)
            .min(),
        attempts
            .iter()
            .map(|attempt| attempt.dispatch_offset_us)
            .max(),
    ) {
        (Some(low), Some(high)) => high - low,
        _ => 0,
    };
    Ok(BurstResult {
        attempts,
        responses,
        makespan_us,
        dispatch_spread_us: spread,
    })
}

async fn run_burst(burst: &Burst<'_>, origin: Instant, base_ordinal: u32) -> Result<BurstResult> {
    let raws = dispatch_burst(burst, origin).await?;
    let makespan_us = offset_us(origin);
    assemble(burst, raws, base_ordinal, makespan_us)
}

async fn dispatch(session: &Session<'_>) -> Result<()> {
    let pool = session
        .plan
        .cells
        .iter()
        .map(|cell| cell.concurrency as usize)
        .max()
        .unwrap_or(1);
    let client = wire::client(session.local_http, pool)?;
    let mut ordinal = 0u32;
    let mut index = 0u32;
    for cell in &session.plan.cells {
        for (phase, bursts) in [
            (Phase::Warmup, cell.warmup_bursts),
            (Phase::Measured, cell.measured_bursts),
        ] {
            for trial in 0..bursts {
                let origin = Instant::now();
                let burst = Burst {
                    client: &client,
                    url: &session.url,
                    auth: session.auth.as_ref(),
                    plan: session.plan,
                    cell,
                    corpus: &session.corpus,
                };
                let result = run_burst(&burst, origin, ordinal).await?;
                ordinal += result.attempts.len() as u32;
                let requests = result.attempts.len() as u32;
                let decisions = result
                    .attempts
                    .iter()
                    .map(|attempt| attempt.decisions)
                    .sum();
                let complete = result
                    .attempts
                    .iter()
                    .all(|attempt| attempt.status == Status::Complete && attempt.errors.is_empty());
                let wave = Wave {
                    index,
                    plan_sha256: session.plan_sha256.clone(),
                    phase,
                    cell: cell.id.clone(),
                    trial,
                    requests,
                    decisions,
                    complete,
                    attempts: result.attempts,
                    makespan_us: result.makespan_us,
                    dispatch_spread_us: result.dispatch_spread_us,
                };
                publish_wave(session.out, &session.corpus, &wave, &result.responses)?;
                evidence::sync(session.out)?;
                index += 1;
            }
        }
    }
    Ok(())
}

pub fn capture(args: &CaptureArgs) -> Result<Inspection> {
    let plan_source = evidence::read(&args.plan, CAP)?;
    let plan = parse_plan(&plan_source)?;
    let plan_sha256 = evidence::digest(&plan_source);
    let slot = plan
        .slots
        .get(args.slot)
        .cloned()
        .ok_or("plan has no slot at the requested index")?;
    let deployment_source = evidence::read(&args.deployment, CAP)?;
    let deployment: Deployment = serde_json::from_slice(&deployment_source)
        .map_err(|e| format!("invalid deployment declaration: {e}"))?;
    deployment.validate()?;
    let url = wire::endpoint(&args.endpoint, args.local_http)?;
    if !url.path().ends_with(PATH) {
        return Err(format!(
            "endpoint path must end with {PATH}; the declared endpoint is not a System One route"
        ));
    }
    let auth = wire::credential(args.auth_env.as_deref())?;
    let mut corpus = BTreeMap::new();
    let mut entries = Vec::with_capacity(plan.requests.len());
    for request in &plan.requests {
        let body = render_body(request)?;
        entries.push(CorpusEntry {
            id: request.id.clone(),
            bytes: body.len(),
            sha256: evidence::digest(body.as_bytes()),
            questions: request.questions.len() as u32,
        });
        corpus.insert(request.id.clone(), body);
    }
    let receipt = Capture {
        version: 1,
        kind: CAPTURE_KIND.into(),
        claim: CLAIM_CAPTURE.into(),
        support: SUPPORT.into(),
        protocol: PROTOCOL.into(),
        path: PATH.into(),
        slot,
        plan_sha256: plan_sha256.clone(),
        plan_waves: planned_bursts(&plan),
        deployment,
        deployment_sha256: evidence::digest(&deployment_source),
        endpoint: args.endpoint.clone(),
        endpoint_sha256: evidence::digest(args.endpoint.as_bytes()),
        collector: Collector {
            version: env!("CARGO_PKG_VERSION").into(),
            source: env!("GRILL_PERF_SOURCE").into(),
            sha256: evidence::binary_digest()?,
        },
        clock: Clock::local(),
        limits: plan.limits,
        corpus: entries,
        started_unix_ms: crate::metrics::unix_ms(),
    };
    evidence::fresh(&args.out)?;
    evidence::write(&args.out.join("plan.json"), &plan_source)?;
    evidence::write(&args.out.join("deployment.json"), &deployment_source)?;
    evidence::publish(&args.out, "receipt.json", &receipt)?;
    evidence::sync(&args.out)?;
    let session = Session {
        plan: &plan,
        plan_sha256,
        corpus,
        url,
        auth,
        local_http: args.local_http,
        out: &args.out,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "cannot build the measurement runtime")?;
    runtime.block_on(dispatch(&session))?;
    Ok(load(&args.out)?.inspection())
}

// -------------------------------------------------------------------------
// Offline verification and inspection
// -------------------------------------------------------------------------

pub struct Loaded {
    pub plan: Plan,
    pub plan_sha256: String,
    pub capture: Capture,
    pub digest: String,
    pub waves: Vec<Option<Wave>>,
}

#[derive(Clone, Serialize)]
pub struct Summary {
    pub samples: usize,
    pub min: f64,
    pub mean: f64,
    pub max: f64,
}

#[derive(Clone, Serialize)]
pub struct CellInspection {
    pub cell: String,
    pub warmup_bursts: usize,
    pub measured_bursts: usize,
    pub eligible_bursts: usize,
    pub requests: usize,
    pub decisions: usize,
    pub latency_us: Option<Summary>,
    pub requests_per_second: Option<Summary>,
    pub decisions_per_second: Option<Summary>,
    pub failure_codes: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct WavesSummary {
    pub planned: u32,
    pub retained: u32,
    pub measured: u32,
    pub complete_measured: u32,
    pub missing: Vec<u32>,
}

#[derive(Clone, Serialize)]
pub struct Inspection {
    pub version: u32,
    pub kind: &'static str,
    pub claim: &'static str,
    pub support: &'static str,
    pub protocol: String,
    pub slot: Slot,
    pub plan_sha256: String,
    pub evidence_sha256: String,
    pub collector: Collector,
    pub deployment: Deployment,
    pub deployment_sha256: String,
    pub endpoint_sha256: String,
    pub clock: Clock,
    pub cells: Vec<CellInspection>,
    pub waves: WavesSummary,
    pub failure_codes: Vec<String>,
    pub outcome: Outcome,
}

fn status_name(status: &Status) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

fn summary(values: &[f64]) -> Option<Summary> {
    let first = *values.first()?;
    let (min, max, total) = values.iter().fold((first, first, 0.0), |acc, value| {
        (acc.0.min(*value), acc.1.max(*value), acc.2 + *value)
    });
    Some(Summary {
        samples: values.len(),
        min,
        mean: total / values.len() as f64,
        max,
    })
}

fn collect_codes(wave: &Wave, codes: &mut BTreeSet<String>) {
    for attempt in &wave.attempts {
        if !attempt.errors.is_empty() {
            codes.extend(attempt.errors.iter().cloned());
        }
        if attempt.status != Status::Complete {
            codes.insert(status_name(&attempt.status));
        }
    }
}

fn cell_inspection(loaded: &Loaded, cell: &Cell) -> CellInspection {
    let mut latencies = Vec::new();
    let mut request_rates = Vec::new();
    let mut decision_rates = Vec::new();
    let mut codes = BTreeSet::new();
    let mut warmups = 0usize;
    let mut measured = 0usize;
    let mut eligible = 0usize;
    let mut requests = 0usize;
    let mut decisions = 0usize;
    let waves = loaded
        .waves
        .iter()
        .flatten()
        .filter(|wave| wave.cell == cell.id);
    for wave in waves {
        collect_codes(wave, &mut codes);
        if wave.phase == Phase::Warmup {
            warmups += 1;
            continue;
        }
        measured += 1;
        if !wave.complete {
            continue;
        }
        eligible += 1;
        requests += wave.requests as usize;
        decisions += wave.decisions as usize;
        for attempt in &wave.attempts {
            latencies.push(attempt.latency_us as f64);
        }
        if wave.makespan_us > 0 {
            request_rates.push(f64::from(wave.requests) * 1_000_000.0 / wave.makespan_us as f64);
            decision_rates.push(f64::from(wave.decisions) * 1_000_000.0 / wave.makespan_us as f64);
        }
    }
    CellInspection {
        cell: cell.id.clone(),
        warmup_bursts: warmups,
        measured_bursts: measured,
        eligible_bursts: eligible,
        requests,
        decisions,
        latency_us: summary(&latencies),
        requests_per_second: summary(&request_rates),
        decisions_per_second: summary(&decision_rates),
        failure_codes: codes.into_iter().collect(),
    }
}

fn inspect_loaded(loaded: &Loaded) -> Inspection {
    let mut retained = 0u32;
    let mut measured = 0u32;
    let mut complete_measured = 0u32;
    let mut missing = Vec::new();
    for (index, wave) in loaded.waves.iter().enumerate() {
        match wave {
            Some(wave) => {
                retained += 1;
                if wave.phase == Phase::Measured {
                    measured += 1;
                    if wave.complete {
                        complete_measured += 1;
                    }
                }
            }
            None => missing.push(index as u32),
        }
    }
    let cells: Vec<CellInspection> = loaded
        .plan
        .cells
        .iter()
        .map(|cell| cell_inspection(loaded, cell))
        .collect();
    let mut codes: BTreeSet<String> = missing
        .iter()
        .map(|index| format!("missing_wave:{index}"))
        .collect();
    for cell in &cells {
        codes.extend(cell.failure_codes.iter().cloned());
    }
    let complete = missing.is_empty()
        && loaded
            .waves
            .iter()
            .flatten()
            .filter(|wave| wave.phase == Phase::Measured)
            .all(|wave| wave.complete);
    let capture = &loaded.capture;
    Inspection {
        version: 1,
        kind: "systemone-inspection-v1",
        claim: CLAIM_CAPTURE,
        support: SUPPORT,
        protocol: capture.protocol.clone(),
        slot: capture.slot.clone(),
        plan_sha256: loaded.plan_sha256.clone(),
        evidence_sha256: loaded.digest.clone(),
        collector: capture.collector.clone(),
        deployment: capture.deployment.clone(),
        deployment_sha256: capture.deployment_sha256.clone(),
        endpoint_sha256: capture.endpoint_sha256.clone(),
        clock: capture.clock.clone(),
        cells,
        waves: WavesSummary {
            planned: capture.plan_waves,
            retained,
            measured,
            complete_measured,
            missing,
        },
        failure_codes: codes.into_iter().collect(),
        outcome: if complete {
            Outcome::Pass
        } else {
            Outcome::Inconclusive
        },
    }
}

impl Loaded {
    pub fn inspection(&self) -> Inspection {
        inspect_loaded(self)
    }
}

fn verify_wave(
    plan: &Plan,
    capture: &Capture,
    corpus: &BTreeMap<&str, &CorpusEntry>,
    root: &Path,
    wave: &Wave,
) -> Result<()> {
    if wave.plan_sha256 != capture.plan_sha256 {
        return Err(format!(
            "retained wave {} does not bind the receipt plan",
            wave.index
        ));
    }
    let cell = plan
        .cells
        .iter()
        .find(|cell| cell.id == wave.cell)
        .ok_or("retained wave names an unknown cell")?;
    if wave.attempts.len() != cell.requests.len() {
        return Err(format!(
            "retained wave {} does not hold one attempt per declared request",
            wave.index
        ));
    }
    let mut requests = 0u32;
    let mut decisions = 0u32;
    let mut complete = true;
    for attempt in &wave.attempts {
        let lane = attempt.lane as usize;
        if lane >= cell.requests.len() {
            return Err(format!(
                "retained wave {} has an out-of-range lane",
                wave.index
            ));
        }
        let decl = plan
            .requests
            .iter()
            .find(|request| request.id == cell.requests[lane])
            .ok_or("cell references an unknown request")?;
        if attempt.request != decl.id {
            return Err(format!(
                "retained wave {} names the wrong request for its lane",
                wave.index
            ));
        }
        let entry = corpus
            .get(decl.id.as_str())
            .ok_or("attempt names an undeclared request")?;
        if attempt.request_bytes != entry.bytes || attempt.request_sha256 != entry.sha256 {
            return Err(format!(
                "retained wave {} request identity does not match the plan",
                wave.index
            ));
        }
        let dir = request_dir(root, attempt.ordinal);
        let retained = evidence::read(&dir.join("body.bin"), REQUEST_CAP)?;
        if retained.len() != entry.bytes || evidence::digest(&retained) != entry.sha256 {
            return Err(format!(
                "retained request bytes changed in wave {}",
                wave.index
            ));
        }
        let response = evidence::read(&dir.join("response.bin"), capture.limits.response_bytes)?;
        if response.len() != attempt.response_bytes
            || evidence::digest(&response) != attempt.response_sha256
        {
            return Err(format!(
                "retained response bytes changed in wave {}",
                wave.index
            ));
        }
        if attempt.status == Status::Complete && attempt.errors.is_empty() {
            let answers = validate_response(decl, &response).map_err(|(_, errors)| {
                format!("retained response no longer satisfies the contract: {errors:?}")
            })?;
            if answers.input_tokens != attempt.input_tokens
                || answers.output_tokens != attempt.output_tokens
                || answers.decisions != attempt.decisions
            {
                return Err(format!(
                    "retained answer facts do not match wave {} attempt {}",
                    wave.index, attempt.ordinal
                ));
            }
        }
        requests += 1;
        decisions += attempt.decisions;
        complete &= attempt.status == Status::Complete && attempt.errors.is_empty();
    }
    if wave.requests != requests || wave.decisions != decisions || wave.complete != complete {
        return Err(format!(
            "retained wave {} aggregates do not match its attempts",
            wave.index
        ));
    }
    Ok(())
}

pub fn load(root: &Path) -> Result<Loaded> {
    evidence::directory(root)?;
    let plan_source = evidence::read(&root.join("plan.json"), CAP)?;
    let plan = parse_plan(&plan_source)?;
    let plan_sha256 = evidence::digest(&plan_source);
    let deployment_source = evidence::read(&root.join("deployment.json"), CAP)?;
    let deployment: Deployment = serde_json::from_slice(&deployment_source)
        .map_err(|e| format!("invalid retained deployment declaration: {e}"))?;
    deployment.validate()?;
    let receipt_source = evidence::read(&root.join("receipt.json"), CAP)?;
    let capture: Capture = serde_json::from_slice(&receipt_source)
        .map_err(|e| format!("invalid capture receipt: {e}"))?;
    if capture.version != 1
        || capture.kind != CAPTURE_KIND
        || capture.protocol != PROTOCOL
        || capture.path != PATH
    {
        return Err("unsupported System One capture receipt identity".into());
    }
    if capture.plan_sha256 != plan_sha256 {
        return Err("capture receipt does not bind the retained plan".into());
    }
    if capture.deployment_sha256 != evidence::digest(&deployment_source)
        || capture.deployment != deployment
    {
        return Err("capture receipt does not bind the retained deployment".into());
    }
    if capture.limits != plan.limits {
        return Err("capture limits do not match the retained plan".into());
    }
    if capture.corpus.len() != plan.requests.len() {
        return Err("capture corpus does not match the retained plan".into());
    }
    for (entry, request) in capture.corpus.iter().zip(&plan.requests) {
        let body = render_body(request)?;
        if entry.id != request.id
            || entry.bytes != body.len()
            || entry.sha256 != evidence::digest(body.as_bytes())
            || entry.questions != request.questions.len() as u32
        {
            return Err("retained corpus identity does not match the plan".into());
        }
    }
    if capture.plan_waves != planned_bursts(&plan) {
        return Err("capture receipt declares the wrong number of bursts".into());
    }
    let corpus_index: BTreeMap<&str, &CorpusEntry> = capture
        .corpus
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let mut waves: Vec<Option<Wave>> = vec![None; capture.plan_waves as usize];
    let mut ordinals = BTreeSet::new();
    let mut manifest = format!("{}\n", evidence::digest(&receipt_source));
    for (index, retained) in waves.iter_mut().enumerate() {
        let path = root.join(format!("wave-{index:06}.json"));
        let bytes = match evidence::read(&path, CAP) {
            Ok(bytes) => bytes,
            Err(_) if std::fs::symlink_metadata(&path).is_err() => {
                manifest.push_str(&format!("{index}:missing\n"));
                continue;
            }
            Err(error) => return Err(error),
        };
        let wave: Wave = serde_json::from_slice(&bytes)
            .map_err(|e| format!("invalid retained wave {index}: {e}"))?;
        if wave.index as usize != index {
            return Err("retained wave files are out of order".into());
        }
        verify_wave(&plan, &capture, &corpus_index, root, &wave)?;
        for attempt in &wave.attempts {
            if !ordinals.insert(attempt.ordinal) {
                return Err("two retained attempts share one request directory".into());
            }
        }
        manifest.push_str(&format!("{index}:{}\n", evidence::digest(&bytes)));
        *retained = Some(wave);
    }
    Ok(Loaded {
        plan,
        plan_sha256,
        capture,
        digest: evidence::digest(manifest.as_bytes()),
        waves,
    })
}

pub fn inspect(path: &Path) -> Result<Inspection> {
    Ok(load(path)?.inspection())
}

// -------------------------------------------------------------------------
// Offline A/B/A2 comparison
// -------------------------------------------------------------------------

#[derive(Serialize)]
pub struct GateResult {
    pub gate: Gate,
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub reference: Option<[Rational; 2]>,
    pub candidate: Option<[Rational; 2]>,
    pub counts: [usize; 3],
}

#[derive(Serialize)]
pub struct Comparison {
    pub version: u32,
    pub kind: &'static str,
    pub claim: &'static str,
    pub support: &'static str,
    pub protocol: String,
    pub plan_sha256: String,
    pub acquisitions: Vec<Inspection>,
    pub reasons: Vec<String>,
    pub gates: Vec<GateResult>,
    pub outcome: Outcome,
}

fn reason_outcome(reason: EnvelopeReason) -> Outcome {
    match reason {
        EnvelopeReason::InvalidRational
        | EnvelopeReason::InvalidBounds
        | EnvelopeReason::ArithmeticOverflow { .. } => Outcome::Error,
        _ => Outcome::Inconclusive,
    }
}

fn arm_samples(gate: &Gate, arms: &[Vec<Loaded>; 3]) -> ([Vec<Rational>; 3], bool) {
    let mut samples: [Vec<Rational>; 3] = std::array::from_fn(|_| Vec::new());
    let mut complete = arms.iter().all(|list| !list.is_empty());
    for (arm, list) in arms.iter().enumerate() {
        for loaded in list {
            if loaded.capture.slot.warmup {
                continue;
            }
            if loaded.waves.iter().any(Option::is_none) {
                complete = false;
            }
            let waves = loaded
                .waves
                .iter()
                .flatten()
                .filter(|wave| wave.cell == gate.cell && wave.phase == Phase::Measured);
            for wave in waves {
                match gate.metric {
                    Metric::LatencyUs => {
                        for attempt in &wave.attempts {
                            if attempt.status == Status::Complete && attempt.errors.is_empty() {
                                samples[arm].push(Rational {
                                    numerator: attempt.latency_us,
                                    denominator: 1,
                                });
                            } else {
                                complete = false;
                            }
                        }
                    }
                    Metric::RequestsPerSecond | Metric::DecisionsPerSecond => {
                        if !wave.complete || wave.makespan_us == 0 {
                            complete = false;
                            continue;
                        }
                        let count = match gate.metric {
                            Metric::DecisionsPerSecond => wave.decisions,
                            _ => wave.requests,
                        };
                        samples[arm].push(Rational {
                            numerator: u64::from(count) * 1_000_000,
                            denominator: wave.makespan_us,
                        });
                    }
                }
            }
        }
    }
    (samples, complete)
}

fn gate_outcome(gate: &Gate, arms: &[Vec<Loaded>; 3]) -> GateResult {
    let (samples, complete) = arm_samples(gate, arms);
    let counts = samples.each_ref().map(Vec::len);
    let reference_values: Vec<Rational> = samples[0].iter().chain(&samples[2]).copied().collect();
    let mut result = GateResult {
        gate: gate.clone(),
        outcome: Outcome::Inconclusive,
        reason: None,
        reference: None,
        candidate: None,
        counts,
    };
    match (
        envelope::range(&samples[1]),
        envelope::range(&reference_values),
    ) {
        (Ok(candidate), Ok(reference)) => {
            result.candidate = candidate;
            result.reference = reference;
            if !complete || counts.iter().any(|count| *count < MIN_ARM_SAMPLES) {
                result.reason =
                    Some("incomplete or under-sampled arms; no survivor-only gate".into());
                return result;
            }
            let (Some(reference), Some(candidate)) = (reference, candidate) else {
                result.reason = Some("no eligible observation in one arm".into());
                return result;
            };
            let direction = match gate.metric {
                Metric::LatencyUs => Direction::LowerBetter,
                _ => Direction::HigherBetter,
            };
            match envelope::assess(
                reference,
                candidate,
                gate.max_adverse_bps,
                gate.max_reference_spread_bps,
                direction,
            ) {
                Ok(assessment) => {
                    result.outcome = match assessment.decision {
                        EnvelopeDecision::Pass => Outcome::Pass,
                        EnvelopeDecision::Regression => Outcome::Regression,
                    };
                }
                Err(reason) => {
                    result.outcome = reason_outcome(reason);
                    result.reason = Some(format!("{reason:?}"));
                }
            }
        }
        (Err(reason), _) | (_, Err(reason)) => {
            result.outcome = reason_outcome(reason);
            result.reason = Some(format!("{reason:?}"));
        }
    }
    result
}

fn reference_binding(arms: &[Vec<Loaded>; 3], report: &mut Comparison) {
    let Some(a) = arms[0].first() else {
        return;
    };
    let clock = a.capture.clock.clone();
    let endpoint = a.capture.endpoint_sha256.clone();
    let deployment = a.capture.deployment_sha256.clone();
    let reference_only = arms[0].iter().chain(&arms[2]);
    let mut drifted = false;
    for loaded in reference_only {
        if loaded.capture.clock != clock {
            drifted = true;
        }
        if loaded.capture.endpoint_sha256 != endpoint
            || loaded.capture.deployment_sha256 != deployment
        {
            drifted = true;
        }
    }
    if drifted {
        report.outcome = report.outcome.max(Outcome::Inconclusive);
        report.reasons.push(
            "the unchanged reference arm recorded a different clock, endpoint or deployment".into(),
        );
    }
    let changed = arms[1].iter().any(|loaded| {
        loaded.capture.endpoint_sha256 != endpoint || loaded.capture.deployment_sha256 != deployment
    });
    if !changed {
        report.outcome = report.outcome.max(Outcome::Inconclusive);
        report.reasons.push(
            "the candidate arm repeated the reference endpoint and deployment; no declared change to assess"
                .into(),
        );
    }
}

fn exposure(arms: &[Vec<Loaded>; 3], report: &mut Comparison) {
    let mut seen: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for loaded in arms.iter().flatten() {
        if loaded.capture.slot.warmup {
            continue;
        }
        let waves = loaded
            .waves
            .iter()
            .flatten()
            .filter(|wave| wave.phase == Phase::Measured);
        for wave in waves {
            for attempt in &wave.attempts {
                if let Some(tokens) = attempt.input_tokens {
                    seen.entry(attempt.request.clone())
                        .or_default()
                        .insert(tokens);
                }
            }
        }
    }
    if seen.values().any(|values| values.len() > 1) {
        report.outcome = report.outcome.max(Outcome::Inconclusive);
        report.reasons.push(
            "reported input-token exposure changed between acquisitions for one declared state"
                .into(),
        );
    }
}

pub fn compare(
    plan_path: &Path,
    a: &[PathBuf],
    b: &[PathBuf],
    a2: &[PathBuf],
) -> Result<Comparison> {
    let source = evidence::read(plan_path, CAP)?;
    let plan = parse_plan(&source)?;
    let plan_sha256 = evidence::digest(&source);
    if a.is_empty() || b.is_empty() || a2.is_empty() {
        return Err("--a, --b and --a2 each need at least one capture directory".into());
    }
    let mut report = Comparison {
        version: 1,
        kind: COMPARISON_KIND,
        claim: CLAIM_COMPARISON,
        support: SUPPORT,
        protocol: PROTOCOL.into(),
        plan_sha256: plan_sha256.clone(),
        acquisitions: Vec::new(),
        reasons: Vec::new(),
        gates: Vec::new(),
        outcome: Outcome::Pass,
    };
    let mut arms: [Vec<Loaded>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut slots = BTreeSet::new();
    let mut digests = BTreeSet::new();
    let mut collector: Option<String> = None;
    for (arm, paths) in [(Arm::A, a), (Arm::B, b), (Arm::A2, a2)] {
        for path in paths {
            let loaded = load(path)?;
            if loaded.plan_sha256 != plan_sha256 {
                return Err(format!(
                    "{} was captured from a different plan than the one supplied",
                    path.display()
                ));
            }
            if loaded.capture.slot.arm != arm {
                return Err(format!(
                    "{} is not an {} arm capture",
                    path.display(),
                    arm_name(arm)
                ));
            }
            match plan.slots.get(loaded.capture.slot.index as usize) {
                Some(expected) if expected == &loaded.capture.slot => {}
                _ => {
                    return Err(format!(
                        "{} does not realize the plan slot it declares",
                        path.display()
                    ));
                }
            }
            if !slots.insert(loaded.capture.slot.index) {
                return Err("a plan slot was supplied more than once".into());
            }
            if !digests.insert(loaded.digest.clone()) {
                return Err("repeated capture evidence is not an independent acquisition".into());
            }
            match &collector {
                Some(sha) if sha != &loaded.capture.collector.sha256 => {
                    return Err("captures did not come from one collector binary".into());
                }
                None => collector = Some(loaded.capture.collector.sha256.clone()),
                _ => {}
            }
            let inspection = loaded.inspection();
            report.outcome = report.outcome.max(inspection.outcome);
            report.acquisitions.push(inspection);
            arms[arm_index(arm)].push(loaded);
        }
    }
    if slots.len() != plan.slots.len() {
        report.outcome = report.outcome.max(Outcome::Inconclusive);
        report
            .reasons
            .push("every declared slot, including warmups and failed captures, is required".into());
    }
    reference_binding(&arms, &mut report);
    exposure(&arms, &mut report);
    for gate in &plan.gates {
        let evaluated = gate_outcome(gate, &arms);
        report.outcome = report.outcome.max(evaluated.outcome);
        report.gates.push(evaluated);
    }
    Ok(report)
}

// -------------------------------------------------------------------------
// CLI
// -------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum Command {
    /// Capture one declared finite System One acquisition against a running server.
    Capture(CaptureArgs),
    /// Replay retained request/response bytes offline and print decision metrics.
    Inspect {
        /// Capture directory written by `systemone capture`.
        capture: PathBuf,
    },
    /// Exact prospective A/B/A2 envelope over latency and decision rates.
    Compare {
        /// Prospective plan; every capture must bind this exact plan.
        #[arg(long)]
        plan: PathBuf,
        /// Unchanged reference arm captures.
        #[arg(long, num_args = 1..)]
        a: Vec<PathBuf>,
        /// Candidate arm captures.
        #[arg(long, num_args = 1..)]
        b: Vec<PathBuf>,
        /// Unchanged reference arm captures repeated after the candidate.
        #[arg(long, num_args = 1..)]
        a2: Vec<PathBuf>,
    },
}

pub fn execute(command: Command) -> Result<u8> {
    match command {
        Command::Capture(args) => {
            let inspection = capture(&args)?;
            crate::print_json(&inspection)?;
            Ok(inspection.outcome.exit())
        }
        Command::Inspect { capture } => {
            let inspection = inspect(&capture)?;
            crate::print_json(&inspection)?;
            Ok(inspection.outcome.exit())
        }
        Command::Compare { plan, a, b, a2 } => {
            let comparison = compare(&plan, &a, &b, &a2)?;
            crate::print_json(&comparison)?;
            Ok(comparison.outcome.exit())
        }
    }
}

#[cfg(test)]
#[path = "../tests/support/systemone_contract.rs"]
mod tests;
