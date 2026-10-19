//! Finite startup and restart-cache observations. No process lifecycle actions.
//! Journal facts are unauthenticated source observations; imports never become native.
use crate::{envelope, evidence, metrics, model, policy::Outcome, wire};
use clap::{Args, Subcommand, ValueEnum};
use model::{Attempt, Limits, Result, Status};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "startup_kv.rs"]
mod kv;
#[path = "startup_kv_registration.rs"]
mod kv_registration;
#[path = "startup_kv_v2.rs"]
mod kv_v2;

const CAP: usize = 4 * 1024 * 1024;
const MAX_REQUESTS: usize = 16;
const POLL_MS: u64 = 10;

#[derive(Subcommand)]
pub enum Command {
    /// Observe an operator-launched process; never launch or restart it.
    Observe(CaptureArgs),
    /// Collect the one declared store request, before the operator's restart.
    Store(CaptureArgs),
    /// Collect the ordered post-restart list, linked to retained store evidence.
    Reload {
        #[command(flatten)]
        capture: CaptureArgs,
        #[arg(long)]
        store: PathBuf,
    },
    /// Copy and verify evidence offline, retaining imported provenance.
    Import {
        source: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify retained raw events, responses and metrics offline.
    Inspect { capture: PathBuf },
    /// Compare every prospective A/B/A2 acquisition; manifest is an ordered path array.
    Compare { plan: PathBuf, manifest: PathBuf },
    /// Join new source-copy journals without relabeling historical startup evidence.
    VerifyKv(kv::VerifyArgs),
    /// Verify separately versioned registered-state/L1 evidence; never native qualification.
    VerifyKvV2(kv_v2::VerifyArgs),
}
#[derive(Args)]
pub struct CaptureArgs {
    pub plan: PathBuf,
    #[arg(long)]
    pub slot: usize,
    #[arg(long)]
    pub pid: u32,
    #[arg(long)]
    pub cache_pid: Option<u32>,
    #[arg(long)]
    pub events: PathBuf,
    /// Exact source file implementing the selected journal protocol; not executed.
    #[arg(long)]
    pub producer_source: PathBuf,
    #[arg(long)]
    pub previous: Option<PathBuf>,
    #[arg(long)]
    pub out: PathBuf,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    NativeObserved,
    Imported,
    Declared,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Adapter {
    NeutralJournalV1,
    RuntimeVllmV1,
    #[serde(rename = "runtime_vllm_487ecf187_v1")]
    RuntimeVllm487V1,
    #[serde(rename = "runtime_vllm_487ecf187_bootstrap_v1")]
    RuntimeVllm487BootstrapV1,
    #[serde(rename = "runtime_vllm_752a3a504_bootstrap_v1")]
    RuntimeVllm752BootstrapV1,
    RuntimeAsgiFixtureV1,
    RuntimeAsgiFixtureBootstrapV1,
    UninstrumentedRecipeLogs,
}
impl Adapter {
    fn runtime(self) -> bool {
        matches!(
            self,
            Self::RuntimeVllmV1
                | Self::RuntimeVllm487V1
                | Self::RuntimeVllm487BootstrapV1
                | Self::RuntimeVllm752BootstrapV1
                | Self::RuntimeAsgiFixtureV1
                | Self::RuntimeAsgiFixtureBootstrapV1
        )
    }
    fn bootstrap(self) -> bool {
        matches!(
            self,
            Self::RuntimeVllm487BootstrapV1
                | Self::RuntimeVllm752BootstrapV1
                | Self::RuntimeAsgiFixtureBootstrapV1
        )
    }
    fn ready_contract(self) -> &'static str {
        if self.runtime() {
            "asgi-lifespan-startup-complete-v1"
        } else {
            "neutral-ready-v1"
        }
    }
    fn source_contract(self) -> &'static str {
        match self {
            Self::RuntimeVllmV1 => "vllm-0.27.0-uvicorn-0.34.0-sha256-v1",
            Self::RuntimeVllm487V1 => "vllm-487ecf187-uvicorn-0.52.4-sha256-v1",
            Self::RuntimeVllm487BootstrapV1 => "vllm-487ecf187-uvicorn-0.52.4-sha256-bootstrap-v1",
            Self::RuntimeVllm752BootstrapV1 => "vllm-752a3a504-uvicorn-0.51.0-sha256-bootstrap-v1",
            Self::RuntimeAsgiFixtureBootstrapV1 => "ordinary-asgi-fixture-bootstrap-v1",
            _ => "ordinary-asgi-fixture-v1",
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Startup,
    Store,
    Reload,
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
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    pub id: String,
    pub kind: String,
    pub units: String,
    pub resolution_us: u64,
    pub synchronization: String,
}
impl Clock {
    fn validate(&self) -> Result<()> {
        if !short(&self.id)
            || self.kind != "monotonic"
            || self.units != "microseconds"
            || self.resolution_us == 0
            || self.resolution_us > 1_000_000
            || self.synchronization != "single-process-origin"
        {
            return Err("unsupported startup clock contract".into());
        }
        Ok(())
    }
    fn compatible(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.units == other.units
            && self.resolution_us == other.resolution_us
            && self.synchronization == other.synchronization
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_ticks: u64,
    pub boot_id: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Engine,
    CacheServer,
    Filesystem,
    CompiledArtifactCache,
    Weights,
    PrefixOffload,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClassIdentity {
    Process {
        identity: ProcessIdentity,
    },
    Artifact {
        content_sha256: String,
        generation: String,
    },
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Temperature {
    Cold,
    Warm,
    Unknown,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClassState {
    pub class: Class,
    pub identity: Option<ClassIdentity>,
    pub declared: Temperature,
    pub observed: Temperature,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StateCondition {
    pub class: Class,
    pub declared: Temperature,
    pub required_observed: Temperature,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    Changed,
    Persisted,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClassPredicate {
    pub class: Class,
    pub transition: Transition,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CacheIdentity {
    pub version: u32,
    pub prompt_sha256: String,
    pub source_sha256: String,
    pub cache_salt: String,
    pub hash_seed_contract: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestSpec {
    pub model: String,
    /// Exact UTF-8 prompt, no substitution or random namespace.
    pub prompt: String,
    pub max_tokens: u32,
    pub expected_answer: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestItem {
    pub id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Study {
    Startup,
    RestartCache {
        identity: CacheIdentity,
        post_restart: Vec<RequestItem>,
        classes: Vec<ClassPredicate>,
        /// Declaration only; qualification requires matching complete source controls.
        no_warming_attestation: bool,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Listening,
    Ready,
    FirstValidInference,
    Communication,
    Bootstrap,
    Reload,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub target: Target,
    pub max_regression_bps: u32,
    pub max_reference_spread_bps: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub kind: String,
    pub study_id: String,
    pub collector_sha256: String,
    pub adapter: Adapter,
    pub adapter_sha256: String,
    /// Runtime bridge only: minimum complete observation after ASGI startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_window_us: Option<u64>,
    /// A and A2 must match; B differs only along this explicitly declared setup axis.
    pub setup_axis: String,
    pub setup_sha256: [String; 3],
    pub slots: Vec<Slot>,
    pub study: Study,
    pub states: Vec<StateCondition>,
    pub request: RequestSpec,
    pub endpoint: String,
    pub local_http: bool,
    pub auth_env: Option<String>,
    pub metrics: Option<metrics::v2::Config>,
    pub max_events: usize,
    pub event_bytes: usize,
    pub deadline_us: u64,
    pub limits: Limits,
    pub gates: Vec<Gate>,
}
fn short(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| e.to_string())
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}
fn settings(plan: &Plan) -> model::RequestSettings {
    model::RequestSettings {
        profile: model::Profile::PortableChatV1,
        stream: false,
        output: model::OutputBudget {
            tokens: plan.request.max_tokens,
            mode: model::OutputMode::Cap,
        },
        warmup_output: None,
        cache: model::Cache::Observe,
        temperature_milli: Some(0),
        top_p_milli: None,
        seed: None,
        thinking: None,
        thinking_control: None,
    }
}
fn request_body(plan: &Plan) -> Result<String> {
    #[derive(Serialize)]
    struct Body<'a> {
        model: &'a str,
        messages: [Message<'a>; 1],
        stream: bool,
        max_tokens: u32,
        temperature: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_salt: Option<&'a str>,
    }
    #[derive(Serialize)]
    struct Message<'a> {
        role: &'static str,
        content: &'a str,
    }
    let salt = match &plan.study {
        Study::Startup => None,
        Study::RestartCache { identity, .. } => Some(identity.cache_salt.as_str()),
    };
    let body = serde_json::to_string(&Body {
        model: &plan.request.model,
        messages: [Message {
            role: "user",
            content: &plan.request.prompt,
        }],
        stream: false,
        max_tokens: plan.request.max_tokens,
        temperature: 0,
        cache_salt: salt,
    })
    .map_err(|e| e.to_string())?;
    if body.len() > model::REQUEST_CAP {
        return Err("startup request exceeds 2 MiB".into());
    }
    Ok(body)
}
impl Plan {
    fn requests(&self, stage: Stage) -> Vec<String> {
        match (&self.study, stage) {
            (Study::RestartCache { post_restart, .. }, Stage::Reload) => {
                post_restart.iter().map(|r| r.id.clone()).collect()
            }
            (_, Stage::Store) => vec!["store".into()],
            _ => vec!["first_inference".into()],
        }
    }
    #[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.kind != "startup-study-v1"
            || !short(&self.study_id)
            || !evidence::is_digest(&self.collector_sha256)
            || !evidence::is_digest(&self.adapter_sha256)
            || !short(&self.setup_axis)
            || self.setup_sha256.iter().any(|h| !evidence::is_digest(h))
            || self.setup_sha256[0] != self.setup_sha256[2]
            || self.slots.len() > 360
            || self.slots.is_empty()
            || !(8..=1024).contains(&self.max_events)
            || !(1024..=CAP).contains(&self.event_bytes)
            || !(1_000..=3_600_000_000).contains(&self.deadline_us)
            || self.gates.is_empty()
            || self.gates.len() > 6
        {
            return Err("invalid finite startup study plan".into());
        }
        match (self.adapter.runtime(), self.runtime_window_us) {
            (true, Some(window)) if window >= 1_000 && window < self.deadline_us => (),
            (false, None) => (),
            _ => {
                return Err(
                    "runtime adapter requires a finite post-readiness coverage window".into(),
                );
            }
        }
        let l = &self.limits;
        if l.total_ms == 0
            || l.total_ms > 600_000
            || l.idle_ms == 0
            || l.idle_ms > l.total_ms
            || l.response_bytes == 0
            || l.response_bytes > 1024 * 1024
            || l.wave_buffer_bytes < l.response_bytes
            || l.wave_buffer_bytes > 16 * 1024 * 1024
        {
            return Err("invalid startup HTTP bounds".into());
        }
        if self.request.model.is_empty()
            || self.request.model.len() > 256
            || self.request.prompt.len() > model::REQUEST_CAP
            || self.request.expected_answer.is_empty()
            || self.request.expected_answer.len() > 65536
        {
            return Err("invalid startup request exposure".into());
        }
        settings(self).validate()?;
        wire::endpoint(&self.endpoint, self.local_http)?;
        let mut state_classes = BTreeSet::new();
        if self.states.is_empty()
            || self.states.len() > 6
            || self.states.iter().any(|s| !state_classes.insert(s.class))
        {
            return Err(
                "startup cache-state exposure must be prospectively declared once per class".into(),
            );
        }
        let mut ids = BTreeSet::new();
        let mut offset = 0;
        let mut counts = None;
        for arm in [Arm::A, Arm::B, Arm::A2] {
            let mut local = [0u32; 2];
            for warmup in [true, false] {
                let side = usize::from(!warmup);
                while let Some(slot) = self
                    .slots
                    .get(offset)
                    .filter(|s| s.arm == arm && s.warmup == warmup)
                {
                    if !short(&slot.id) || !ids.insert(&slot.id) || slot.index != local[side] {
                        return Err("duplicate, unordered or invalid startup slot".into());
                    }
                    local[side] += 1;
                    offset += 1;
                }
            }
            if !(1..=20).contains(&local[0])
                || !(3..=100).contains(&local[1])
                || counts.is_some_and(|n| n != local)
            {
                return Err(
                    "each role needs equal prospective warmups and at least three measurements"
                        .into(),
                );
            }
            counts = Some(local);
        }
        if offset != self.slots.len() {
            return Err("startup slots must be ordered A/B/A2".into());
        }
        if let Study::RestartCache {
            identity,
            post_restart,
            classes,
            ..
        } = &self.study
        {
            if identity.version != 1
                || identity.prompt_sha256 != evidence::digest(self.request.prompt.as_bytes())
                || !evidence::is_digest(&identity.source_sha256)
                || !short(&identity.cache_salt)
                || identity.hash_seed_contract != "PYTHONHASHSEED=0:engine+cache_server"
                || post_restart.is_empty()
                || post_restart.len() > MAX_REQUESTS
                || post_restart[0].id != "first_reload"
                || classes.len() > 6
            {
                return Err(
                    "invalid frozen restart cache identity or first-reload request list".into(),
                );
            }
            let mut names = BTreeSet::new();
            if post_restart
                .iter()
                .any(|r| !short(&r.id) || !names.insert(&r.id))
            {
                return Err("duplicate post-restart request".into());
            }
            let mut seen = BTreeSet::new();
            if classes.iter().any(|c| !seen.insert(c.class))
                || !classes
                    .iter()
                    .any(|c| c.class == Class::Engine && c.transition == Transition::Changed)
                || !classes
                    .iter()
                    .any(|c| c.class == Class::CacheServer && c.transition == Transition::Persisted)
            {
                return Err(
                    "restart requires changed engine and persisted cache-server predicates".into(),
                );
            }
            if self.metrics.is_none() {
                return Err("restart-cache requires explicit metrics2 collection".into());
            }
        }
        let n = match &self.study {
            Study::Startup => 1,
            Study::RestartCache { post_restart, .. } => post_restart.len(),
        };
        let scrape_allowance_us = if self.metrics.is_some() { 4_000_000 } else { 0 };
        if n * l.response_bytes > l.wave_buffer_bytes
            || n as u64 * (u64::from(l.total_ms) * 1000 + scrape_allowance_us) > self.deadline_us
        {
            return Err("request and scrape allowances exceed prospective memory/deadline".into());
        }
        if let Some(config) = &self.metrics {
            config.validate(n, self.local_http, self.auth_env.as_deref())?;
        }
        for (i, gate) in self.gates.iter().enumerate() {
            if gate.max_regression_bps > 9999
                || gate.max_reference_spread_bps > 1_000_000
                || self.gates[..i].iter().any(|g| g.target == gate.target)
                || (matches!(gate.target, Target::Reload)
                    && !matches!(self.study, Study::RestartCache { .. }))
            {
                return Err("unsupported or duplicate startup gate".into());
            }
        }
        request_body(self)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheResult {
    Stored,
    PersistedHit,
    Recomputed,
    Unavailable,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventKind {
    Launched,
    /// Bridge entrypoint before serving imports, explicitly NOT OS process launch.
    RuntimeStart {
        producer_sha256: String,
        source_contract: String,
        pid_namespace: String,
        time_namespace: String,
    },
    /// Completion of all admitted requests after the prospective runtime window.
    RuntimeCoverage {
        window_us: u64,
        admission_closed: bool,
        active_requests: usize,
        engine_hook: bool,
    },
    /// An actual AsyncLLM admission correlated to the active ASGI request.
    RuntimeEngineRequest {
        id: String,
    },
    Listening,
    Ready {
        contract: String,
    },
    CommunicationStart,
    CommunicationEnd,
    /// Frontend synchronous EngineCoreClient construction, not worker/NCCL timing.
    BootstrapStart,
    BootstrapEnd,
    Controls {
        smoke_suppressed: bool,
        shape_warmup_suppressed: bool,
        hash_seed_contract: String,
    },
    State {
        state: ClassState,
    },
    RequestStart {
        id: String,
        body_sha256: String,
    },
    InferenceComplete {
        id: String,
        response_sha256: String,
        cache_identity: Option<CacheIdentity>,
        cache_result: CacheResult,
    },
    Failure {
        detail: String,
    },
    /// The producer closes inference admission before writing this final event.
    Sealed {
        requests: usize,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: u32,
    pub sequence: usize,
    pub clock: Clock,
    pub offset_us: u64,
    pub engine: ProcessIdentity,
    pub event: EventKind,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestRecord {
    pub id: String,
    pub body_sha256: String,
    pub attempt: Attempt,
    pub clock: Clock,
    pub metrics: Option<metrics::Reference>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub version: u32,
    pub kind: String,
    pub provenance: Provenance,
    pub plan_sha256: String,
    pub collector_sha256: String,
    pub slot: usize,
    pub stage: Stage,
    pub previous_sha256: Option<String>,
    pub store_sha256: Option<String>,
    pub engine: Option<ProcessIdentity>,
    pub cache_server: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_namespaces: Option<[String; 2]>,
    pub events_sha256: String,
    pub requests: Vec<RequestRecord>,
    pub failures: Vec<String>,
}
#[derive(Serialize)]
pub struct Inspection {
    pub version: u32,
    pub claim: &'static str,
    pub provenance: Provenance,
    pub slot: Slot,
    pub stage: Stage,
    pub outcome: Outcome,
    pub launch_anchor: &'static str,
    pub reasons: Vec<String>,
    pub durations: Vec<DurationSample>,
    pub states: Vec<ClassState>,
    pub cache_result: Option<CacheResult>,
    pub request_usage: Vec<model::Usage>,
    pub metrics: Option<metrics::Summary>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DurationSample {
    pub target: Target,
    pub clock: Clock,
    pub duration_us: u64,
}
struct Loaded {
    plan: Plan,
    capture: Capture,
    digest: String,
    inspection: Inspection,
}
fn process(pid: u32) -> Result<ProcessIdentity> {
    if pid == 0 {
        return Err("PID zero is not an attach target".into());
    }
    let raw = evidence::read(Path::new(&format!("/proc/{pid}/stat")), 8192)?;
    let text = std::str::from_utf8(&raw).map_err(|_| "invalid process stat")?;
    let end = text
        .rfind(')')
        .ok_or("invalid process stat comm boundary")?;
    let fields: Vec<_> = text[end + 1..].split_whitespace().collect();
    if fields.len() < 20 || matches!(fields[0], "Z" | "X" | "x") {
        return Err("observed process exited".into());
    }
    let boot = evidence::read(Path::new("/proc/sys/kernel/random/boot_id"), 128)?;
    Ok(ProcessIdentity {
        pid,
        start_ticks: fields[19]
            .parse()
            .map_err(|_| "invalid process starttime")?,
        boot_id: std::str::from_utf8(&boot)
            .map_err(|_| "invalid boot identity")?
            .trim()
            .into(),
    })
}
fn parse_events(raw: &[u8], plan: &Plan) -> Result<Vec<Event>> {
    if raw.len() > plan.event_bytes {
        return Err("event source byte budget exhausted".into());
    }
    let mut events = Vec::new();
    let mut clocks: std::collections::BTreeMap<String, (Clock, u64)> =
        std::collections::BTreeMap::new();
    for line in raw.split_inclusive(|b| *b == b'\n') {
        if line.last() != Some(&b'\n') {
            return Err("incomplete journal event".into());
        }
        if events.len() == plan.max_events {
            return Err("event count budget exhausted".into());
        }
        let e: Event = decode(line)?;
        e.clock.validate()?;
        if e.version != 1 || e.sequence != events.len() || e.offset_us > plan.deadline_us {
            return Err("event version, sequence or timestamp budget mismatch".into());
        }
        if let Some((clock, offset)) = clocks.get(&e.clock.id)
            && (clock != &e.clock || *offset > e.offset_us)
        {
            return Err("clock contract changed or time regressed".into());
        }
        clocks.insert(e.clock.id.clone(), (e.clock.clone(), e.offset_us));
        if let Some(previous) = events.last() {
            let previous: &Event = previous;
            if previous.clock.id == e.clock.id
                && (previous.clock != e.clock || previous.offset_us > e.offset_us)
            {
                return Err("clock changed or journal timestamps regressed".into());
            }
            if matches!(previous.event, EventKind::Sealed { .. }) {
                return Err("events after sealed admission".into());
            }
        }
        events.push(e);
    }
    Ok(events)
}

/// File polling is bounded and only observes the explicitly selected ordinary file.
/// Every prefix and inode must remain stable; malformed bytes are retained, not skipped.
struct Journal {
    path: PathBuf,
    inode: Option<(u64, u64)>,
    raw: Vec<u8>,
}
impl Journal {
    fn poll(&mut self, plan: &Plan) -> Result<()> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && self.inode.is_none() => {
                return Ok(());
            }
            Err(e) => return Err(format!("event source read: {e}")),
        };
        let m = file.metadata().map_err(|e| e.to_string())?;
        if !m.is_file() {
            return Err("event source is not a regular file".into());
        }
        let identity = (m.dev(), m.ino());
        if self.inode.is_some_and(|old| old != identity) {
            return Err("event source incarnation changed".into());
        }
        self.inode = Some(identity);
        let mut next = Vec::new();
        file.take(plan.event_bytes as u64 + 1)
            .read_to_end(&mut next)
            .map_err(|e| e.to_string())?;
        if !next.starts_with(&self.raw) {
            return Err("event source rewrote retained prefix".into());
        }
        self.raw = next;
        if self.raw.len() > plan.event_bytes {
            self.raw.truncate(plan.event_bytes);
            return Err("event source exceeded retained byte budget".into());
        }
        Ok(())
    }
    fn complete_events(&self, plan: &Plan) -> Result<Vec<Event>> {
        let end = self
            .raw
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |n| n + 1);
        parse_events(&self.raw[..end], plan)
    }
}
fn runtime_namespaces(pid: u32) -> Result<[String; 2]> {
    let mut observed = [String::new(), String::new()];
    for (index, kind) in ["pid", "time"].iter().enumerate() {
        let target =
            std::fs::read_link(format!("/proc/{pid}/ns/{kind}")).map_err(|e| e.to_string())?;
        let local =
            std::fs::read_link(format!("/proc/self/ns/{kind}")).map_err(|e| e.to_string())?;
        if target != local {
            return Err(
                "runtime bridge requires shared collector/producer PID and time namespaces".into(),
            );
        }
        observed[index] = target.to_str().ok_or("invalid namespace identity")?.into();
    }
    Ok(observed)
}

async fn await_journal(
    journal: &mut Journal,
    plan: &Plan,
    engine: &Option<ProcessIdentity>,
    deadline: Instant,
    sealed: bool,
) -> Result<()> {
    loop {
        journal.poll(plan)?;
        let events = journal.complete_events(plan)?;
        if events
            .iter()
            .any(|e| matches!(e.event, EventKind::Failure { .. }))
        {
            return Err("producer reported a failed startup or inference".into());
        }
        if plan.adapter.runtime()
            && let Some(event) = events.first()
        {
            let id = engine
                .as_ref()
                .ok_or("runtime process identity unavailable")?;
            let namespaces = runtime_namespaces(id.pid)?;
            match &event.event {
                EventKind::RuntimeStart {
                    producer_sha256,
                    source_contract,
                    pid_namespace,
                    time_namespace,
                } if event.engine == *id
                    && producer_sha256 == &plan.adapter_sha256
                    && source_contract == plan.adapter.source_contract()
                    && pid_namespace == &namespaces[0]
                    && time_namespace == &namespaces[1] => {}
                _ => {
                    return Err(
                        "runtime journal process, source or namespace binding mismatch".into(),
                    );
                }
            }
        }
        if let Some(id) = engine
            && process(id.pid)? != *id
        {
            return Err("attached engine incarnation changed".into());
        }
        let found = if sealed {
            events
                .last()
                .is_some_and(|e| matches!(e.event, EventKind::Sealed { .. }))
        } else {
            events.iter().any(|e| matches!(&e.event, EventKind::Ready { contract } if contract == plan.adapter.ready_contract()))
                && (!plan.adapter.runtime() || events.iter().any(|e| matches!(e.event, EventKind::Listening)))
        };
        if found {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("startup observation deadline exhausted".into());
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
}

fn request_dir(root: &Path, index: usize) -> PathBuf {
    root.join(format!("request-{index:02}"))
}

async fn collect_request(
    plan: &Plan,
    root: &Path,
    id: &str,
    index: usize,
    plan_hash: &str,
    window: std::ops::Range<Instant>,
    budget: &mut metrics::Budget,
) -> Result<RequestRecord> {
    let (origin, deadline) = (window.start, window.end);
    let dir = request_dir(root, index);
    evidence::fresh(&dir)?;
    let body = request_body(plan)?;
    let body_sha256 = evidence::digest(body.as_bytes());
    evidence::write(&dir.join("body.bin"), body.as_bytes())?;
    let client = wire::client(plan.local_http, 1)?;
    let url = wire::endpoint(&plan.endpoint, plan.local_http)?;
    let auth = wire::credential(plan.auth_env.as_deref())?;
    let mut request = wire::request(&client, &url, auth.as_ref(), body, false)?;
    if plan.adapter.runtime() {
        request.headers_mut().insert(
            "x-grill-startup-request",
            id.parse().map_err(|_| "invalid startup request header")?,
        );
        request.headers_mut().insert(
            "x-grill-startup-plan",
            plan_hash
                .parse()
                .map_err(|_| "invalid startup plan header")?,
        );
    }
    let clock = Clock {
        id: "client-acquisition".into(),
        kind: "monotonic".into(),
        units: "microseconds".into(),
        resolution_us: 1,
        synchronization: "single-process-origin".into(),
    };
    let metrics_auth = wire::credential(plan.metrics.as_ref().and_then(|c| c.auth_env.as_deref()))?;
    let ctx = metrics::CaptureCtx {
        origin,
        auth: metrics_auth.as_ref(),
        cancelled: None,
    };
    let before = if let Some(config) = &plan.metrics {
        Some(metrics::v2::scrape(&client, config, budget, &dir, "before", &ctx).await?)
    } else {
        None
    };
    let measured_origin = metrics::offset_us(origin, Instant::now())?;
    let measured_unix = metrics::unix_ms();
    let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
    let mut collected = wire::collect(
        client.clone(),
        request,
        plan.limits.clone(),
        settings(plan),
        wire::CollectContext {
            lane: 0,
            origin,
            deadline: Some(deadline),
            cancel,
            first_generated: None,
            tool_expectation: None,
        },
    )
    .await;
    let measured_duration = metrics::offset_us(origin, Instant::now())?
        .checked_sub(measured_origin)
        .ok_or("request clock underflow")?;
    collected.attempt.response_sha256 = evidence::digest(&collected.body);
    evidence::write(&dir.join("response.bin"), &collected.body)?;
    let reference = if let (Some(config), Some(before)) = (&plan.metrics, before) {
        let after = metrics::v2::scrape(&client, config, budget, &dir, "after", &ctx).await?;
        let duration = before
            .duration_us
            .checked_add(after.duration_us)
            .ok_or("metrics duration overflow")?;
        let publication = Instant::now();
        let mut reference = metrics::v2::publish(
            &dir,
            &metrics::v2::Receipt {
                version: 2,
                plan_sha256: plan_hash.into(),
                wave: index as u32,
                before,
                after,
                measured_origin_offset_us: measured_origin,
                measured_origin_unix_ms: measured_unix,
                measured_duration_us: measured_duration,
            },
        )?;
        // Include snapshots and publication in the same charged overhead convention.
        reference.overhead_us = duration
            .checked_add(wire::us(publication))
            .ok_or("metrics overhead overflow")?;
        budget.finish_wave(reference.overhead_us, duration)?;
        Some(reference)
    } else {
        None
    };
    evidence::sync(&dir)?;
    Ok(RequestRecord {
        id: id.into(),
        body_sha256,
        attempt: collected.attempt,
        clock,
        metrics: reference,
    })
}

#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
pub fn capture(args: &CaptureArgs, stage: Stage, store: Option<&Path>) -> Result<Inspection> {
    let plan_bytes = evidence::read(&args.plan, CAP)?;
    let plan: Plan = decode(&plan_bytes)?;
    plan.validate()?;
    if args.slot >= plan.slots.len()
        || plan.adapter == Adapter::UninstrumentedRecipeLogs
        || !matches!(
            (&plan.study, stage),
            (Study::Startup, Stage::Startup)
                | (Study::RestartCache { .. }, Stage::Store | Stage::Reload)
        )
    {
        return Err("capture stage, slot or native producer is unsupported".into());
    }
    let producer = evidence::read(&args.producer_source, CAP)?;
    if evidence::digest(&producer) != plan.adapter_sha256 {
        return Err("producer source pin mismatch".into());
    }
    let binary = evidence::binary_digest()?;
    if binary != plan.collector_sha256 {
        return Err("collector binary pin mismatch".into());
    }
    // Resolve credentials before creating a partial acquisition, not from process environments.
    wire::credential(plan.auth_env.as_deref())?;
    wire::credential(plan.metrics.as_ref().and_then(|c| c.auth_env.as_deref()))?;
    let plan_hash = evidence::digest(&plan_bytes);
    let previous_sha256 = if let Some(previous) = &args.previous {
        let loaded = load(previous)?;
        if loaded.capture.slot + 1 != args.slot
            || loaded.capture.stage == Stage::Store
            || loaded.capture.plan_sha256 != plan_hash
        {
            return Err("previous acquisition is not the preceding declared slot".into());
        }
        Some(loaded.digest)
    } else {
        if args.slot != 0 {
            return Err("later slot requires --previous retained acquisition".into());
        }
        None
    };
    let stored = if let Some(path) = store {
        let loaded = load(path)?;
        if stage != Stage::Reload
            || loaded.capture.stage != Stage::Store
            || loaded.capture.plan_sha256 != plan_hash
            || loaded.capture.slot != args.slot
            || loaded.capture.previous_sha256 != previous_sha256
        {
            return Err("store evidence belongs to another study, slot or predecessor".into());
        }
        Some(loaded)
    } else {
        if stage == Stage::Reload {
            return Err("reload requires retained store evidence".into());
        }
        None
    };
    evidence::fresh(&args.out)?;
    evidence::write(&args.out.join("plan.json"), &plan_bytes)?;
    evidence::write(&args.out.join("producer.bin"), &producer)?;
    let store_sha256 = if let (Some(path), Some(loaded)) = (store, &stored) {
        copy_capture(path, &args.out.join("store"), loaded, false)?;
        Some(evidence::digest(&evidence::read(
            &args.out.join("store/capture.json"),
            CAP,
        )?))
    } else {
        None
    };
    let mut capture = Capture {
        version: 1,
        kind: "startup-observation-v1".into(),
        provenance: Provenance::NativeObserved,
        plan_sha256: plan_hash,
        collector_sha256: binary,
        slot: args.slot,
        stage,
        previous_sha256,
        store_sha256,
        engine: None,
        cache_server: None,
        runtime_namespaces: None,
        events_sha256: String::new(),
        requests: Vec::new(),
        failures: Vec::new(),
    };
    match process(args.pid) {
        Ok(id) => capture.engine = Some(id),
        Err(e) => capture.failures.push(e),
    }
    if let Some(pid) = args.cache_pid {
        match process(pid) {
            Ok(id) => capture.cache_server = Some(id),
            Err(e) => capture.failures.push(e),
        }
    }
    if plan.adapter.runtime() {
        match runtime_namespaces(args.pid) {
            Ok(namespaces) => capture.runtime_namespaces = Some(namespaces),
            Err(e) => capture.failures.push(e),
        }
    }
    let mut journal = Journal {
        path: args.events.clone(),
        inode: None,
        raw: Vec::new(),
    };
    let capture_origin = Instant::now();
    let deadline = capture_origin + Duration::from_micros(plan.deadline_us);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        if capture.failures.is_empty()
            && let Err(e) =
                await_journal(&mut journal, &plan, &capture.engine, deadline, false).await
        {
            capture.failures.push(e);
        }
        let mut budget = metrics::Budget::default();
        if capture.failures.is_empty() {
            for (index, id) in plan.requests(stage).iter().enumerate() {
                // Reserve the after-scrape allowance before dispatch; no automatic retry.
                if Instant::now()
                    + Duration::from_millis(u64::from(plan.limits.total_ms))
                    + Duration::from_secs(if plan.metrics.is_some() { 4 } else { 0 })
                    > deadline
                {
                    capture
                        .failures
                        .push("insufficient remaining request/scrape allowance".into());
                    break;
                }
                match collect_request(
                    &plan,
                    &args.out,
                    id,
                    index,
                    &capture.plan_sha256,
                    capture_origin..deadline,
                    &mut budget,
                )
                .await
                {
                    Ok(record) => {
                        let failed = record.attempt.status != Status::Complete;
                        capture.requests.push(record);
                        if failed {
                            capture
                                .failures
                                .push("declared inference failed; no replacement request".into());
                            break;
                        }
                    }
                    Err(e) => {
                        capture.failures.push(e);
                        break;
                    }
                }
            }
            if let Err(e) =
                await_journal(&mut journal, &plan, &capture.engine, deadline, true).await
            {
                capture.failures.push(e);
            }
        }
        if let Err(e) = journal.poll(&plan) {
            capture.failures.push(e);
        }
    });
    for id in [&capture.engine, &capture.cache_server]
        .into_iter()
        .flatten()
    {
        if process(id.pid).as_ref() != Ok(id) {
            capture
                .failures
                .push("process exited or changed incarnation during observation".into());
        }
    }
    if plan.adapter.runtime() && runtime_namespaces(args.pid).ok() != capture.runtime_namespaces {
        capture
            .failures
            .push("runtime PID/time namespace changed during observation".into());
    }
    capture.events_sha256 = evidence::digest(&journal.raw);
    evidence::write(&args.out.join("events.bin"), &journal.raw)?;
    evidence::publish(&args.out, "capture.json", &capture)?;
    Ok(load(&args.out)?.inspection)
}

fn add_reason(report: &mut Inspection, outcome: Outcome, reason: impl Into<String>) {
    report.outcome = report.outcome.max(outcome);
    report.reasons.push(reason.into());
}
fn duration(report: &mut Inspection, target: Target, start: Option<&Event>, end: Option<&Event>) {
    match (start, end) {
        (Some(a), Some(b)) if a.clock == b.clock && a.engine == b.engine => {
            if let Some(us) = b.offset_us.checked_sub(a.offset_us).filter(|n| *n > 0) {
                report.durations.push(DurationSample {
                    target,
                    clock: a.clock.clone(),
                    duration_us: us,
                });
            } else {
                add_reason(
                    report,
                    Outcome::Error,
                    "nonpositive or reversed milestone duration",
                );
            }
        }
        _ => add_reason(
            report,
            Outcome::Inconclusive,
            format!("{target:?}: missing boundary or incompatible clock/incarnation"),
        ),
    }
}
fn response_valid(plan: &Plan, record: &RequestRecord, bytes: &[u8]) -> Result<()> {
    let a = &record.attempt;
    a.timing.validate(false)?;
    a.timing.validate_tools(false)?;
    if a.response_sha256 != evidence::digest(bytes)
        || a.response_bytes != bytes.len()
        || a.lane != 0
        || a.sequence.is_some()
        || record.body_sha256 != evidence::digest(request_body(plan)?.as_bytes())
    {
        return Err("retained request/response integrity mismatch".into());
    }
    if a.status == Status::Complete {
        if !a.dispatched
            || a.http_status != Some(200)
            || a.finish_reason.as_deref() != Some("stop")
            || a.surplus_observed_bytes != 0
            || !a.eligibility_errors.is_empty()
            || a.timing.settle_us == 0
            || a.timing.settle_us > u64::from(plan.limits.total_ms) * 1000
            || a.usage.prompt_tokens.is_none_or(|n| n == 0)
            || a.usage
                .completion_tokens
                .is_none_or(|n| n == 0 || n > u64::from(plan.request.max_tokens))
        {
            return Err("invalid complete startup inference exposure".into());
        }
        wire::verify_complete(a, bytes, false, true, model::Profile::PortableChatV1)?;
        let response: serde_json::Value = decode(bytes)?;
        if response
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            != Some(plan.request.expected_answer.as_str())
        {
            return Err(
                "startup inference answer differs from the prospective exact answer".into(),
            );
        }
    }
    Ok(())
}

#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
fn analyze(root: &Path, plan: &Plan, c: &Capture, raw: &[u8]) -> Result<Inspection> {
    let mut report = Inspection {
        version: 1,
        claim: "finite-domain-observation-not-live-qualification",
        provenance: c.provenance,
        slot: plan.slots[c.slot].clone(),
        stage: c.stage,
        launch_anchor: if plan.adapter.runtime() {
            "bridge-entrypoint-not-os-launch"
        } else {
            "fixture-first-event-not-os-launch"
        },
        outcome: Outcome::Pass,
        reasons: Vec::new(),
        durations: Vec::new(),
        states: Vec::new(),
        cache_result: None,
        request_usage: c.requests.iter().map(|r| r.attempt.usage.clone()).collect(),
        metrics: None,
    };
    for reason in &c.failures {
        add_reason(&mut report, Outcome::Inconclusive, reason.clone());
    }
    let events = match parse_events(raw, plan) {
        Ok(events) => events,
        Err(e) => {
            add_reason(
                &mut report,
                Outcome::Error,
                format!("malformed journal: {e}"),
            );
            Vec::new()
        }
    };
    let expected = plan.requests(c.stage);
    if c.requests.len() != expected.len() {
        add_reason(
            &mut report,
            Outcome::Inconclusive,
            "missing declared request acquisitions",
        );
    }
    let mut budget = metrics::Budget::default();
    let mut metric_waves = Vec::new();
    for (index, record) in c.requests.iter().enumerate() {
        record.clock.validate()?;
        if record.clock.id != "client-acquisition" || record.clock.resolution_us != 1 {
            return Err("unsupported retained client request clock".into());
        }
        if expected.get(index) != Some(&record.id) {
            return Err("request order/identity mismatch".into());
        }
        let dir = request_dir(root, index);
        let body = evidence::read(&dir.join("body.bin"), model::REQUEST_CAP)?;
        let response = evidence::read(&dir.join("response.bin"), plan.limits.response_bytes)?;
        if body != request_body(plan)?.as_bytes() {
            return Err("request bytes differ from frozen study identity".into());
        }
        if let Err(e) = response_valid(plan, record, &response) {
            add_reason(&mut report, Outcome::Error, e);
        }
        if record.attempt.status != Status::Complete {
            add_reason(
                &mut report,
                Outcome::Inconclusive,
                format!("{}: {:?}", record.id, record.attempt.status),
            );
        }
        match (&plan.metrics, &record.metrics) {
            (Some(config), Some(reference)) => {
                let summary = metrics::v2::load(
                    &dir,
                    reference,
                    config,
                    &c.plan_sha256,
                    index as u32,
                    &mut budget,
                )?;
                let start = summary.receipt.measured_origin_offset_us;
                let finish = start
                    .checked_add(summary.receipt.measured_duration_us)
                    .ok_or("metrics interval overflow")?;
                let a = &record.attempt.timing;
                if a.dispatch_offset_us < start
                    || a.dispatch_offset_us
                        .checked_add(a.settle_us)
                        .is_none_or(|end| end > finish)
                {
                    return Err("metrics interval does not contain actual request".into());
                }
                if summary.receipt.before.status != metrics::v2::Status::Complete
                    || summary.receipt.after.status != metrics::v2::Status::Complete
                {
                    add_reason(
                        &mut report,
                        Outcome::Inconclusive,
                        "required metrics2 snapshots unavailable",
                    );
                }
                if summary
                    .accounting
                    .iter()
                    .any(|a| a.status == "inconsistent")
                {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "provider accounting contradiction",
                    );
                }
                metric_waves.push(metrics::WaveReport::V2(summary));
            }
            (None, None) => (),
            _ => add_reason(
                &mut report,
                Outcome::Inconclusive,
                "required metrics companion missing",
            ),
        }
    }
    let protocol = plan
        .metrics
        .clone()
        .map(|config| metrics::Protocol::V2(Box::new(config)));
    report.metrics = metrics::summarize(protocol.as_ref(), budget, metric_waves);
    let mut launched = None;
    let mut listening = None;
    let mut ready = None;
    let mut communication_start = None;
    let mut communication_end = None;
    let mut bootstrap_start = None;
    let mut bootstrap_end = None;
    let mut bootstrap_invalid = false;
    let mut first_inference = None;
    let mut starts = Vec::new();
    let mut finishes = Vec::new();
    let mut controls = None;
    let mut sealed = None;
    let mut runtime_coverage = None;
    let mut classes = BTreeSet::new();
    let mut engine_requests = 0usize;
    for event in &events {
        if c.engine.as_ref() != Some(&event.engine) {
            add_reason(
                &mut report,
                Outcome::Error,
                "stale or missing engine incarnation on event",
            );
        }
        if runtime_coverage.is_some() && !matches!(event.event, EventKind::Sealed { .. }) {
            add_reason(
                &mut report,
                Outcome::Error,
                "runtime events after admission coverage closure",
            );
        }
        match &event.event {
            EventKind::RuntimeStart {
                producer_sha256,
                source_contract,
                pid_namespace,
                time_namespace,
            } => {
                let expected_contract = plan.adapter.source_contract();
                if !plan.adapter.runtime()
                    || launched.replace(event).is_some()
                    || event.sequence != 0
                    || producer_sha256 != &plan.adapter_sha256
                    || source_contract != expected_contract
                    || c.runtime_namespaces.as_ref().is_none_or(|namespaces| {
                        namespaces[0] != *pid_namespace || namespaces[1] != *time_namespace
                    })
                {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "runtime source, namespace or entrypoint anchor mismatch",
                    );
                }
            }
            EventKind::RuntimeCoverage {
                window_us,
                admission_closed,
                active_requests,
                engine_hook,
            } => {
                if !plan.adapter.runtime()
                    || runtime_coverage.replace(event).is_some()
                    || Some(*window_us) != plan.runtime_window_us
                    || !admission_closed
                    || *active_requests != 0
                    || (matches!(
                        plan.adapter,
                        Adapter::RuntimeVllmV1
                            | Adapter::RuntimeVllm487V1
                            | Adapter::RuntimeVllm487BootstrapV1
                            | Adapter::RuntimeVllm752BootstrapV1
                    ) && !engine_hook)
                    || starts.len() != finishes.len()
                    || listening.is_none()
                    || ready.is_none_or(|start: &Event| {
                        start.clock != event.clock
                            || start.engine != event.engine
                            || event
                                .offset_us
                                .checked_sub(start.offset_us)
                                .is_none_or(|elapsed| elapsed < *window_us)
                    })
                {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "incomplete runtime request coverage interval",
                    );
                }
            }
            EventKind::RuntimeEngineRequest { id } => {
                if !plan.adapter.runtime()
                    || starts.len() != finishes.len() + 1
                    || starts.last().map(|(request, _)| *request) != Some(id)
                {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "uncorrelated runtime engine admission",
                    );
                }
                engine_requests += 1;
            }
            EventKind::Launched => {
                if plan.adapter.runtime()
                    || launched.replace(event).is_some()
                    || event.sequence != 0
                {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "duplicate or noninitial launch event",
                    );
                }
            }
            EventKind::Listening => {
                if listening.replace(event).is_some() || launched.is_none() {
                    add_reason(&mut report, Outcome::Error, "listening source order");
                }
            }
            EventKind::Ready { contract } => {
                if plan.adapter.bootstrap()
                    && bootstrap_end.is_some_and(|end: &Event| {
                        end.clock != event.clock
                            || end.engine != event.engine
                            || end.offset_us >= event.offset_us
                    })
                {
                    bootstrap_invalid = true;
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "bootstrap end must precede ASGI ready on the frontend clock",
                    );
                }
                if contract != plan.adapter.ready_contract()
                    || ready.replace(event).is_some()
                    || launched.is_none()
                    || (!plan.adapter.runtime() && listening.is_none())
                {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "unsupported/malformed readiness contract or order",
                    );
                }
            }
            EventKind::BootstrapStart => {
                if !plan.adapter.bootstrap()
                    || bootstrap_start.replace(event).is_some()
                    || bootstrap_end.is_some()
                    || ready.is_some()
                    || launched.is_none_or(|start: &Event| {
                        !matches!(start.event, EventKind::RuntimeStart { .. })
                            || start.clock != event.clock
                            || start.engine != event.engine
                            || start.offset_us >= event.offset_us
                    })
                {
                    bootstrap_invalid = true;
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "bootstrap start contract, clock or order",
                    );
                }
            }
            EventKind::BootstrapEnd => {
                if !plan.adapter.bootstrap()
                    || bootstrap_end.replace(event).is_some()
                    || ready.is_some()
                    || bootstrap_start.is_none_or(|start: &Event| {
                        start.clock != event.clock
                            || start.engine != event.engine
                            || start.offset_us >= event.offset_us
                    })
                {
                    bootstrap_invalid = true;
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "bootstrap end contract, clock or order",
                    );
                }
            }
            EventKind::CommunicationStart => {
                if communication_start.replace(event).is_some() {
                    add_reason(&mut report, Outcome::Error, "duplicate communication start");
                }
            }
            EventKind::CommunicationEnd => {
                if communication_end.replace(event).is_some() || communication_start.is_none() {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "communication substage boundary order",
                    );
                }
            }
            EventKind::Controls {
                smoke_suppressed,
                shape_warmup_suppressed,
                hash_seed_contract,
            } => {
                if controls
                    .replace((
                        *smoke_suppressed,
                        *shape_warmup_suppressed,
                        hash_seed_contract,
                    ))
                    .is_some()
                    || !starts.is_empty()
                {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "late or duplicate suppression controls",
                    );
                }
            }
            EventKind::State { state } => {
                if !classes.insert(state.class) {
                    add_reason(&mut report, Outcome::Error, "duplicate class identity");
                }
                if let Some(identity) = &state.identity {
                    let valid = match (state.class, identity) {
                        (Class::Engine, ClassIdentity::Process { identity }) => {
                            c.engine.as_ref() == Some(identity)
                        }
                        (Class::CacheServer, ClassIdentity::Process { identity }) => {
                            c.cache_server.as_ref() == Some(identity)
                        }
                        (Class::Engine | Class::CacheServer, _) => false,
                        (
                            _,
                            ClassIdentity::Artifact {
                                content_sha256,
                                generation,
                            },
                        ) => evidence::is_digest(content_sha256) && short(generation),
                        _ => false,
                    };
                    if !valid {
                        add_reason(
                            &mut report,
                            Outcome::Error,
                            "class identity type or native process binding mismatch",
                        );
                    }
                }
                let mut observed = state.clone();
                if plan.adapter.runtime()
                    && state.class == Class::CacheServer
                    && state.identity.is_none()
                {
                    // Identity of the explicitly selected process, not proof it served KV.
                    observed.identity = c
                        .cache_server
                        .clone()
                        .map(|identity| ClassIdentity::Process { identity });
                }
                report.states.push(observed);
            }
            EventKind::RequestStart { id, body_sha256 } => {
                if ready.is_none()
                    || starts.len() != finishes.len()
                    || (plan.adapter.runtime() && listening.is_none())
                {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "request before readiness or overlapping/reordered source requests",
                    );
                }
                starts.push((id, body_sha256));
            }
            EventKind::InferenceComplete {
                id,
                response_sha256,
                cache_identity,
                cache_result,
            } => {
                let index = finishes.len();
                if starts.get(index).map(|(id, _)| *id) != Some(id) {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "response has no ordered request start",
                    );
                }
                if plan.adapter.runtime() && engine_requests != 1 {
                    add_reason(
                        &mut report,
                        Outcome::Inconclusive,
                        "runtime inference requires exactly one engine admission",
                    );
                }
                engine_requests = 0;
                if let Some(record) = c.requests.get(index) {
                    if record.id != *id
                        || record.attempt.response_sha256 != *response_sha256
                        || record.attempt.status != Status::Complete
                    {
                        add_reason(
                            &mut report,
                            Outcome::Error,
                            "source inference is not supported by actual complete client response",
                        );
                    } else if index == 0 {
                        first_inference = Some(event);
                    }
                } else {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "extra inference not in retained client requests",
                    );
                }
                if let Study::RestartCache { identity, .. } = &plan.study {
                    if cache_identity.as_ref() != Some(identity) {
                        add_reason(
                            &mut report,
                            Outcome::Error,
                            "changed or missing prompt/cache/hash-seed identity",
                        );
                    }
                    if let Some(record) = c.requests.get(index) {
                        match (cache_result, record.attempt.usage.cached_prompt_tokens) {
                            (CacheResult::PersistedHit, Some(n)) if n > 0 => (),
                            (CacheResult::Stored | CacheResult::Recomputed, Some(0)) => (),
                            (CacheResult::Unavailable, _) => add_reason(
                                &mut report,
                                Outcome::Inconclusive,
                                "per-request cache accounting unavailable",
                            ),
                            _ => add_reason(
                                &mut report,
                                Outcome::Error,
                                "source cache result contradicts actual response usage",
                            ),
                        }
                    }
                }
                if index == 0 {
                    report.cache_result = Some(*cache_result);
                }
                finishes.push(id);
            }
            EventKind::Failure { detail } => {
                if bootstrap_start.is_some() && bootstrap_end.is_none() {
                    bootstrap_invalid = true;
                }
                add_reason(
                    &mut report,
                    Outcome::Inconclusive,
                    format!("source failure: {detail}"),
                );
            }
            EventKind::Sealed { requests } => {
                sealed = Some(*requests);
                if *requests != starts.len() {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "sealed request accounting mismatch",
                    );
                }
                if plan.adapter.runtime() && runtime_coverage.is_none() {
                    add_reason(
                        &mut report,
                        Outcome::Error,
                        "runtime seal without observed coverage closure",
                    );
                }
            }
        }
    }
    if starts.len() != expected.len()
        || finishes.len() != expected.len()
        || sealed != Some(expected.len())
    {
        // All adapters require complete source coverage, never an attestation alone.
        add_reason(
            &mut report,
            Outcome::Inconclusive,
            "complete request-order/admission-closure evidence unavailable",
        );
    }
    if starts.len() > expected.len() || finishes.len() > expected.len() {
        add_reason(
            &mut report,
            Outcome::Error,
            "extra/hidden inference requests",
        );
    }
    if plan.adapter.runtime() && runtime_coverage.is_none() {
        add_reason(
            &mut report,
            Outcome::Inconclusive,
            "runtime coverage seal missing",
        );
    }
    if plan.adapter.bootstrap()
        && (bootstrap_start.is_none() || bootstrap_end.is_none() || ready.is_none())
    {
        add_reason(
            &mut report,
            Outcome::Inconclusive,
            "bootstrap completion before ASGI ready unavailable",
        );
    }
    let body_hash = evidence::digest(request_body(plan)?.as_bytes());
    for (index, (id, body)) in starts.iter().enumerate() {
        if expected.get(index) != Some(*id) || **body != body_hash {
            add_reason(
                &mut report,
                Outcome::Error,
                "hidden, reordered or changed request bytes",
            );
        }
    }
    for condition in &plan.states {
        match report.states.iter().find(|s| s.class == condition.class) {
            Some(state) if state.declared != condition.declared => {
                add_reason(
                    &mut report,
                    Outcome::Error,
                    "cache-state declaration changed from prospective exposure",
                );
            }
            Some(state) if state.observed == condition.required_observed => (),
            _ => add_reason(
                &mut report,
                Outcome::Inconclusive,
                format!(
                    "{:?} required observed cache state unavailable or refuted",
                    condition.class
                ),
            ),
        }
    }
    for target in plan.gates.iter().map(|g| &g.target) {
        match target {
            Target::Listening => duration(&mut report, target.clone(), launched, listening),
            Target::Ready => duration(&mut report, target.clone(), launched, ready),
            Target::FirstValidInference => {
                duration(&mut report, target.clone(), launched, first_inference)
            }
            Target::Communication => duration(
                &mut report,
                target.clone(),
                communication_start,
                communication_end,
            ),
            Target::Bootstrap => {
                let end = bootstrap_end.filter(|_| !bootstrap_invalid && ready.is_some());
                duration(&mut report, target.clone(), bootstrap_start, end);
            }
            Target::Reload if c.stage == Stage::Reload => {
                if let Some(record) = c
                    .requests
                    .first()
                    .filter(|r| r.attempt.status == Status::Complete)
                {
                    report.durations.push(DurationSample {
                        target: Target::Reload,
                        clock: record.clock.clone(),
                        duration_us: record.attempt.timing.settle_us,
                    });
                } else {
                    add_reason(
                        &mut report,
                        Outcome::Inconclusive,
                        "actual first-reload response unavailable",
                    );
                }
            }
            Target::Reload => (),
        }
    }
    if let Study::RestartCache { identity, .. } = &plan.study {
        if !matches!(controls, Some((true, true, seed)) if seed == &identity.hash_seed_contract) {
            add_reason(
                &mut report,
                Outcome::Inconclusive,
                "observed smoke/shape-warmup suppression or stable hash contract unavailable",
            );
        }
        if plan.adapter.runtime() {
            add_reason(
                &mut report,
                Outcome::Inconclusive,
                "ASGI/frontend order observed; backend warmup, cache-server hash seed and per-request persisted KV transfer remain unobservable",
            );
        }
        if plan.adapter == Adapter::UninstrumentedRecipeLogs
            || c.provenance != Provenance::NativeObserved
        {
            add_reason(
                &mut report,
                Outcome::Inconclusive,
                "import/declaration or uninstrumented logs cannot establish first-reload persistence",
            );
        }
    }
    if plan.adapter == Adapter::UninstrumentedRecipeLogs {
        add_reason(
            &mut report,
            Outcome::Inconclusive,
            "recipe source mapping does not provide complete supported native milestone evidence",
        );
    }
    Ok(report)
}

