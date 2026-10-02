use crate::model::*;
use grill_sse::{Flow, Parser, SseLimits};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::watch;
#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}
#[derive(Serialize)]
#[serde(untagged)]
enum ChatTemplateKwargs {
    Legacy {
        thinking: bool,
    },
    EnableThinking {
        enable_thinking: bool,
    },
    ChatTemplateThinking {
        thinking: bool,
        enable_thinking: bool,
    },
}
#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_template_kwargs: Option<ChatTemplateKwargs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ignore_eos: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_salt: Option<String>,
}
pub struct BodyContext<'a> {
    pub workload: &'a Workload,
    pub model: &'a str,
    pub cache_namespace: Option<&'a str>,
}
impl<'a> From<&'a Plan> for BodyContext<'a> {
    fn from(plan: &'a Plan) -> Self {
        Self {
            workload: &plan.workload,
            model: &plan.model,
            cache_namespace: plan.cache_namespace.as_deref(),
        }
    }
}

pub fn request_body(context: &BodyContext<'_>, wave: &WaveSpec, lane: u32) -> Result<String> {
    let r = crate::schedule::settings(context.workload, wave, lane)?;
    let case_id = crate::schedule::case(wave, lane)?;
    let case = context
        .workload
        .cases
        .iter()
        .find(|c| c.id == case_id)
        .ok_or("unknown case")?;
    let exact = r.profile == Profile::VllmFixedV1 && r.output.mode == OutputMode::Exact;
    let salt = match r.cache {
        Cache::Observe => None,
        cache => {
            let nonce = context.cache_namespace.ok_or("missing cache namespace")?;
            Some(if cache == Cache::ReportedPrefixZero {
                format!("{nonce}-{}-{lane}", wave.index)
            } else {
                format!("{nonce}-{}-{lane}", wave.cell)
            })
        }
    };
    let messages = if let Some(fill) = &case.fill {
        let namespace = context.cache_namespace.ok_or("missing cache namespace")?;
        let text_salt = format!("{}-{}-{lane}", &namespace[..16], wave.index);
        Cow::Owned(
            case.messages
                .iter()
                .map(|message| {
                    let content = if message.content.contains("{salt}") {
                        Cow::Owned(message.content.replace("{salt}", &text_salt))
                    } else {
                        Cow::Borrowed(message.content.as_str())
                    };
                    let content = match content.split_once("{fill}") {
                        Some((header, footer)) => {
                            let mut rendered =
                                String::with_capacity(header.len() + footer.len() + fill.bytes());
                            rendered.push_str(header);
                            match fill {
                                Fill::RepeatedUnit { unit, repeat } => {
                                    for _ in 0..*repeat {
                                        rendered.push_str(unit);
                                    }
                                }
                                Fill::GeneratedProse {
                                    kind: ProseKind::V1,
                                    characters,
                                } => {
                                    crate::prose::v1(
                                        &mut rendered,
                                        *characters as usize,
                                        &text_salt,
                                    );
                                }
                            }
                            rendered.push_str(footer);
                            rendered
                        }
                        None => content.into_owned(),
                    };
                    Message {
                        role: message.role,
                        content,
                    }
                })
                .collect::<Vec<_>>(),
        )
    } else {
        Cow::Borrowed(case.messages.as_slice())
    };
    let body = serde_json::to_string(&Body {
        model: context.model,
        messages: &messages,
        stream: r.stream,
        max_tokens: r.output.tokens,
        temperature: r.temperature_milli.map(|n| f64::from(n) / 1000.0),
        top_p: r.top_p_milli.map(|n| f64::from(n) / 1000.0),
        seed: r.seed.map(|seed| {
            seed + i64::from(wave.trial) * 64
                + if wave.lanes.is_some() {
                    0
                } else {
                    i64::from(lane)
                }
        }),
        chat_template_kwargs: match (r.thinking, r.thinking_control) {
            (Some(thinking), None) => Some(ChatTemplateKwargs::Legacy { thinking }),
            (None, Some(ThinkingControl::VllmEnableThinkingV1 { enabled })) => {
                Some(ChatTemplateKwargs::EnableThinking {
                    enable_thinking: enabled,
                })
            }
            (None, Some(ThinkingControl::ChatTemplateThinkingV1 { enabled })) => {
                Some(ChatTemplateKwargs::ChatTemplateThinking {
                    thinking: enabled,
                    enable_thinking: enabled,
                })
            }
            (None, None) => None,
            (Some(_), Some(_)) => {
                return Err(
                    "request.thinking and request.thinking_control are mutually exclusive".into(),
                );
            }
        },
        stream_options: r.stream.then_some(StreamOptions {
            include_usage: true,
        }),
        min_tokens: exact.then_some(r.output.tokens),
        ignore_eos: exact.then_some(true),
        cache_salt: salt,
    })
    .map_err(|e| e.to_string())?;
    if body.len() > crate::acquisition::input_cap(context.workload) {
        return Err("encoded request exceeds 2 MiB".into());
    }
    Ok(body)
}

