// One typed text-only Chat Completions adapter: endpoint admission, a reusable
// client with every automatic behavior disabled, exact request encoding and the
// strict response semantics for one choice. No warmup, discovery, retry,
// redirect, proxy, compression or fallback request exists here.

use crate::contract::*;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;
use std::io::Write;
use std::marker::PhantomData;

pub(crate) const PROFILE: &str = "chat-completions-json-post-v1";
pub(crate) const USER_AGENT: &str = concat!("grill/", env!("CARGO_PKG_VERSION"));
const NONSTREAM_TYPE: &str = "application/json";
const STREAM_TYPE: &str = "text/event-stream";

pub(crate) struct Endpoint {
    url: reqwest::Url,
    pub(crate) local: bool,
}

impl Endpoint {
    /// Owner-selected absolute URL of the Chat Completions resource. HTTPS is
    /// normal; plain HTTP requires the explicit local mode and a loopback IP
    /// literal. Userinfo, query strings and fragments are rejected outright.
    pub(crate) fn parse(text: &str, local_http: bool) -> Result<Self> {
        let url = reqwest::Url::parse(text).map_err(|_| "endpoint is not an absolute URL")?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("endpoint must not carry userinfo".into());
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("endpoint must not carry a query string or fragment".into());
        }
        // The url crate is only reachable through reqwest's re-export; inspect
        // its canonical host serialization instead of naming its Host type.
        let host = url.host_str().ok_or("endpoint requires a host")?;
        let loopback = host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
            || host
                .strip_prefix('[')
                .and_then(|h| h.strip_suffix(']'))
                .and_then(|h| h.parse::<std::net::Ipv6Addr>().ok())
                .is_some_and(|ip| ip.is_loopback());
        let local = match url.scheme() {
            "https" => false,
            "http" if local_http && loopback => true,
            "http" => {
                return Err(
                    "plain http requires --local-http and a loopback IP literal host".into(),
                );
            }
            _ => return Err("endpoint scheme must be https or local http".into()),
        };
        Ok(Self { url, local })
    }

    pub(crate) fn text(&self) -> &str {
        self.url.as_str()
    }
}

/// Validate that a named credential exists and is header-safe. The value is
/// dropped here; dispatch resolves it again and never records it.
pub(crate) fn credential(name: &str) -> Result<()> {
    let value = std::env::var(name)
        .map_err(|_| "credential environment variable is unset or not Unicode")?;
    if value.is_empty() || value.len() > 8192 || !value.bytes().all(|b| (0x21..0x7f).contains(&b)) {
        return Err("credential must be nonempty visible ASCII of at most 8192 bytes".into());
    }
    Ok(())
}

pub(crate) struct Client {
    inner: reqwest::Client,
    url: reqwest::Url,
    accept: &'static str,
}

impl Client {
    pub(crate) fn new(endpoint: &Endpoint, stream: bool) -> Result<Self> {
        let inner = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .https_only(!endpoint.local)
            .http1_only()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .referer(false)
            .pool_max_idle_per_host(1)
            .build()
            .map_err(|_| "client construction failed")?;
        Ok(Self {
            inner,
            url: endpoint.url.clone(),
            accept: if stream { STREAM_TYPE } else { NONSTREAM_TYPE },
        })
    }

    /// Build one request. The credential is resolved from the named variable
    /// right here and lives only inside the request's sensitive header.
    pub(crate) fn request(&self, body: String, auth_env: Option<&str>) -> Result<reqwest::Request> {
        let mut builder = self
            .inner
            .post(self.url.clone())
            .header(CONTENT_TYPE, NONSTREAM_TYPE)
            .header(ACCEPT, self.accept)
            .header(ACCEPT_ENCODING, "identity")
            .body(body);
        if let Some(name) = auth_env {
            let token = std::env::var(name).map_err(|_| "credential unavailable at dispatch")?;
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| "credential is not a valid header value")?;
            value.set_sensitive(true);
            builder = builder.header(AUTHORIZATION, value);
        }
        builder
            .build()
            .map_err(|_| "request construction failed".into())
    }

    pub(crate) fn execute(
        &self,
        request: reqwest::Request,
    ) -> impl Future<Output = reqwest::Result<reqwest::Response>> {
        self.inner.execute(request)
    }
}

/// Never surface reqwest's Display output: it can embed the request URL.
pub(crate) fn classify(error: &reqwest::Error) -> &'static str {
    if error.is_connect() {
        "connection failed"
    } else if error.is_timeout() {
        "client-side timeout"
    } else if error.is_body() || error.is_decode() {
        "body transfer failed before entity end"
    } else if error.is_request() {
        "request failed"
    } else if error.is_builder() {
        "client refused the request"
    } else {
        "unclassified transport error"
    }
}

/// Nonsecret entity facts retained in the terminal receipt.
pub(crate) fn entity(response: &reqwest::Response) -> Http {
    Http {
        status: response.status().as_u16(),
        version: format!("{:?}", response.version()),
        content_type: response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.chars().take(128).collect()),
        content_length: response.content_length(),
    }
}

