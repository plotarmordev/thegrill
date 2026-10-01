use crate::{evidence, metrics, model::*, run, selection, wire};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ACQUISITIONS: usize = 8;
const CRITICAL: f64 = 2.364624251;
const METRIC_CONTRACT: &str = "generated-text-arrival-v2";
// Cross-host captures are ordered by each client's clock; the gap absorbs ordinary clock skew.
const DEPLOYMENT_GAP_MS: u64 = 60_000;
const ASSUMPTIONS: &str = "Model-based interval assumes stable, independent acquisition-level variation and an adequate log-scale mean model. Resetting a client does not prove independence. Positive autocorrelation can make the interval narrower than justified, increasing false directional conclusions. Sequential captures cannot remove time or carryover confounding; no causal attribution, equivalence, noninferiority, or guaranteed precision/power is established.";
const UNAVAILABLE_SCOPE: &str =
    "unavailable: capture workload identity was not verified for this report";
const UNCHANGED_SCOPE: &str = "Unchanged-deployment control: any directional result is an observed capture-period shift requiring repeatability investigation, not evidence of a serving-change effect. An inconclusive result does not establish equality or repeatability.";
const SELECTED_SCOPE: &str = "Explicit selected workload: descriptive per-cell/acquisition observations only. No pooled improvement, C1 inference, capacity, tail-SLO, equivalence or causal claim; concurrent lanes are correlated, not independent acquisitions.";
const OVERHEAD_STOP: &str =
    "whole-capture time allowance exhausted during setup or publication overhead";
const DEPLOYMENT_SCOPE: &str = "Whole-deployment comparison: request throughput at C1 including prefill. Hardware, runtime, model build, settings, endpoint and client placement are confounded; no single-factor attribution, such as to a quantization format, is supported. Only the baseline deployment has a drift control; the candidate deployment has none. With same-host placement on different hosts, capture order relies on each client's clock. The synthetic count prompt is highly predictable, so speculative decoding on either side dominates it.";
const PENDING: &str = "Verdict pending: after this candidate, capture the unchanged reference with `check <baseline> --change none`, then run `compare <baseline> <candidate> --reference <reference>`.";