pub fn endpoint(text: &str, local_http: bool) -> Result<reqwest::Url> {
    if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
        return Err("endpoint must be nonempty, control-free text within 4096 bytes".into());
    }
    let url = reqwest::Url::parse(text).map_err(|_| "invalid endpoint URL")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err("endpoint must have a host and no credentials, query or fragment".into());
    }
    let loopback = url
        .host_str()
        .and_then(|h| h.trim_matches(['[', ']']).parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback());
    if url.scheme() != "https" && !(local_http && url.scheme() == "http" && loopback) {
        return Err("use HTTPS, or --local-http with a literal loopback address".into());
    }
    Ok(url)
}
pub fn credential(name: Option<&str>) -> Result<Option<HeaderValue>> {
    let Some(name) = name else {
        return Ok(None);
    };
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("invalid credential environment-variable name".into());
    }
    let value = std::env::var(name).map_err(|_| {
        format!("credential variable {name} is unavailable; set that named variable in this process environment before capture")
    })?;
    if value.is_empty() || value.len() > 8192 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("credential must be nonempty visible ASCII within 8192 bytes".into());
    }
    let mut header = HeaderValue::from_str(&format!("Bearer {value}"))
        .map_err(|_| "invalid credential header")?;
    header.set_sensitive(true);
    Ok(Some(header))
}
pub fn client(local: bool, pool: usize) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .https_only(!local)
        .http1_only()
        .referer(false)
        .pool_max_idle_per_host(pool)
        .build()
        .map_err(|_| "could not construct HTTP client; verify runtime prerequisites, including a readable system CA certificate store".into())
}
pub fn request(
    client: &reqwest::Client,
    url: &reqwest::Url,
    auth: Option<&HeaderValue>,
    body: String,
    stream: bool,
) -> Result<reqwest::Request> {
    let mut builder = client
        .post(url.clone())
        .header("content-type", "application/json")
        .header(
            "accept",
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .header("accept-encoding", "identity")
        .header(
            "user-agent",
            concat!("grill-perf/", env!("CARGO_PKG_VERSION")),
        )
        .body(body);
    if let Some(auth) = auth {
        builder = builder.header(AUTHORIZATION, auth.clone());
    }
    builder
        .build()
        .map_err(|_| "request construction failed".into())
}
fn error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_connect() {
        "connection failed"
    } else if error.is_timeout() {
        "transport timeout"
    } else if error.is_body() {
        "body transfer failed"
    } else {
        "transport request failed"
    }
}
pub fn us(start: Instant) -> u64 {
    start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

struct Object<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
            type Value = Object<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                T::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Object)
            }
        }
        d.deserialize_map(Visitor(std::marker::PhantomData))
    }
}

pub(crate) fn object<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<T, D::Error> {
    Object::<T>::deserialize(d).map(|object| object.0)
}

pub(crate) fn objects<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Vec<T>, D::Error> {
    struct Visitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an array of JSON objects")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(Object(value)) = seq.next_element::<Object<T>>()? {
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Visitor(std::marker::PhantomData))
}

fn bounded_depth(bytes: &[u8]) -> bool {
    let (mut depth, mut quoted, mut escaped) = (0u32, false, false);
    for byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 64 {
                        return false;
                    }
                }
                b'}' | b']' => {
                    let Some(next) = depth.checked_sub(1) else {
                        return false;
                    };
                    depth = next;
                }
                _ => (),
            }
        }
    }
    depth == 0 && !quoted
}

