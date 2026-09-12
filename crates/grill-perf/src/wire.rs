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
    Legacy { thinking: bool },
    EnableThinking { enable_thinking: bool },
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
pub fn request_body(plan: &Plan, wave: &WaveSpec, lane: u32) -> Result<String> {
    let r = crate::sequence::settings(&plan.workload, wave);
    let case = plan
        .workload
        .cases
        .iter()
        .find(|c| c.id == wave.case)
        .ok_or("unknown case")?;
    let exact = r.profile == Profile::VllmFixedV1 && r.output.mode == OutputMode::Exact;
    let salt = match r.cache {
        Cache::Observe => None,
        cache => {
            let nonce = plan
                .cache_namespace
                .as_deref()
                .ok_or("missing cache namespace")?;
            Some(if cache == Cache::ReportedPrefixZero {
                format!("{nonce}-{}-{lane}", wave.index)
            } else {
                format!("{nonce}-{}-{lane}", wave.cell)
            })
        }
    };
    let messages = if let Some(fill) = &case.fill {
        let namespace = plan
            .cache_namespace
            .as_deref()
            .ok_or("missing cache namespace")?;
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
                            let mut rendered = String::with_capacity(
                                header.len()
                                    + footer.len()
                                    + fill.unit.len() * fill.repeat as usize,
                            );
                            rendered.push_str(header);
                            for _ in 0..fill.repeat {
                                rendered.push_str(&fill.unit);
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
        model: &plan.model,
        messages: &messages,
        stream: r.stream,
        max_tokens: r.output.tokens,
        temperature: r.temperature_milli.map(|n| f64::from(n) / 1000.0),
        top_p: r.top_p_milli.map(|n| f64::from(n) / 1000.0),
        seed: r
            .seed
            .map(|seed| seed + i64::from(wave.trial) * 64 + i64::from(lane)),
        chat_template_kwargs: match (r.thinking, r.thinking_control) {
            (Some(thinking), None) => Some(ChatTemplateKwargs::Legacy { thinking }),
            (None, Some(ThinkingControl::VllmEnableThinkingV1 { enabled })) => {
                Some(ChatTemplateKwargs::EnableThinking {
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
    if body.len() > REQUEST_CAP {
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
struct Semantic {
    allow_tools: bool,
    id: Option<String>,
    finish: Option<String>,
    usage: Usage,
    content: bool,
    generated: bool,
    answer: Option<String>,
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
            return if self.finish.is_some() {
                timing.terminal_us = Some(observed);
                Ok(Flow::Stop)
            } else {
                Err((Status::Malformed, "DONE before finish"))
            };
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
            if (tools && !self.allow_tools)
                || present(delta.function_call)
                || present(delta.audio)
                || present(delta.refusal)
            {
                return Err((Status::Unsupported, "non-text answer or refusal"));
            }
            if tools && self.allow_tools {
                if stream || self.finish.is_some() {
                    return Err((
                        Status::Unsupported,
                        "tool profile requires one nonstreaming message",
                    ));
                }
                self.generated = true;
            }
            let content = delta.content.as_ref().is_some_and(|s| !s.is_empty());
            let reasoning = delta.reasoning.as_ref().is_some_and(|s| !s.is_empty())
                || delta
                    .reasoning_content
                    .as_ref()
                    .is_some_and(|s| !s.is_empty());
            let generated = content || reasoning;
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
            if let (Some(answer), Some(content)) = (&mut self.answer, delta.content.as_ref()) {
                if answer.len() + content.len() > 64 * 1024 {
                    return Err((
                        Status::ResponseLimit,
                        "sequence answer exceeds response bound",
                    ));
                }
                answer.push_str(content);
            }
            self.content |= content;
            self.generated |= generated;
        }
        if let Some(reason) = choice.finish_reason {
            if !matches!(reason.as_ref(), "stop" | "length")
                && !(self.allow_tools && !stream && reason == "tool_calls")
            {
                return Err((Status::Unsupported, "unsupported finish reason"));
            }
            if self.finish.is_some() {
                return Err((Status::Malformed, "duplicate finish"));
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
}

pub fn verify_complete(
    attempt: &Attempt,
    body: &[u8],
    stream: bool,
    arrival_contract: bool,
    profile: Profile,
) -> Result<()> {
    complete_semantic(attempt, body, stream, arrival_contract, profile, false).map(|_| ())
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
        true,
    )?
    .answer
    .ok_or_else(|| "sequence response lacks answer text".into())
}

fn complete_semantic(
    attempt: &Attempt,
    body: &[u8],
    stream: bool,
    arrival_contract: bool,
    profile: Profile,
    retain_answer: bool,
) -> Result<Semantic> {
    let mut semantic = Semantic {
        allow_tools: profile == Profile::VllmConversationV2,
        answer: retain_answer.then(String::new),
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

pub struct Collected {
    pub attempt: Attempt,
    pub body: Vec<u8>,
}
pub async fn collect(
    client: reqwest::Client,
    request: reqwest::Request,
    limits: Limits,
    settings: RequestSettings,
    lane: u32,
    window: (Instant, Option<Instant>),
    mut cancel: watch::Receiver<bool>,
) -> Collected {
    let (origin, deadline) = window;
    let stream = settings.stream;
    let sent = Instant::now();
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
                let mut done = false;
                if parse && stream {
                    let mut output_exceeded = false;
                    match parser.feed(&chunk[..retained], |event| {
                        let result = semantic.event(event, true, observed, &mut a.timing);
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
    a.response_bytes = body.len();
    Collected { attempt: a, body }
}

#[cfg(test)]
#[path = "../tests/support/measurement.rs"]
mod measurement_fixtures;

#[cfg(test)]
#[path = "../tests/support/measurement_wire.rs"]
mod measurement_tests;