fn restart_predicates(report: &mut Inspection, store: &Inspection, plan: &Plan) {
    report.outcome = report.outcome.max(store.outcome);
    if store.outcome != Outcome::Pass {
        report
            .reasons
            .push("store acquisition is unqualified; retained under store/".into());
    }
    let Study::RestartCache { classes, .. } = &plan.study else {
        return;
    };
    for predicate in classes {
        let before = store
            .states
            .iter()
            .find(|s| s.class == predicate.class)
            .and_then(|s| s.identity.as_ref());
        let after = report
            .states
            .iter()
            .find(|s| s.class == predicate.class)
            .and_then(|s| s.identity.as_ref());
        match (before, after) {
            (Some(a), Some(b)) if (a != b) == (predicate.transition == Transition::Changed) => (),
            (Some(_), Some(_)) => add_reason(
                report,
                Outcome::Error,
                format!("{:?} incarnation predicate refuted", predicate.class),
            ),
            _ => add_reason(
                report,
                Outcome::Inconclusive,
                format!("{:?} before/after identity unavailable", predicate.class),
            ),
        }
    }
    match report.cache_result {
        Some(CacheResult::PersistedHit) => (),
        Some(CacheResult::Recomputed) => add_reason(
            report,
            Outcome::Inconclusive,
            "actual first reload recomputed; persistence claim withheld",
        ),
        _ => add_reason(
            report,
            Outcome::Inconclusive,
            "promised persisted-hit accounting unavailable",
        ),
    }
    if let Some(record) = report.metrics.as_ref().and_then(|m| m.waves.first()) {
        if let metrics::WaveReport::V2(wave) = record {
            // Existing metrics2 is diagnostic: never promote server totals to a per-request hit.
            let supported = wave.cache.len() == 1
                && wave.cache.first().is_some_and(|view| {
                    view.external_queries.status == "available"
                        && view.external_hits.status == "available"
                        && view.external_queries.delta.is_some_and(|n| n > 0.0)
                        && view.external_hits.delta.is_some_and(|n| n > 0.0)
                });
            if !supported {
                add_reason(
                    report,
                    Outcome::Inconclusive,
                    "complete single-label external query/hit diagnostics unavailable or zero",
                );
            }
        }
    } else {
        add_reason(
            report,
            Outcome::Inconclusive,
            "actual reload metrics2 evidence unavailable",
        );
    }
}