#[derive(clap::Args)]
pub struct BaselineOptions {
    #[arg(long)]
    pub endpoint: String,
    #[arg(long)]
    pub model: String,
    #[arg(long)]
    pub deployment: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long)]
    pub local_http: bool,
    #[arg(long)]
    pub auth_env: Option<String>,
    /// Pin an explicit bounded workload and descriptive scope; check inherits it.
    #[arg(long)]
    pub selection: Option<PathBuf>,
    /// Built-in workload to capture; the capture records its name.
    #[arg(long, value_enum, default_value_t = Builtin::BaselineV2, conflicts_with = "selection")]
    pub workload: Builtin,
    /// Where this client runs relative to the server; deployment comparisons require equal placement.
    #[arg(long, value_enum, required = true)]
    pub client_placement: Option<Placement>,
    /// Whole-capture ceiling, including warmups; per-request limits remain fixed.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub seconds: u64,
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct CheckOptions {
    pub baseline: PathBuf,
    #[arg(long)]
    pub deployment: PathBuf,
    #[arg(long, value_enum)]
    pub change: Change,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub seconds: u64,
    #[arg(long)]
    pub json: bool,
    /// With --change deployment only, like --model, --auth-env and --local-http; unset ones
    /// inherit the baseline's.
    #[arg(long)]
    pub endpoint: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub auth_env: Option<String>,
    #[arg(long, requires = "endpoint")]
    pub local_http: bool,
    /// With --change deployment only, and required there; it must equal the baseline's.
    #[arg(long, value_enum, required_if_eq("change", "deployment"))]
    pub client_placement: Option<Placement>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Builtin {
    BaselineV1,
    BaselineV2,
    PortableV1,
}

impl Builtin {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::BaselineV1 => include_bytes!("../examples/baseline-v1.json"),
            Self::BaselineV2 => include_bytes!("../examples/baseline-v2.json"),
            Self::PortableV1 => include_bytes!("../examples/portable-v1.json"),
        }
    }
    fn scope(self) -> &'static str {
        match self {
            Self::BaselineV1 | Self::BaselineV2 => {
                "One short synthetic structured C1/exact400 workload; not general concurrency, long-context, model quality, tail SLOs, or full serving qualification. Deployment values are operator declarations, not server attestation. A settings fingerprint cannot prove only one internal knob changed."
            }
            Self::PortableV1 => {
                "One short synthetic count C1 workload with 400 output tokens verified by cap-reached; not general concurrency, long-context, model quality, tail SLOs, or full serving qualification. Deployment values are operator declarations, not server attestation. A settings fingerprint cannot prove only one internal knob changed."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    /// The client runs on the serving host.
    SameHost,
    /// The client reaches the server over a network.
    Network,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Collector {
    pub version: String,
    pub source_commit: String,
    pub target: String,
}

impl Collector {
    fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            source_commit: env!("GRILL_PERF_SOURCE").into(),
            target: env!("GRILL_PERF_TARGET").into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    #[value(name = "model_revision")]
    ModelRevision,
    Runtime,
    Hardware,
    Settings,
    Deployment,
    #[serde(rename = "none")]
    #[value(name = "none")]
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CaptureStatus {
    Complete,
    Incomplete,
    Invalid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Acquisition {
    directory: String,
    plan_sha256: Option<String>,
    evidence_sha256: Option<String>,
    started_unix_ms: Option<u64>,
    finished_unix_ms: Option<u64>,
    status: Option<String>,
    median_achieved_completion_tokens_per_second: Option<f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    version: u32,
    kind: String,
    metric_contract: String,
    collector_sha256: String,
    workload_sha256: String,
    deployment_sha256: String,
    endpoint: String,
    model: String,
    auth_env: Option<String>,
    local_http: bool,
    seconds: u64,
    request_ceiling: usize,
    output_token_ceiling: usize,
    baseline_sha256: Option<String>,
    change: Option<Change>,
    status: CaptureStatus,
    stop_reason: Option<String>,
    acquisitions: Vec<Acquisition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    collector: Option<Collector>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_placement: Option<Placement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workload: Option<Builtin>,
}

struct Verified {
    manifest: Capture,
    sha256: String,
    deployment: Deployment,
    observations: Vec<f64>,
    accounting: Accounting,
    first_failure: Option<Failure>,
    selected: Option<SelectedReport>,
    complete_acquisitions: usize,
    median_prompt_tokens: Option<f64>,
}

#[derive(Clone, Serialize)]
pub struct Accounting {
    pub dispatched_requests: usize,
    pub complete_requests: usize,
    pub known_reported_completion_tokens: u64,
    pub missing_completion_usage_requests: usize,
    pub reported_completion_tokens: Option<u64>,
    pub request_ceiling: usize,
    pub output_token_ceiling: usize,
    pub allowance_seconds: u64,
    pub minimum_full_capture_tokens_per_second: Option<f64>,
}

#[derive(Clone, Serialize)]
pub struct Failure {
    pub acquisition: usize,
    pub acquisition_status: String,
    pub wave: WaveSpec,
    pub lane: u32,
    pub status: Status,
    pub http_status: Option<u16>,
    pub usage: Usage,
    pub expected_completion_tokens: u32,
    pub detail: String,
    pub eligibility_errors: Vec<String>,
    pub receipt_path: PathBuf,
    pub response_path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    Improved,
    Regressed,
    Inconclusive,
    Descriptive,
    /// A complete deployment candidate awaiting its unchanged reference.
    Pending,
    Invalid,
}

#[derive(Serialize)]
pub struct DeploymentSide {
    pub endpoint: String,
    pub model: String,
    pub declaration: Deployment,
    pub collector: Collector,
    pub collector_sha256: String,
    pub client_placement: Placement,
    /// Prefill work differs when servers template the same prompt differently.
    pub median_prompt_tokens: Option<f64>,
}

#[derive(Serialize)]
pub struct Interval {
    pub result: Outcome,
    pub observed_change_percent: Option<f64>,
    pub model_based_interval_percent: Option<[f64; 2]>,
}

#[derive(Serialize)]
pub struct DeploymentReport {
    pub baseline: DeploymentSide,
    pub candidate: DeploymentSide,
    pub reference_path: Option<PathBuf>,
    pub reference_capture_sha256: Option<String>,
    pub candidate_against_reference: Option<Interval>,
    pub reference_against_baseline: Option<Interval>,
}

#[derive(Serialize)]
pub struct AcquisitionReport {
    pub acquisition: usize,
    pub status: String,
    pub cells: Vec<evidence::CellSummary>,
    pub waves: Vec<Option<Wave>>,
    pub summary: run::Summary,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureTiming {
    pub version: u32,
    pub capture_sha256: String,
    pub elapsed_through_capture_publication_us: u64,
}

#[derive(Serialize)]
pub struct SelectedReport {
    pub manifest: selection::Manifest,
    pub manifest_sha256: String,
    pub request: RequestSettings,
    pub limits: Limits,
    pub cells: Vec<Cell>,
    pub baseline_acquisitions: Vec<AcquisitionReport>,
    pub candidate_acquisitions: Vec<AcquisitionReport>,
    pub baseline_timing: CaptureTiming,
    pub candidate_timing: Option<CaptureTiming>,
}

#[derive(Serialize)]
pub struct Report {
    pub version: u32,
    pub kind: &'static str,
    pub result: Outcome,
    pub result_scope: &'static str,
    pub model: Option<String>,
    pub baseline_path: Option<PathBuf>,
    pub candidate_path: Option<PathBuf>,
    pub declared_change: Option<Change>,
    pub baseline_accounting: Option<Accounting>,
    pub candidate_accounting: Option<Accounting>,
    pub baseline_ready: bool,
    pub baseline_complete_acquisitions: usize,
    pub candidate_complete_acquisitions: usize,
    pub baseline_acquisition_medians: Vec<f64>,
    pub candidate_acquisition_medians: Vec<f64>,
    pub observed_change_percent: Option<f64>,
    pub model_based_interval_percent: Option<[f64; 2]>,
    pub model_based_confidence_level: Option<f64>,
    pub log_mean_difference: Option<f64>,
    pub heteroscedastic_standard_error: Option<f64>,
    pub critical_value: f64,
    pub conservative_degrees_of_freedom: usize,
    pub baseline_capture_sha256: Option<String>,
    pub candidate_capture_sha256: Option<String>,
    pub report_path: PathBuf,
    pub reasons: Vec<String>,
    pub stop_reason: Option<String>,
    pub first_failure: Option<Failure>,
    pub assumptions: &'static str,
    pub scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<SelectedReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<DeploymentReport>,
}

impl Report {
    fn new(path: PathBuf) -> Self {
        Self {
            version: 1,
            kind: "performance-capture-comparison-v1",
            result: Outcome::Inconclusive,
            result_scope: "between these capture periods; not causal attribution",
            model: None,
            baseline_path: None,
            candidate_path: None,
            declared_change: None,
            baseline_accounting: None,
            candidate_accounting: None,
            baseline_ready: false,
            baseline_complete_acquisitions: 0,
            candidate_complete_acquisitions: 0,
            baseline_acquisition_medians: Vec::new(),
            candidate_acquisition_medians: Vec::new(),
            observed_change_percent: None,
            model_based_interval_percent: None,
            model_based_confidence_level: None,
            log_mean_difference: None,
            heteroscedastic_standard_error: None,
            critical_value: CRITICAL,
            conservative_degrees_of_freedom: ACQUISITIONS - 1,
            baseline_capture_sha256: None,
            candidate_capture_sha256: None,
            report_path: path,
            reasons: Vec::new(),
            stop_reason: None,
            first_failure: None,
            assumptions: ASSUMPTIONS,
            scope: UNAVAILABLE_SCOPE,
            selected: None,
            deployment: None,
        }
    }

    pub fn exit(&self) -> u8 {
        match self.result {
            Outcome::Invalid => 1,
            Outcome::Improved | Outcome::Descriptive | Outcome::Pending => 0,
            Outcome::Inconclusive if self.baseline_ready => 0,
            Outcome::Regressed | Outcome::Inconclusive => 2,
        }
    }
}

fn declaration(bytes: &[u8]) -> Result<Deployment> {
    let value: Deployment = serde_json::from_slice(bytes)
        .map_err(|e| format!("invalid deployment declaration: {e}"))?;
    for (field, declared) in [
        ("model_revision", &value.model_revision),
        ("runtime", &value.runtime),
        ("hardware", &value.hardware),
        ("settings", &value.settings),
    ] {
        if declared.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return Err(format!(
                "deployment declaration requires nonempty {field}; supply its actual deployment identifier in --deployment, not a guessed value"
            ));
        }
    }
    value.validate()?;
    Ok(value)
}

fn declared_change(before: &Deployment, after: &Deployment, selected: Change) -> Result<()> {
    if selected == Change::Deployment {
        if before == after {
            return Err("declaration mismatch: --change deployment requires at least one deployment field to differ".into());
        }
        return Ok(());
    }
    for (field, a, b) in [
        (
            Change::ModelRevision,
            &before.model_revision,
            &after.model_revision,
        ),
        (Change::Runtime, &before.runtime, &after.runtime),
        (Change::Hardware, &before.hardware, &after.hardware),
        (Change::Settings, &before.settings, &after.settings),
    ] {
        if (a != b) != (field == selected) {
            if selected == Change::Unchanged {
                return Err(format!(
                    "declaration mismatch: --change none requires every deployment field to match; {field:?} differs"
                ));
            }
            return Err(format!(
                "declaration mismatch: exactly the selected {selected:?} field must differ; {field:?} violates that rule"
            ));
        }
    }
    Ok(())
}

fn directory(index: usize) -> String {
    format!("acquisition-{index:02}")
}

fn inspect(loaded: &evidence::Loaded, selected: bool) -> Result<(CaptureStatus, Option<f64>)> {
    if loaded.history.count != 1 || loaded.history.open {
        return Err("acquisition must have exactly one settled native execution session; no continuation or missing receipts".into());
    }
    let status = loaded
        .history
        .last_status
        .as_deref()
        .ok_or("missing acquisition outcome")?;
    let mut interrupted = false;
    for wave in loaded.waves.iter().flatten() {
        if wave.attempts.iter().any(|attempt| {
            attempt.status == Status::Complete
                && attempt
                    .sequence
                    .as_ref()
                    .is_some_and(|check| !crate::sequence::passed(check))
        }) {
            return Ok((CaptureStatus::Invalid, None));
        }
        if !wave.eligible {
            for attempt in &wave.attempts {
                if run::irrevocable_invalid(attempt) {
                    return Ok((CaptureStatus::Invalid, None));
                }
                if attempt.status == Status::Interrupted {
                    interrupted = true;
                } else if attempt.status != Status::Complete
                    || !attempt.eligibility_errors.is_empty()
                {
                    return Ok((CaptureStatus::Invalid, None));
                }
            }
            if !interrupted {
                return Ok((CaptureStatus::Invalid, None));
            }
        }
    }
    if status == "completed" && !interrupted {
        if selected {
            return Ok((CaptureStatus::Complete, None));
        }
        let summaries = evidence::summarize(loaded);
        let median = summaries
            .first()
            .and_then(|s| s.median_achieved_completion_tokens_per_second)
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or("complete acquisition has no positive finite throughput median")?;
        return Ok((CaptureStatus::Complete, Some(median)));
    }
    if status == "interrupted" || status == "budget-exhausted" || status == "paused" {
        Ok((CaptureStatus::Incomplete, None))
    } else {
        Ok((CaptureStatus::Invalid, None))
    }
}

fn selected_identity(capture_sha256: &str, timing: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"grill-perf-selected-capture-v2\0");
    hash.update(capture_sha256.as_bytes());
    hash.update(timing);
    evidence::hex(&hash.finalize())
}

#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
fn load(root: &Path) -> Result<Verified> {
    evidence::directory(root)?;
    let bytes = evidence::read(&root.join("capture.json"), FILE_CAP)?;
    let capture_sha256 = evidence::digest(&bytes);
    let mut identity = capture_sha256.clone();
    let mut manifest: Capture =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid capture manifest: {e}"))?;
    let v3 = manifest.version == 3;
    if !matches!(
        (
            manifest.version,
            manifest.kind.as_str(),
            manifest.selection_sha256.is_some()
        ),
        (1, "performance-capture-v1", false)
            | (2, "performance-capture-v2", true)
            | (3, "performance-capture-v3", _)
    ) || v3 != manifest.collector.is_some()
        || v3 != manifest.client_placement.is_some()
        || manifest.workload.is_some() != (v3 && manifest.selection_sha256.is_none())
        || manifest.metric_contract != METRIC_CONTRACT
        || manifest.acquisitions.len() != ACQUISITIONS
        || !(1..=3600).contains(&manifest.seconds)
        || manifest.baseline_sha256.is_some() != manifest.change.is_some()
        || manifest.collector_sha256.len() != 64
        || !manifest
            .collector_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err("unsupported or incompatible capture contract".into());
    }
    let workload = evidence::read(&root.join("workload.json"), FILE_CAP)?;
    if evidence::digest(&workload) != manifest.workload_sha256 {
        return Err("capture workload source hash mismatch".into());
    }
    let mut selected = if let Some(hash) = &manifest.selection_sha256 {
        let bytes = evidence::read(&root.join("selection.json"), selection::CAP)?;
        if evidence::digest(&bytes) != *hash {
            return Err("capture selection manifest hash mismatch".into());
        }
        let selection = selection::parse(&bytes, &workload)?;
        let workload = selection::workload(&workload)?;
        let timing_bytes = evidence::read(&root.join("capture-timing.json"), FILE_CAP)?;
        let timing: CaptureTiming = serde_json::from_slice(&timing_bytes)
            .map_err(|e| format!("invalid capture timing receipt: {e}"))?;
        if timing.version != 1 || timing.capture_sha256 != capture_sha256 {
            return Err("capture timing receipt does not bind this capture".into());
        }
        identity = selected_identity(&capture_sha256, &timing_bytes);
        Some(SelectedReport {
            manifest: selection,
            manifest_sha256: hash.clone(),
            request: workload.request,
            limits: workload.limits,
            cells: workload.cells,
            baseline_acquisitions: Vec::new(),
            candidate_acquisitions: Vec::new(),
            baseline_timing: timing,
            candidate_timing: None,
        })
    } else {
        // Only v3 names its built-in; v1 always used baseline-v1.
        if workload != manifest.workload.unwrap_or(Builtin::BaselineV1).bytes() {
            return Err("capture workload differs from its built-in workload".into());
        }
        None
    };
    let admitted_workload = selection::workload(&workload)?;
    let (warmup, measured, tokens) = selection::budgets(&admitted_workload, ACQUISITIONS)?;
    if manifest.request_ceiling != warmup + measured || manifest.output_token_ceiling != tokens {
        return Err("capture ceilings differ from the pinned workload schedule".into());
    }
    let deployment_bytes = evidence::read(&root.join("deployment.json"), 32 * 1024)?;
    if evidence::digest(&deployment_bytes) != manifest.deployment_sha256 {
        return Err("capture deployment bytes do not match their hash".into());
    }
    let deployment = declaration(&deployment_bytes)?;
    wire::endpoint(&manifest.endpoint, manifest.local_http)?;
    let mut observations = Vec::with_capacity(ACQUISITIONS);
    let mut first_failure = None;
    let mut complete_acquisitions = 0;
    let mut stopped = false;
    let mut observed_status = CaptureStatus::Complete;
    let mut identities = std::collections::HashSet::new();
    let mut lineages = std::collections::HashSet::new();
    let mut previous_finish = None;
    let mut prompt_tokens = Vec::new();
    let mut accounting = Accounting {
        dispatched_requests: 0,
        complete_requests: 0,
        known_reported_completion_tokens: 0,
        missing_completion_usage_requests: 0,
        reported_completion_tokens: Some(0),
        request_ceiling: manifest.request_ceiling,
        output_token_ceiling: manifest.output_token_ceiling,
        allowance_seconds: manifest.seconds,
        // A known output count per request bounds the rate needed to finish in the allowance.
        minimum_full_capture_tokens_per_second: ((warmup == 0
            || admitted_workload
                .request
                .effective_output(Phase::Warmup)
                .mode
                != OutputMode::Cap)
            && (measured == 0
                || admitted_workload
                    .request
                    .effective_output(Phase::Measured)
                    .mode
                    != OutputMode::Cap))
            .then_some(manifest.output_token_ceiling as f64 / manifest.seconds as f64),
    };
    for (index, acquisition) in manifest.acquisitions.iter().enumerate() {
        if acquisition.directory != directory(index) {
            return Err("acquisition inventory is not the fixed ordered schedule".into());
        }
        let path = root.join(&acquisition.directory);
        evidence::directory(&path)?;
        if acquisition.plan_sha256.is_none() {
            if acquisition.evidence_sha256.is_some()
                || acquisition.status.is_some()
                || acquisition
                    .median_achieved_completion_tokens_per_second
                    .is_some()
                || acquisition.started_unix_ms.is_some()
                || acquisition.finished_unix_ms.is_some()
                || std::fs::read_dir(&path)
                    .map_err(|e| e.to_string())?
                    .next()
                    .is_some()
            {
                return Err(
                    "unstarted acquisition contains evidence or a claimed observation".into(),
                );
            }
            stopped = true;
            if observed_status == CaptureStatus::Complete {
                observed_status = CaptureStatus::Incomplete;
            }
            continue;
        }
        if stopped {
            return Err("capture continued after an incomplete or invalid acquisition".into());
        }
        let loaded = evidence::load(&path)?;
        let plan = &loaded.plan;
        if Some(&loaded.plan_sha256) != acquisition.plan_sha256.as_ref()
            || Some(&loaded.evidence_sha256) != acquisition.evidence_sha256.as_ref()
            || loaded.history.last_status != acquisition.status
            || plan.version != 3
            || plan.metric_contract.as_deref() != Some(METRIC_CONTRACT)
            || plan.collector_sha256 != manifest.collector_sha256
            || manifest
                .collector
                .as_ref()
                .is_some_and(|c| c.version != plan.tool_version)
            || plan.source_sha256 != manifest.workload_sha256
            || plan.workload != admitted_workload
            || plan.deployment.as_ref() != Some(&deployment)
            || plan.endpoint != manifest.endpoint
            || plan.model != manifest.model
            || plan.auth_env != manifest.auth_env
            || plan.local_http != manifest.local_http
            || plan.metrics.is_some()
            || plan.policy_sha256.is_some()
        {
            return Err("native acquisition identity, contract, or evidence hash differs from capture manifest".into());
        }
        if !identities.insert(loaded.evidence_sha256.clone())
            || !lineages.insert(loaded.lineage_sha256.clone())
        {
            return Err(
                "duplicate native acquisition evidence cannot count as separate observations"
                    .into(),
            );
        }
        let started = acquisition
            .started_unix_ms
            .ok_or("missing acquisition start")?;
        let finished = acquisition
            .finished_unix_ms
            .ok_or("missing acquisition finish")?;
        if started != plan.started_unix_ms
            || finished < started
            || previous_finish.is_some_and(|previous| started < previous)
        {
            return Err("acquisition time declarations overlap, go backwards, or differ from native plan start".into());
        }
        // Wall-clock declarations have millisecond resolution; equal adjacent boundaries are valid.
        let window_us = finished
            .checked_sub(started)
            .and_then(|n| n.checked_add(1))
            .and_then(|n| n.checked_mul(1000))
            .ok_or("acquisition time range overflows")?;
        let waves_us = loaded
            .waves
            .iter()
            .flatten()
            .try_fold(0u64, |total, wave| total.checked_add(wave.elapsed_us))
            .ok_or("acquisition wave durations overflow")?;
        if waves_us > window_us {
            return Err(format!(
                "native wave durations exceed the declared acquisition time window ({}: waves_us={waves_us}, window_us={window_us}; durations in microseconds; window includes +1ms allowance for millisecond-resolution declarations)",
                acquisition.directory
            ));
        }
        previous_finish = Some(finished);
        if first_failure.is_none() {
            first_failure = loaded.waves.iter().flatten().find_map(|wave| {
                wave.attempts
                    .iter()
                    .find(|attempt| {
                        attempt.status != Status::Complete
                            || !attempt.eligibility_errors.is_empty()
                            || attempt
                                .sequence
                                .as_ref()
                                .is_some_and(|check| !crate::sequence::passed(check))
                    })
                    .map(|attempt| Failure {
                        acquisition: index,
                        acquisition_status: loaded.history.last_status.clone().unwrap_or_default(),
                        wave: wave.spec.clone(),
                        lane: attempt.lane,
                        status: attempt.status.clone(),
                        http_status: attempt.http_status,
                        usage: attempt.usage.clone(),
                        expected_completion_tokens: plan
                            .workload
                            .request
                            .effective_output(wave.spec.phase)
                            .tokens,
                        detail: attempt.detail.clone(),
                        eligibility_errors: attempt.eligibility_errors.clone(),
                        receipt_path: evidence::wave_dir(&path, wave.spec.index).join("wave.json"),
                        response_path: evidence::wave_dir(&path, wave.spec.index)
                            .join(format!("response-{:04}.bin", attempt.lane)),
                    })
            });
        }
        for attempt in loaded
            .waves
            .iter()
            .flatten()
            .flat_map(|wave| &wave.attempts)
        {
            if attempt.dispatched {
                accounting.dispatched_requests += 1;
                accounting.complete_requests += usize::from(attempt.status == Status::Complete);
                if let Some(tokens) = attempt.usage.completion_tokens {
                    accounting.known_reported_completion_tokens = accounting
                        .known_reported_completion_tokens
                        .checked_add(tokens)
                        .ok_or("reported completion token sum overflows")?;
                } else {
                    accounting.missing_completion_usage_requests += 1;
                }
            }
        }
        prompt_tokens.extend(
            loaded
                .waves
                .iter()
                .flatten()
                .filter(|wave| wave.spec.phase == Phase::Measured)
                .flat_map(|wave| &wave.attempts)
                .filter_map(|attempt| attempt.usage.prompt_tokens)
                .map(|tokens| tokens as f64),
        );
        let (status, median) = inspect(&loaded, selected.is_some())?;
        complete_acquisitions += usize::from(status == CaptureStatus::Complete);
        if median != acquisition.median_achieved_completion_tokens_per_second {
            return Err("acquisition median differs from verified native evidence".into());
        }
        if let Some(value) = median {
            observations.push(value);
        }
        if status != CaptureStatus::Complete {
            observed_status = status;
            stopped = true;
        }
        if let Some(selected) = &mut selected {
            selected.baseline_acquisitions.push(AcquisitionReport {
                acquisition: index,
                status: loaded.history.last_status.clone().unwrap_or_default(),
                cells: evidence::summarize(&loaded),
                summary: serde_json::from_slice(&evidence::read(&path.join("run.json"), FILE_CAP)?)
                    .map_err(|e| format!("invalid native summary: {e}"))?,
                waves: loaded.waves,
            });
        }
    }
    // A local publication/preparation error may occur before any native receipt exists.
    if manifest.status != observed_status
        && !(manifest.status == CaptureStatus::Invalid && manifest.stop_reason.is_some())
        && !(manifest.selection_sha256.is_some()
            && manifest.status == CaptureStatus::Incomplete
            && manifest.stop_reason.as_deref() == Some(OVERHEAD_STOP))
    {
        return Err("capture completion claim contradicts acquisition coverage".into());
    }
    if let Some(selected) = &selected {
        let lower = selected
            .baseline_acquisitions
            .iter()
            .try_fold(0u64, |total, acquisition| {
                let wave_time = acquisition
                    .waves
                    .iter()
                    .flatten()
                    .try_fold(0u64, |sum, wave| sum.checked_add(wave.elapsed_us))?;
                total
                    .checked_add(wave_time)?
                    .checked_add(acquisition.summary.wave_preparation_us)?
                    .checked_add(acquisition.summary.wave_publication_us)
            })
            .ok_or("capture native elapsed accounting overflows")?;
        if selected
            .baseline_timing
            .elapsed_through_capture_publication_us
            < lower
        {
            return Err(
                "capture elapsed observation is below retained native timing bounds".into(),
            );
        }
    }
    if selected.as_ref().is_some_and(|selected| {
        selected
            .baseline_timing
            .elapsed_through_capture_publication_us
            >= manifest.seconds * 1_000_000
    }) && manifest.status == CaptureStatus::Complete
    {
        manifest.status = CaptureStatus::Incomplete;
        manifest.stop_reason = Some(OVERHEAD_STOP.into());
    }
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("acquisition-")
            && !manifest.acquisitions.iter().any(|a| a.directory == name)
        {
            return Err("acquisition outside the fixed capture schedule".into());
        }
    }
    accounting.reported_completion_tokens = (accounting.missing_completion_usage_requests == 0)
        .then_some(accounting.known_reported_completion_tokens);
    Ok(Verified {
        manifest,
        sha256: identity,
        deployment,
        observations,
        accounting,
        first_failure,
        selected,
        complete_acquisitions,
        median_prompt_tokens: evidence::median(prompt_tokens),
    })
}

fn collect(
    root: &Path,
    mut manifest: Capture,
    deployment: &[u8],
    workload: &[u8],
    selection: Option<&[u8]>,
    deadline: Instant,
) -> Result<()> {
    let started = deadline - Duration::from_secs(manifest.seconds);
    evidence::write(&root.join("workload.json"), workload)?;
    if let Some(bytes) = selection {
        evidence::write(&root.join("selection.json"), bytes)?;
    }
    evidence::write(&root.join("deployment.json"), deployment)?;
    for index in 0..ACQUISITIONS {
        if Instant::now() >= deadline {
            manifest.status = CaptureStatus::Incomplete;
            manifest.stop_reason =
                Some("whole-capture time allowance exhausted before next acquisition".into());
            break;
        }
        let options = run::Options {
            common: run::CommonArgs {
                workload: root.join("workload.json"),
                endpoint: manifest.endpoint.clone(),
                model: manifest.model.clone(),
                deployment: Some(root.join("deployment.json")),
                policy: None,
                metrics_url: None,
                metrics_version: 1,
                metrics_auth_env: None,
                metrics_isolation: None,
                auth_env: manifest.auth_env.clone(),
                local_http: manifest.local_http,
            },
            out: root.join(directory(index)),
            json: true,
        };
        let outcome = run::execute_bounded(&options, deadline);
        let finished_unix_ms = metrics::unix_ms_checked()?;
        match evidence::load(&options.out) {
            Ok(loaded) => {
                let (status, median) = match inspect(&loaded, manifest.selection_sha256.is_some()) {
                    Ok(observation) => observation,
                    Err(error) => {
                        manifest.stop_reason = Some(error);
                        (CaptureStatus::Invalid, None)
                    }
                };
                manifest.acquisitions[index] = Acquisition {
                    directory: directory(index),
                    plan_sha256: Some(loaded.plan_sha256),
                    evidence_sha256: Some(loaded.evidence_sha256),
                    started_unix_ms: Some(loaded.plan.started_unix_ms),
                    finished_unix_ms: Some(finished_unix_ms),
                    status: loaded.history.last_status,
                    median_achieved_completion_tokens_per_second: median,
                };
                manifest.status = status;
                if let Err(error) = outcome {
                    manifest.status = CaptureStatus::Invalid;
                    manifest.stop_reason = Some(error);
                }
                if manifest.status != CaptureStatus::Complete {
                    if manifest.stop_reason.is_none() {
                        manifest.stop_reason = manifest.acquisitions[index].status.clone();
                    }
                    break;
                }
            }
            Err(error) => {
                manifest.status = CaptureStatus::Invalid;
                manifest.stop_reason = Some(outcome.err().unwrap_or(error));
                break;
            }
        }
    }
    for acquisition in &manifest.acquisitions {
        let path = root.join(&acquisition.directory);
        if !crate::lifecycle::exists(&path)? {
            evidence::fresh(&path)?;
        }
    }
    finalize_capture(root, manifest, || started.elapsed())
}

fn finalize_capture(
    root: &Path,
    mut manifest: Capture,
    mut elapsed: impl FnMut() -> Duration,
) -> Result<()> {
    if manifest.selection_sha256.is_some()
        && manifest.status == CaptureStatus::Complete
        && elapsed() >= Duration::from_secs(manifest.seconds)
    {
        manifest.status = CaptureStatus::Incomplete;
        manifest.stop_reason = Some(OVERHEAD_STOP.into());
    }
    evidence::publish(root, "capture.json", &manifest)?;
    if manifest.selection_sha256.is_some() {
        let elapsed =
            u64::try_from(elapsed().as_micros()).map_err(|_| "capture elapsed time overflows")?;
        evidence::publish(
            root,
            "capture-timing.json",
            &CaptureTiming {
                version: 1,
                capture_sha256: evidence::digest(&evidence::read(
                    &root.join("capture.json"),
                    FILE_CAP,
                )?),
                elapsed_through_capture_publication_us: elapsed,
            },
        )?;
    }
    Ok(())
}

fn manifest(
    options: &BaselineOptions,
    version: u32,
    deployment: &[u8],
    collector_sha256: String,
    link: Option<(String, Change)>,
    source: &[u8],
    selected: Option<&[u8]>,
) -> Result<Capture> {
    let (baseline_sha256, change) = match link {
        Some((hash, change)) => (Some(hash), Some(change)),
        None => (None, None),
    };
    let workload = selection::workload(source)?;
    let (warmup, measured, tokens) = selection::budgets(&workload, ACQUISITIONS)?;
    Ok(Capture {
        version,
        kind: format!("performance-capture-v{version}"),
        metric_contract: METRIC_CONTRACT.into(),
        collector_sha256,
        workload_sha256: evidence::digest(source),
        deployment_sha256: evidence::digest(deployment),
        endpoint: wire::endpoint(&options.endpoint, options.local_http)?.to_string(),
        model: options.model.clone(),
        auth_env: options.auth_env.clone(),
        local_http: options.local_http,
        seconds: options.seconds,
        request_ceiling: warmup + measured,
        output_token_ceiling: tokens,
        baseline_sha256,
        change,
        status: CaptureStatus::Complete,
        stop_reason: None,
        selection_sha256: selected.map(evidence::digest),
        collector: (version == 3).then(Collector::current),
        client_placement: options.client_placement.filter(|_| version == 3),
        workload: (version == 3 && selected.is_none()).then_some(options.workload),
        acquisitions: (0..ACQUISITIONS)
            .map(|index| Acquisition {
                directory: directory(index),
                plan_sha256: None,
                evidence_sha256: None,
                status: None,
                started_unix_ms: None,
                finished_unix_ms: None,
                median_achieved_completion_tokens_per_second: None,
            })
            .collect(),
    })
}

fn preflight(
    options: &BaselineOptions,
    workload: &Workload,
    selected: Option<&selection::Manifest>,
) -> Result<()> {
    if matches!(workload.version, 4..=6) {
        return Err("workloads4..6 require native run/preflight/compare/decide; legacy selected capture is unsupported".into());
    }
    wire::endpoint(&options.endpoint, options.local_http)?;
    wire::credential(options.auth_env.as_deref())?;
    if options.model.is_empty()
        || options.model.len() > 4096
        || options.model.chars().any(char::is_control)
    {
        return Err("model must be a nonempty selector within 4096 bytes".into());
    }
    let (warmup, measured, tokens) = selection::budgets(workload, ACQUISITIONS)?;
    let scope = selected.map_or(options.workload.scope(), |s| s.scope.as_str());
    let controls = serde_json::to_string(&workload.request).map_err(|e| e.to_string())?;
    let mut text = format!(
        "Preflight: workload {}; selection {}; scope: {}\nControls: {}\n",
        workload.name,
        selected.map_or("builtin-default", |s| s.id.as_str()),
        scope,
        controls
    );
    if let Some(selected) = selected {
        text.push_str(&format!(
            "Operation scope: {:?}; source {}; normalized workload {}\n",
            selected.operation_scope, selected.source_sha256, selected.workload_sha256
        ));
    }
    for cell in &workload.cells {
        text.push_str(&format!("Cell {}: case {}; concurrency {}; warmup waves {}; measured trials {} per acquisition\n",
            cell.id, cell.case, cell.concurrency, cell.warmup_trials, cell.trials));
        if workload.version == 2 {
            let spec = WaveSpec {
                index: 0,
                phase: Phase::Measured,
                cell: cell.id.clone(),
                case: Some(cell.case.clone()),
                trial: 0,
                concurrency: cell.concurrency,
                lanes: None,
                acquisition: None,
            };
            let effective = crate::sequence::settings(workload, &spec);
            text.push_str(&format!(
                "  Effective step controls: stream {}; cache {:?}; first-output timing {}\n",
                effective.stream,
                effective.cache,
                if effective.stream {
                    "observed from generated-text SSE arrival"
                } else {
                    "unavailable (nonstreaming)"
                }
            ));
        }
    }
    text.push_str(&format!("{ACQUISITIONS} acquisitions: {warmup} warmup + {measured} measured = {} requests; {tokens} requested output tokens ceiling; {}s whole-capture allowance including setup, verification and publication. Request total {}ms / idle {}ms; response {} bytes / wave buffer {} bytes.\n",
        warmup + measured, options.seconds, workload.limits.total_ms, workload.limits.idle_ms,
        workload.limits.response_bytes, workload.limits.wave_buffer_bytes));
    text.push_str("Order: all warmups before measured cells, declared cell/trial order; admitted lanes settle before publication and the next wave. No retries or replacement acquisitions. Deadline checks stop admission, not guarantee completion; filesystem/OS stalls can outlast the allowance. Backend control support remains operator-qualified, not detected offline.\n");
    eprint!("{text}");
    Ok(())
}

fn selected_report(
    report: &mut Report,
    selected: Option<SelectedReport>,
    builtin: Option<Builtin>,
) {
    if selected.is_some() {
        report.version = 2;
        report.kind = "performance-capture-comparison-v2";
        report.scope = SELECTED_SCOPE;
        report.assumptions = SELECTED_SCOPE;
        report.result_scope = SELECTED_SCOPE;
        report.selected = selected;
    } else {
        report.scope = builtin.unwrap_or(Builtin::BaselineV1).scope();
    }
}

fn publish_report(root: &Path, report: &Report) -> Result<()> {
    evidence::publish(root, "report.json", report)?;
    evidence::write(&root.join("report.txt"), human(report).as_bytes())?;
    evidence::sync(root)
}

pub fn baseline(options: &BaselineOptions) -> Result<Report> {
    let deadline = Instant::now() + Duration::from_secs(options.seconds);
    evidence::fresh(&options.out)?;
    let mut report = Report::new(options.out.join("report.json"));
    report.model = Some(options.model.clone());
    report.baseline_path = Some(options.out.clone());
    let operation = (|| {
        let deployment = evidence::read(&options.deployment, 32 * 1024)?;
        declaration(&deployment)?;
        let selected = options
            .selection
            .as_deref()
            .map(selection::load)
            .transpose()?;
        let source = selected
            .as_ref()
            .map_or(options.workload.bytes(), |s| s.source.as_slice());
        let selection_bytes = selected.as_ref().map(|s| s.bytes.as_slice());
        let default_workload;
        let workload = if let Some(selected) = &selected {
            &selected.workload
        } else {
            default_workload = selection::workload(source)?;
            &default_workload
        };
        preflight(options, workload, selected.as_ref().map(|s| &s.manifest))?;
        let capture = manifest(
            options,
            3,
            &deployment,
            evidence::binary_digest()?,
            None,
            source,
            selection_bytes,
        )?;
        collect(
            &options.out,
            capture,
            &deployment,
            source,
            selection_bytes,
            deadline,
        )?;
        let verified = load(&options.out)?;
        report.first_failure = verified.first_failure;
        report.stop_reason = verified.manifest.stop_reason.clone();
        report.baseline_accounting = Some(verified.accounting);
        report.baseline_complete_acquisitions = verified.complete_acquisitions;
        report.baseline_acquisition_medians = verified.observations;
        report.baseline_capture_sha256 = Some(verified.sha256);
        report.baseline_ready = verified.manifest.status == CaptureStatus::Complete;
        selected_report(&mut report, verified.selected, verified.manifest.workload);
        if verified.manifest.status == CaptureStatus::Invalid {
            report.result = Outcome::Invalid;
        }
        report.reasons.push(if report.baseline_ready {
            "Baseline ready. Run check with --change none and the same deployment declaration for an unchanged control, or change serving state yourself and select the changed declaration field; TheGrill does not change the server.".into()
        } else {
            verified.manifest.stop_reason.unwrap_or_else(|| "Baseline incomplete; no directional comparison is available. Record a new baseline in a fresh directory.".into())
        });
        Ok::<(), String>(())
    })();
    if let Err(error) = operation {
        report.result = Outcome::Invalid;
        report.reasons.push(error);
    }
    publish_report(&options.out, &report)?;
    Ok(report)
}

pub fn check(options: &CheckOptions) -> Result<Report> {
    let deadline = Instant::now() + Duration::from_secs(options.seconds);
    let deployment_check = options.change == Change::Deployment;
    if !deployment_check
        && (options.endpoint.is_some()
            || options.model.is_some()
            || options.auth_env.is_some()
            || options.client_placement.is_some())
    {
        return Err("--endpoint, --model, --auth-env, --local-http and --client-placement apply only to --change deployment; other checks inherit them from the baseline".into());
    }
    if let Ok(baseline) = std::fs::canonicalize(&options.baseline) {
        let parent = options
            .out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if std::fs::canonicalize(parent)
            .map_err(|e| {
                format!(
                    "resolve output parent {}: {e}; create the parent or choose an existing nonsymlink directory",
                    parent.display()
                )
            })?
            .starts_with(&baseline)
        {
            return Err("candidate output must be outside the immutable baseline directory".into());
        }
    }
    evidence::fresh(&options.out)?;
    let mut report = Report::new(options.out.join("report.json"));
    report.baseline_path = Some(options.baseline.clone());
    report.candidate_path = Some(options.out.clone());
    report.declared_change = Some(options.change);
    if options.change == Change::Unchanged {
        report.result_scope = UNCHANGED_SCOPE;
    }
    let operation = (|| {
        let before = load(&options.baseline)?;
        report.first_failure = before.first_failure.clone();
        report.stop_reason = before.manifest.stop_reason.clone();
        report.model = Some(before.manifest.model.clone());
        report.baseline_accounting = Some(before.accounting.clone());
        report.baseline_complete_acquisitions = before.complete_acquisitions;
        report.baseline_acquisition_medians = before.observations;
        report.baseline_capture_sha256 = Some(before.sha256.clone());
        selected_report(&mut report, before.selected, before.manifest.workload);
        if before.manifest.baseline_sha256.is_some() {
            return Err("check requires an original baseline capture, not a previous check".into());
        }
        if before.manifest.status == CaptureStatus::Invalid {
            return Err(
                "baseline evidence is invalid; record a new baseline in a fresh directory".into(),
            );
        }
        let collector_sha256 = evidence::binary_digest()?;
        if !deployment_check && collector_sha256 != before.manifest.collector_sha256 {
            return Err("collector binary differs from baseline; use the original collector or record a new baseline".into());
        }
        let deployment = evidence::read(&options.deployment, 32 * 1024)?;
        let after = declaration(&deployment)?;
        declared_change(&before.deployment, &after, options.change)?;
        if before.manifest.status != CaptureStatus::Complete {
            report.reasons.push("Baseline acquisition coverage is incomplete; no candidate requests dispatched. Record a new complete baseline.".into());
            return Ok(());
        }
        let inherited = inherit(options, &before.manifest);
        let source = evidence::read(&options.baseline.join("workload.json"), FILE_CAP)?;
        let selection_bytes = before
            .manifest
            .selection_sha256
            .as_ref()
            .map(|_| evidence::read(&options.baseline.join("selection.json"), selection::CAP))
            .transpose()?;
        if evidence::digest(&source) != before.manifest.workload_sha256
            || selection_bytes.as_deref().map(evidence::digest) != before.manifest.selection_sha256
        {
            return Err("baseline selection or workload changed during preflight".into());
        }
        let capture_bytes = evidence::read(&options.baseline.join("capture.json"), FILE_CAP)?;
        let current_identity = if before.manifest.selection_sha256.is_some() {
            selected_identity(
                &evidence::digest(&capture_bytes),
                &evidence::read(&options.baseline.join("capture-timing.json"), FILE_CAP)?,
            )
        } else {
            evidence::digest(&capture_bytes)
        };
        if current_identity != before.sha256 {
            return Err("baseline capture or timing identity changed during preflight".into());
        }
        let selected = selection_bytes
            .as_deref()
            .map(|bytes| selection::parse(bytes, &source))
            .transpose()?;
        let capture = manifest(
            &inherited,
            before.manifest.version,
            &deployment,
            collector_sha256,
            Some((before.sha256, options.change)),
            &source,
            selection_bytes.as_deref(),
        )?;
        if deployment_check {
            deployment_pair(&before.manifest, &capture)?;
        }
        preflight(
            &inherited,
            &selection::workload(&source)?,
            selected.as_ref(),
        )?;
        collect(
            &options.out,
            capture,
            &deployment,
            &source,
            selection_bytes.as_deref(),
            deadline,
        )?;
        report = compare(&options.baseline, &options.out, None);
        // A complete deployment candidate has no verdict until its unchanged reference exists.
        if deployment_check
            && report.result == Outcome::Inconclusive
            && report.candidate_complete_acquisitions == ACQUISITIONS
        {
            report.result = Outcome::Pending;
        }
        Ok::<(), String>(())
    })();
    if let Err(error) = operation {
        report.result = Outcome::Invalid;
        report.reasons.push(error);
    }
    publish_report(&options.out, &report)?;
    Ok(report)
}

/// The baseline's request settings, except what a deployment check declares anew.
fn inherit(options: &CheckOptions, before: &Capture) -> BaselineOptions {
    let (endpoint, local_http) = match &options.endpoint {
        Some(endpoint) => (endpoint.clone(), options.local_http),
        None => (before.endpoint.clone(), before.local_http),
    };
    BaselineOptions {
        endpoint,
        model: options
            .model
            .clone()
            .unwrap_or_else(|| before.model.clone()),
        deployment: options.deployment.clone(),
        out: options.out.clone(),
        local_http,
        auth_env: options.auth_env.clone().or_else(|| before.auth_env.clone()),
        selection: None,
        workload: before.workload.unwrap_or(Builtin::BaselineV1),
        client_placement: options.client_placement.or(before.client_placement),
        seconds: options.seconds,
        json: options.json,
    }
}

/// Admits a deployment pair on collector version and recorded source instead of the platform binary.
fn deployment_pair(before: &Capture, after: &Capture) -> Result<()> {
    let (Some(a), Some(b)) = (&before.collector, &after.collector) else {
        return Err("deployment comparison requires a capture v3 baseline; record a new baseline with this collector".into());
    };
    if before.workload.is_none() {
        return Err("deployment comparison requires a built-in workload, not a selection".into());
    }
    if before.client_placement != after.client_placement {
        return Err("client placement differs from the baseline; deployment comparisons require equal placement".into());
    }
    if a.version != b.version || a.source_commit != b.source_commit {
        return Err(format!(
            "collector {} (source {}) differs from baseline collector {} (source {}); build both sides from the same commit",
            b.version, b.source_commit, a.version, a.source_commit
        ));
    }
    if a.source_commit == "unrecorded" {
        return Err("deployment comparison requires collectors built from a recorded source commit; build both from a clean git checkout".into());
    }
    Ok(())
}

pub fn is_capture(path: &Path) -> bool {
    std::fs::symlink_metadata(path.join("capture.json")).is_ok()
        || std::fs::symlink_metadata(path.join("acquisition-00")).is_ok()
        || (std::fs::symlink_metadata(path.join("report.json")).is_ok()
            && std::fs::symlink_metadata(path.join("plan.json")).is_err())
}

/// Verifies a whole capture and returns its fixed acquisitions' run directories, `None` when unstarted.
pub fn acquisition_runs(root: &Path) -> Result<Vec<Option<PathBuf>>> {
    Ok(load(root)?
        .manifest
        .acquisitions
        .iter()
        .map(|a| a.plan_sha256.is_some().then(|| root.join(&a.directory)))
        .collect())
}

pub fn compare(baseline: &Path, candidate: &Path, reference: Option<&Path>) -> Report {
    let mut report = Report::new(candidate.join("report.json"));
    report.baseline_path = Some(baseline.to_owned());
    report.candidate_path = Some(candidate.to_owned());
    let operation = (|| {
        let mut before = load(baseline)?;
        let mut after = load(candidate)?;
        report.model = Some(before.manifest.model.clone());
        report.declared_change = after.manifest.change;
        if after.manifest.change == Some(Change::Unchanged) {
            report.result_scope = UNCHANGED_SCOPE;
        }
        report.baseline_accounting = Some(before.accounting.clone());
        report.candidate_accounting = Some(after.accounting.clone());
        report.baseline_complete_acquisitions = before.complete_acquisitions;
        report.candidate_complete_acquisitions = after.complete_acquisitions;
        report.baseline_acquisition_medians = before.observations.clone();
        report.candidate_acquisition_medians = after.observations.clone();
        report.baseline_capture_sha256 = Some(before.sha256.clone());
        report.candidate_capture_sha256 = Some(after.sha256.clone());
        report.first_failure = before
            .first_failure
            .clone()
            .or_else(|| after.first_failure.clone());
        report.stop_reason = after
            .manifest
            .stop_reason
            .clone()
            .or_else(|| before.manifest.stop_reason.clone());
        let mut selected = before.selected.take();
        if let (Some(selected), Some(candidate)) = (&mut selected, after.selected.take()) {
            selected.candidate_acquisitions = candidate.baseline_acquisitions;
            selected.candidate_timing = Some(candidate.baseline_timing);
        }
        selected_report(&mut report, selected, before.manifest.workload);
        linked(&before, &after)?;
        if after.manifest.change == Some(Change::Deployment) {
            return deployment(&mut report, &before, &after, reference);
        }
        if reference.is_some() {
            return Err("a capture reference applies only to a deployment candidate".into());
        }
        if !coverage(&mut report, &[&before, &after])? {
            return Ok(());
        }
        if report.selected.is_some() {
            report.result = Outcome::Descriptive;
            report.reasons.push("Explicit selection is descriptive only: retain every cell and acquisition; no pooled percentage, confidence interval or directional label is supported.".into());
            return Ok(());
        }
        assess(&mut report, &before.observations, &after.observations)?;
        Ok::<(), String>(())
    })();
    if let Err(error) = operation {
        report.result = Outcome::Invalid;
        report.reasons.push(error);
    }
    report
}

/// Verifies that `after` is a compatible capture of the immutable baseline `before`.
fn linked(before: &Verified, after: &Verified) -> Result<()> {
    let (a, b) = (&before.manifest, &after.manifest);
    let deployment = b.change == Some(Change::Deployment);
    if a.baseline_sha256.is_some()
        || b.baseline_sha256.as_deref() != Some(before.sha256.as_str())
        || (!deployment
            && (a.endpoint != b.endpoint
                || a.model != b.model
                || a.auth_env != b.auth_env
                || a.local_http != b.local_http
                || a.collector_sha256 != b.collector_sha256
                || a.client_placement != b.client_placement))
        || a.workload_sha256 != b.workload_sha256
        || a.metric_contract != b.metric_contract
        || a.version != b.version
        || a.selection_sha256 != b.selection_sha256
    {
        return Err(
            "captures are incompatible or candidate does not reference this immutable baseline"
                .into(),
        );
    }
    if deployment {
        deployment_pair(a, b)?;
    }
    if !ordered(a, b, 0) {
        return Err("candidate capture period overlaps or precedes baseline capture period".into());
    }
    declared_change(
        &before.deployment,
        &after.deployment,
        b.change
            .ok_or("candidate has no selected declaration change")?,
    )
}

/// Whether `later` starts at least `gap_ms` after `earlier` finished; captures without
/// recorded periods are incomplete and withheld elsewhere.
fn ordered(earlier: &Capture, later: &Capture, gap_ms: u64) -> bool {
    let finished = earlier
        .acquisitions
        .iter()
        .rev()
        .find_map(|a| a.finished_unix_ms);
    let started = later.acquisitions.iter().find_map(|a| a.started_unix_ms);
    !matches!((finished, started), (Some(finished), Some(started)) if started < finished.saturating_add(gap_ms))
}

/// Whether every capture is complete; an invalid capture fails the comparison.
fn coverage(report: &mut Report, captures: &[&Verified]) -> Result<bool> {
    if captures
        .iter()
        .any(|c| c.manifest.status == CaptureStatus::Invalid)
    {
        return Err("capture contains an invalid response, observation, or local collection failure; inspect capture.json and native receipts".into());
    }
    if captures
        .iter()
        .any(|c| c.manifest.status != CaptureStatus::Complete)
    {
        report.reasons.push("Incomplete acquisition coverage: no model-based interval or directional conclusion. All acquisitions must complete without replacement.".into());
        return Ok(false);
    }
    Ok(true)
}

/// Whole-deployment comparison; its verdict needs the unchanged reference captured after the candidate.
fn deployment(
    report: &mut Report,
    before: &Verified,
    after: &Verified,
    reference: Option<&Path>,
) -> Result<()> {
    report.kind = "performance-deployment-comparison-v1";
    report.scope = DEPLOYMENT_SCOPE;
    report.model = None;
    let control = reference
        .map(|path| control(before, after, path))
        .transpose()?;
    let captures: Vec<&Verified> = [before, after].into_iter().chain(&control).collect();
    let complete = coverage(report, &captures)?;
    let (candidate_against_reference, reference_against_baseline) = match &control {
        Some(control) if complete => {
            let (reference, drift) = verdict(report, before, after, control)?;
            (Some(reference), Some(drift))
        }
        None if complete => {
            report.reasons.push(PENDING.into());
            (None, None)
        }
        _ => (None, None),
    };
    if before.median_prompt_tokens != after.median_prompt_tokens {
        report.reasons.push(format!(
            "Median reported prompt tokens differ (baseline {}, candidate {}): the servers render the same prompt differently, so prefill work differs.",
            OrNull(before.median_prompt_tokens),
            OrNull(after.median_prompt_tokens)
        ));
    }
    report.deployment = Some(DeploymentReport {
        baseline: side(before),
        candidate: side(after),
        reference_path: reference.map(Path::to_owned),
        reference_capture_sha256: control.map(|c| c.sha256),
        candidate_against_reference,
        reference_against_baseline,
    });
    Ok(())
}

/// The unchanged reference: `check <baseline> --change none`, captured after the candidate.
fn control(before: &Verified, after: &Verified, path: &Path) -> Result<Verified> {
    let control = load(path)?;
    if control.manifest.change != Some(Change::Unchanged) {
        return Err("the reference must be an unchanged control of the baseline (check <baseline> --change none)".into());
    }
    linked(before, &control)?;
    if !ordered(&before.manifest, &after.manifest, DEPLOYMENT_GAP_MS)
        || !ordered(&after.manifest, &control.manifest, DEPLOYMENT_GAP_MS)
    {
        return Err("baseline, candidate and reference must be captured in that order, each starting at least 60 s after the previous one finished".into());
    }
    Ok(control)
}

/// A direction only when the candidate differs the same way from both baseline capture periods.
fn verdict(
    report: &mut Report,
    before: &Verified,
    after: &Verified,
    control: &Verified,
) -> Result<(Interval, Interval)> {
    let directional = |result| matches!(result, Outcome::Improved | Outcome::Regressed);
    assess(report, &before.observations, &after.observations)?;
    let reference = interval(&control.observations, &after.observations)?;
    let drift = interval(&before.observations, &control.observations)?;
    if !directional(report.result) || reference.result != report.result {
        report.result = Outcome::Inconclusive;
        report
            .reasons
            .push("candidate is not directional against both baseline capture periods".into());
    }
    if directional(drift.result) {
        report
            .reasons
            .push("baseline deployment shifted between capture periods".into());
    }
    Ok((reference, drift))
}

fn interval(before: &[f64], after: &[f64]) -> Result<Interval> {
    let mut scratch = Report::new(PathBuf::new());
    assess(&mut scratch, before, after)?;
    Ok(Interval {
        result: scratch.result,
        observed_change_percent: scratch.observed_change_percent,
        model_based_interval_percent: scratch.model_based_interval_percent,
    })
}

fn side(capture: &Verified) -> DeploymentSide {
    let manifest = &capture.manifest;
    DeploymentSide {
        endpoint: manifest.endpoint.clone(),
        model: manifest.model.clone(),
        declaration: capture.deployment.clone(),
        collector: manifest
            .collector
            .clone()
            .expect("deployment pairs are admitted only as v3 captures"),
        collector_sha256: manifest.collector_sha256.clone(),
        client_placement: manifest
            .client_placement
            .expect("deployment pairs are admitted only as v3 captures"),
        median_prompt_tokens: capture.median_prompt_tokens,
    }
}

fn assess(report: &mut Report, before: &[f64], after: &[f64]) -> Result<()> {
    if before.len() != ACQUISITIONS || after.len() != ACQUISITIONS {
        report
            .reasons
            .push("Incomplete acquisition coverage: interval and direction withheld.".into());
        return Ok(());
    }
    if before
        .iter()
        .chain(after)
        .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return Err("acquisition observations must be positive and finite".into());
    }
    let moments = |values: &[f64]| {
        let center = values[0].ln();
        let mean =
            center + values.iter().map(|v| v.ln() - center).sum::<f64>() / ACQUISITIONS as f64;
        let variance =
            values.iter().map(|v| (v.ln() - mean).powi(2)).sum::<f64>() / (ACQUISITIONS - 1) as f64;
        (mean, variance)
    };
    let (a, va) = moments(before);
    let (b, vb) = moments(after);
    let difference = b - a;
    let se = ((va + vb) / ACQUISITIONS as f64).sqrt();
    let interval = [
        (difference - CRITICAL * se).exp_m1() * 100.0,
        (difference + CRITICAL * se).exp_m1() * 100.0,
    ];
    let observed = difference.exp_m1() * 100.0;
    if !observed.is_finite() || interval.iter().any(|v| !v.is_finite()) {
        return Err("model-based assessment exceeds finite numeric bounds".into());
    }
    report.log_mean_difference = Some(difference);
    report.heteroscedastic_standard_error = Some(se);
    report.observed_change_percent = Some(observed);
    if se == 0.0 {
        report.reasons.push("No estimable acquisition variation at recorded resolution; interval and confidence withheld.".into());
        return Ok(());
    }
    report.model_based_interval_percent = Some(interval);
    report.model_based_confidence_level = Some(0.95);
    report.result = if interval[0] > 0.0 {
        Outcome::Improved
    } else if interval[1] < 0.0 {
        Outcome::Regressed
    } else {
        Outcome::Inconclusive
    };
    Ok(())
}

