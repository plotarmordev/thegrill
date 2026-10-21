//! OpenAI-style speech servers: serial requests against a server the operator already runs.
use super::*;
use crate::wire;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_TYPE, USER_AGENT};

const BODY: &str = "body.bin";
const FINISH_REASON: &str = "x-finish-reason";
/// The OpenAI-style speech `input` limit, counted in characters.
const INPUT_CHARS: usize = 4096;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Format {
    Wav,
    Pcm,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::media) struct Server {
    version: u32,
    id: String,
    endpoint: String,
    model: String,
    #[serde(default, deserialize_with = "present")]
    voice: Option<String>,
    response_format: Format,
    pub(in crate::media) stream: bool,
    #[serde(default, deserialize_with = "present")]
    pcm: Option<Pcm>,
}

/// Raw reply framing; the sample rate is the workload request's.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pcm {
    channels: u16,
    sample_bytes: u16,
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    input: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<&'a str>,
    response_format: Format,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Reservation {
    cell: String,
    phase: Phase,
    seed: u64,
    body: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reply {
    pub(super) end_to_end_us: u64,
    pub(super) first_audio_us: Option<u64>,
    http_status: Option<u16>,
    headers: Vec<[String; 2]>,
    /// `X-Finish-Reason` values; null when the server sent none, so the reason is unknown.
    pub(super) finish_reason: Option<String>,
    pub(super) body: Stream,
    /// Deadline, transport or cancellation outcome that ended the exchange early.
    failure: Option<Reason>,
    pub(super) audio: Option<crate::wavfile::Checked>,
    wav_failure: Option<crate::wavfile::Failure>,
}
impl Reply {
    pub(super) fn cancelled(&self) -> bool {
        matches!(
            self.failure,
            Some(Reason::Cancelled | Reason::CancelledBeforeLaunch)
        )
    }
}

struct Judged {
    finish_reason: Option<String>,
    audio: Option<crate::wavfile::Checked>,
    wav_failure: Option<crate::wavfile::Failure>,
    reasons: Vec<Reason>,
}

fn sample_rate(request: &Request) -> u32 {
    request
        .sample_rate
        .expect("speech requests require sample_rate")
}

impl Server {
    pub(in crate::media) fn validate(&self, w: &Workload) -> Result<()> {
        if self.version != 1 {
            return Err("runner.version must be 1".into());
        }
        if !identifier(&self.id) {
            return Err(format!("runner.id is invalid: {}", self.id));
        }
        self.url()?;
        for (field, value) in [
            ("model", Some(self.model.as_str())),
            ("voice", self.voice.as_deref()),
        ] {
            if value.is_some_and(|v| v.is_empty() || v.chars().any(char::is_control)) {
                return Err(format!("runner.{field} must be nonempty and control-free"));
            }
        }
        if self.stream && self.response_format != Format::Pcm {
            return Err("runner.stream requires response_format pcm".into());
        }
        match (self.response_format, self.pcm) {
            (Format::Pcm, Some(pcm)) if pcm.channels > 0 && pcm.sample_bytes > 0 => {}
            (Format::Wav, None) => {}
            _ => {
                return Err(
                    "runner.pcm declares positive channels and sample_bytes exactly for response_format pcm"
                        .into(),
                );
            }
        }
        if !w.outputs.is_empty() {
            return Err("outputs must be empty: the reply body is a speech server's output".into());
        }
        for (context, request) in
            std::iter::once(("request".to_string(), &w.request)).chain(w.cells.iter().filter_map(
                |cell| Some((format!("cell {}.request", cell.id), cell.request.as_ref()?)),
            ))
        {
            if request.negative_prompt.is_some() || request.guidance_milli.is_some() {
                return Err(format!(
                    "{context}: speech server requests accept only prompt, seeds and sample_rate"
                ));
            }
            if request.prompt.chars().count() > INPUT_CHARS {
                return Err(format!(
                    "{context}.prompt must be at most {INPUT_CHARS} characters for a speech server"
                ));
            }
        }
        Ok(())
    }

    pub(in crate::media) fn url(&self) -> Result<reqwest::Url> {
        wire::endpoint(&self.endpoint, true).map_err(|_| {
            "runner.endpoint must be a control-free URL of at most 4096 bytes with a host and no credentials, query or fragment, using HTTPS or plain HTTP to a literal loopback address".into()
        })
    }

    pub(in crate::media) fn body(&self, request: &Request) -> Result<String> {
        serde_json::to_string(&Body {
            model: &self.model,
            input: &request.prompt,
            voice: self.voice.as_deref(),
            response_format: self.response_format,
            stream: self.stream,
        })
        .map_err(|error| error.to_string())
    }

    pub(super) fn reservation(&self, sample: &Scheduled<'_>) -> Result<Reservation> {
        Ok(Reservation {
            cell: sample.cell.id.clone(),
            phase: sample.phase,
            seed: sample.seed,
            body: self.body(sample.request)?,
        })
    }

    /// Derives audio and eligibility from retained facts only, so inspect repeats it offline.
    fn judge(&self, reply: &Reply, body: &[u8], sample_rate: u32, deadline_ms: u32) -> Judged {
        let values = reply
            .headers
            .iter()
            .filter(|[name, _]| name == FINISH_REASON)
            .map(|[_, value]| value.as_str())
            .collect::<Vec<_>>();
        let mut judged = Judged {
            finish_reason: (!values.is_empty()).then(|| values.join(", ")),
            audio: None,
            wav_failure: None,
            reasons: reply.failure.into_iter().collect(),
        };
        let reasons = &mut judged.reasons;
        if reply.end_to_end_us >= u64::from(deadline_ms) * 1000 {
            reasons.push(Reason::DeadlineExceeded);
        }
        if reply.body.truncated {
            reasons.push(Reason::OutputOversized);
        }
        if reply.http_status.is_some_and(|status| status != 200) {
            reasons.push(Reason::HttpStatus);
        }
        if values.iter().any(|value| *value != "stop") {
            reasons.push(Reason::FinishNotStop);
        }
        if reply.failure.is_none() && !reply.body.truncated && reply.http_status == Some(200) {
            let audio = match self.pcm {
                None => crate::wavfile::check(body).map_err(|failure| {
                    judged.wav_failure = Some(failure);
                    Reason::OutputInvalid
                }),
                Some(pcm) => pcm_audio(body, pcm, sample_rate),
            };
            match audio {
                Ok(audio) => {
                    if audio.sample_rate != sample_rate {
                        reasons.push(Reason::SampleRateMismatch);
                    }
                    if audio.duration_us == 0 {
                        reasons.push(Reason::OutputMissing);
                    }
                    judged.audio = Some(audio);
                }
                Err(reason) => reasons.push(reason),
            }
        }
        reasons.sort();
        reasons.dedup();
        judged
    }

    pub(super) fn verify(
        &self,
        dir: &Path,
        run: &Run,
        sample: &Scheduled<'_>,
        limits: &Limits,
    ) -> Result<()> {
        let reply = run
            .reply
            .as_ref()
            .filter(|_| {
                run.process.is_none()
                    && run.markers.is_none()
                    && run.outputs.is_empty()
                    && run.cache_after.is_empty()
            })
            .ok_or("run record does not match its runner kind")?;
        let cap =
            usize::try_from(limits.output_bytes).map_err(|_| "output cap exceeds platform")?;
        let body = evidence::read(&dir.join(BODY), cap)?;
        if body.len() != reply.body.bytes || evidence::digest(&body) != reply.body.sha256 {
            return Err("retained reply body hash mismatch".into());
        }
        let streamed_audio = self.stream && reply.http_status == Some(200) && !body.is_empty();
        if !matches!(
            reply.failure,
            None | Some(
                Reason::DeadlineExceeded
                    | Reason::TransportFailed
                    | Reason::Cancelled
                    | Reason::CancelledBeforeLaunch
            )
        ) || (reply.failure.is_none() && reply.http_status.is_none())
            || reply.first_audio_us.is_some() != streamed_audio
            || reply
                .first_audio_us
                .is_some_and(|us| us > reply.end_to_end_us)
            || (reply.body.truncated && body.len() != cap)
        {
            return Err("reply record mismatch".into());
        }
        let judged = self.judge(
            reply,
            &body,
            sample_rate(sample.request),
            limits.run_deadline_ms,
        );
        if reply.finish_reason != judged.finish_reason
            || reply.audio != judged.audio
            || reply.wav_failure != judged.wav_failure
            || run.reasons != judged.reasons
            || run.eligible != run.reasons.is_empty()
        {
            return Err("reply eligibility mismatch".into());
        }
        Ok(())
    }
}

fn pcm_audio(
    body: &[u8],
    pcm: Pcm,
    sample_rate: u32,
) -> std::result::Result<crate::wavfile::Checked, Reason> {
    let frame = usize::from(pcm.channels) * usize::from(pcm.sample_bytes);
    if !body.len().is_multiple_of(frame) {
        return Err(Reason::PartialFrame);
    }
    // Bodies are bounded by memory, so whole frames times 10^6 fit in u64.
    let frames = (body.len() / frame) as u64;
    Ok(crate::wavfile::Checked {
        channels: pcm.channels,
        sample_rate,
        duration_us: frames * 1_000_000 / u64::from(sample_rate),
    })
}

#[derive(Default)]
struct Exchanged {
    end_to_end_us: u64,
    first_audio_us: Option<u64>,
    http_status: Option<u16>,
    headers: Vec<[String; 2]>,
    body: Vec<u8>,
    truncated: bool,
    failure: Option<Reason>,
    detail: Option<String>,
}

async fn exchange(
    client: &reqwest::Client,
    request: reqwest::Request,
    deadline: Duration,
    cap: usize,
    stream: bool,
) -> Exchanged {
    let mut exchanged = Exchanged::default();
    let cancelled = async {
        while !process::program_signals::cancelled() {
            tokio::time::sleep(process::PROGRAM_POLL).await;
        }
    };
    tokio::pin!(cancelled);
    let start = Instant::now();
    let expired = tokio::time::sleep_until(tokio::time::Instant::from_std(start + deadline));
    tokio::pin!(expired);
    let mut response = tokio::select! {
        biased;
        response = client.execute(request) => match response {
            Ok(response) => response,
            Err(error) => {
                exchanged.end_to_end_us = wire::us(start);
                exchanged.failure = Some(Reason::TransportFailed);
                exchanged.detail = Some(wire::error_kind(&error).into());
                return exchanged;
            }
        },
        () = &mut expired => {
            exchanged.end_to_end_us = wire::us(start);
            exchanged.failure = Some(Reason::DeadlineExceeded);
            return exchanged;
        }
        () = &mut cancelled => {
            exchanged.end_to_end_us = wire::us(start);
            exchanged.failure = Some(Reason::Cancelled);
            return exchanged;
        }
    };
    exchanged.http_status = Some(response.status().as_u16());
    let audio = stream && exchanged.http_status == Some(200);
    // Keep received chunks as they arrive; copying waits until timing ends.
    let mut chunks = Vec::new();
    let mut bytes = 0;
    loop {
        let chunk = tokio::select! {
            biased;
            chunk = response.chunk() => chunk,
            () = &mut expired => {
                exchanged.failure = Some(Reason::DeadlineExceeded);
                break;
            }
            () = &mut cancelled => {
                exchanged.failure = Some(Reason::Cancelled);
                break;
            }
        };
        match chunk {
            Ok(Some(chunk)) => {
                if audio && exchanged.first_audio_us.is_none() && !chunk.is_empty() {
                    exchanged.first_audio_us = Some(wire::us(start));
                }
                if chunk.len() > cap - bytes {
                    chunks.push(chunk.slice(..cap - bytes));
                    exchanged.truncated = true;
                    break;
                }
                bytes += chunk.len();
                chunks.push(chunk);
            }
            Ok(None) => break,
            Err(error) => {
                exchanged.failure = Some(Reason::TransportFailed);
                exchanged.detail = Some(wire::error_kind(&error).into());
                break;
            }
        }
    }
    exchanged.end_to_end_us = wire::us(start);
    exchanged.headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            [
                name.as_str().into(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            ]
        })
        .collect();
    exchanged.body = chunks.concat();
    exchanged
}