fn load(root: &Path) -> Result<Loaded> {
    evidence::directory(root)?;
    let plan_bytes = evidence::read(&root.join("plan.json"), CAP)?;
    let plan: Plan = decode(&plan_bytes)?;
    plan.validate()?;
    let bytes = evidence::read(&root.join("capture.json"), CAP)?;
    let c: Capture = decode(&bytes)?;
    if c.version != 1
        || c.kind != "startup-observation-v1"
        || c.slot >= plan.slots.len()
        || c.plan_sha256 != evidence::digest(&plan_bytes)
        || c.collector_sha256 != plan.collector_sha256
        || c.requests.len() > MAX_REQUESTS
        || c.failures.len() > 64
        || c.failures.iter().any(|s| s.len() > 4096)
        || c.previous_sha256
            .as_ref()
            .is_some_and(|s| !evidence::is_digest(s))
        || c.previous_sha256.is_none() != (c.slot == 0)
        || (c.stage == Stage::Reload) != c.store_sha256.is_some()
        || !matches!(
            (&plan.study, c.stage),
            (Study::Startup, Stage::Startup)
                | (Study::RestartCache { .. }, Stage::Store | Stage::Reload)
        )
    {
        return Err("startup capture identity, stage or bounds mismatch".into());
    }
    let producer = evidence::read(&root.join("producer.bin"), CAP)?;
    if evidence::digest(&producer) != plan.adapter_sha256 {
        return Err("source producer pin mismatch".into());
    }
    let raw = evidence::read(&root.join("events.bin"), plan.event_bytes)?;
    if evidence::digest(&raw) != c.events_sha256 {
        return Err("event source digest mismatch".into());
    }
    let mut inspection = analyze(root, &plan, &c, &raw)?;
    if let Some(hash) = &c.store_sha256 {
        // A store cannot itself reference another store: precheck before recursive load.
        let store_bytes = evidence::read(&root.join("store/capture.json"), CAP)?;
        let store_capture: Capture = decode(&store_bytes)?;
        if store_capture.stage != Stage::Store
            || store_capture.store_sha256.is_some()
            || evidence::digest(&store_bytes) != *hash
        {
            return Err("invalid nested store link".into());
        }
        let stored = load(&root.join("store"))?;
        if stored.capture.plan_sha256 != c.plan_sha256
            || stored.capture.slot != c.slot
            || stored.capture.previous_sha256 != c.previous_sha256
        {
            return Err("store/reload acquisition lineage mismatch".into());
        }
        restart_predicates(&mut inspection, &stored.inspection, &plan);
    }
    let identity_bytes = if c.provenance == Provenance::Imported {
        let original = evidence::read(&root.join("imported-capture.json"), CAP)?;
        let mut imported: Capture = decode(&original)?;
        imported.provenance = Provenance::Imported;
        imported.store_sha256.clone_from(&c.store_sha256);
        if encode(&imported)? != encode(&c)? {
            return Err("import wrapper changed original acquisition facts".into());
        }
        original
    } else {
        bytes
    };
    Ok(Loaded {
        plan,
        capture: c,
        digest: evidence::digest(&identity_bytes),
        inspection,
    })
}

