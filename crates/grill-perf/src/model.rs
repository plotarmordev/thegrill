use serde::{Deserialize, Serialize};

pub type Result<T, E = String> = std::result::Result<T, E>;
pub const FILE_CAP: usize = 4 * 1024 * 1024;
pub const FRAME_CAP: usize = 256 * 1024;
pub const MAX_ATTEMPTS: u64 = 10_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: Role,
    pub content: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    PortableChatV1,
    VllmFixedV1,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Cache {
    Observe,
    ReportedPrefixZero,
    ReportedPrefixHit,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    Cap,
    Exact,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutputBudget {
    pub tokens: u32,
    pub mode: OutputMode,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestSettings {
    pub profile: Profile,
    pub stream: bool,
    pub output: OutputBudget,
    pub cache: Cache,
    pub temperature_milli: Option<u16>,
    pub top_p_milli: Option<u16>,
    pub seed: Option<i64>,
    // Omitted when absent so plans recorded before this field keep their digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub total_ms: u32,
    pub idle_ms: u32,
    pub response_bytes: usize,
    pub wave_buffer_bytes: usize,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub messages: Vec<Message>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub id: String,
    pub case: String,
    pub concurrency: u32,
    pub warmup_trials: u32,
    pub trials: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub version: u32,
    pub name: String,
    pub request: RequestSettings,
    pub limits: Limits,
    pub cases: Vec<Case>,
    pub cells: Vec<Cell>,
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
impl Workload {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || !identifier(&self.name) {
            return Err("expected workload version 1 and a short ASCII name".into());
        }
        if self.cases.is_empty()
            || self.cases.len() > 128
            || self.cells.is_empty()
            || self.cells.len() > 64
        {
            return Err("workload requires 1..128 cases and 1..64 cells".into());
        }
        let mut ids = std::collections::HashSet::new();
        for case in &self.cases {
            if !identifier(&case.id) || !ids.insert(case.id.as_str()) {
                return Err("case IDs must be unique short ASCII identifiers".into());
            }
            if case.messages.is_empty()
                || case.messages.len() > 64
                || case.messages.iter().map(|m| m.content.len()).sum::<usize>() > 128 * 1024
            {
                return Err(format!("case {} exceeds message bounds", case.id));
            }
        }
        let r = &self.request;
        if r.output.tokens == 0
            || r.output.tokens > 32_768
            || r.temperature_milli.is_some_and(|n| n > 2000)
            || r.top_p_milli.is_some_and(|n| n == 0 || n > 1000)
        {
            return Err("invalid output budget or sampling controls".into());
        }
        if r.profile == Profile::PortableChatV1
            && (r.output.mode == OutputMode::Exact
                || r.cache != Cache::Observe
                || r.thinking.is_some())
        {
            return Err("exact output, required prefix evidence, and thinking controls need the explicit vllm-fixed-v1 request profile".into());
        }
        let l = &self.limits;
        if l.total_ms == 0
            || l.total_ms > 600_000
            || l.idle_ms == 0
            || l.idle_ms > l.total_ms
            || !(1024..=8 * 1024 * 1024).contains(&l.response_bytes)
            || l.wave_buffer_bytes > 512 * 1024 * 1024
        {
            return Err("invalid deadline or buffering limits".into());
        }
        let mut names = std::collections::HashSet::new();
        let mut attempts = 0u64;
        let mut waves = 0u64;
        for cell in &self.cells {
            if !identifier(&cell.id)
                || !names.insert(cell.id.as_str())
                || !ids.contains(cell.case.as_str())
            {
                return Err("cell IDs must be unique and reference an existing case".into());
            }
            if !(1..=64).contains(&cell.concurrency)
                || cell.trials == 0
                || cell.trials > 100
                || cell.warmup_trials > 20
            {
                return Err(format!("cell {} exceeds concurrency/trial bounds", cell.id));
            }
            if r.cache == Cache::ReportedPrefixHit && cell.warmup_trials == 0 {
                return Err("reported-prefix-hit requires explicit warmup priming".into());
            }
            // Streaming semantics are frame-bounded; nonstreaming decoding is body-bounded.
            let per_request = if r.stream {
                2 * l.response_bytes + 6 * FRAME_CAP + 512 * 1024
            } else {
                6 * l.response_bytes + 512 * 1024
            };
            if per_request * cell.concurrency as usize > l.wave_buffer_bytes {
                return Err(format!(
                    "cell {} exceeds the admitted wave buffer bound",
                    cell.id
                ));
            }
            let n = u64::from(cell.trials) + u64::from(cell.warmup_trials);
            waves += n;
            attempts += n * u64::from(cell.concurrency);
        }
        if attempts > MAX_ATTEMPTS || waves > 1024 {
            return Err("workload exceeds 10,000 attempts or 1,024 waves".into());
        }
        if let Some(seed) = r.seed {
            seed.checked_add(100 * 64 + 63)
                .ok_or("seed range overflows i64")?;
        }
        Ok(())
    }
    pub fn waves(&self) -> Vec<WaveSpec> {
        let mut waves = Vec::new();
        for phase in [Phase::Warmup, Phase::Measured] {
            for cell in &self.cells {
                let trials = if phase == Phase::Warmup {
                    cell.warmup_trials
                } else {
                    cell.trials
                };
                for trial in 0..trials {
                    waves.push(WaveSpec {
                        index: waves.len() as u32,
                        phase,
                        cell: cell.id.clone(),
                        case: cell.case.clone(),
                        trial,
                        concurrency: cell.concurrency,
                    });
                }
            }
        }
        waves
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Warmup,
    Measured,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WaveSpec {
    pub index: u32,
    pub phase: Phase,
    pub cell: String,
    pub case: String,
    pub trial: u32,
    pub concurrency: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub model_revision: Option<String>,
    pub runtime: Option<String>,
    pub hardware: Option<String>,
    pub settings: Option<String>,
}
impl Deployment {
    pub fn validate(&self) -> Result<()> {
        for value in [
            &self.model_revision,
            &self.runtime,
            &self.hardware,
            &self.settings,
        ]
        .into_iter()
        .flatten()
        {
            if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                return Err(
                    "deployment declarations must be nonempty text within 4096 bytes".into(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub kind: String,
    pub tool_version: String,
    pub collector_sha256: String,
    pub workload: Workload,
    pub workload_sha256: String,
    pub source_sha256: String,
    pub model: String,
    pub deployment: Option<Deployment>,
    pub cache_mechanism: Option<String>,
    pub cache_evidence_source: String,
    pub endpoint: String,
    pub auth_env: Option<String>,
    pub local_http: bool,
    pub pool_max_idle_per_host: usize,
    pub started_unix_ms: u64,
    pub cache_namespace: Option<String>,
    pub waves: Vec<WaveSpec>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextChannel {
    Answer,
    Reasoning,
    MixedEvent,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    pub dispatch_offset_us: u64,
    pub headers_us: Option<u64>,
    pub first_body_us: Option<u64>,
    pub first_generated_text_us: Option<u64>,
    pub first_generated_channel: Option<TextChannel>,
    pub first_answer_text_us: Option<u64>,
    pub settle_us: u64,
    pub capture_parse_us: u64,
}
impl Timing {
    pub fn validate(&self, stream: bool) -> Result<()> {
        if !stream
            && (self.first_generated_text_us.is_some()
                || self.first_answer_text_us.is_some()
                || self.first_generated_channel.is_some())
        {
            return Err("nonstreaming evidence cannot contain first-text timings".into());
        }
        if self.first_generated_text_us.is_some() != self.first_generated_channel.is_some()
            || (self.first_body_us.is_some() && self.headers_us.is_none())
            || (self.first_generated_text_us.is_some() && self.first_body_us.is_none())
            || (self.first_answer_text_us.is_some() && self.first_generated_text_us.is_none())
            || self.capture_parse_us > self.settle_us
        {
            return Err("inconsistent timing observation presence".into());
        }
        let mut previous = 0;
        for value in [
            self.headers_us,
            self.first_body_us,
            self.first_generated_text_us,
            self.first_answer_text_us,
            Some(self.settle_us),
        ]
        .into_iter()
        .flatten()
        {
            if value < previous {
                return Err("response timing observations are out of order".into());
            }
            previous = value;
        }
        if matches!(
            self.first_generated_channel,
            Some(TextChannel::Answer | TextChannel::MixedEvent)
        ) && self.first_answer_text_us != self.first_generated_text_us
        {
            return Err("first generated answer event has inconsistent timing".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Complete,
    HttpError,
    Unsupported,
    Malformed,
    Incomplete,
    TotalTimeout,
    IdleTimeout,
    ResponseLimit,
    Interrupted,
    TransportError,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub lane: u32,
    pub dispatched: bool,
    pub status: Status,
    pub detail: String,
    pub http_status: Option<u16>,
    pub finish_reason: Option<String>,
    pub usage: Usage,
    pub timing: Timing,
    pub response_bytes: usize,
    pub response_sha256: String,
    pub terminal_offset: Option<usize>,
    pub surplus_observed_bytes: usize,
    pub eligibility_errors: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reservation {
    pub version: u32,
    pub plan_sha256: String,
    pub wave: WaveSpec,
    pub requests: Vec<String>,
    pub request_sha256: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Wave {
    pub version: u32,
    pub plan_sha256: String,
    pub reservation_sha256: String,
    pub spec: WaveSpec,
    pub attempts: Vec<Attempt>,
    pub elapsed_us: u64,
    pub dispatch_spread_us: u64,
    pub preparation_us: u64,
    pub reservation_publication_us: u64,
    pub body_publication_us: u64,
    pub completion_tokens: Option<u64>,
    pub achieved_completion_tokens_per_second: Option<f64>,
    pub eligible: bool,
}
pub fn eligibility(a: &Attempt, r: &RequestSettings, phase: Phase) -> Vec<String> {
    let mut errors = Vec::new();
    if a.status != Status::Complete {
        errors.push("response_not_complete".into());
    }
    match a.usage.completion_tokens {
        None => errors.push("completion_usage_unavailable".into()),
        Some(n) if n > u64::from(r.output.tokens) => {
            errors.push("reported_output_exceeds_cap".into())
        }
        Some(n) if r.output.mode == OutputMode::Exact && n != u64::from(r.output.tokens) => {
            errors.push("reported_output_not_exact".into())
        }
        _ => (),
    }
    if r.cache != Cache::Observe && !(r.cache == Cache::ReportedPrefixHit && phase == Phase::Warmup)
    {
        match a.usage.cached_prompt_tokens {
            None => errors.push("provider_prefix_cache_usage_unavailable".into()),
            Some(n) if r.cache == Cache::ReportedPrefixZero && n != 0 => {
                errors.push("provider_reported_prefix_cache_nonzero".into())
            }
            Some(0) if r.cache == Cache::ReportedPrefixHit => {
                errors.push("provider_reported_prefix_cache_miss".into())
            }
            _ => (),
        }
    }
    if let (Some(c), Some(p)) = (a.usage.cached_prompt_tokens, a.usage.prompt_tokens)
        && c > p
    {
        errors.push("inconsistent_provider_cache_usage".into());
    }
    if let (Some(reasoning), Some(completion)) =
        (a.usage.reasoning_tokens, a.usage.completion_tokens)
        && reasoning > completion
    {
        errors.push("inconsistent_provider_reasoning_usage".into());
    }
    errors
}