#[derive(Default, Deserialize)]
struct Delta<'a> {
    #[serde(borrow)]
    content: Option<Cow<'a, str>>,
    #[serde(borrow)]
    reasoning: Option<Cow<'a, str>>,
    #[serde(borrow)]
    reasoning_content: Option<Cow<'a, str>>,
    #[serde(borrow)]
    refusal: Option<&'a RawValue>,
    #[serde(borrow)]
    tool_calls: Option<&'a RawValue>,
    #[serde(borrow)]
    function_call: Option<&'a RawValue>,
    #[serde(borrow)]
    audio: Option<&'a RawValue>,
}
#[derive(Deserialize)]
struct Choice<'a> {
    index: Option<u32>,
    #[serde(borrow)]
    delta: Option<Object<Delta<'a>>>,
    #[serde(borrow)]
    message: Option<Object<Delta<'a>>>,
    #[serde(borrow)]
    finish_reason: Option<Cow<'a, str>>,
}
#[derive(Default)]
struct Choices<'a>(Option<Choice<'a>>);
impl<'de: 'a, 'a> Deserialize<'de> for Choices<'a> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de: 'a, 'a> serde::de::Visitor<'de> for Visitor<'a> {
            type Value = Choices<'a>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an array with at most one choice")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let first = seq.next_element::<Object<Choice<'a>>>()?.map(|v| v.0);
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::custom("multiple choices unsupported"));
                }
                Ok(Choices(first))
            }
        }
        d.deserialize_seq(Visitor(std::marker::PhantomData))
    }
}
#[derive(Default, Deserialize)]
struct PromptDetails {
    cached_tokens: Option<u64>,
}
#[derive(Default, Deserialize)]
struct CompletionDetails {
    reasoning_tokens: Option<u64>,
}
#[derive(Default, Deserialize)]
struct WireUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
    prompt_tokens_details: Option<Object<PromptDetails>>,
    completion_tokens_details: Option<Object<CompletionDetails>>,
}
impl From<WireUsage> for Usage {
    fn from(w: WireUsage) -> Self {
        Self {
            prompt_tokens: w.prompt_tokens,
            completion_tokens: w.completion_tokens,
            total_tokens: w.total_tokens,
            cached_prompt_tokens: w.prompt_tokens_details.and_then(|p| p.0.cached_tokens),
            reasoning_tokens: w
                .completion_tokens_details
                .and_then(|p| p.0.reasoning_tokens),
        }
    }
}
#[derive(Deserialize)]
struct Event<'a> {
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    choices: Option<Choices<'a>>,
    usage: Option<Object<WireUsage>>,
    #[serde(borrow)]
    error: Option<&'a RawValue>,
}
fn present(raw: Option<&RawValue>) -> bool {
    raw.is_some_and(|v| !matches!(v.get().trim(), "null" | "[]" | "\"\""))
}
#[derive(Default)]
enum Retain {
    #[default]
    Nothing,
    /// The answer a conversation carries into its next request, bounded like request text.
    SequenceAnswer,
    /// Both generated channels, bounded only by the retained response.
    Channels,
}
#[derive(Default)]
struct Semantic {
    allow_tools: bool,
    tool: Option<crate::sequence::ToolStream>,
    id: Option<String>,
    finish: Option<String>,
    usage: Usage,
    content: bool,
    generated: bool,
    retain: Retain,
    answer: String,
    reasoning: String,
}
impl Semantic {
    fn event(
        &mut self,
        bytes: &[u8],
        stream: bool,
        observed: u64,
        timing: &mut Timing,
    ) -> std::result::Result<Flow, (Status, &'static str)> {
        if stream && bytes == b"[DONE]" {
            if self.finish.is_none() {
                return Err((Status::Malformed, "DONE before finish"));
            }
            if let Some(tool) = &self.tool {
                timing.first_validated_tool_call_us =
                    Some(tool.validated_us().map_err(|_| {
                        (Status::Malformed, "incomplete or invalid fixed tool call")
                    })?);
            }
            timing.terminal_us = Some(observed);
            return Ok(Flow::Stop);
        }
        if !stream && std::str::from_utf8(bytes).is_err() {
            return Err((Status::Malformed, "nonstreaming body is not UTF-8"));
        }
        if !bounded_depth(bytes) {
            return Err((Status::Malformed, "JSON nesting or shape exceeds bounds"));
        }
        let Object(event): Object<Event<'_>> = serde_json::from_slice(bytes)
            .map_err(|_| (Status::Malformed, "invalid response JSON or field shape"))?;
        if present(event.error) {
            return Err((Status::Unsupported, "provider error envelope"));
        }
        if let Some(id) = event.id {
            if self.id.as_deref().is_some_and(|old| old != id) {
                return Err((Status::Malformed, "response identity changed"));
            }
            if self.id.is_none() {
                self.id = Some(id.into_owned());
            }
        }
        if let Some(usage) = event.usage {
            self.usage = usage.0.into();
        }
        let Some(choices) = event.choices else {
            return Err((Status::Malformed, "missing choices"));
        };
        let Some(choice) = choices.0 else {
            return if stream {
                Ok(Flow::Continue)
            } else {
                Err((Status::Malformed, "missing response choice"))
            };
        };
        if choice.index.is_some_and(|i| i != 0) {
            return Err((Status::Unsupported, "only choice zero is supported"));
        }
        let delta = if stream {
            if choice.message.is_some() {
                return Err((Status::Unsupported, "message in streaming response"));
            }
            choice.delta
        } else {
            if choice.delta.is_some() {
                return Err((Status::Unsupported, "delta in nonstreaming response"));
            }
            choice.message
        };
        if let Some(Object(delta)) = delta {
            let tools = present(delta.tool_calls);
            if (tools && !self.allow_tools && self.tool.is_none())
                || present(delta.function_call)
                || present(delta.audio)
                || present(delta.refusal)
            {
                return Err((Status::Unsupported, "non-text answer or refusal"));
            }
            let content = delta.content.as_ref().is_some_and(|s| !s.is_empty());
            let reasoning = delta.reasoning.as_ref().is_some_and(|s| !s.is_empty())
                || delta
                    .reasoning_content
                    .as_ref()
                    .is_some_and(|s| !s.is_empty());
            let generated = content || reasoning;
            if self.tool.is_some() && generated {
                return Err((Status::Malformed, "text in fixed tool response"));
            }
            if tools && let Some(tool) = self.tool.as_mut() {
                if !stream || self.finish.is_some() {
                    return Err((Status::Malformed, "tool fragments outside active stream"));
                }
                tool.delta(delta.tool_calls.expect("present tools").get(), observed)
                    .map_err(|_| (Status::Malformed, "invalid fixed tool fragment"))?;
                timing.first_tool_delta_us = tool.first_delta_us;
                self.generated |= tool.first_delta_us.is_some();
            } else if tools && self.allow_tools {
                if stream || self.finish.is_some() {
                    return Err((
                        Status::Unsupported,
                        "tool profile requires one nonstreaming message",
                    ));
                }
                self.generated = true;
            }
            if generated && self.finish.is_some() {
                return Err((Status::Malformed, "text after finish"));
            }
            if stream && generated && timing.first_generated_text_us.is_none() {
                timing.first_generated_text_us = Some(observed);
                timing.first_generated_channel = Some(match (content, reasoning) {
                    (true, true) => TextChannel::MixedEvent,
                    (true, false) => TextChannel::Answer,
                    _ => TextChannel::Reasoning,
                });
            }
            if stream && generated {
                timing.last_generated_text_us = Some(observed);
            }
            if stream && content && timing.first_answer_text_us.is_none() {
                timing.first_answer_text_us = Some(observed);
            }
            self.retain_text(&delta)?;
            self.content |= content;
            self.generated |= generated;
        }
        if let Some(reason) = choice.finish_reason {
            if !matches!(reason.as_ref(), "stop" | "length")
                && !((self.allow_tools && !stream || self.tool.is_some()) && reason == "tool_calls")
            {
                return Err((Status::Unsupported, "unsupported finish reason"));
            }
            if self.finish.is_some() {
                return Err((Status::Malformed, "duplicate finish"));
            }
            if let Some(tool) = &self.tool
                && (!matches!(reason.as_ref(), "tool_calls" | "stop")
                    || tool.validated_us().is_err())
            {
                return Err((Status::Malformed, "invalid fixed tool call at finish"));
            }
            self.finish = Some(reason.into_owned());
        }
        if !stream {
            if self.finish.is_none() {
                return Err((Status::Malformed, "missing finish reason"));
            }
            timing.terminal_us = Some(observed);
            return Ok(Flow::Stop);
        }
        Ok(Flow::Continue)
    }
    fn retain_text(
        &mut self,
        delta: &Delta<'_>,
    ) -> std::result::Result<(), (Status, &'static str)> {
        match self.retain {
            Retain::Nothing => (),
            Retain::SequenceAnswer => {
                if let Some(content) = &delta.content {
                    if self.answer.len() + content.len() > 64 * 1024 {
                        return Err((
                            Status::ResponseLimit,
                            "sequence answer exceeds response bound",
                        ));
                    }
                    self.answer.push_str(content);
                }
            }
            Retain::Channels => {
                self.answer
                    .push_str(delta.content.as_deref().unwrap_or_default());
                // One reasoning field per event, so text mirrored in both is not doubled.
                self.reasoning.push_str(
                    delta
                        .reasoning
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .or(delta.reasoning_content.as_deref())
                        .unwrap_or_default(),
                );
            }
        }
        Ok(())
    }
}

