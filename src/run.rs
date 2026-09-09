// Serial run lifecycle: zero-network preflight, frozen snapshots, durably
// reserved attempts, one active request with cooperative deadlines and
// first-Ctrl-C interruption, explicit terminal receipts, then an initial
// offline grade view recomputed from what was actually persisted.
use crate::contract::*;
use crate::store::{self, RunDir};
use crate::transport::{self, Client, Collector, Endpoint};
use crate::{identity, pack};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::time::sleep_until;

// The first SIGINT is latched here synchronously by the handler itself, so it
// survives blocking local publication and canceled awaits and is checked
// before every dispatch. Tokio's Signal (registered afterwards, chaining this
// handler) only wakes pending network awaits.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

#[cfg(target_os = "linux")]
fn latch_interrupt() -> Result<()> {
    extern "C" fn on_interrupt(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }
    // SAFETY: installs an async-signal-safe handler that only stores an atomic;
    // the action struct is zero-initialized and fully populated before use.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = libc::SA_RESTART;
        if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0 {
            return Err("interrupt latch installation failed".into());
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn latch_interrupt() -> Result<()> {
    Err("the runner's interrupt latch is supported on Linux only".into())
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum CapField {
    #[value(name = "max_completion_tokens")]
    MaxCompletionTokens,
    #[value(name = "max_tokens")]
    MaxTokens,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum ProfileArg {
    DeclaredChatCompletionsV1,
    DeclaredChatCompletionsV2,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum EffortArg {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(clap::Args)]
pub(crate) struct Options {
    /// Inline pack to admit in serial case order.
    pub pack: PathBuf,
    /// Absolute Chat Completions URL; https, or plain http on a loopback IP with --local-http.
    #[arg(long)]
    pub endpoint: String,
    /// Requested model selector sent verbatim.
    #[arg(long)]
    pub model: String,
    /// Fresh run directory; must not exist.
    #[arg(long)]
    pub out: PathBuf,
    /// System label for reports; defaults to the model selector.
    #[arg(long)]
    pub system_name: Option<String>,
    /// Use incremental server-sent events instead of one complete entity.
    #[arg(long)]
    pub stream: bool,
    /// Exact token-cap field spelling sent with --token-cap.
    #[arg(long, value_enum, default_value_t = CapField::MaxCompletionTokens)]
    pub token_cap_field: CapField,
    /// Requested output-token cap (1..=1048576); provider enforcement is unknown.
    #[arg(long)]
    pub token_cap: u32,
    /// Temperature in thousandths (0..=2000); omitted when absent.
    #[arg(long)]
    pub temperature_milli: Option<u16>,
    /// Top-p in thousandths (1..=1000); omitted when absent.
    #[arg(long)]
    pub top_p_milli: Option<u16>,
    /// Seed; omitted when absent.
    #[arg(long)]
    pub seed: Option<i64>,
    /// Declared request profile; v2 adds reasoning_effort and include_usage.
    #[arg(long, value_enum, default_value_t = ProfileArg::DeclaredChatCompletionsV1)]
    pub profile: ProfileArg,
    /// Requested reasoning effort sent verbatim (profile v2); per-model support and effective effort are unverified.
    #[arg(long, value_enum)]
    pub reasoning_effort: Option<EffortArg>,
    /// Request streaming usage via stream_options.include_usage (profile v2, streaming only).
    #[arg(long)]
    pub include_usage: bool,
    /// Total collection-protection deadline per attempt in milliseconds.
    #[arg(long, default_value_t = 30_000)]
    pub total_ms: u32,
    /// Nonempty-body idle deadline per attempt in milliseconds.
    #[arg(long, default_value_t = 10_000)]
    pub idle_ms: u32,
    /// Raw entity byte cap per attempt.
    #[arg(long, default_value_t = RESPONSE_CAP as u32)]
    pub response_bytes: u32,
    /// Final artifact byte cap per attempt.
    #[arg(long, default_value_t = ARTIFACT_CAP as u32)]
    pub artifact_bytes: u32,
    /// Declared (not executed) rendering template identifier.
    #[arg(long, requires = "rendering_tokenizer")]
    pub rendering_template: Option<String>,
    /// Declared (not executed) tokenizer identifier.
    #[arg(long, requires = "rendering_template")]
    pub rendering_tokenizer: Option<String>,
    /// Bearer credential variable; validated at preparation, read again at dispatch, never stored.
    #[arg(long)]
    pub auth_env: Option<String>,
    /// Owner-declared local mode: allow plain http to a loopback IP literal.
    #[arg(long)]
    pub local_http: bool,
}

pub(crate) struct Summary {
    pub interrupted: bool,
    pub paused: bool,
    pub attempts: u32,
    pub view: RunView,
}

// Owned admission results shared by plan and run. Requests remain one-at-a-time;
// preparation counts their exact encoding without retaining every body.
struct Prepared {
    pack: Pack,
    pack_bytes: Vec<u8>,
    inputs: Vec<String>,
    endpoint: Endpoint,
    plan: Plan,
    plan_bytes: Vec<u8>,
    sizes: Vec<CaseSize>,
    out: PathBuf,
}

#[derive(serde::Serialize)]
pub(crate) struct CaseSize {
    pub case_id: String,
    pub prompt_content_bytes: u64,
    pub messages_json_bytes: u64,
    pub request_bytes: u64,
}

#[derive(serde::Serialize)]
pub(crate) struct PlanningReport {
    pub version: u32,
    pub claim: &'static str,
    pub plan: Plan,
    pub protocol_identity: String,
    pub pack_bytes: u64,
    pub plan_bytes: u64,
    pub prompt_content_bytes: u64,
    pub messages_json_bytes: u64,
    pub request_bytes: u64,
    pub cases: Vec<CaseSize>,
    pub requested_output_tokens: u64,
    pub provider_cap_enforcement: &'static str,
    pub payload_bound_status: &'static str,
}

pub(crate) fn plan(o: &Options) -> Result<PlanningReport> {
    let prepared = prepare(o)?;
    Ok(PlanningReport {
        version: 1,
        claim: "offline-preparation-not-dispatched",
        protocol_identity: identity::protocol(&prepared.plan.protocol)?,
        pack_bytes: prepared.pack_bytes.len() as u64,
        plan_bytes: prepared.plan_bytes.len() as u64,
        prompt_content_bytes: prepared.sizes.iter().map(|s| s.prompt_content_bytes).sum(),
        messages_json_bytes: prepared.sizes.iter().map(|s| s.messages_json_bytes).sum(),
        request_bytes: prepared.sizes.iter().map(|s| s.request_bytes).sum(),
        requested_output_tokens: u64::from(prepared.plan.cases)
            .checked_mul(u64::from(prepared.plan.protocol.token_cap.value))
            .ok_or("requested token envelope overflow")?,
        provider_cap_enforcement: "unknown",
        payload_bound_status: "conservative-file-payload-bound-not-reserved-capacity",
        plan: prepared.plan,
        cases: prepared.sizes,
    })
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn protocol(o: &Options) -> Result<Protocol> {
    let protocol = Protocol {
        profile: match o.profile {
            ProfileArg::DeclaredChatCompletionsV1 => Profile::DeclaredChatCompletionsV1,
            ProfileArg::DeclaredChatCompletionsV2 => Profile::DeclaredChatCompletionsV2,
        },
        stream: o.stream,
        token_cap: TokenCap {
            field: match o.token_cap_field {
                CapField::MaxCompletionTokens => TokenField::MaxCompletionTokens,
                CapField::MaxTokens => TokenField::MaxTokens,
            },
            value: o.token_cap,
        },
        temperature_milli: o.temperature_milli,
        top_p_milli: o.top_p_milli,
        seed: o.seed,
        reasoning_effort: o.reasoning_effort.map(|effort| match effort {
            EffortArg::Minimal => Effort::Minimal,
            EffortArg::Low => Effort::Low,
            EffortArg::Medium => Effort::Medium,
            EffortArg::High => Effort::High,
            EffortArg::Xhigh => Effort::Xhigh,
            EffortArg::Max => Effort::Max,
        }),
        // Explicit under v2; None under v1 so a stray flag is rejected by
        // validation instead of silently dropped.
        include_usage: match o.profile {
            ProfileArg::DeclaredChatCompletionsV1 => o.include_usage.then_some(true),
            ProfileArg::DeclaredChatCompletionsV2 => Some(o.include_usage),
        },
        collection: Collection {
            total_ms: o.total_ms,
            idle_ms: o.idle_ms,
            response_bytes: o.response_bytes,
            artifact_bytes: o.artifact_bytes,
        },
        rendering: match (&o.rendering_template, &o.rendering_tokenizer) {
            (Some(template), Some(tokenizer)) => Rendering {
                status: RenderingStatus::Known,
                template: Some(template.clone()),
                tokenizer: Some(tokenizer.clone()),
            },
            _ => Rendering {
                status: RenderingStatus::Unknown,
                template: None,
                tokenizer: None,
            },
        },
    };
    pack::protocol(&protocol)?;
    Ok(protocol)
}

fn bound(pack_bytes: usize, plan_bytes: usize, cases: usize, c: &Collection) -> Result<u64> {
    let per_attempt = (RESERVATION_CAP as u64)
        .checked_add(u64::from(c.response_bytes))
        .and_then(|n| n.checked_add(u64::from(c.artifact_bytes)))
        .and_then(|n| n.checked_add(TERMINAL_CAP as u64))
        .ok_or("run bound overflow")?;
    per_attempt
        .checked_mul(cases as u64)
        .and_then(|n| n.checked_add(pack_bytes as u64))
        .and_then(|n| n.checked_add(plan_bytes as u64))
        .and_then(|n| n.checked_add(RECEIPT_CAP as u64))
        .and_then(|n| n.checked_add(((CASE_CAP + 1) * (RECEIPT_CAP + 3 * 8192)) as u64))
        .ok_or_else(|| "run bound overflow".into())
}

/// The receipt for this pack must fit even when every case yields a terminal
/// and an artifact; refuse before creating anything rather than drop cases.
fn receipt_fits(pack: &Pack, inputs: &[String], identities: &PackIdentities) -> Result<()> {
    let digest = "f".repeat(64);
    let view = RunView {
        version: 1,
        claim: RunViewClaim::ClientCollectedArtifactsNotAuthenticatedExecution,
        identities: RunIdentities {
            pack: PackIdentities {
                source: identities.source.clone(),
                tasks: identities.tasks.clone(),
                target: identities.target.clone(),
                qualification: identities.qualification.clone(),
                grading: identities.grading.clone(),
            },
            plan: digest.clone(),
            protocol: digest.clone(),
            system: digest.clone(),
        },
        cases: pack
            .cases
            .iter()
            .zip(inputs)
            .enumerate()
            .map(|(i, (case, input))| RunCaseGrade {
                case_id: case.id.clone(),
                input: input.clone(),
                attempt: i as u32,
                terminal: Some(digest.clone()),
                reason: Some(Reason::UnsupportedResponse),
                artifact: Some(digest.clone()),
                evidence: RunEvidence::Uncommitted,
                outcome: Outcome::Malformed,
            })
            .collect(),
    };
    store::receipt(&view).map(|_| ())
}

struct Deadline {
    total: Instant,
    idle: Duration,
    last: Instant,
}

impl Deadline {
    fn start(total: Duration, idle: Duration) -> Self {
        let now = Instant::now();
        Self {
            total: now + total,
            idle,
            last: now,
        }
    }
    fn activity(&mut self) {
        self.last = Instant::now();
    }
    /// Next cooperative wake-up and which limit it represents; total wins ties.
    fn next(&self) -> (tokio::time::Instant, Reason) {
        let idle = self.last + self.idle;
        if self.total <= idle {
            (
                tokio::time::Instant::from_std(self.total),
                Reason::TotalDeadline,
            )
        } else {
            (tokio::time::Instant::from_std(idle), Reason::IdleDeadline)
        }
    }
    /// Rechecked after bounded parsing and immediately before commitment so a
    /// result that completes after expiry stays uncommitted; total wins.
    fn expired(&self) -> Option<Reason> {
        let now = Instant::now();
        if now >= self.total {
            Some(Reason::TotalDeadline)
        } else if now >= self.last + self.idle {
            Some(Reason::IdleDeadline)
        } else {
            None
        }
    }
}

/// Serial buffered capture of exact received entity bytes with an incremental
/// hash; owned outside every cancellable future so partial evidence survives.
struct Capture {
    file: BufWriter<File>,
    hash: identity::Hasher,
    bytes: u64,
    cap: u64,
    truncated: bool,
}

impl Capture {
    fn open(path: &Path, cap: u64) -> Result<Self> {
        Ok(Self {
            file: BufWriter::with_capacity(64 * 1024, store::open_new(path)?),
            hash: identity::Hasher::new(),
            bytes: 0,
            cap,
            truncated: false,
        })
    }
    /// Retain at most the cap; returns how many bytes of this chunk were kept.
    fn write(&mut self, chunk: &[u8]) -> Result<usize> {
        let room = usize::try_from(self.cap - self.bytes).unwrap_or(usize::MAX);
        let take = chunk.len().min(room);
        self.file
            .write_all(&chunk[..take])
            .map_err(|e| format!("write body evidence: {e}"))?;
        self.hash.update(&chunk[..take]);
        self.bytes += take as u64;
        self.truncated |= take < chunk.len();
        Ok(take)
    }
    fn finish(self) -> Result<(u64, String, bool)> {
        let mut file = self
            .file
            .into_inner()
            .map_err(|e| format!("flush body evidence: {}", e.error()))?;
        file.flush()
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("sync body evidence: {e}"))?;
        Ok((self.bytes, self.hash.finish(), self.truncated))
    }
}

struct Settle {
    reason: Reason,
    detail: &'static str,
    stop: Option<Finish>,
    artifact: Option<Vec<u8>>,
    usage: Option<Usage>,
    complete: bool,
    terminal_offset: Option<u64>,
    surplus_retained: u64,
    surplus_observed: u64,
}

impl Settle {
    fn plain(reason: Reason, detail: &'static str) -> Self {
        Self {
            reason,
            detail,
            stop: None,
            artifact: None,
            usage: None,
            complete: false,
            terminal_offset: None,
            surplus_retained: 0,
            surplus_observed: 0,
        }
    }
    fn fault(fault: &transport::Fault) -> Self {
        Self::plain(fault.reason(), fault.detail())
    }
    fn completed(settled: transport::Settled) -> Self {
        let (reason, detail, artifact) = match settled.delivery {
            transport::Delivery::Answer(artifact) => (
                Reason::Committed,
                "recognized finish and protocol commitment",
                Some(artifact),
            ),
            transport::Delivery::Refused => (
                Reason::Refused,
                "completed refusal or content filter; observed failed answer delivery",
                None,
            ),
            transport::Delivery::Absent => (
                Reason::UnsupportedResponse,
                "absent final content channel",
                None,
            ),
        };
        Self {
            reason,
            detail,
            stop: Some(settled.stop),
            artifact,
            usage: settled.usage,
            complete: true,
            terminal_offset: None,
            surplus_retained: 0,
            surplus_observed: 0,
        }
    }
    fn commitment(&self) -> Commitment {
        match self.reason {
            Reason::Committed | Reason::Refused => Commitment::Committed,
            _ => Commitment::Uncommitted,
        }
    }
}

// Reqwest yields chunks as `bytes::Bytes`; the generic keeps that crate unnamed
// and the chunk is only borrowed as `&[u8]`, never queued or cloned.
enum Step<B> {
    Interrupted,
    Expired(Reason),
    Chunk(reqwest::Result<Option<B>>),
}

struct Exchange<'a> {
    client: &'a Client,
    stream: bool,
    response_cap: u64,
    artifact_cap: usize,
    interrupt: &'a mut Signal,
    capture: Capture,
    deadline: Deadline,
    headers_ms: Option<u64>,
    first_body_ms: Option<u64>,
    http: Option<Http>,
}

impl Exchange<'_> {
    /// Precedence for every settlement decision after a bounded step: the
    /// latched interrupt, then the frozen deadline order, then the step result.
    fn preempted(&self) -> Option<(Reason, &'static str)> {
        if interrupted() {
            Some((Reason::Interrupted, "first interrupt before commitment"))
        } else {
            self.deadline
                .expired()
                .map(|reason| (reason, "deadline before commitment"))
        }
    }

    async fn collect(&mut self, request: reqwest::Request, sent: Instant) -> Settle {
        let acquired = tokio::select! {
            biased;
            _ = self.interrupt.recv() => {
                Err(Settle::plain(Reason::Interrupted, "first interrupt before response headers"))
            }
            _ = sleep_at(self.deadline.next()) => {
                Err(Settle::plain(self.deadline.next().1, "deadline before response headers"))
            }
            r = self.client.execute(request) => r.map_err(|e| {
                Settle::plain(Reason::TransportFailure, transport::classify(&e))
            }),
        };
        let mut response = match acquired {
            Ok(response) => response,
            Err(settle) => return settle,
        };
        // Headers are not body activity: only nonempty body reads reset idle.
        self.headers_ms = Some(ms(sent.elapsed()));
        let facts = transport::entity(&response);
        let verdict: Option<(Reason, &'static str)> = if facts.status != 200 {
            Some((
                Reason::HttpStatus,
                "non-200 status; entity retained as evidence",
            ))
        } else {
            transport::entity_fault(&response, self.stream).map(|d| (Reason::HttpEntity, d))
        };
        // A complete entity is required for nonstreaming, so a declared length
        // beyond the cap can never commit. A stream may still commit on an
        // in-cap DONE regardless of how long the whole entity would be.
        let declared = facts.content_length;
        self.http = Some(facts);
        if !self.stream && verdict.is_none() && declared.is_some_and(|n| n > self.response_cap) {
            return Settle::plain(
                Reason::ResponseCap,
                "declared content-length exceeds the response cap",
            );
        }
        let mut entity: Vec<u8> = Vec::with_capacity(if self.stream {
            0
        } else {
            usize::try_from(declared.unwrap_or(0).min(self.response_cap)).unwrap_or(0)
        });
        let mut collector = Collector::new(self.artifact_cap);
        // A pre-decided status/entity verdict names the reason for anything
        // short of a local failure; a local failure always wins and stops admission.
        let decided = |fallback: Settle, complete: bool| -> Settle {
            let mut settle = match verdict {
                Some((reason, detail)) => Settle::plain(reason, detail),
                None => fallback,
            };
            settle.complete = complete;
            settle
        };
        loop {
            if let Some((reason, detail)) = self.preempted() {
                return decided(Settle::plain(reason, detail), false);
            }
            let step = tokio::select! {
                biased;
                _ = self.interrupt.recv() => Step::Interrupted,
                _ = sleep_at(self.deadline.next()) => Step::Expired(self.deadline.next().1),
                c = response.chunk() => Step::Chunk(c),
            };
            match step {
                Step::Interrupted => {
                    return decided(
                        Settle::plain(Reason::Interrupted, "first interrupt before commitment"),
                        false,
                    );
                }
                Step::Expired(reason) => {
                    return decided(Settle::plain(reason, "deadline before commitment"), false);
                }
                Step::Chunk(Err(e)) => {
                    return decided(
                        Settle::plain(Reason::TransportFailure, transport::classify(&e)),
                        false,
                    );
                }
                Step::Chunk(Ok(None)) => {
                    if let Some((reason, detail)) = verdict {
                        let mut settle = Settle::plain(reason, detail);
                        settle.complete = true;
                        return settle;
                    }
                    if self.stream {
                        let mut settle = Settle::plain(
                            Reason::StreamIncomplete,
                            "entity ended before a blank-line-terminated DONE",
                        );
                        settle.complete = true;
                        return settle;
                    }
                    if let Some((reason, detail)) = self.preempted() {
                        return Settle::plain(reason, detail);
                    }
                    let parsed = collector.entity(&entity);
                    let mut settle = match (self.preempted(), parsed) {
                        (Some((reason, detail)), _) => Settle::plain(reason, detail),
                        (None, Ok(settled)) => Settle::completed(settled),
                        (None, Err(fault)) => Settle::fault(&fault),
                    };
                    settle.complete = true;
                    return settle;
                }
                Step::Chunk(Ok(Some(chunk))) => {
                    let chunk: &[u8] = &chunk;
                    if chunk.is_empty() {
                        continue;
                    }
                    if self.first_body_ms.is_none() {
                        self.first_body_ms = Some(ms(sent.elapsed()));
                    }
                    self.deadline.activity();
                    let retained = match self.capture.write(chunk) {
                        Ok(retained) => retained,
                        Err(_) => {
                            return Settle::plain(Reason::LocalIo, "local evidence write failed");
                        }
                    };
                    let overflow = retained < chunk.len();
                    if verdict.is_some() || !self.stream {
                        if overflow {
                            return decided(
                                Settle::plain(
                                    Reason::ResponseCap,
                                    "entity exceeds the response cap",
                                ),
                                false,
                            );
                        }
                        if !self.stream {
                            entity.extend_from_slice(chunk);
                        }
                        continue;
                    }
                    // Only retained (in-cap) bytes are parsed; a DONE that lands
                    // inside the cap commits no matter what was co-read after it.
                    let before = self.capture.bytes - retained as u64;
                    let fed = collector.feed(&chunk[..retained]);
                    let settle = match (self.preempted(), fed) {
                        (None, Ok(None)) if !overflow => continue,
                        (Some((reason, detail)), _) => Settle::plain(reason, detail),
                        (None, Ok(None)) => Settle::plain(
                            Reason::ResponseCap,
                            "entity exceeds the response cap before DONE",
                        ),
                        (None, Ok(Some(consumed))) => {
                            let mut settle = Settle::completed(collector.settle());
                            settle.terminal_offset = Some(before + consumed as u64);
                            settle.surplus_retained = (retained - consumed) as u64;
                            settle.surplus_observed = (chunk.len() - consumed) as u64;
                            settle
                        }
                        (None, Err(fault)) => Settle::fault(&fault),
                    };
                    return settle;
                }
            }
        }
    }
}

fn sleep_at((at, _): (tokio::time::Instant, Reason)) -> tokio::time::Sleep {
    sleep_until(at)
}

struct Attempt<'a> {
    dir: PathBuf,
    attempt: u32,
    case: &'a Case,
    input: &'a str,
    started_unix_ms: u64,
    reservation_digest: String,
    request_digest: String,
    reserve_ms: u64,
}

/// Persist the reservation for one attempt. Every recorded field is derived
/// from the frozen plan and case, exactly as the loader later recomputes it.
fn reserve<'a>(
    run: &RunDir,
    attempt: u32,
    case: &'a Case,
    input: &'a str,
    plan: &Plan,
    plan_digest: &str,
) -> Result<(Attempt<'a>, String)> {
    let started = Instant::now();
    let started_unix_ms = unix_ms();
    let body = transport::request_body(&plan.system.model, &case.messages, &plan.protocol)?;
    if body.len() > REQUEST_CAP {
        return Err(format!(
            "request for case {} exceeds the request cap",
            case.id
        ));
    }
    let request_digest = identity::bytes("request", body.as_bytes());
    let reservation = Reservation {
        version: 1,
        attempt,
        case_id: case.id.clone(),
        input: input.to_owned(),
        plan: plan_digest.to_owned(),
        endpoint: plan.system.endpoint.clone().unwrap_or_default(),
        method: "POST".into(),
        headers: transport::headers(plan.protocol.stream),
        auth_env: plan.transport.auth_env.clone(),
        request: request_digest.clone(),
        body,
        started_unix_ms,
    };
    let bytes = serde_json::to_vec_pretty(&reservation)
        .map_err(|e| format!("reservation encoding: {e}"))?;
    if bytes.len() > RESERVATION_CAP {
        return Err("reservation exceeds its byte limit".into());
    }
    let dir = run.attempt(attempt);
    store::create_directory(&dir)?;
    store::sync_directory(&run.attempts)?;
    store::write_new(&dir.join("reservation.json"), &bytes)?;
    store::sync_directory(&dir)?;
    Ok((
        Attempt {
            dir,
            attempt,
            case,
            input,
            started_unix_ms,
            reservation_digest: identity::bytes("reservation-source", &bytes),
            request_digest,
            reserve_ms: ms(started.elapsed()),
        },
        reservation.body,
    ))
}

fn finalize(
    a: &Attempt<'_>,
    run: &RunDir,
    exchange: Exchange<'_>,
    settle: Settle,
    dispatched: bool,
    settle_ms: u64,
) -> Result<Terminal> {
    let settled_at = Instant::now();
    let (bytes, sha256, truncated) = exchange.capture.finish()?;
    store::publish_link(&a.dir.join("body.partial"), &a.dir.join("body.bin"))?;
    let final_artifact = match &settle.artifact {
        Some(artifact) => {
            store::write_new(&a.dir.join("final.txt"), artifact)?;
            Some(Artifact {
                bytes: artifact.len() as u64,
                sha256: identity::sha256(artifact),
            })
        }
        None => None,
    };
    store::sync_directory(&a.dir)?;
    let terminal = Terminal {
        version: 2,
        attempt: a.attempt,
        case_id: a.case.id.clone(),
        input: a.input.to_owned(),
        reservation: a.reservation_digest.clone(),
        request: a.request_digest.clone(),
        dispatched,
        reason: settle.reason,
        detail: settle.detail.to_owned(),
        commitment: settle.commitment(),
        stop: settle.stop,
        http: exchange.http,
        body: Body {
            bytes,
            sha256,
            complete: settle.complete,
            truncated,
            terminal_offset: settle.terminal_offset,
            surplus_retained: settle.surplus_retained,
            surplus_observed: settle.surplus_observed,
        },
        final_artifact,
        usage: settle.usage,
        timing: Timing {
            started_unix_ms: a.started_unix_ms,
            reserve_ms: a.reserve_ms,
            headers_ms: exchange.headers_ms,
            first_body_ms: exchange.first_body_ms,
            settle_ms,
            local_ms: ms(settled_at.elapsed()),
        },
    };
    let receipt =
        serde_json::to_vec_pretty(&terminal).map_err(|e| format!("terminal encoding: {e}"))?;
    if receipt.len() > TERMINAL_CAP {
        return Err("terminal receipt exceeds its byte limit".into());
    }
    store::publish_receipt(&a.dir, "terminal.json", &receipt)?;
    store::sync_directory(&run.attempts)?;
    Ok(terminal)
}

fn prepare(o: &Options) -> Result<Prepared> {
    // Offline admission and preflight: no client, runtime, socket or output.
    let pack_bytes = store::read(&o.pack, PACK_CAP)?;
    let pack = pack::admit(&pack_bytes)?;
    let (identities, inputs) = identity::pack(&pack, &pack_bytes)?;
    let endpoint = Endpoint::parse(&o.endpoint, o.local_http)?;
    let system = System {
        name: o.system_name.clone().unwrap_or_else(|| o.model.clone()),
        model: o.model.clone(),
        endpoint: Some(endpoint.text().to_owned()),
    };
    pack::system(&system)?;
    let protocol = protocol(o)?;
    if let Some(name) = &o.auth_env {
        transport::credential(name)?;
    }
    let mut sizes = Vec::with_capacity(pack.cases.len());
    for case in &pack.cases {
        let request_bytes = transport::request_size(&o.model, &case.messages, &protocol)?;
        if request_bytes > REQUEST_CAP {
            return Err(format!(
                "request for case {} exceeds the request cap",
                case.id
            ));
        }
        sizes.push(CaseSize {
            case_id: case.id.clone(),
            prompt_content_bytes: case.messages.iter().map(|m| m.content.len() as u64).sum(),
            messages_json_bytes: transport::messages_size(&case.messages)? as u64,
            request_bytes: request_bytes as u64,
        });
    }
    receipt_fits(&pack, &inputs, &identities)?;
    let mut plan = Plan {
        version: 1,
        claim: PlanClaim::ClientCollectionIntentNotAuthenticatedExecution,
        pack: identities,
        cases: pack.cases.len() as u32,
        system,
        protocol,
        transport: transport::plan_transport(endpoint.local, o.stream, o.auth_env.clone()),
        provenance: Provenance {
            grill: env!("CARGO_PKG_VERSION").into(),
            lockfile: identity::bytes("lockfile", include_bytes!("../Cargo.lock")),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            started_unix_ms: unix_ms(),
            pid: std::process::id(),
        },
        bound_bytes: 0,
    };
    plan.bound_bytes = bound(
        pack_bytes.len(),
        PLAN_CAP,
        pack.cases.len(),
        &plan.protocol.collection,
    )?;
    let plan_bytes = serde_json::to_vec_pretty(&plan).map_err(|e| format!("plan encoding: {e}"))?;
    if plan_bytes.len() > PLAN_CAP {
        return Err("plan exceeds its byte limit".into());
    }
    store::fresh_destination(&o.out)?;
    Ok(Prepared {
        pack,
        pack_bytes,
        inputs,
        endpoint,
        plan,
        plan_bytes,
        sizes,
        out: o.out.clone(),
    })
}

pub(crate) fn run(o: &Options) -> Result<Summary> {
    let prepared = prepare(o)?;
    let run = store::create_run(&prepared.out, &prepared.pack_bytes, &prepared.plan_bytes)?;
    let owner = crate::lifecycle::Lock::run(&run.root)?;
    let loaded = store::load(&run.root)?;
    let session = crate::lifecycle::Session::create(&run.root, &loaded, true, unix_ms())?;
    collect(prepared, run, owner, session, 0)
}

pub(crate) fn resume(root: &Path) -> Result<Summary> {
    let owner = crate::lifecycle::Lock::run(root)?;
    let loaded = store::load(root)?;
    let next = crate::lifecycle::resumable(root, &loaded)?;
    if next == loaded.attempts.len() {
        return Ok(Summary {
            interrupted: false,
            paused: false,
            attempts: 0,
            view: store::run_view(&loaded)?,
        });
    }
    let endpoint = Endpoint::parse(
        loaded
            .plan
            .system
            .endpoint
            .as_deref()
            .ok_or("missing endpoint")?,
        true,
    )?;
    if let Some(name) = &loaded.plan.transport.auth_env {
        transport::credential(name)?;
    }
    let session = crate::lifecycle::Session::create(root, &loaded, false, unix_ms())?;
    let prepared = Prepared {
        pack: loaded.pack,
        pack_bytes: loaded.pack_bytes,
        inputs: loaded.inputs,
        endpoint,
        plan: loaded.plan,
        plan_bytes: loaded.plan_bytes,
        sizes: Vec::new(),
        out: root.to_path_buf(),
    };
    let run = RunDir {
        root: root.to_path_buf(),
        attempts: root.join("attempts"),
    };
    collect(prepared, run, owner, session, next)
}

fn collect(
    prepared: Prepared,
    run: RunDir,
    _owner: crate::lifecycle::Lock,
    session: crate::lifecycle::Session,
    next: usize,
) -> Result<Summary> {
    let Prepared {
        pack,
        inputs,
        endpoint,
        plan,
        plan_bytes,
        ..
    } = prepared;
    let plan_digest = identity::bytes("plan-source", &plan_bytes);

    // Latch before any dispatch; tokio's Signal registers afterwards and chains it.
    latch_interrupt()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    let collection = &plan.protocol.collection;
    let total = Duration::from_millis(u64::from(collection.total_ms));
    let idle = Duration::from_millis(u64::from(collection.idle_ms));
    let (attempts, local_failure, paused) = runtime.block_on(async {
        let client = Client::new(&endpoint, plan.protocol.stream)?;
        let mut interrupt =
            signal(SignalKind::interrupt()).map_err(|e| format!("interrupt handler: {e}"))?;
        let mut attempts = 0u32;
        let mut local_failure = false;
        let mut paused = false;
        for (i, (case, input)) in pack.cases.iter().zip(&inputs).enumerate().skip(next) {
            // Checked synchronously before every reservation: an interrupt latched
            // during the previous attempt's publication stops admission here.
            if interrupted() {
                break;
            }
            let (admission, requested) = session.admission()?;
            if requested {
                paused = true;
                break;
            }
            let attempt = i as u32;
            let (a, body) = reserve(&run, attempt, case, input, &plan, &plan_digest)?;
            drop(admission);
            let mut exchange = Exchange {
                client: &client,
                stream: plan.protocol.stream,
                response_cap: u64::from(collection.response_bytes),
                artifact_cap: collection.artifact_bytes as usize,
                interrupt: &mut interrupt,
                capture: Capture::open(
                    &a.dir.join("body.partial"),
                    u64::from(collection.response_bytes),
                )?,
                deadline: Deadline::start(total, idle),
                headers_ms: None,
                first_body_ms: None,
                http: None,
            };
            // Budgets began with the deadline inside the exchange, after the durable
            // reservation and immediately before dispatch. An interrupt latched
            // during the reservation's synchronous publication stops here: the
            // reservation stands as honest "reserved, never dispatched" evidence.
            let sent = Instant::now();
            let (settle, dispatched) = if interrupted() {
                (
                    Settle::plain(Reason::Interrupted, "first interrupt before dispatch"),
                    false,
                )
            } else {
                match client.request(body, plan.transport.auth_env.as_deref()) {
                    Ok(request) => (exchange.collect(request, sent).await, true),
                    Err(_) => (
                        Settle::plain(Reason::TransportFailure, "request construction failed"),
                        false,
                    ),
                }
            };
            let settle_ms = ms(sent.elapsed());
            let reason = settle.reason;
            finalize(&a, &run, exchange, settle, dispatched, settle_ms)?;
            attempts += 1;
            if reason == Reason::LocalIo {
                // Local capture failed: never start more cases on failing storage.
                local_failure = true;
                break;
            }
        }
        Ok::<_, String>((attempts, local_failure, paused))
    })?;
    drop(runtime);
    // The initial view is recomputed from persisted evidence by the same offline
    // path regrade uses, so it can only describe what actually reached disk.
    let loaded = store::load(&run.root)?;
    let view = store::run_view(&loaded)?;
    if session.index == 0 {
        store::publish_initial_view(&run, &store::receipt(&view)?)?;
    }
    session.finish(
        &loaded,
        if interrupted() {
            "interrupted"
        } else if local_failure {
            "blocked"
        } else if paused {
            "paused"
        } else {
            "completed"
        },
    )?;
    if local_failure {
        return Err(format!(
            "local evidence capture failed after {attempts} attempts; admission stopped and the initial view was written"
        ));
    }
    Ok(Summary {
        interrupted: interrupted(),
        paused,
        attempts,
        view,
    })
}