pub(super) struct Session<'a> {
    server: &'a Server,
    workload: &'a Workload,
    receipt_sha256: &'a str,
    url: reqwest::Url,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}
impl<'a> Session<'a> {
    pub(super) fn new(
        server: &'a Server,
        workload: &'a Workload,
        receipt_sha256: &'a str,
    ) -> Result<Self> {
        let url = server.url()?;
        Ok(Self {
            client: wire::client(url.scheme() == "http", 1)?,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("runtime initialization failed: {error}"))?,
            url,
            server,
            workload,
            receipt_sha256,
        })
    }

    pub(super) fn sample(&self, root: &Path, sample: &Scheduled<'_>) -> Result<Run> {
        let dir = root.join(sample.directory());
        evidence::fresh(&dir)?;
        let reservation = self.server.reservation(sample)?;
        evidence::publish(&dir, "reservation.json", &reservation)?;
        let limits = &self.workload.limits;
        let cap = usize::try_from(limits.output_bytes)
            .map_err(|_| "output_bytes exceeds platform size")?;
        let request = self
            .client
            .post(self.url.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT_ENCODING, "identity")
            .header(
                USER_AGENT,
                concat!("grill-perf/", env!("CARGO_PKG_VERSION")),
            )
            .body(reservation.body)
            .build()
            .map_err(|_| "request construction failed")?;
        let exchanged = if process::program_signals::cancelled() {
            Exchanged {
                failure: Some(Reason::CancelledBeforeLaunch),
                ..Exchanged::default()
            }
        } else {
            self.runtime.block_on(exchange(
                &self.client,
                request,
                Duration::from_millis(u64::from(limits.run_deadline_ms)),
                cap,
                self.server.stream,
            ))
        };
        evidence::write(&dir.join(BODY), &exchanged.body)?;
        let mut reply = Reply {
            end_to_end_us: exchanged.end_to_end_us,
            first_audio_us: exchanged.first_audio_us,
            http_status: exchanged.http_status,
            headers: exchanged.headers,
            finish_reason: None,
            body: Stream {
                sha256: evidence::digest(&exchanged.body),
                bytes: exchanged.body.len(),
                truncated: exchanged.truncated,
            },
            failure: exchanged.failure,
            audio: None,
            wav_failure: None,
        };
        let judged = self.server.judge(
            &reply,
            &exchanged.body,
            sample_rate(sample.request),
            limits.run_deadline_ms,
        );
        reply.finish_reason = judged.finish_reason;
        reply.audio = judged.audio;
        reply.wav_failure = judged.wav_failure;
        let run = Run {
            receipt_sha256: self.receipt_sha256.into(),
            cell: sample.cell.id.clone(),
            phase: sample.phase,
            seed: sample.seed,
            process: None,
            reply: Some(reply),
            markers: None,
            eligible: judged.reasons.is_empty(),
            outputs: Vec::new(),
            reasons: judged.reasons,
            details: exchanged.detail.into_iter().collect(),
            cache_after: Vec::new(),
            cache_mutated_during_measurement: Some(false),
        };
        evidence::publish(&dir, "run.json", &run)?;
        Ok(run)
    }
}