pub fn verify_complete(
    attempt: &Attempt,
    body: &[u8],
    stream: bool,
    arrival_contract: bool,
    profile: Profile,
) -> Result<()> {
    complete_semantic(
        attempt,
        body,
        stream,
        arrival_contract,
        profile,
        Retain::Nothing,
    )
    .map(|_| ())
}

pub(crate) fn sequence_answer(
    attempt: &Attempt,
    body: &[u8],
    arrival_contract: bool,
) -> Result<String> {
    complete_semantic(
        attempt,
        body,
        true,
        arrival_contract,
        Profile::VllmConversationV2,
        Retain::SequenceAnswer,
    )
    .map(|semantic| semantic.answer)
}

/// Replays verified completion evidence into its (answer, reasoning) channel text.
pub(crate) fn completion_text(
    attempt: &Attempt,
    body: &[u8],
    stream: bool,
    arrival_contract: bool,
    profile: Profile,
) -> Result<(String, String)> {
    complete_semantic(
        attempt,
        body,
        stream,
        arrival_contract,
        profile,
        Retain::Channels,
    )
    .map(|semantic| (semantic.answer, semantic.reasoning))
}

fn complete_semantic(
    attempt: &Attempt,
    body: &[u8],
    stream: bool,
    arrival_contract: bool,
    profile: Profile,
    retain: Retain,
) -> Result<Semantic> {
    let mut semantic = Semantic {
        allow_tools: profile == Profile::VllmConversationV2,
        retain,
        ..Semantic::default()
    };
    let mut timing = Timing::default();
    let end = if stream {
        Parser::new(SseLimits {
            line_bytes: FRAME_CAP,
            event_bytes: FRAME_CAP,
        })
        .map_err(|_| "invalid framing limits")?
        .feed(body, |event| semantic.event(event, true, 0, &mut timing))
        .map_err(|_| "complete receipt has malformed response evidence")?
    } else {
        semantic
            .event(body, false, 0, &mut timing)
            .map_err(|_| "complete receipt has malformed JSON evidence")?;
        Some(body.len())
    };
    if end.is_none()
        || end != attempt.terminal_offset
        || !semantic.generated
        || semantic.finish != attempt.finish_reason
        || semantic.usage != attempt.usage
        || (stream
            && (timing.first_generated_text_us.is_some()
                != attempt.timing.first_generated_text_us.is_some()
                || timing.first_answer_text_us.is_some()
                    != attempt.timing.first_answer_text_us.is_some()))
        || timing.first_generated_channel != attempt.timing.first_generated_channel
        || (arrival_contract
            && (timing.last_generated_text_us.is_some()
                != attempt.timing.last_generated_text_us.is_some()
                || timing.terminal_us.is_some() != attempt.timing.terminal_us.is_some()))
    {
        return Err("response evidence does not support its completion facts".into());
    }
    Ok(semantic)
}