/// Whether a content-type value's media type is the one this mode parses.
pub(crate) fn media_type_matches(value: &str, stream: bool) -> bool {
    let expected = if stream { STREAM_TYPE } else { NONSTREAM_TYPE };
    value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case(expected)
}

/// Names the reason a 200 entity is outside the profile, if any. Every header
/// occurrence and list member counts: duplicate media types are ambiguous and
/// any non-identity coding is out of profile.
pub(crate) fn entity_fault(response: &reqwest::Response, stream: bool) -> Option<&'static str> {
    let headers = response.headers();
    let mut types = headers.get_all(CONTENT_TYPE).iter();
    let (Some(content_type), None) = (types.next(), types.next()) else {
        return Some("missing or repeated content-type");
    };
    if !content_type
        .to_str()
        .is_ok_and(|v| media_type_matches(v, stream))
    {
        return Some("unexpected content-type");
    }
    for encoding in headers.get_all(reqwest::header::CONTENT_ENCODING) {
        let Ok(list) = encoding.to_str() else {
            return Some("unreadable content-encoding");
        };
        if !list
            .split(',')
            .all(|coding| coding.trim().eq_ignore_ascii_case("identity"))
        {
            return Some("encoded content is outside the profile");
        }
    }
    None
}

/// Nonsecret request headers for the profile; recorded in every reservation.
pub(crate) fn headers(stream: bool) -> Vec<(String, String)> {
    vec![
        ("content-type".into(), NONSTREAM_TYPE.into()),
        (
            "accept".into(),
            if stream { STREAM_TYPE } else { NONSTREAM_TYPE }.into(),
        ),
        ("accept-encoding".into(), "identity".into()),
        ("user-agent".into(), USER_AGENT.into()),
    ]
}

/// The fixed transport declaration for a plan; replays recompute and compare it.
pub(crate) fn plan_transport(local: bool, stream: bool, auth_env: Option<String>) -> Transport {
    Transport {
        profile: PROFILE.into(),
        http: "http1-only".into(),
        tls: if local {
            "owner-local-plain-http-loopback".into()
        } else {
            "https-rustls-platform-verifier".into()
        },
        auth_env,
        redirects: "none".into(),
        retries: "none".into(),
        proxy: "none".into(),
        content_encoding: "identity".into(),
        commitment: if stream {
            "finish-then-blank-line-terminated-DONE".into()
        } else {
            "complete-entity-one-finished-choice".into()
        },
        request_cap: REQUEST_CAP as u32,
        sse_line_cap: SSE_LINE_CAP as u32,
        sse_event_cap: SSE_EVENT_CAP as u32,
    }
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

fn request_record<'a>(model: &'a str, messages: &'a [Message], p: &Protocol) -> Request<'a> {
    let (max_tokens, max_completion_tokens) = match p.token_cap.field {
        TokenField::MaxTokens => (Some(p.token_cap.value), None),
        TokenField::MaxCompletionTokens => (None, Some(p.token_cap.value)),
    };
    Request {
        model,
        messages,
        stream: p.stream,
        max_tokens,
        max_completion_tokens,
        temperature: p.temperature_milli.map(|t| f64::from(t) / 1000.0),
        top_p: p.top_p_milli.map(|t| f64::from(t) / 1000.0),
        seed: p.seed,
        reasoning_effort: p.reasoning_effort,
        // include_usage=true is requested verbatim; a declared false is not a
        // request and stays off the wire, exactly like an omitted option.
        stream_options: (p.include_usage == Some(true)).then_some(StreamOptions {
            include_usage: true,
        }),
    }
}

struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Encoded size without allocating the body; used by zero-network preflight.
pub(crate) fn request_size(model: &str, messages: &[Message], p: &Protocol) -> Result<usize> {
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, &request_record(model, messages, p))
        .map_err(|e| format!("request encoding: {e}"))?;
    Ok(counter.0)
}

pub(crate) fn messages_size(messages: &[Message]) -> Result<usize> {
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, messages).map_err(|e| format!("message encoding: {e}"))?;
    Ok(counter.0)
}

pub(crate) fn request_body(model: &str, messages: &[Message], p: &Protocol) -> Result<String> {
    serde_json::to_string(&request_record(model, messages, p))
        .map_err(|e| format!("request encoding: {e}"))
}