fn copy_capture(source: &Path, out: &Path, loaded: &Loaded, imported: bool) -> Result<()> {
    evidence::fresh(out)?;
    for (name, cap) in [
        ("plan.json", CAP),
        ("producer.bin", CAP),
        ("events.bin", loaded.plan.event_bytes),
    ] {
        evidence::write(&out.join(name), &evidence::read(&source.join(name), cap)?)?;
    }
    for (index, record) in loaded.capture.requests.iter().enumerate() {
        let from = request_dir(source, index);
        let to = request_dir(out, index);
        evidence::fresh(&to)?;
        for (name, cap) in [
            ("body.bin", model::REQUEST_CAP),
            ("response.bin", loaded.plan.limits.response_bytes),
        ] {
            evidence::write(&to.join(name), &evidence::read(&from.join(name), cap)?)?;
        }
        if record.metrics.is_some() {
            for name in ["metrics.json", "metrics-before.bin", "metrics-after.bin"] {
                evidence::write(
                    &to.join(name),
                    &evidence::read(&from.join(name), 1024 * 1024)?,
                )?;
            }
        }
        evidence::sync(&to)?;
    }
    // Raw imported receipt is retained separately. The authoritative local wrapper is imported.
    let original = evidence::read(
        &source.join(if loaded.capture.provenance == Provenance::Imported {
            "imported-capture.json"
        } else {
            "capture.json"
        }),
        CAP,
    )?;
    let mut receipt = loaded.capture.clone();
    if loaded.capture.store_sha256.is_some() {
        let nested = load(&source.join("store"))?;
        copy_capture(&source.join("store"), &out.join("store"), &nested, imported)?;
        receipt.store_sha256 = Some(evidence::digest(&evidence::read(
            &out.join("store/capture.json"),
            CAP,
        )?));
    }
    if imported || loaded.capture.provenance == Provenance::Imported {
        evidence::write(&out.join("imported-capture.json"), &original)?;
        receipt.provenance = Provenance::Imported;
    }
    evidence::publish(out, "capture.json", &receipt)
}