pub fn verify_partial_arrivals(
    attempt: &Attempt,
    body: &[u8],
    stream: bool,
    profile: Profile,
) -> Result<()> {
    let observed = &attempt.timing;
    if attempt.http_status != Some(200) {
        return if attempt.usage == Usage::default()
            && attempt.finish_reason.is_none()
            && observed.first_generated_text_us.is_none()
            && observed.terminal_us.is_none()
            && attempt.terminal_offset.is_none()
        {
            Ok(())
        } else {
            Err("unsuccessful HTTP response cannot contain parsed completion facts".into())
        };
    }
    if observed.first_generated_text_us.is_none()
        && observed.terminal_us.is_none()
        && (!stream || attempt.status == Status::Unsupported)
        && attempt.usage == Usage::default()
        && attempt.finish_reason.is_none()
    {
        return Ok(());
    }
    let mut semantic = Semantic {
        allow_tools: profile == Profile::VllmConversationV2,
        ..Semantic::default()
    };
    let mut timing = Timing::default();
    let end = if stream {
        Parser::new(SseLimits {
            line_bytes: FRAME_CAP,
            event_bytes: FRAME_CAP,
        })
        .map_err(|_| "invalid framing limits")?
        .feed(body, |event| semantic.event(event, true, 0, &mut timing))
        .ok()
        .flatten()
    } else {
        semantic
            .event(body, false, 0, &mut timing)
            .ok()
            .and_then(|flow| matches!(flow, Flow::Stop).then_some(body.len()))
    };
    if timing.first_generated_text_us.is_some() != observed.first_generated_text_us.is_some()
        || timing.first_answer_text_us.is_some() != observed.first_answer_text_us.is_some()
        || timing.last_generated_text_us.is_some() != observed.last_generated_text_us.is_some()
        || timing.terminal_us.is_some() != observed.terminal_us.is_some()
        || timing.first_generated_channel != observed.first_generated_channel
        || end != attempt.terminal_offset
        || semantic.usage != attempt.usage
        || semantic.finish != attempt.finish_reason
    {
        return Err(
            "partial response does not support its semantic or arrival observations".into(),
        );
    }
    Ok(())
}