// ----- Strict response semantics -----

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    Unsupported(&'static str),
    Malformed(&'static str),
    Provider,
    ArtifactCap,
    Frame(grill_sse::Framing),
}

impl Fault {
    pub(crate) fn reason(&self) -> Reason {
        match self {
            Fault::Unsupported(_) => Reason::UnsupportedResponse,
            Fault::Malformed(_) => Reason::MalformedEnvelope,
            Fault::Provider => Reason::ProviderError,
            Fault::ArtifactCap => Reason::ArtifactCap,
            Fault::Frame(grill_sse::Framing::Utf8) => Reason::MalformedEnvelope,
            Fault::Frame(_) => Reason::FrameCap,
        }
    }
    pub(crate) fn detail(&self) -> &'static str {
        match self {
            Fault::Unsupported(d) | Fault::Malformed(d) => d,
            Fault::Provider => "non-null top-level provider error",
            Fault::ArtifactCap => "final content exceeds the artifact cap",
            Fault::Frame(grill_sse::Framing::Line) => "SSE line exceeds the line cap",
            Fault::Frame(grill_sse::Framing::Event) => "SSE event data exceeds the event cap",
            Fault::Frame(grill_sse::Framing::Utf8) => "SSE stream is not strict UTF-8",
        }
    }
}

/// Depth-guarded discard of one ignored value. Unlike serde's IgnoredAny this
/// drives the JSON parser's own recursive path, so its nesting limit applies to
/// provider metadata; nothing is retained.
struct Skip;

