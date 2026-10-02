use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T, E = String> = std::result::Result<T, E>;
pub const FILE_CAP: usize = 4 * 1024 * 1024;
pub const FRAME_CAP: usize = 256 * 1024;
pub const REQUEST_CAP: usize = 2 * 1024 * 1024;
pub const MAX_ATTEMPTS: u64 = 10_000;
pub const METRIC_CONTRACT: &str = "generated-text-arrival-v2";

/// Human-report rendering of an optional value: the value, or `null` like the JSON evidence.
pub struct OrNull<T>(pub Option<T>);
impl<T: fmt::Display> fmt::Display for OrNull<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("null"),
        }
    }
}

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
    VllmConversationV2,
    VllmConversationV3,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Cache {
    Observe,
    ReportedPrefixZero,
    ReportedPrefixHit,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OutputMode {
    Cap,
    Exact,
    CapReached,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "RequestSettings::deserialize_warmup_output"
    )]
    pub warmup_output: Option<OutputBudget>,
    pub cache: Cache,
    pub temperature_milli: Option<u16>,
    pub top_p_milli: Option<u16>,
    pub seed: Option<i64>,
    // Omitted when absent so plans recorded before this field keep their digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_control: Option<ThinkingControl>,
}
impl RequestSettings {
    fn deserialize_warmup_output<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<OutputBudget>, D::Error> {
        OutputBudget::deserialize(deserializer).map(Some)
    }