/// Replay tool semantics using retained client chunk clocks, not timestamps
/// inferred from response bytes. State supplies the same prospective context.
pub(crate) fn sequence_tool(
    attempt: &Attempt,
    body: &[u8],
    expected: crate::sequence::ToolExpectation,
) -> Result<Option<serde_json::Value>> {
    attempt.timing.validate_tools(true)?;
    let arrivals = attempt
        .timing
        .tool_stream_arrivals
        .as_ref()
        .ok_or("missing tool arrival trace")?;
    if arrivals.len() > TOOL_ARRIVAL_CAP {
        return Err("tool arrival trace exceeds its finite allowance".into());
    }
    let mut previous_offset = 0;
    let mut previous_us = attempt.timing.headers_us.unwrap_or(0);
    for arrival in arrivals {
        if arrival.end_offset <= previous_offset
            || arrival.end_offset > body.len()
            || arrival.observed_us < previous_us
            || arrival.observed_us > attempt.timing.settle_us
        {
            return Err("invalid tool chunk offset or monotonic clock".into());
        }
        previous_offset = arrival.end_offset;
        previous_us = arrival.observed_us;
    }
    if previous_offset != body.len()
        || arrivals.first().map(|arrival| arrival.observed_us) != attempt.timing.first_body_us
    {
        return Err("tool arrival trace does not cover retained response bytes".into());
    }
    // The existing transport receipt distinguishes unsuccessful HTTP and
    // unsupported media from parsed responses, without inventing response facts.
    if attempt.http_status != Some(200)
        || (attempt.status == Status::Unsupported
            && attempt.timing.first_tool_delta_us.is_none()
            && attempt.finish_reason.is_none()
            && attempt.usage == Usage::default())
    {
        return if attempt.status != Status::Complete
            && attempt.timing.first_tool_delta_us.is_none()
            && attempt.timing.first_validated_tool_call_us.is_none()
            && attempt.timing.terminal_us.is_none()
            && attempt.terminal_offset.is_none()
            && attempt.finish_reason.is_none()
            && attempt.usage == Usage::default()
        {
            Ok(None)
        } else {
            Err("unparsed tool response contains semantic observations".into())
        };
    }
    let mut semantic = Semantic {
        tool: Some(crate::sequence::ToolStream::new(expected)),
        ..Semantic::default()
    };
    let mut timing = Timing::default();
    let mut parser = Parser::new(SseLimits {
        line_bytes: FRAME_CAP,
        event_bytes: FRAME_CAP,
    })
    .map_err(|_| "invalid framing limits")?;
    let mut offset = 0;
    let mut terminal_offset = None;
    let mut failed = false;
    for (index, arrival) in arrivals.iter().enumerate() {
        let result = parser.feed(&body[offset..arrival.end_offset], |event| {
            semantic.event(event, true, arrival.observed_us, &mut timing)
        });
        match result {
            Ok(Some(consumed)) => terminal_offset = Some(offset + consumed),
            Ok(None) => (),
            Err(_) => failed = true,
        }
        if (failed || terminal_offset.is_some()) && index + 1 != arrivals.len() {
            return Err("tool trace continues after parser settlement".into());
        }
        offset = arrival.end_offset;
    }
    if attempt.status == Status::Complete {
        if failed || terminal_offset.is_none() || !semantic.generated {
            return Err("complete tool receipt lacks a valid complete stream".into());
        }
    } else {
        timing.first_validated_tool_call_us = None;
    }
    if timing.first_tool_delta_us != attempt.timing.first_tool_delta_us
        || timing.first_validated_tool_call_us != attempt.timing.first_validated_tool_call_us
        || timing.terminal_us != attempt.timing.terminal_us
        || terminal_offset != attempt.terminal_offset
        || semantic.finish != attempt.finish_reason
        || semantic.usage != attempt.usage
    {
        return Err(
            "tool response does not reproduce its retained semantic timing boundaries".into(),
        );
    }
    if attempt.status == Status::Complete {
        semantic
            .tool
            .ok_or("missing fixed tool context")?
            .message()
            .map(Some)
    } else {
        Ok(None)
    }
}