impl<'de> Deserialize<'de> for Skip {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Skip;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Skip, E> {
                Ok(Skip)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Skip, E> {
                Ok(Skip)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Skip, E> {
                Ok(Skip)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Skip, E> {
                Ok(Skip)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Skip, E> {
                Ok(Skip)
            }
            fn visit_bytes<E: de::Error>(self, _: &[u8]) -> Result<Skip, E> {
                Ok(Skip)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Skip, E> {
                Ok(Skip)
            }
            fn visit_none<E: de::Error>(self) -> Result<Skip, E> {
                Ok(Skip)
            }
            fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Skip, D2::Error> {
                d.deserialize_any(V)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Skip, A::Error> {
                while seq.next_element::<Skip>()?.is_some() {}
                Ok(Skip)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Skip, A::Error> {
                while map.next_entry::<Skip, Skip>()?.is_some() {}
                Ok(Skip)
            }
        }
        d.deserialize_any(V)
    }
}

/// A wire record: a JSON object whose known fields are read once each and
/// whose unknown fields are skipped with the depth guard. Positional arrays
/// are not records.
trait Wire: Default {
    const NAME: &'static str;
    fn field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error>;
}

fn wire<'de, D: Deserializer<'de>, T: Wire>(d: D) -> Result<T, D::Error> {
    struct V<T>(PhantomData<T>);
    impl<'de, T: Wire> Visitor<'de> for V<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a {} object", T::NAME)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<T, A::Error> {
            let mut record = T::default();
            while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                if !record.field(&key, &mut map)? {
                    map.next_value::<Skip>()?;
                }
            }
            Ok(record)
        }
    }
    d.deserialize_map(V(PhantomData))
}

/// Read a known field exactly once.
fn once<'de, A: MapAccess<'de>, T: Deserialize<'de>>(
    seen: &mut bool,
    name: &'static str,
    map: &mut A,
) -> Result<T, A::Error> {
    if std::mem::replace(seen, true) {
        return Err(de::Error::duplicate_field(name));
    }
    map.next_value()
}

/// A known answer channel: absent, null, text, or a known-unsupported shape.
#[derive(Default)]
enum Channel {
    #[default]
    Absent,
    Null,
    Text(String),
    Unsupported,
}

impl<'de> Deserialize<'de> for Channel {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Channel;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("null or a string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Channel, E> {
                Ok(Channel::Text(v.to_owned()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Channel, E> {
                Ok(Channel::Text(v))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Channel, E> {
                Ok(Channel::Null)
            }
            fn visit_none<E: de::Error>(self) -> Result<Channel, E> {
                Ok(Channel::Null)
            }
            fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Channel, D2::Error> {
                d.deserialize_any(V)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Channel, E> {
                Ok(Channel::Unsupported)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Channel, E> {
                Ok(Channel::Unsupported)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Channel, E> {
                Ok(Channel::Unsupported)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Channel, E> {
                Ok(Channel::Unsupported)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Channel, A::Error> {
                while seq.next_element::<Skip>()?.is_some() {}
                Ok(Channel::Unsupported)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Channel, A::Error> {
                while map.next_entry::<Skip, Skip>()?.is_some() {}
                Ok(Channel::Unsupported)
            }
        }
        d.deserialize_any(V)
    }
}

/// Presence of a known non-answer channel (tool/function calls, audio, top-level
/// error). Null and empty arrays count as absent because some servers always
/// emit them.
#[derive(Default)]
struct Presence(bool);

impl<'de> Deserialize<'de> for Presence {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Presence;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("null, an array or an object")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Presence, E> {
                Ok(Presence(false))
            }
            fn visit_none<E: de::Error>(self) -> Result<Presence, E> {
                Ok(Presence(false))
            }
            fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Presence, D2::Error> {
                d.deserialize_any(V)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Presence, A::Error> {
                let mut present = false;
                while seq.next_element::<Skip>()?.is_some() {
                    present = true;
                }
                Ok(Presence(present))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Presence, A::Error> {
                while map.next_entry::<Skip, Skip>()?.is_some() {}
                Ok(Presence(true))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Presence, E> {
                Ok(Presence(true))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Presence, E> {
                Ok(Presence(true))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Presence, E> {
                Ok(Presence(true))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Presence, E> {
                Ok(Presence(true))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Presence, E> {
                Ok(Presence(true))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Default)]
struct Turn {
    seen: [bool; 6],
    role: Option<String>,
    content: Channel,
    refusal: Channel,
    tool_calls: Presence,
    function_call: Presence,
    audio: Presence,
}

impl Wire for Turn {
    const NAME: &'static str = "message";
    fn field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error> {
        match key {
            "role" => self.role = once(&mut self.seen[0], "role", map)?,
            "content" => self.content = once(&mut self.seen[1], "content", map)?,
            "refusal" => self.refusal = once(&mut self.seen[2], "refusal", map)?,
            "tool_calls" => self.tool_calls = once(&mut self.seen[3], "tool_calls", map)?,
            "function_call" => self.function_call = once(&mut self.seen[4], "function_call", map)?,
            "audio" => self.audio = once(&mut self.seen[5], "audio", map)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl<'de> Deserialize<'de> for Turn {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        wire(d)
    }
}

#[derive(Default)]
struct Choice {
    seen: [bool; 4],
    index: Option<u64>,
    message: Option<Turn>,
    delta: Option<Turn>,
    finish_reason: Option<String>,
}

impl Wire for Choice {
    const NAME: &'static str = "choice";
    fn field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error> {
        match key {
            "index" => self.index = once(&mut self.seen[0], "index", map)?,
            "message" => self.message = once(&mut self.seen[1], "message", map)?,
            "delta" => self.delta = once(&mut self.seen[2], "delta", map)?,
            "finish_reason" => {
                self.finish_reason = once(&mut self.seen[3], "finish_reason", map)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl<'de> Deserialize<'de> for Choice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        wire(d)
    }
}

/// Provider usage may carry extra detail objects; only the counts are retained.
#[derive(Default)]
struct WireUsage {
    seen: [bool; 3],
    usage: Usage,
}

impl Wire for WireUsage {
    const NAME: &'static str = "usage";
    fn field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error> {
        match key {
            "prompt_tokens" => {
                self.usage.prompt_tokens = once(&mut self.seen[0], "prompt_tokens", map)?;
            }
            "completion_tokens" => {
                self.usage.completion_tokens = once(&mut self.seen[1], "completion_tokens", map)?;
            }
            "total_tokens" => {
                self.usage.total_tokens = once(&mut self.seen[2], "total_tokens", map)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl<'de> Deserialize<'de> for WireUsage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        wire(d)
    }
}

/// Only the first choice is retained; further choices are counted, not stored.
struct Choices {
    first: Option<Choice>,
    count: usize,
}

impl<'de> Deserialize<'de> for Choices {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Choices;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an array of choice objects")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Choices, A::Error> {
                let first = seq.next_element::<Choice>()?;
                let mut count = usize::from(first.is_some());
                while seq.next_element::<Skip>()?.is_some() {
                    count += 1;
                }
                Ok(Choices { first, count })
            }
        }
        d.deserialize_seq(V)
    }
}

#[derive(Default)]
struct Envelope {
    seen: [bool; 4],
    choices: Option<Choices>,
    usage: Option<WireUsage>,
    id: Option<String>,
    error: Presence,
}

impl Wire for Envelope {
    const NAME: &'static str = "chat completion envelope";
    fn field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error> {
        match key {
            "choices" => self.choices = Some(once(&mut self.seen[0], "choices", map)?),
            "usage" => self.usage = once(&mut self.seen[1], "usage", map)?,
            "id" => {
                // An empty id (some preamble chunks) carries no identity.
                self.id = once::<_, Option<Cow<'de, str>>>(&mut self.seen[2], "id", map)?
                    .filter(|id| !id.is_empty())
                    .map(Cow::into_owned);
            }
            "error" => self.error = once(&mut self.seen[3], "error", map)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// One closed envelope from validated UTF-8 text; trailing bytes are malformed.
fn parse(bytes: &[u8]) -> Result<Envelope, Fault> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| Fault::Malformed("entity is not strict UTF-8"))?;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let envelope: Envelope = wire(&mut deserializer)
        .map_err(|_| Fault::Malformed("envelope is not a closed JSON object of the profile"))?;
    deserializer
        .end()
        .map_err(|_| Fault::Malformed("trailing bytes after the envelope"))?;
    if envelope.error.0 {
        return Err(Fault::Provider);
    }
    Ok(envelope)
}

fn finish(reason: &str) -> Result<Finish, Fault> {
    match reason {
        "stop" => Ok(Finish::Stop),
        "length" => Ok(Finish::Length),
        "content_filter" => Ok(Finish::ContentFilter),
        "tool_calls" | "function_call" => Err(Fault::Unsupported("tool or function finish")),
        _ => Err(Fault::Unsupported("unrecognized finish_reason")),
    }
}

fn single(choices: Option<Choices>) -> Result<Option<Choice>, Fault> {
    let choices = choices.ok_or(Fault::Malformed("envelope without choices"))?;
    match choices.count {
        0 => Ok(None),
        1 => {
            let choice = choices.first.expect("counted");
            if choice.index.is_some_and(|i| i != 0) {
                return Err(Fault::Unsupported("choice index is not 0"));
            }
            Ok(Some(choice))
        }
        _ => Err(Fault::Unsupported("more than one choice")),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Answer(Vec<u8>),
    Refused,
    Absent,
}

pub(crate) struct Settled {
    pub stop: Finish,
    pub delivery: Delivery,
    pub usage: Option<Usage>,
}

struct Semantic {
    artifact_cap: usize,
    artifact: Vec<u8>,
    content_seen: bool,
    refused: bool,
    finish: Option<Finish>,
    usage: Option<Usage>,
    id: Option<String>,
}

impl Semantic {
    fn identity(&mut self, id: Option<String>) -> Result<(), Fault> {
        let Some(id) = id else {
            return Ok(());
        };
        match &self.id {
            Some(seen) if *seen != id => Err(Fault::Malformed("response identity changed")),
            Some(_) => Ok(()),
            None => {
                self.id = Some(id);
                Ok(())
            }
        }
    }

    fn turn(&mut self, turn: Turn) -> Result<(), Fault> {
        if turn.role.as_deref().is_some_and(|r| r != "assistant") {
            return Err(Fault::Unsupported("non-assistant role"));
        }
        if turn.tool_calls.0 || turn.function_call.0 {
            return Err(Fault::Unsupported("tool or function call channel"));
        }
        if turn.audio.0 {
            return Err(Fault::Unsupported("audio channel"));
        }
        match turn.refusal {
            Channel::Text(text) if !text.is_empty() => self.refused = true,
            Channel::Unsupported => return Err(Fault::Malformed("refusal is not a string")),
            _ => {}
        }
        match turn.content {
            Channel::Text(text) => {
                self.content_seen = true;
                if self.artifact.len() + text.len() > self.artifact_cap {
                    return Err(Fault::ArtifactCap);
                }
                self.artifact.extend_from_slice(text.as_bytes());
            }
            Channel::Unsupported => return Err(Fault::Malformed("content is not a string")),
            Channel::Absent | Channel::Null => {}
        }
        Ok(())
    }

    fn event(&mut self, data: &[u8]) -> Result<grill_sse::Flow, Fault> {
        if data == b"[DONE]" {
            if self.finish.is_none() {
                return Err(Fault::Malformed("DONE before a recognized finish"));
            }
            return Ok(grill_sse::Flow::Stop);
        }
        let envelope = parse(data)?;
        self.identity(envelope.id)?;
        if let Some(usage) = envelope.usage {
            self.usage = Some(usage.usage);
        }
        let Some(choice) = single(envelope.choices)? else {
            return Ok(grill_sse::Flow::Continue);
        };
        if self.finish.is_some() {
            return Err(Fault::Malformed("choice content after finish"));
        }
        if choice.message.is_some() {
            return Err(Fault::Malformed("message channel in a streamed choice"));
        }
        let delta = choice
            .delta
            .ok_or(Fault::Malformed("streamed choice without delta"))?;
        self.turn(delta)?;
        if let Some(reason) = choice.finish_reason {
            self.finish = Some(finish(&reason)?);
        }
        Ok(grill_sse::Flow::Continue)
    }

    fn settle(self) -> Settled {
        let stop = self.finish.expect("settled only after finish");
        let delivery = if self.refused || stop == Finish::ContentFilter {
            Delivery::Refused
        } else if self.content_seen {
            Delivery::Answer(self.artifact)
        } else {
            Delivery::Absent
        };
        Settled {
            stop,
            delivery,
            usage: self.usage,
        }
    }
}

pub(crate) struct Collector {
    parser: grill_sse::Parser,
    state: Semantic,
}

impl Collector {
    pub(crate) fn new(artifact_cap: usize) -> Self {
        Self {
            parser: grill_sse::Parser::new(grill_sse::SseLimits {
                line_bytes: SSE_LINE_CAP,
                event_bytes: SSE_EVENT_CAP,
            })
            .expect("fixed quality SSE limits are valid"),
            state: Semantic {
                artifact_cap,
                artifact: Vec::new(),
                content_seen: false,
                refused: false,
                finish: None,
                usage: None,
                id: None,
            },
        }
    }

    /// Streaming: returns `Some(consumed)` once the decoded `[DONE]` event and
    /// its terminating blank line ended `consumed` bytes into this chunk.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Result<Option<usize>, Fault> {
        let state = &mut self.state;
        self.parser
            .feed(chunk, |data| state.event(data))
            .map_err(|e| match e {
                grill_sse::Error::Framing(f) => Fault::Frame(f),
                grill_sse::Error::Handler(f) => f,
            })
    }

    /// Nonstreaming: the complete entity must be strict UTF-8, one closed
    /// envelope, one finished assistant message and nothing else.
    pub(crate) fn entity(mut self, bytes: &[u8]) -> Result<Settled, Fault> {
        let envelope = parse(bytes)?;
        self.state.usage = envelope.usage.map(|u| u.usage);
        let choice =
            single(envelope.choices)?.ok_or(Fault::Malformed("no choice in the envelope"))?;
        if choice.delta.is_some() {
            return Err(Fault::Malformed("delta channel in a complete entity"));
        }
        let reason = choice
            .finish_reason
            .as_deref()
            .ok_or(Fault::Malformed("missing finish_reason"))?;
        self.state.finish = Some(finish(reason)?);
        let message = choice
            .message
            .ok_or(Fault::Malformed("choice without message"))?;
        self.state.turn(message)?;
        Ok(self.state.settle())
    }

    pub(crate) fn settle(self) -> Settled {
        self.state.settle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nonstream(body: &str) -> Result<Settled, Fault> {
        Collector::new(64).entity(body.as_bytes())
    }

    fn stream(frames: &[&str]) -> Result<Settled, Fault> {
        let mut c = Collector::new(64);
        for frame in frames {
            let text = format!("data: {frame}\n\n");
            if c.feed(text.as_bytes())?.is_some() {
                return Ok(c.settle());
            }
        }
        Err(Fault::Malformed("no DONE"))
    }

    #[test]
    fn nonstream_envelopes() {
        let ok = nonstream(
            r#"{"id":"x","object":"chat.completion","created":1,"model":"m","system_fingerprint":null,"choices":[{"index":0,"message":{"role":"assistant","content":"{\"answer\":\"T\"}","refusal":null,"tool_calls":[],"annotations":[],"reasoning_content":"private"},"logprobs":null,"finish_reason":"length","stop_reason":null}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3,"extra":{"deep":[[[1]]]}},"prompt_logprobs":null}"#,
        )
        .unwrap();
        assert_eq!(ok.stop, Finish::Length);
        assert_eq!(ok.delivery, Delivery::Answer(br#"{"answer":"T"}"#.to_vec()));
        assert_eq!(ok.usage.unwrap().total_tokens, Some(3));
        let refused = nonstream(
            r#"{"choices":[{"index":0,"message":{"role":"assistant","content":null,"refusal":"no"},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        assert_eq!(refused.delivery, Delivery::Refused);
        let filtered = nonstream(
            r#"{"choices":[{"message":{"content":"partial"},"finish_reason":"content_filter"}]}"#,
        )
        .unwrap();
        assert_eq!(filtered.delivery, Delivery::Refused);
        let empty = nonstream(r#"{"choices":[{"message":{"content":""},"finish_reason":"stop"}]}"#)
            .unwrap();
        assert_eq!(empty.delivery, Delivery::Answer(Vec::new()));
        let absent = nonstream(
            r#"{"choices":[{"message":{"content":null,"reasoning_content":"r"},"finish_reason":"length"}],"usage":{"completion_tokens":64}}"#,
        ).unwrap();
        assert_eq!(absent.delivery, Delivery::Absent);
        assert_eq!(absent.stop, Finish::Length);
        assert_eq!(absent.usage.unwrap().completion_tokens, Some(64));
        for (body, expected) in [
            (
                r#"{"choices":[{"message":{"content":"a","tool_calls":[{"id":"1"}]},"finish_reason":"stop"}]}"#,
                Fault::Unsupported("tool or function call channel"),
            ),
            (
                r#"{"choices":[{"message":{"content":"a"},"finish_reason":"tool_calls"}]}"#,
                Fault::Unsupported("tool or function finish"),
            ),
            (
                r#"{"choices":[{"message":{"content":"a"},"finish_reason":"stop"},{"message":{"content":"b"},"finish_reason":"stop"}]}"#,
                Fault::Unsupported("more than one choice"),
            ),
            (
                r#"{"choices":[{"index":1,"message":{"content":"a"},"finish_reason":"stop"}]}"#,
                Fault::Unsupported("choice index is not 0"),
            ),
            (
                r#"{"choices":[{"message":{"content":"a"},"finish_reason":"eos"}]}"#,
                Fault::Unsupported("unrecognized finish_reason"),
            ),
            (
                r#"{"choices":[{"message":{"content":"a"},"finish_reason":null}]}"#,
                Fault::Malformed("missing finish_reason"),
            ),
            (
                r#"{"choices":[{"message":{"content":[{"type":"text","text":"a"}]},"finish_reason":"stop"}]}"#,
                Fault::Malformed("content is not a string"),
            ),
            (
                r#"{"choices":[]}"#,
                Fault::Malformed("no choice in the envelope"),
            ),
            (
                r#"{"choices":[{"message":{"content":"a"},"finish_reason":"stop"}]}{}"#,
                Fault::Malformed("trailing bytes after the envelope"),
            ),
            (
                r#"{"choices":[{"message":{"content":"a"},"finish_reason":"stop"}],"choices":[]}"#,
                Fault::Malformed("envelope is not a closed JSON object of the profile"),
            ),
            (
                r#"[{"choices":[]}]"#,
                Fault::Malformed("envelope is not a closed JSON object of the profile"),
            ),
            (
                r#"{"choices":[{"message":{"content":"0123456789012345678901234567890123456789012345678901234567890123456789"},"finish_reason":"stop"}]}"#,
                Fault::ArtifactCap,
            ),
            (
                r#"{"choices":[{"message":{"role":"user","content":"a"},"finish_reason":"stop"}]}"#,
                Fault::Unsupported("non-assistant role"),
            ),
            (
                r#"{"choices":[{"message":{"role":7,"content":"a"},"finish_reason":"stop"}]}"#,
                Fault::Malformed("envelope is not a closed JSON object of the profile"),
            ),
            (
                r#"{"error":{"message":"quota"},"choices":[{"message":{"content":"a"},"finish_reason":"stop"}]}"#,
                Fault::Provider,
            ),
            (
                r#"{"choices":[{"message":{"content":"a"},"delta":{"tool_calls":[{"id":"1"}]},"finish_reason":"stop"}]}"#,
                Fault::Malformed("delta channel in a complete entity"),
            ),
            (
                r#"{"choices":[["assistant","a"]]}"#,
                Fault::Malformed("envelope is not a closed JSON object of the profile"),
            ),
            (
                r#"{"choices":[{"message":["assistant","a"],"finish_reason":"stop"}]}"#,
                Fault::Malformed("envelope is not a closed JSON object of the profile"),
            ),
            (
                r#"{"choices":[{"message":{"content":"a"},"finish_reason":"stop"}],"usage":[1,2,3]}"#,
                Fault::Malformed("envelope is not a closed JSON object of the profile"),
            ),
        ] {
            // The recorded reason category is the contract; detail phrases are not.
            assert_eq!(fault(nonstream(body)).reason(), expected.reason(), "{body}");
        }
        // Invalid raw UTF-8 inside ignored metadata is a whole-entity defect.
        let mut raw =
            br#"{"choices":[{"message":{"content":"a"},"finish_reason":"stop"}],"meta":"#.to_vec();
        raw.extend_from_slice(b"\"\xff\"}");
        assert_eq!(
            fault(Collector::new(64).entity(&raw)).reason(),
            Reason::MalformedEnvelope
        );
        // Ignored metadata nesting stays under the parser's recursion guard.
        let deep = format!(
            r#"{{"choices":[{{"message":{{"content":"a"}},"finish_reason":"stop"}}],"meta":{}1{}}}"#,
            "[".repeat(200),
            "]".repeat(200)
        );
        assert_eq!(fault(nonstream(&deep)).reason(), Reason::MalformedEnvelope);
        let shallow = format!(
            r#"{{"choices":[{{"message":{{"content":"a"}},"finish_reason":"stop"}}],"meta":{}1{}}}"#,
            "[".repeat(100),
            "]".repeat(100)
        );
        assert!(nonstream(&shallow).is_ok());
    }

    fn fault(result: Result<Settled, Fault>) -> Fault {
        match result {
            Err(fault) => fault,
            Ok(_) => panic!("settled where a fault was expected"),
        }
    }

    #[test]
    fn stream_transitions() {
        let ok = stream(&[
            r#"{"choices":[],"prompt_filter_results":[{"index":0}]}"#,
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"{\"answer\":"},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"\"T\"}","reasoning_content":"x"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"completion_tokens":4}}"#,
            "[DONE]",
        ])
        .unwrap();
        assert_eq!(ok.delivery, Delivery::Answer(br#"{"answer":"T"}"#.to_vec()));
        assert_eq!(ok.usage.unwrap().completion_tokens, Some(4));
        let refused = stream(&[
            r#"{"choices":[{"delta":{"refusal":"no"},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ])
        .unwrap();
        assert_eq!(refused.delivery, Delivery::Refused);
        let absent = stream(&[
            r#"{"choices":[{"delta":{"reasoning_content":"r"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"completion_tokens":4}}"#,
            "[DONE]",
        ])
        .unwrap();
        assert_eq!(absent.delivery, Delivery::Absent);
        assert_eq!(absent.stop, Finish::Stop);
        assert_eq!(absent.usage.unwrap().completion_tokens, Some(4));
        for (frames, expected) in [
            (
                vec![
                    r#"{"choices":[{"delta":{"content":"a"},"finish_reason":"stop"}]}"#,
                    r#"{"choices":[{"delta":{"content":"b"},"finish_reason":null}]}"#,
                ],
                Fault::Malformed("choice content after finish"),
            ),
            (
                vec![
                    r#"{"choices":[{"delta":{"content":"a"},"finish_reason":null}]}"#,
                    "[DONE]",
                ],
                Fault::Malformed("DONE before a recognized finish"),
            ),
            (
                vec![
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0}]},"finish_reason":null}]}"#,
                ],
                Fault::Unsupported("tool or function call channel"),
            ),
            (
                vec![r#"{"choices":[{"message":{"content":"a"},"finish_reason":null}]}"#],
                Fault::Malformed("streamed choice without delta"),
            ),
            (
                vec!["not json"],
                Fault::Malformed("envelope is not a closed JSON object of the profile"),
            ),
            (
                vec![
                    r#"{"id":"one","choices":[{"delta":{"content":"a"},"finish_reason":null}]}"#,
                    r#"{"id":"two","choices":[{"delta":{"content":"b"},"finish_reason":"stop"}]}"#,
                    "[DONE]",
                ],
                Fault::Malformed("response identity changed"),
            ),
            (
                vec![
                    r#"{"choices":[{"delta":{"content":"a"},"message":{"content":"a"},"finish_reason":"stop"}]}"#,
                ],
                Fault::Malformed("message channel in a streamed choice"),
            ),
            (
                vec![
                    r#"{"choices":[{"delta":{"role":"user","content":"a"},"finish_reason":null}]}"#,
                ],
                Fault::Unsupported("non-assistant role"),
            ),
            (
                vec![r#"{"error":{"code":1},"choices":[]}"#],
                Fault::Provider,
            ),
        ] {
            assert_eq!(fault(stream(&frames)).reason(), expected.reason());
        }
        // Empty preamble ids carry no identity; a stable nonempty id is fine.
        assert!(
            stream(&[
                r#"{"id":"","choices":[]}"#,
                r#"{"id":"same","choices":[{"delta":{"content":"a"},"finish_reason":null}]}"#,
                r#"{"id":"same","choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ])
            .is_ok()
        );
        let mut c = Collector::new(64);
        assert!(
            c.feed(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"},\"finish_reason\":null}]}\n"
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn endpoint_admission_and_request_shape() {
        assert!(Endpoint::parse("https://example.invalid/v1/chat/completions", false).is_ok());
        assert!(Endpoint::parse("http://127.0.0.1:8080/v1/chat/completions", true).is_ok());
        assert!(Endpoint::parse("http://[::1]:8080/v1/chat/completions", true).is_ok());
        for (url, local) in [
            ("http://127.0.0.1:8080/v1", false),
            ("http://localhost:8080/v1", true),
            ("http://10.0.0.1:8080/v1", true),
            ("https://user:pw@example.invalid/v1", false),
            ("https://example.invalid/v1?x=1", false),
            ("https://example.invalid/v1#frag", false),
            ("ftp://example.invalid/v1", false),
            ("/v1/chat/completions", true),
        ] {
            assert!(Endpoint::parse(url, local).is_err(), "{url}");
        }
        let messages = vec![Message {
            role: Role::User,
            content: "q".into(),
        }];
        let mut p = Protocol {
            profile: Profile::DeclaredChatCompletionsV1,
            stream: false,
            token_cap: TokenCap {
                field: TokenField::MaxCompletionTokens,
                value: 16,
            },
            temperature_milli: None,
            top_p_milli: None,
            seed: None,
            reasoning_effort: None,
            include_usage: None,
            collection: Collection {
                total_ms: 1,
                idle_ms: 1,
                response_bytes: 1,
                artifact_bytes: 1,
            },
            rendering: Rendering {
                status: RenderingStatus::Unknown,
                template: None,
                tokenizer: None,
            },
        };
        let body = request_body("m", &messages, &p).unwrap();
        assert_eq!(
            body,
            r#"{"model":"m","messages":[{"role":"user","content":"q"}],"stream":false,"max_completion_tokens":16}"#
        );
        assert_eq!(request_size("m", &messages, &p).unwrap(), body.len());
        p.token_cap.field = TokenField::MaxTokens;
        p.stream = true;
        p.temperature_milli = Some(700);
        p.top_p_milli = Some(1000);
        p.seed = Some(-1);
        assert_eq!(
            request_body("m", &messages, &p).unwrap(),
            r#"{"model":"m","messages":[{"role":"user","content":"q"}],"stream":true,"max_tokens":16,"temperature":0.7,"top_p":1.0,"seed":-1}"#
        );
        // Profile v2 controls append in declaration order; a declared-false
        // include_usage is not requested and leaves the wire bytes unchanged.
        p.profile = Profile::DeclaredChatCompletionsV2;
        p.reasoning_effort = Some(Effort::Xhigh);
        p.include_usage = Some(true);
        assert_eq!(
            request_body("m", &messages, &p).unwrap(),
            r#"{"model":"m","messages":[{"role":"user","content":"q"}],"stream":true,"max_tokens":16,"temperature":0.7,"top_p":1.0,"seed":-1,"reasoning_effort":"xhigh","stream_options":{"include_usage":true}}"#
        );
        p.reasoning_effort = None;
        p.include_usage = Some(false);
        assert_eq!(
            request_body("m", &messages, &p).unwrap(),
            r#"{"model":"m","messages":[{"role":"user","content":"q"}],"stream":true,"max_tokens":16,"temperature":0.7,"top_p":1.0,"seed":-1}"#
        );
    }
}