#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
pub fn human(report: &Report) -> String {
    let label = if report.baseline_ready && report.selected.is_some() {
        "BASELINE READY - DESCRIPTIVE ONLY: observations collected; comparison not yet performed"
    } else if report.baseline_ready {
        "Baseline ready; comparison not yet performed"
    } else {
        match report.result {
            Outcome::Descriptive => {
                "COMPLETE - DESCRIPTIVE ONLY: comparison completed successfully; no performance verdict"
            }
            Outcome::Improved => "MEASURED FASTER: higher throughput in these capture periods",
            Outcome::Regressed => "MEASURED SLOWER: lower throughput in these capture periods",
            Outcome::Inconclusive => {
                "INCONCLUSIVE: no direction established; this does not mean equivalent performance"
            }
            Outcome::Pending => {
                "VERDICT PENDING: deployment candidate captured; the verdict needs the unchanged reference"
            }
            Outcome::Invalid => "INVALID: evidence or declarations cannot support this comparison",
        }
    };
    let scope = if let Some(selected) = &report.selected {
        selected.manifest.scope.as_str()
    } else if report.scope == Builtin::BaselineV2.scope() {
        "structured C1 (one concurrent request), exactly 400 output tokens per request."
    } else if report.scope == Builtin::PortableV1.scope() {
        "count C1 (one concurrent request), 400 output tokens per request verified by cap-reached."
    } else {
        report.scope
    };
    let mut text = format!("{label}\nScope: {scope}\nComplete acquisitions: ");
    for (side, accounting, complete) in [
        (
            "baseline ",
            &report.baseline_accounting,
            report.baseline_complete_acquisitions,
        ),
        (
            "; candidate ",
            &report.candidate_accounting,
            report.candidate_complete_acquisitions,
        ),
    ] {
        text.push_str(side);
        if accounting.is_some() {
            text.push_str(&format!("{complete}/{ACQUISITIONS}"));
        } else {
            text.push_str("unavailable");
        }
    }
    text.push('\n');
    if let Some(selected) = &report.selected {
        text.push_str(&format!(
            "Selection {}: {}; operation scope {:?} (operator declaration)\n{}\n",
            selected.manifest.id,
            selected.manifest_sha256,
            selected.manifest.operation_scope,
            SELECTED_SCOPE
        ));
        text.push_str(&format!("Elapsed through capture publication: baseline {}us; candidate {}us. Includes setup and publication, excludes this timing receipt and reports; OS/filesystem stalls have no hard wall-clock guarantee.\n",
            selected.baseline_timing.elapsed_through_capture_publication_us,
            OrNull(selected.candidate_timing.as_ref().map(|t| t.elapsed_through_capture_publication_us))));
        for cell in &selected.cells {
            text.push_str(&format!(
                "Cell {}: concurrency {}; warmup {}; measured trials {} per acquisition\n",
                cell.id, cell.concurrency, cell.warmup_trials, cell.trials
            ));
        }
        for (side, acquisitions) in [
            ("Baseline", &selected.baseline_acquisitions),
            ("Candidate", &selected.candidate_acquisitions),
        ] {
            for acquisition in acquisitions {
                text.push_str(&format!("{side} acquisition {}: {}; preparation {}us, reservation publication {}us (included in preparation), wave publication {}us\n",
                    acquisition.acquisition, acquisition.status, acquisition.summary.wave_preparation_us,
                    acquisition.summary.reservation_publication_us, acquisition.summary.wave_publication_us));
                for cell in &acquisition.cells {
                    text.push_str(&format!("  {}: {}/{} observed trials, {} eligible; makespan median {}us; aggregate achieved {} tokens/s; per-stream settlement decode {}, text-window decode {}, prefill {} tokens/s\n",
                        cell.cell, cell.observed_trials, cell.planned_trials, cell.eligible_trials,
                        OrNull(cell.median_wave_latency_us), OrNull(cell.median_achieved_completion_tokens_per_second),
                        OrNull(cell.median_decode_tokens_per_second), OrNull(cell.median_text_decode_tokens_per_second),
                        OrNull(cell.median_prefill_tokens_per_second)));
                    if let Some(check) = &cell.sequence_check {
                        text.push_str(&format!("    history {}; parent {}; semantic correct {}; strict match {}; canonical formatting {}; detail {}\n",
                            check.history, OrNull(check.parent.as_deref()), check.correct, OrNull(check.strict_match), OrNull(check.canonical_match), OrNull(check.error.as_deref())));
                    }
                }
                for wave in acquisition.waves.iter().flatten() {
                    for lane in wave.attempts.iter().filter(|lane| {
                        lane.status != Status::Complete
                            || !lane.eligibility_errors.is_empty()
                            || lane
                                .sequence
                                .as_ref()
                                .is_some_and(|check| !crate::sequence::passed(check))
                    }) {
                        text.push_str(&format!("  {} {:?} trial {}: performance eligible {}; makespan {}us; dispatch spread {}us\n",
                            wave.spec.cell, wave.spec.phase, wave.spec.trial, wave.eligible, wave.elapsed_us, wave.dispatch_spread_us));
                        text.push_str(&format!("    lane {}: {:?}; dispatched {}; first generated {}us; first answer {}us; terminal {}us; settlement {}us; errors {}\n",
                            lane.lane, lane.status, lane.dispatched, OrNull(lane.timing.first_generated_text_us),
                            OrNull(lane.timing.first_answer_text_us), OrNull(lane.timing.terminal_us), lane.timing.settle_us,
                            if lane.eligibility_errors.is_empty() { "none".into() } else { lane.eligibility_errors.join("; ") }));
                    }
                }
            }
        }
        text.push_str("Complete per-wave/per-lane timings, dispatch spreads, cached token counts and raw evidence references are retained in report.json and native receipts; null means unavailable.\n");
    }
    if let Some(model) = &report.model {
        text.push_str(&format!("Model: {model}\n"));
    }
    if let Some(deployment) = &report.deployment {
        text.push_str(&deployment_text(deployment));
    }
    for (label, path, accounting) in [
        (
            "Baseline",
            &report.baseline_path,
            &report.baseline_accounting,
        ),
        (
            "Candidate",
            &report.candidate_path,
            &report.candidate_accounting,
        ),
    ] {
        if let Some(path) = path {
            text.push_str(&format!("{label}: {}\n", path.display()));
        }
        if let Some(a) = accounting {
            text.push_str(&format!(
                "  Requests: {} dispatched, {} complete; reported output: ",
                a.dispatched_requests, a.complete_requests
            ));
            match a.reported_completion_tokens {
                Some(tokens) => text.push_str(&format!("{tokens} tokens\n")),
                None => text.push_str(&format!(
                    "unknown total ({} known tokens; {} dispatched requests missing usage)\n",
                    a.known_reported_completion_tokens, a.missing_completion_usage_requests
                )),
            }
            text.push_str(&format!(
                "  Ceiling: {} requests / {} output tokens; allowance {}s.",
                a.request_ceiling, a.output_token_ceiling, a.allowance_seconds
            ));
            if let Some(rate) = a.minimum_full_capture_tokens_per_second {
                text.push_str(&format!(" Full completion needs >{rate:.1} reported output tokens/s including overhead; use --seconds for a larger allowance.\n"));
            } else {
                text.push_str(" At least one planned phase uses capped output; no minimum completion rate inferred.\n");
            }
        }
    }
    if let Some(change) = report.declared_change {
        if change == Change::Unchanged {
            text.push_str(UNCHANGED_SCOPE);
            text.push('\n');
        } else {
            let name =
                clap::ValueEnum::to_possible_value(&change).expect("every change is a CLI value");
            text.push_str(&format!(
                "Selected declaration change: {}\n",
                name.get_name()
            ));
        }
    }
    if let Some(observed) = report.observed_change_percent {
        text.push_str(&format!(
            "Observed throughput change between capture periods: {observed:+.2}%\n"
        ));
    }
    if let Some([lower, upper]) = report.model_based_interval_percent {
        text.push_str(&format!("Uncertainty range (model-based, nominal 95%) for the measured change: [{lower:+.2}%, {upper:+.2}%]\n"));
    }
    for reason in &report.reasons {
        text.push_str(reason);
        text.push('\n');
    }
    if let Some(reason) = &report.stop_reason
        && !report.reasons.contains(reason)
    {
        text.push_str(&format!("Capture stop: {reason}\n"));
    }
    if let Some(failure) = &report.first_failure {
        let exact = report.selected.as_ref().is_none_or(|s| {
            s.request.effective_output(failure.wave.phase).mode == OutputMode::Exact
        });
        if failure
            .usage
            .completion_tokens
            .is_some_and(|n| n > u64::from(failure.expected_completion_tokens))
            || (failure.status == Status::Complete
                && exact
                && failure
                    .usage
                    .completion_tokens
                    .is_some_and(|n| n != u64::from(failure.expected_completion_tokens)))
        {
            let requirement = if exact {
                format!(
                    "exactly {} were required",
                    failure.expected_completion_tokens
                )
            } else {
                format!(
                    "the declared cap was {}",
                    failure.expected_completion_tokens
                )
            };
            text.push_str(&format!(
                "Stopped: the server reported {} output tokens; {requirement}.\n",
                failure.usage.completion_tokens.unwrap()
            ));
        } else if failure.status == Status::HttpError {
            match failure.http_status {
                Some(status) => {
                    text.push_str(&format!("Stopped: the server returned HTTP {status}.\n"))
                }
                None => text.push_str("Stopped: HTTP failure without a retained status code.\n"),
            }
        } else if failure.acquisition_status == "stopped-after-sequence-check" {
            text.push_str("Stopped: a declared sequence semantic or strict check failed; performance timings remain separate observations, not a performance regression verdict.\n");
        } else if failure.acquisition_status == "budget-exhausted" {
            text.push_str("Stopped: the measurement budget expired. The unfinished request is retained; no further requests were sent.\n");
        } else if failure.status == Status::Complete && failure.usage.completion_tokens.is_none() {
            text.push_str("Stopped: the server did not report completion-token usage. No token estimate was substituted.\n");
        } else {
            text.push_str(&format!("Stopped: {}.\n", failure.detail));
            if !failure.eligibility_errors.is_empty() {
                text.push_str(&format!(
                    "  Measurement errors: {}\n",
                    failure.eligibility_errors.join(", ")
                ));
            }
        }
        text.push_str(&format!(
            "  Receipt: {}\n  Raw response: {}\n",
            failure.receipt_path.display(),
            failure.response_path.display()
        ));
    }
    if report.baseline_ready
        && let Some(root) = report.report_path.parent()
    {
        let quoted_root = root.to_string_lossy().replace('\'', "'\\''");
        text.push_str(&format!("Next: grill-perf check '{quoted_root}' --deployment serving-after.json --change settings --out after\nSelect the declaration field you changed; the other fields must match.\nFor an unchanged-deployment control: grill-perf check '{quoted_root}' --deployment '{quoted_root}/deployment.json' --change none --out control\nTo compare another deployment: grill-perf check '{quoted_root}' --change deployment --deployment other.json --endpoint URL --model NAME --client-placement PLACEMENT --out candidate, then the unchanged control, then compare with --reference (see INSTALL.md).\n"));
    }
    if report.observed_change_percent.is_some() {
        text.push_str("Limits: a measured direction does not establish its cause or practical importance. These sequential measurements may be correlated or drift over time, making the uncertainty range too narrow. No guaranteed precision.\n");
    }
    text.push_str(&format!(
        "Serving identity is declared, not attested.\nReport: {}\n",
        report.report_path.display()
    ));
    text
}