pub struct Collected {
    pub attempt: Attempt,
    pub body: Vec<u8>,
}
pub struct CollectContext {
    pub lane: u32,
    pub origin: Instant,
    pub deadline: Option<Instant>,
    pub cancel: watch::Receiver<bool>,
    pub first_generated: Option<tokio::sync::mpsc::Sender<crate::schedule::FirstGenerated>>,
    pub tool_expectation: Option<crate::sequence::ToolExpectation>,
}
#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
pub async fn collect(
    client: reqwest::Client,
    request: reqwest::Request,
    limits: Limits,
    settings: RequestSettings,
    context: CollectContext,
) -> Collected {
    let CollectContext {
        lane,
        origin,
        deadline,
        mut cancel,
        first_generated,
        tool_expectation,
    } = context;
    let stream = settings.stream;
    let sent = Instant::now();
    let tool_step = tool_expectation.is_some();
    let mut a = Attempt {
        lane,
        dispatched: false,
        status: Status::Incomplete,
        detail: "stream ended before completion".into(),
        http_status: None,
        finish_reason: None,
        usage: Usage::default(),
        timing: Timing {
            dispatch_offset_us: sent
                .duration_since(origin)
                .as_micros()
                .min(u128::from(u64::MAX)) as u64,
            tool_stream_arrivals: tool_step.then(|| Vec::with_capacity(TOOL_ARRIVAL_CAP)),
            ..Timing::default()
        },
        response_bytes: 0,
        response_sha256: String::new(),
        terminal_offset: None,
        surplus_observed_bytes: 0,
        eligibility_errors: Vec::new(),
        sequence: None,
    };
    let mut body = Vec::new();
    let mut semantic = Semantic {
        allow_tools: settings.profile == Profile::VllmConversationV2,
        tool: tool_expectation.map(crate::sequence::ToolStream::new),
        ..Semantic::default()
    };
    let total =
        tokio::time::Instant::from_std(sent) + Duration::from_millis(u64::from(limits.total_ms));
    let mut idle =
        tokio::time::Instant::from_std(sent) + Duration::from_millis(u64::from(limits.idle_ms));
    let whole = deadline.map(tokio::time::Instant::from_std);
    let total = whole.map_or(total, |whole| total.min(whole));
    if *cancel.borrow() || deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        a.status = Status::Interrupted;
        a.detail = if *cancel.borrow() {
            "canceled before dispatch"
        } else {
            "whole-capture deadline before dispatch"
        }
        .into();
    } else {
        a.dispatched = true;
        let response = tokio::select! {
            biased;
            _ = cancel.changed() => { a.status = Status::Interrupted; a.detail = "canceled before headers".into(); None },
            _ = tokio::time::sleep_until(total) => { a.status = Status::TotalTimeout; a.detail = "total deadline before headers".into(); None },
            _ = tokio::time::sleep_until(idle) => { a.status = Status::IdleTimeout; a.detail = "idle deadline before body".into(); None },
            r = client.execute(request) => match r { Ok(r) => Some(r), Err(e) => { a.status = Status::TransportError; a.detail = error_kind(&e).into(); None } },
        };
        if let Some(mut response) = response {
            a.http_status = Some(response.status().as_u16());
            a.timing.headers_us = Some(us(sent));
            let wanted = if stream {
                "text/event-stream"
            } else {
                "application/json"
            };
            let media_ok = {
                let mut types = response.headers().get_all("content-type").iter();
                types.next().is_some_and(|value| {
                    value.to_str().is_ok_and(|v| {
                        v.split(';')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .eq_ignore_ascii_case(wanted)
                    })
                }) && types.next().is_none()
            };
            let encoding_ok = response
                .headers()
                .get_all("content-encoding")
                .iter()
                .all(|v| {
                    v.to_str().is_ok_and(|s| {
                        s.split(',')
                            .all(|p| p.trim().eq_ignore_ascii_case("identity"))
                    })
                });
            let parse = response.status().as_u16() == 200 && media_ok && encoding_ok;
            if response.status().as_u16() != 200 {
                a.status = Status::HttpError;
                a.detail = "non-200 HTTP response".into();
            } else if !parse {
                a.status = Status::Unsupported;
                a.detail = "unsupported response media type or encoding".into();
            }
            let mut parser = Parser::new(SseLimits {
                line_bytes: FRAME_CAP,
                event_bytes: FRAME_CAP,
            })
            .expect("constant valid framing limits");
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancel.changed() => { a.status = Status::Interrupted; a.detail = "canceled while receiving body".into(); break; },
                    _ = tokio::time::sleep_until(total) => { a.status = Status::TotalTimeout; a.detail = "total response deadline".into(); break; },
                    _ = tokio::time::sleep_until(idle) => { a.status = Status::IdleTimeout; a.detail = "idle response deadline".into(); break; },
                    r = response.chunk() => r,
                };
                let chunk = match chunk {
                    Err(e) => {
                        a.status = Status::TransportError;
                        a.detail = error_kind(&e).into();
                        break;
                    }
                    Ok(None) => {
                        if parse && !stream {
                            let processing = Instant::now();
                            match semantic.event(&body, false, us(sent), &mut a.timing) {
                                Ok(Flow::Stop) => {
                                    a.status = Status::Complete;
                                    a.detail = "complete JSON response".into();
                                    a.terminal_offset = Some(body.len());
                                }
                                Ok(Flow::Continue) => (),
                                Err((status, detail)) => {
                                    a.status = status;
                                    a.detail = detail.into();
                                }
                            }
                            a.timing.capture_parse_us += us(processing);
                        }
                        break;
                    }
                    Ok(Some(chunk)) if chunk.is_empty() => continue,
                    Ok(Some(chunk)) => chunk,
                };
                let observed = us(sent);
                if a.timing.first_body_us.is_none() {
                    a.timing.first_body_us = Some(observed);
                }
                idle =
                    tokio::time::Instant::now() + Duration::from_millis(u64::from(limits.idle_ms));
                let processing = Instant::now();
                let previous = body.len();
                let retained = chunk.len().min(limits.response_bytes - previous);
                body.extend_from_slice(&chunk[..retained]);
                if let Some(arrivals) = &mut a.timing.tool_stream_arrivals
                    && retained != 0
                {
                    arrivals.push(ToolArrival {
                        end_offset: body.len(),
                        observed_us: observed,
                    });
                }
                let mut done = false;
                if parse && stream {
                    let mut output_exceeded = false;
                    let mut notification_failed = false;
                    match parser.feed(&chunk[..retained], |event| {
                        let had_first = a.timing.first_generated_text_us.is_some();
                        let result = semantic.event(event, true, observed, &mut a.timing);
                        if !had_first
                            && let Some(first) = a.timing.first_generated_text_us
                            && let Some(sender) = &first_generated
                        {
                            notification_failed =
                                a.timing.dispatch_offset_us.checked_add(first).is_none_or(
                                    |offset| {
                                        sender
                                            .try_send(crate::schedule::FirstGenerated {
                                                lane,
                                                offset_us: offset,
                                            })
                                            .is_err()
                                    },
                                );
                        }
                        output_exceeded |= semantic
                            .usage
                            .completion_tokens
                            .is_some_and(|tokens| tokens > u64::from(settings.output.tokens));
                        result
                    }) {
                        Ok(Some(consumed)) => {
                            a.status = Status::Complete;
                            a.detail = "complete SSE response".into();
                            a.terminal_offset = Some(previous + consumed);
                            a.surplus_observed_bytes = chunk.len() - consumed;
                            done = true;
                        }
                        Ok(None) => (),
                        Err(grill_sse::Error::Handler((status, detail))) => {
                            a.status = status;
                            a.detail = detail.into();
                            done = true;
                        }
                        Err(grill_sse::Error::Framing(_)) => {
                            a.status = Status::Malformed;
                            a.detail = "SSE framing or UTF-8 limit violated".into();
                            done = true;
                        }
                    }
                    // Keep replayable semantic facts from this retained chunk.
                    // A local channel failure must not impersonate a wire parse error.
                    if notification_failed {
                        a.status = Status::Unsupported;
                        a.detail = "schedule first-generated notification failed".into();
                        done = true;
                    }
                    if output_exceeded {
                        a.status = Status::Unsupported;
                        a.detail = "reported output exceeds declared cap".into();
                        done = true;
                    }
                }
                a.timing.capture_parse_us += us(processing);
                if done && a.status == Status::Complete && tokio::time::Instant::now() >= total {
                    a.status = Status::TotalTimeout;
                    a.detail = "total deadline during completion parsing".into();
                }
                if done {
                    break;
                }
                if a.timing
                    .tool_stream_arrivals
                    .as_ref()
                    .is_some_and(|arrivals| arrivals.len() == TOOL_ARRIVAL_CAP)
                {
                    a.status = Status::ResponseLimit;
                    a.detail = "tool arrival trace reached 4096 chunks".into();
                    break;
                }
                if retained < chunk.len() {
                    a.status = Status::ResponseLimit;
                    a.detail = "response byte cap reached".into();
                    break;
                }
                if tokio::time::Instant::now() >= total {
                    a.status = Status::TotalTimeout;
                    a.detail = "total deadline during parsing".into();
                    break;
                }
            }
            if !parse
                && matches!(
                    a.status,
                    Status::Interrupted | Status::TotalTimeout | Status::IdleTimeout
                )
            {
                // A later budget/cancellation cannot erase an already observed invalid response.
                if a.http_status != Some(200) {
                    a.status = Status::HttpError;
                    a.detail = "non-200 HTTP response".into();
                } else {
                    a.status = Status::Unsupported;
                    a.detail = "unsupported response media type or encoding".into();
                }
            }
        }
    }
    if a.status == Status::Complete && !semantic.generated {
        a.status = Status::Unsupported;
        a.detail = "completed response contained no generated text".into();
    }
    if semantic
        .usage
        .completion_tokens
        .is_some_and(|tokens| tokens > u64::from(settings.output.tokens))
    {
        a.status = Status::Unsupported;
        a.detail = "reported output exceeds declared cap".into();
    }
    a.finish_reason = semantic.finish;
    a.usage = semantic.usage;
    a.timing.settle_us = us(sent);
    if a.status == Status::Complete && tokio::time::Instant::now() >= total {
        a.status = Status::TotalTimeout;
        a.detail = "total deadline before completion settlement".into();
    }
    if whole.is_some_and(|whole| tokio::time::Instant::now() >= whole)
        && matches!(
            a.status,
            Status::Complete | Status::TotalTimeout | Status::Interrupted
        )
    {
        a.status = Status::Interrupted;
        a.detail = "whole-capture deadline".into();
    }
    if a.status != Status::Complete {
        a.timing.first_validated_tool_call_us = None;
    }
    a.response_bytes = body.len();
    if !a.dispatched && first_generated.is_some() {
        // Scheduling cancellation before the future's first poll is not dispatch.
        a.timing = Timing::default();
    }
    Collected { attempt: a, body }
}

#[cfg(test)]
#[path = "../tests/support/measurement.rs"]
mod measurement_fixtures;

#[cfg(test)]
#[path = "../tests/support/measurement_wire.rs"]
mod measurement_tests;

#[cfg(test)]
#[path = "../tests/support/tool_stream_wire.rs"]
mod tool_stream_tests;

#[cfg(test)]
#[path = "../tests/support/prose_wire.rs"]
mod prose_tests;