#[derive(Serialize)]
pub struct GateResult {
    pub gate: Gate,
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub reference: Option<[envelope::Rational; 2]>,
    pub candidate: Option<[envelope::Rational; 2]>,
    pub counts: [usize; 3],
}
#[derive(Serialize)]
pub struct Comparison {
    pub version: u32,
    pub kind: &'static str,
    pub scope: &'static str,
    pub outcome: Outcome,
    pub reasons: Vec<String>,
    pub acquisitions: Vec<Inspection>,
    pub gates: Vec<GateResult>,
}
pub fn compare(plan_path: &Path, manifest: &Path) -> Result<Comparison> {
    let source = evidence::read(plan_path, CAP)?;
    let plan: Plan = decode(&source)?;
    plan.validate()?;
    let paths: Vec<PathBuf> = decode(&evidence::read(manifest, CAP)?)?;
    if paths.len() > plan.slots.len() {
        return Err("extra undeclared startup acquisitions".into());
    }
    let mut result = Comparison {
        version: 1,
        kind: "startup-comparison-v1",
        scope: "native-unauthenticated-domain-comparison-not-live-qualification",
        outcome: Outcome::Pass,
        reasons: Vec::new(),
        acquisitions: Vec::new(),
        gates: Vec::new(),
    };
    let mut previous = None;
    let mut distinct = BTreeSet::new();
    let mut incarnations = BTreeSet::new();
    let mut exposure: Option<Vec<(Option<u64>, Option<u64>)>> = None;
    for (index, path) in paths.iter().enumerate() {
        let loaded = load(path)?;
        if loaded.capture.plan_sha256 != evidence::digest(&source)
            || loaded.capture.slot != index
            || loaded.capture.stage == Stage::Store
            || loaded.capture.previous_sha256 != previous
            || !distinct.insert(loaded.digest.clone())
        {
            return Err("comparison plan, acquisition order or predecessor mismatch".into());
        }
        if let Some(engine) = &loaded.capture.engine
            && !incarnations.insert((engine.boot_id.clone(), engine.pid, engine.start_ticks))
        {
            return Err(
                "repeated engine incarnation is not an independent startup acquisition".into(),
            );
        }
        let usage: Vec<_> = loaded
            .inspection
            .request_usage
            .iter()
            .map(|u| (u.prompt_tokens, u.completion_tokens))
            .collect();
        if exposure.as_ref().is_some_and(|old| *old != usage) {
            result.outcome = result.outcome.max(Outcome::Inconclusive);
            result
                .reasons
                .push("actual prompt/output token exposure changed across acquisitions".into());
        }
        exposure.get_or_insert(usage);
        previous = Some(loaded.digest.clone());
        if loaded.capture.provenance != Provenance::NativeObserved {
            result.scope = "imported-comparison-only-not-real-adapter-exercised";
        }
        result.outcome = result.outcome.max(loaded.inspection.outcome);
        result.acquisitions.push(loaded.inspection);
    }
    if paths.len() != plan.slots.len() {
        result.outcome = result.outcome.max(Outcome::Inconclusive);
        result
            .reasons
            .push("all declared acquisitions, including failed warmups, are required".into());
    }
    for gate in &plan.gates {
        let mut samples: [Vec<envelope::Rational>; 3] = std::array::from_fn(|_| Vec::new());
        let mut clock: Option<&Clock> = None;
        let mut complete = paths.len() == plan.slots.len();
        for acquisition in &result.acquisitions {
            let duration = acquisition
                .durations
                .iter()
                .find(|d| d.target == gate.target);
            if acquisition.outcome != Outcome::Pass || duration.is_none() {
                complete = false;
            }
            if let Some(duration) = duration {
                if clock.is_some_and(|c| !c.compatible(&duration.clock)) {
                    complete = false;
                }
                clock = Some(&duration.clock);
                if !acquisition.slot.warmup {
                    let arm = match acquisition.slot.arm {
                        Arm::A => 0,
                        Arm::B => 1,
                        Arm::A2 => 2,
                    };
                    samples[arm].push(envelope::Rational {
                        numerator: duration.duration_us,
                        denominator: 1,
                    });
                }
            }
        }
        let counts = samples.each_ref().map(Vec::len);
        let candidate = envelope::range(&samples[1]).map_err(|e| format!("{e:?}"))?;
        let reference_values: Vec<_> = samples[0].iter().chain(&samples[2]).copied().collect();
        let reference = envelope::range(&reference_values).map_err(|e| format!("{e:?}"))?;
        let mut evaluated = GateResult {
            gate: gate.clone(),
            outcome: Outcome::Inconclusive,
            reason: None,
            reference,
            candidate,
            counts,
        };
        if complete && counts.iter().all(|n| *n >= 3) {
            if let (Some(reference), Some(candidate)) = (reference, candidate) {
                match envelope::assess(
                    reference,
                    candidate,
                    gate.max_regression_bps,
                    gate.max_reference_spread_bps,
                    envelope::Direction::LowerBetter,
                ) {
                    Ok(assessment) => {
                        evaluated.outcome = match assessment.decision {
                            envelope::EnvelopeDecision::Pass => Outcome::Pass,
                            envelope::EnvelopeDecision::Regression => Outcome::Regression,
                        }
                    }
                    Err(e) => {
                        evaluated.outcome = e.into();
                        evaluated.reason = Some(format!("{e:?}"));
                    }
                }
            }
        } else {
            evaluated.reason = Some(
                "incomplete, unqualified or incompatible-clock acquisitions; no survivor-only gate"
                    .into(),
            );
        }
        result.outcome = result.outcome.max(evaluated.outcome);
        result.gates.push(evaluated);
    }
    Ok(result)
}