fn deployment_text(deployment: &DeploymentReport) -> String {
    let mut text = String::new();
    for (label, side) in [
        ("Baseline", &deployment.baseline),
        ("Candidate", &deployment.candidate),
    ] {
        let declared = &side.declaration;
        let placement = clap::ValueEnum::to_possible_value(&side.client_placement)
            .expect("every placement is a CLI value");
        text.push_str(&format!(
            "{label} deployment: model {}; endpoint {}; model_revision {}; runtime {}; hardware {}; settings {}; client placement {}; collector {} (source {}, target {}); median prompt tokens {}\n",
            side.model,
            side.endpoint,
            OrNull(declared.model_revision.as_deref()),
            OrNull(declared.runtime.as_deref()),
            OrNull(declared.hardware.as_deref()),
            OrNull(declared.settings.as_deref()),
            placement.get_name(),
            side.collector.version,
            side.collector.source_commit,
            side.collector.target,
            OrNull(side.median_prompt_tokens),
        ));
    }
    if let Some(path) = &deployment.reference_path {
        text.push_str(&format!("Reference: {}\n", path.display()));
    }
    for (label, interval) in [
        (
            "Candidate against the reference period",
            &deployment.candidate_against_reference,
        ),
        (
            "Reference against the baseline period (baseline drift)",
            &deployment.reference_against_baseline,
        ),
    ] {
        if let Some(interval) = interval {
            text.push_str(&format!(
                "{label}: observed {:+.2}%; uncertainty range (model-based, nominal 95%) ",
                OrNull(interval.observed_change_percent)
            ));
            match interval.model_based_interval_percent {
                Some([lower, upper]) => {
                    text.push_str(&format!("[{lower:+.2}%, {upper:+.2}%]\n"));
                }
                None => text.push_str("null\n"),
            }
        }
    }
    text
}

#[cfg(test)]
#[path = "../tests/support/calibration.rs"]
mod calibration;

#[cfg(test)]
#[path = "../tests/support/finalization.rs"]
mod finalization;