    pub fn effective_output(&self, phase: Phase) -> &OutputBudget {
        match phase {
            Phase::Warmup => self.warmup_output.as_ref().unwrap_or(&self.output),
            Phase::Measured => &self.output,
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let r = self;
        if std::iter::once(&r.output)
            .chain(r.warmup_output.as_ref())
            .any(|output| !(1..=32_768).contains(&output.tokens))
            || r.temperature_milli.is_some_and(|n| n > 2000)
            || r.top_p_milli.is_some_and(|n| n == 0 || n > 1000)
        {
            return Err("invalid output budget or sampling controls".into());
        }
        if r.thinking.is_some() && r.thinking_control.is_some() {
            return Err(
                "request.thinking and request.thinking_control are mutually exclusive".into(),
            );
        }
        if r.profile == Profile::PortableChatV1
            && (r.output.mode == OutputMode::Exact
                || r.warmup_output
                    .as_ref()
                    .is_some_and(|output| output.mode == OutputMode::Exact)
                || r.cache != Cache::Observe
                || matches!(
                    r.thinking_control,
                    Some(ThinkingControl::VllmEnableThinkingV1 { .. })
                )
                || r.thinking.is_some())
        {
            return Err("exact output, required prefix evidence, and vLLM thinking controls need the explicit vllm-fixed-v1 request profile".into());
        }
        if let Some(seed) = r.seed {
            seed.checked_add(100 * 64 + 63)
                .ok_or("seed range overflows i64")?;
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ThinkingControl {
    #[serde(rename = "vllm-enable-thinking-v1")]
    VllmEnableThinkingV1 { enabled: bool },
    #[serde(rename = "chat-template-thinking-v1")]
    ChatTemplateThinkingV1 { enabled: bool },
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<crate::sequence::Step>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub enum Fill {
    RepeatedUnit { unit: String, repeat: u32 },
    GeneratedProse { kind: ProseKind, characters: u32 },
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ProseKind {
    #[serde(rename = "generated-prose-v1")]
    V1,
}
impl Fill {
    fn valid(&self) -> bool {
        match self {
            Self::RepeatedUnit { unit, repeat } => {
                !unit.is_empty() && unit.len() <= 64 && (1..=1_000_000).contains(repeat)
            }
            Self::GeneratedProse { characters, .. } => {
                (1..=REQUEST_CAP).contains(&(*characters as usize))
            }
        }
    }

    pub fn bytes(&self) -> usize {
        match self {
            Self::RepeatedUnit { unit, repeat } => unit.len() * *repeat as usize,
            // The versioned word list, spaces and punctuation are all ASCII.
            Self::GeneratedProse { characters, .. } => *characters as usize,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::RepeatedUnit { .. } => "repeated-unit",
            Self::GeneratedProse {
                kind: ProseKind::V1,
                ..
            } => "generated-prose-v1",
        }
    }
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub schedule: Option<Vec<crate::schedule::Scenario>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub acquisition: Option<crate::acquisition::Protocol>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub resources: Option<crate::serving_resources::Config>,
}
pub(crate) fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

pub(crate) fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
impl Workload {
    pub fn fill_kinds(&self) -> Vec<&'static str> {
        let mut kinds = Vec::new();
        for case in &self.cases {
            let kind = case.fill.as_ref().map_or("none", Fill::kind);
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        kinds
    }

    pub fn salted(&self) -> bool {
        crate::acquisition::conversation(self) || self.cases.iter().any(|case| case.fill.is_some())
    }
    pub fn cache_namespace_required(&self) -> bool {
        self.request.cache != Cache::Observe
            || self.salted()
            || self
                .schedule
                .iter()
                .flatten()
                .flat_map(|s| &s.lanes)
                .any(|l| {
                    l.request
                        .as_ref()
                        .is_some_and(|r| r.cache != Cache::Observe)
                })
    }
    pub fn cache_mechanism_required(&self) -> bool {
        self.request.profile != Profile::PortableChatV1
            || self
                .schedule
                .iter()
                .flatten()
                .flat_map(|s| &s.lanes)
                .any(|l| {
                    l.request
                        .as_ref()
                        .is_some_and(|r| r.profile != Profile::PortableChatV1)
                })
    }
    #[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.version, 1..=6) || !identifier(&self.name) {
            return Err("expected workload version 1 through 6 and a short ASCII name".into());
        }
        if self.version < 3 && self.request.warmup_output.is_some() {
            return Err("request.warmup_output requires workload version 3".into());
        }
        if self.version == 5 && self.request.warmup_output.is_some() {
            return Err("conversation workload 5 has no warmup output override".into());
        }
        if self.version != 4 && self.schedule.is_some() {
            return Err("schedule requires workload version 4".into());
        }
        crate::acquisition::validate(self)?;
        if self.version != 6 && self.resources.is_some() {
            return Err("resources require workload6".into());
        }
        crate::sequence::validate(self)?;
        if self.cases.is_empty()
            || self.cases.len() > 128
            || (self.version != 4 && self.cells.is_empty())
            || self.cells.len() > if self.version == 6 { 128 } else { 64 }
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
                || case.messages.iter().map(|m| m.content.len()).sum::<usize>()
                    > self
                        .acquisition
                        .as_ref()
                        .map_or(128 * 1024, crate::acquisition::Protocol::input_bytes)
            {
                return Err(format!("case {} exceeds message bounds", case.id));
            }
            if let Some(fill) = &case.fill {
                if !fill.valid()
                    || case
                        .messages
                        .iter()
                        .map(|m| m.content.matches("{fill}").count())
                        .sum::<usize>()
                        != 1
                    || case
                        .messages
                        .iter()
                        .map(|m| m.content.matches("{salt}").count())
                        .sum::<usize>()
                        > 1
                {
                    return Err(format!(
                        "case {} has invalid fill controls or placeholders",
                        case.id
                    ));
                }
                if case.messages.iter().map(|m| m.content.len()).sum::<usize>() + fill.bytes()
                    > REQUEST_CAP
                {
                    return Err(format!(
                        "case {} exceeds the rendered request bound",
                        case.id
                    ));
                }
            }
        }
        let r = &self.request;
        if self.version == 6 {
            let mut controls = r.clone();
            controls.seed = None;
            controls.validate()?;
        } else {
            r.validate()?;
        }
        let l = &self.limits;
        if l.total_ms == 0 || l.total_ms > 3_600_000 {
            return Err(format!(
                "limits.total_ms={} must be in 1..=3600000",
                l.total_ms
            ));
        }
        if l.idle_ms == 0 {
            return Err(format!("limits.idle_ms={} must be positive", l.idle_ms));
        }
        if l.idle_ms > l.total_ms {
            return Err(format!(
                "limits.idle_ms={} must be <= limits.total_ms={}",
                l.idle_ms, l.total_ms
            ));
        }
        if !(1024..=8 * 1024 * 1024).contains(&l.response_bytes) {
            return Err(format!(
                "limits.response_bytes={} must be in 1024..=8388608",
                l.response_bytes
            ));
        }
        if l.wave_buffer_bytes > 512 * 1024 * 1024 {
            return Err(format!(
                "limits.wave_buffer_bytes={} must be <= 536870912",
                l.wave_buffer_bytes
            ));
        }
        if self.version == 4 {
            return crate::schedule::validate(self);
        }
        let mut names = std::collections::HashSet::new();
        let mut attempts = 0u64;
        let mut waves = 0u64;
        for cell in &self.cells {
            let Some(case) = self.cases.iter().find(|case| case.id == cell.case) else {
                return Err("cell IDs must be unique and reference an existing case".into());
            };
            if !identifier(&cell.id) || !names.insert(cell.id.as_str()) {
                return Err("cell IDs must be unique and reference an existing case".into());
            }
            if !(1..=64).contains(&cell.concurrency)
                || cell.trials == 0
                || cell.trials > if self.version == 6 { 1000 } else { 100 }
                || cell.warmup_trials > 20
            {
                return Err(format!("cell {} exceeds concurrency/trial bounds", cell.id));
            }
            if r.cache == Cache::ReportedPrefixHit && cell.warmup_trials == 0 {
                return Err("reported-prefix-hit requires explicit warmup priming".into());
            }
            let fill_bytes = case.fill.as_ref().map_or(0, Fill::bytes);
            // Streaming semantics are frame-bounded; nonstreaming decoding is body-bounded.
            let per_request = if r.stream {
                2 * l.response_bytes + 6 * FRAME_CAP + 512 * 1024
            } else {
                6 * l.response_bytes + 512 * 1024
            } + fill_bytes
                + if matches!(self.version, 5 | 6)
                    && case.step.as_ref().is_some_and(|step| {
                        matches!(step.expect, crate::sequence::Expected::Tool { .. })
                            || crate::sequence::shared_tools(self, &step.history)
                    })
                {
                    TOOL_TRACE_ALLOWANCE
                } else {
                    0
                };
            let required = if let Some(protocol) = &self.acquisition {
                per_request
                    .checked_add(protocol.input_bytes())
                    .and_then(|n| n.checked_mul(cell.concurrency as usize))
                    .and_then(|n| n.checked_add(protocol.history_bytes()))
                    .ok_or("acquisition buffer budget overflow")?
            } else {
                per_request * cell.concurrency as usize
            };
            if required > l.wave_buffer_bytes {
                return Err(format!(
                    "cell {} requires limits.wave_buffer_bytes >= {required}, supplied {}; concurrency={}, stream={}, response_bytes={}, fill_bytes={fill_bytes}",
                    cell.id, l.wave_buffer_bytes, cell.concurrency, r.stream, l.response_bytes
                ));
            }
            let (trials, warmups) = self
                .acquisition
                .as_ref()
                .map_or((cell.trials, cell.warmup_trials), |p| p.counts(cell));
            if self.version == 6
                && let Some(seed) = r.seed
            {
                let max_trial = trials.max(warmups).saturating_sub(1);
                seed.checked_add(i64::from(max_trial) * 64 + i64::from(cell.concurrency - 1))
                    .ok_or("acquisition seed range overflows i64")?;
            }
            let n = u64::from(trials) + u64::from(warmups);
            waves = waves.checked_add(n).ok_or("wave budget overflow")?;
            attempts = n
                .checked_mul(u64::from(cell.concurrency))
                .and_then(|n| attempts.checked_add(n))
                .ok_or("attempt budget overflow")?;
        }
        if attempts > MAX_ATTEMPTS
            || waves
                > if self.version == 6 {
                    crate::acquisition::WAVE_CAP as u64
                } else {
                    1024
                }
        {
            return Err("workload exceeds attempt or versioned wave limit".into());
        }
        if let Some(resources) = &self.resources {
            resources.validate(self)?;
        }
        Ok(())
    }
    pub fn waves(&self) -> Vec<WaveSpec> {
        if self.version == 6 {
            return crate::acquisition::waves(self);
        }
        if self.version == 4 {
            return crate::schedule::waves(self);
        }
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
                        case: Some(cell.case.clone()),
                        trial,
                        concurrency: cell.concurrency,
                        lanes: None,
                        acquisition: None,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub case: Option<String>,
    pub trial: u32,
    pub concurrency: u32,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub lanes: Option<Vec<crate::schedule::ResolvedLane>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub acquisition: Option<crate::acquisition::AcquisitionIdentity>,
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
        for (field, value) in [
            ("model_revision", &self.model_revision),
            ("runtime", &self.runtime),
            ("hardware", &self.hardware),
            ("settings", &self.settings),
        ] {
            let Some(value) = value else {
                continue;
            };
            if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                return Err(format!(
                    "deployment.{field}: {} bytes; must be nonempty, control-free text within 4096 bytes",
                    value.len()
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric_contract: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<crate::metrics::Protocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_sha256: Option<String>,
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

pub const TOOL_ARRIVAL_CAP: usize = 4096;
// Covers the in-memory arrivals, bounded call context and pretty JSON trace.
pub const TOOL_TRACE_ALLOWANCE: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolArrival {
    pub end_offset: usize,
    pub observed_us: u64,
}

fn present_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

fn tool_arrivals<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<ToolArrival>>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<ToolArrival>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("at most 4096 tool response chunk arrivals")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == TOOL_ARRIVAL_CAP {
                    return Err(serde::de::Error::custom(
                        "tool arrival trace exceeds 4096 chunks",
                    ));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Visitor).map(Some)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_generated_text_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_us: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub first_tool_delta_us: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub first_validated_tool_call_us: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "tool_arrivals"
    )]
    pub tool_stream_arrivals: Option<Vec<ToolArrival>>,
    pub settle_us: u64,
    pub capture_parse_us: u64,
}
impl Timing {
    pub fn validate_tools(&self, tool_step: bool) -> Result<()> {
        if !tool_step {
            return if self.first_tool_delta_us.is_none()
                && self.first_validated_tool_call_us.is_none()
                && self.tool_stream_arrivals.is_none()
            {
                Ok(())
            } else {
                Err("tool observations require workload5/plan4 conversation-v3 tool step".into())
            };
        }
        if self.tool_stream_arrivals.is_none()
            || (self.first_tool_delta_us.is_some() && self.first_body_us.is_none())
            || (self.first_validated_tool_call_us.is_some()
                && (self.first_tool_delta_us.is_none() || self.terminal_us.is_none()))
            || self.first_generated_text_us.is_some()
            || self.first_answer_text_us.is_some()
            || self.last_generated_text_us.is_some()
            || self.first_generated_channel.is_some()
        {
            return Err("inconsistent fixed tool timing presence".into());
        }
        let mut previous = 0;
        for value in [
            self.headers_us,
            self.first_body_us,
            self.first_tool_delta_us,
            self.first_validated_tool_call_us,
            self.terminal_us,
            Some(self.settle_us),
        ]
        .into_iter()
        .flatten()
        {
            if value < previous {
                return Err("fixed tool timing observations are out of order".into());
            }
            previous = value;
        }
        Ok(())
    }

    pub fn validate(&self, stream: bool) -> Result<()> {
        if !stream
            && (self.first_generated_text_us.is_some()
                || self.first_answer_text_us.is_some()
                || self.last_generated_text_us.is_some()
                || self.first_generated_channel.is_some())
        {
            return Err("nonstreaming evidence cannot contain first-text timings".into());
        }
        if self.first_generated_text_us.is_some() != self.first_generated_channel.is_some()
            || (self.first_body_us.is_some() && self.headers_us.is_none())
            || (self.first_generated_text_us.is_some() && self.first_body_us.is_none())
            || (self.first_answer_text_us.is_some() && self.first_generated_text_us.is_none())
            || (self.last_generated_text_us.is_some() && self.first_generated_text_us.is_none())
            || (self.terminal_us.is_some() && self.first_body_us.is_none())
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
            self.last_generated_text_us,
            self.terminal_us,
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
#[derive(Clone, Debug, Deserialize, Serialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<crate::sequence::Check>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<crate::metrics::Reference>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub schedule: Option<crate::schedule::Observation>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub acquisition_clock: Option<crate::acquisition::StepClock>,
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
        Some(n) if n != u64::from(r.output.tokens) => match r.output.mode {
            OutputMode::Cap => (),
            OutputMode::Exact => errors.push("reported_output_not_exact".into()),
            OutputMode::CapReached => errors.push("reported_output_below_cap".into()),
        },
        _ => (),
    }
    if r.output.mode == OutputMode::CapReached && a.finish_reason.as_deref() != Some("length") {
        errors.push("cap_not_reported_as_length".into());
    }
    if r.thinking_control == Some(ThinkingControl::ChatTemplateThinkingV1 { enabled: false })
        && (a.usage.reasoning_tokens.is_some_and(|n| n > 0)
            || matches!(
                a.timing.first_generated_channel,
                Some(TextChannel::Reasoning | TextChannel::MixedEvent)
            ))
    {
        errors.push("reasoning_reported_with_thinking_disabled".into());
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