pub fn execute(command: Command) -> Result<u8> {
    match command {
        Command::Observe(args) => {
            let report = capture(&args, Stage::Startup, None)?;
            crate::print_json(&report)?;
            Ok(report.outcome.exit())
        }
        Command::Store(args) => {
            let report = capture(&args, Stage::Store, None)?;
            crate::print_json(&report)?;
            Ok(report.outcome.exit())
        }
        Command::Reload {
            capture: args,
            store,
        } => {
            let report = capture(&args, Stage::Reload, Some(&store))?;
            crate::print_json(&report)?;
            Ok(report.outcome.exit())
        }
        Command::Import { source, out } => {
            let loaded = load(&source)?;
            copy_capture(&source, &out, &loaded, true)?;
            let report = load(&out)?.inspection;
            crate::print_json(&report)?;
            Ok(report.outcome.exit())
        }
        Command::Inspect { capture } => {
            let report = load(&capture)?.inspection;
            crate::print_json(&report)?;
            Ok(report.outcome.exit())
        }
        Command::Compare { plan, manifest } => {
            let report = compare(&plan, &manifest)?;
            crate::print_json(&report)?;
            Ok(report.outcome.exit())
        }
        Command::VerifyKv(args) => {
            let report = kv::verify(&args)?;
            crate::print_json(&report)?;
            Ok(report.outcome.exit())
        }
        Command::VerifyKvV2(args) => {
            let report = kv_v2::verify(&args)?;
            crate::print_json(&report)?;
            Ok(report.outcome.exit())
        }
    }
}
