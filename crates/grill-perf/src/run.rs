use crate::{evidence, lifecycle, metrics, model::*, wire};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio::task::JoinSet;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn latch(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}
static LATCH_INSTALLATION: LazyLock<Result<()>> = LazyLock::new(|| {
    // Later acquisitions must not replace Tokio's process-wide chained handler.
    for signal in [libc::SIGINT, libc::SIGTERM] {
        if unsafe { libc::signal(signal, latch as *const () as libc::sighandler_t) }
            == libc::SIG_ERR
        {
            return Err("cannot install interrupt latch".into());
        }
    }
    Ok(())
});
fn install_latch() -> Result<()> {
    LATCH_INSTALLATION.clone()
}
fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}
#[derive(clap::Args)]
pub struct Options {
    pub workload: PathBuf,
    #[arg(long)]
    pub endpoint: String,
    #[arg(long)]
    pub model: String,
    #[arg(long)]
    pub out: PathBuf,
    /// Optional JSON declarations of model revision, runtime, hardware and settings.
    #[arg(long)]
    pub deployment: Option<PathBuf>,
    /// Capture an exact observed-envelope declaration before dispatch.
    #[arg(long)]
    pub policy: Option<PathBuf>,
    /// Optional bounded server-wide diagnostics; can perturb between-wave cadence.
    #[arg(long)]
    pub metrics_url: Option<String>,
    #[arg(long)]
    pub auth_env: Option<String>,
    #[arg(long)]
    pub local_http: bool,
    #[arg(long)]
    pub json: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub version: u32,
    pub session: usize,
    pub first_wave: usize,
    pub status: String,
    pub planned_waves: usize,
    pub published_waves: usize,
    pub measured_waves: usize,
    pub eligible_measured_waves: usize,
    pub wave_publication_us: u64,
    pub wave_preparation_us: u64,
    pub reservation_publication_us: u64,
    pub ineligible_warmup_waves: usize,
    pub claim: String,
}
pub fn execute(o: &Options) -> Result<Summary> {
    execute_inner(o, None)
}

pub fn execute_bounded(o: &Options, deadline: Instant) -> Result<Summary> {
    if o.metrics_url.is_some() {
        return Err("metrics diagnostics are unsupported in bounded capture; use the advanced unbounded run path".into());
    }
    execute_inner(o, Some(deadline))
}

fn execute_inner(o: &Options, deadline: Option<Instant>) -> Result<Summary> {
    let source = evidence::read(&o.workload, FILE_CAP)?;
    let workload: Workload =
        serde_json::from_slice(&source).map_err(|e| format!("invalid workload: {e}"))?;
    workload.validate()?;
    if workload.version == 2 && o.policy.is_some() {
        return Err("conversation steps have descriptive per-step semantics, not an advanced policy contract".into());
    }
    if o.model.is_empty() || o.model.len() > 4096 || o.model.chars().any(char::is_control) {
        return Err("model must be a nonempty selector within 4096 bytes".into());
    }
    let url = wire::endpoint(&o.endpoint, o.local_http)?;
    let deployment: Option<Deployment> = o
        .deployment
        .as_ref()
        .map(|path| {
            let bytes = evidence::read(path, 32 * 1024)?;
            let value: Deployment = serde_json::from_slice(&bytes)
                .map_err(|e| format!("invalid deployment declaration: {e}"))?;
            value.validate()?;
            Ok::<_, String>(value)
        })
        .transpose()?;
    wire::credential(o.auth_env.as_deref())?;
    let pool = workload
        .cells
        .iter()
        .map(|c| c.concurrency as usize)
        .max()
        .unwrap_or(1);
    let waves = workload.waves();
    let metrics = o
        .metrics_url
        .as_ref()
        .map(|endpoint| {
            wire::endpoint(endpoint, o.local_http)
                .map(|url| metrics::Config::new(url.to_string(), waves.len()))
        })
        .transpose()?;
    let normalized = serde_json::to_vec(&workload).map_err(|e| e.to_string())?;
    let cache_namespace = if workload.request.cache == Cache::Observe && !workload.salted() {
        None
    } else {
        let mut random = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut random))
            .map_err(|_| "cannot obtain cache-salt entropy")?;
        Some(evidence::digest(&random))
    };
    let cache_mechanism = (workload.request.profile != Profile::PortableChatV1)
        .then(|| "declared-vllm-prefix-cache".into());
    let policy_bytes = o
        .policy
        .as_ref()
        .map(|path| evidence::read(path, crate::policy::CAP))
        .transpose()?;
    let plan = Plan {
        version: 3,
        metric_contract: Some(METRIC_CONTRACT.into()),
        kind: "performance-run-v1".into(),
        tool_version: env!("CARGO_PKG_VERSION").into(),
        collector_sha256: evidence::binary_digest()?,
        workload,
        workload_sha256: evidence::digest(&normalized),
        source_sha256: evidence::digest(&source),
        model: o.model.clone(),
        deployment,
        cache_mechanism,
        cache_evidence_source: "provider-reported:usage.prompt_tokens_details.cached_tokens".into(),
        endpoint: url.to_string(),
        auth_env: o.auth_env.clone(),
        local_http: o.local_http,
        pool_max_idle_per_host: pool,
        started_unix_ms: unix_ms(),
        cache_namespace,
        waves,
        metrics,
        policy_sha256: policy_bytes.as_deref().map(evidence::digest),
    };
    if plan.workload.version == 2 {
        for (case, spec) in plan.workload.cases.iter().zip(&plan.waves) {
            if case.step.as_ref().is_some_and(|step| step.parent.is_none()) {
                let mut root = spec.clone();
                root.index = 0;
                crate::sequence::State::default().request(&plan, &root, 0)?;
            }
        }
    }
    if let Some(bytes) = &policy_bytes {
        crate::policy::parse(bytes, &plan).map_err(|e| e.as_str().to_owned())?;
    }
    // Seed magnitude peaks at a corner of the trial/lane range and index 1023 is
    // the widest index, but lane 0 renders one digit narrower than lanes 10..63 in
    // each of the cache salt and the text salt, so an interior body can exceed a
    // corner sample by two bytes. Bound with that slack rather than serializing
    // every repeated prompt.
    for cell in &plan.workload.cells {
        for (trial, lane) in [(0, 0), (100, 63)] {
            let bound = WaveSpec {
                index: 1023,
                phase: Phase::Measured,
                cell: cell.id.clone(),
                case: cell.case.clone(),
                trial,
                concurrency: cell.concurrency,
            };
            let body = wire::request_body(&plan, &bound, lane)?;
            if body.len() + 2 > REQUEST_CAP {
                return Err("encoded request exceeds 2 MiB".into());
            }
            // Reservation receipts embed each body as a JSON string; the 40 MiB
            // loader cap must hold after that second escaping plus pretty-print.
            let escaped = serde_json::to_string(&body).map_err(|e| e.to_string())?;
            if (escaped.len() + 2) * cell.concurrency as usize > 32 * 1024 * 1024 {
                return Err(format!(
                    "cell {} exceeds the reservation receipt bound",
                    cell.id
                ));
            }
        }
    }
    evidence::fresh(&o.out)?;
    let _owner = lifecycle::ownership(&o.out)?;
    evidence::write(&o.out.join("workload.json"), &source)?;
    if let Some(bytes) = &policy_bytes {
        evidence::write(&o.out.join("policy.json"), bytes)?;
    }
    let plan_bytes = evidence::json(&o.out.join("plan.json"), &plan)?;
    evidence::sync(&o.out)?;
    let plan_hash = evidence::digest(&plan_bytes);
    collect(
        &o.out,
        &plan,
        &plan_hash,
        (0, 0),
        o.json,
        metrics::Budget::default(),
        deadline,
    )
}

pub fn resume(root: &std::path::Path, json: bool) -> Result<Summary> {
    let _owner = lifecycle::ownership(root)?;
    let mut loaded = evidence::load(root)?;
    if loaded.plan.workload.version == 2 {
        return Err("bounded conversation sequences cannot resume; retain the partial sequence and start a new explicitly budgeted capture".into());
    }
    if !matches!(loaded.plan.version, 2 | 3) {
        return Err("legacy runs cannot be resumed; no execution-session provenance".into());
    }
    if loaded.plan.collector_sha256 != evidence::binary_digest()?
        || loaded.plan.tool_version != env!("CARGO_PKG_VERSION")
    {
        return Err("continuation requires the original collector binary and version".into());
    }
    let metrics_budget = loaded
        .metrics
        .take()
        .map_or(metrics::Budget::default(), |m| m.budget);
    let history = &loaded.history;
    if history.open || history.last_status.as_deref() != Some("paused") {
        return Err("continuation requires a settled cooperative pause; interrupted, failed or reserved/unsettled work is not replayed".into());
    }
    if history.next_wave == loaded.plan.waves.len() {
        return Err("run is already complete; no never-started waves remain".into());
    }
    let plan_hash = evidence::digest(&evidence::read(&root.join("plan.json"), 8 * 1024 * 1024)?);
    collect(
        root,
        &loaded.plan,
        &plan_hash,
        (history.count, history.next_wave),
        json,
        metrics_budget,
        None,
    )
}

fn collect(
    root: &std::path::Path,
    plan: &Plan,
    plan_hash: &str,
    cursor: (usize, usize),
    json: bool,
    mut metrics_budget: metrics::Budget,
    deadline: Option<Instant>,
) -> Result<Summary> {
    let (session, first_wave) = cursor;
    let url = wire::endpoint(&plan.endpoint, plan.local_http)?;
    let auth = wire::credential(plan.auth_env.as_deref())?;
    let session_dir = lifecycle::start(
        root,
        &lifecycle::Session {
            version: 1,
            index: session,
            first_wave,
            plan_sha256: plan_hash.into(),
            collector_sha256: plan.collector_sha256.clone(),
            started_unix_ms: unix_ms(),
        },
    )?;
    install_latch()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime initialization failed: {e}"))?;
    let mut summary = Summary {
        version: 1,
        session,
        first_wave,
        status: "completed".into(),
        planned_waves: plan.waves.len(),
        published_waves: 0,
        measured_waves: 0,
        eligible_measured_waves: 0,
        wave_publication_us: 0,
        wave_preparation_us: 0,
        reservation_publication_us: 0,
        ineligible_warmup_waves: 0,
        claim: if session == 0 {
            "client-observed-fixed-wave-performance-not-attested-execution-or-steady-state-capacity"
        } else {
            "continued-session-wave-observations-not-an-uninterrupted-timing-comparison"
        }
        .into(),
    };
    let operation = runtime.block_on(async {
        let client = wire::client(plan.local_http, plan.pool_max_idle_per_host)?;
        let metrics_client = plan.metrics.as_ref().map(|_| wire::client(plan.local_http, 1)).transpose()?;
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).map_err(|_| "cannot subscribe to interrupt signal")?;
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|_| "cannot subscribe to termination signal")?;
        let mut sequence = crate::sequence::State::default();
        for spec in &plan.waves[first_wave..] {
            if interrupted() { summary.status = "interrupted".into(); break; }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) { summary.status = "budget-exhausted".into(); break; }
            let admission = match deadline {
                Some(deadline) => match lifecycle::admission_bounded(&session_dir, deadline)? {
                    Some(admission) => admission,
                    None => { summary.status = "budget-exhausted".into(); break; }
                },
                None => lifecycle::admission(&session_dir)?,
            };
            if lifecycle::requested(&session_dir)? { summary.status = "paused".into(); break; }
            let preparation = Instant::now();
            let requests: Vec<_> = (0..spec.concurrency).map(|lane| sequence.request(plan, spec, lane)).collect::<Result<_>>()?;
            let hashes = requests.iter().map(|r| evidence::digest(r.as_bytes())).collect();
            let reservation = Reservation { version: 1, plan_sha256: plan_hash.into(), wave: spec.clone(), requests, request_sha256: hashes };
            if interrupted() { summary.status = "interrupted".into(); break; }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) { summary.status = "budget-exhausted".into(); break; }
            let reservation_start = Instant::now();
            let dir = evidence::wave_dir(root, spec.index);
            evidence::fresh(&dir)?;
            let reserved = evidence::json(&dir.join("reservation.json"), &reservation)?;
            evidence::sync(&dir)?; evidence::sync(root)?;
            let reservation_sha256 = evidence::digest(&reserved);
            drop(reserved);
            drop(admission);
            let reservation_publication_us = wire::us(reservation_start);
            let settings = crate::sequence::settings(&plan.workload, spec);
            let prepared: Vec<_> = reservation.requests.into_iter().map(|body| wire::request(&client, &url, auth.as_ref(), body, settings.stream)).collect::<Result<_>>()?;
            let telemetry_start = plan.metrics.as_ref().map(|_| Instant::now());
            let before = match (&plan.metrics, &metrics_client) {
                (Some(config), Some(client)) => Some(metrics::scrape(client, config, &mut metrics_budget, &dir, "before").await?),
                _ => None,
            };
            let before_overhead_us = telemetry_start.map_or(0, wire::us);
            let (stop, cancellation) = watch::channel(interrupted());
            let mut tasks = JoinSet::new();
            let preparation_us = wire::us(preparation).saturating_sub(before_overhead_us);
            let measured_origin_unix_ms = plan.metrics.as_ref().map(|_| metrics::unix_ms());
            let origin = Instant::now();
            for (lane, request) in prepared.into_iter().enumerate() {
                tasks.spawn(wire::collect(client.clone(), request, plan.workload.limits.clone(), settings.clone(), lane as u32, (origin, deadline), cancellation.clone()));
            }
            let mut settled = Vec::with_capacity(spec.concurrency as usize);
            while !tasks.is_empty() {
                tokio::select! {
                    biased;
                    _ = signal.recv(), if !*cancellation.borrow() => { let _ = stop.send(true); },
                    _ = terminate.recv(), if !*cancellation.borrow() => { let _ = stop.send(true); },
                    result = tasks.join_next() => {
                        let Some(result) = result else { break; };
                        let collected = result.map_err(|_| "collector task failed; wave reservation retained but no success claimed")?;
                        settled.push(collected);
                    }
                }
            }
            let measured_duration_us = wire::us(origin);
            let metrics = match (&plan.metrics, &metrics_client, before, measured_origin_unix_ms) {
                (Some(config), Some(client), Some(before), Some(measured_origin_unix_ms)) => {
                    let after_start = Instant::now();
                    let after = metrics::scrape(client, config, &mut metrics_budget, &dir, "after").await?;
                    let snapshots_us = before.duration_us + after.duration_us;
                    let mut reference = metrics::publish(&dir, &metrics::Receipt {
                        version: 1, plan_sha256: plan_hash.into(), wave: spec.index,
                        before, after, measured_origin_unix_ms, measured_duration_us,
                    })?;
                    reference.overhead_us = before_overhead_us + wire::us(after_start);
                    metrics_budget.finish_wave(reference.overhead_us, snapshots_us)?;
                    Some(reference)
                }
                _ => None,
            };
            // No hashing, filesystem writes or receipt publication while any peer is active.
            let publication = Instant::now();
            settled.sort_by_key(|a| a.attempt.lane);
            let first = settled.iter().map(|a| a.attempt.timing.dispatch_offset_us).min().unwrap_or(0);
            let last = settled.iter().map(|a| a.attempt.timing.dispatch_offset_us + a.attempt.timing.settle_us).max().unwrap_or(first);
            let last_dispatch = settled.iter().map(|a| a.attempt.timing.dispatch_offset_us).max().unwrap_or(first);
            let mut attempts = Vec::with_capacity(settled.len());
            for collected in settled {
                let mut a = collected.attempt;
                if plan.version < 3 {
                    a.timing.last_generated_text_us = None;
                    a.timing.terminal_us = None;
                }
                a.response_sha256 = evidence::digest(&collected.body);
                a.sequence = sequence.observe(plan, spec, &a, &collected.body)?;
                a.eligibility_errors = eligibility(&a, &settings, spec.phase);
                evidence::write(&dir.join(format!("response-{:04}.bin", a.lane)), &collected.body)?;
                attempts.push(a);
            }
            let network_failed = attempts.iter().any(|a| {
                !matches!(a.status, Status::Complete | Status::Interrupted)
                    || a.http_status.is_some_and(|status| status != 200)
            });
            let (eligible, tokens, rate) = evidence::throughput(&attempts, last - first);
            let wave = Wave { version: 1, plan_sha256: plan_hash.into(), reservation_sha256, spec: spec.clone(), attempts, elapsed_us: last - first, dispatch_spread_us: last_dispatch - first, preparation_us, reservation_publication_us, body_publication_us: wire::us(publication), completion_tokens: tokens, achieved_completion_tokens_per_second: rate, eligible, metrics };
            evidence::publish(&dir, "wave.json", &wave)?;
            summary.wave_publication_us += wire::us(publication);
            summary.wave_preparation_us += preparation_us;
            summary.reservation_publication_us += reservation_publication_us;
            summary.published_waves += 1;
            if spec.phase == Phase::Measured { summary.measured_waves += 1; summary.eligible_measured_waves += usize::from(eligible); }
            if spec.phase == Phase::Warmup && !eligible { summary.ineligible_warmup_waves += 1; }
            if !json { eprintln!("{} trial {}: {}", spec.cell, spec.trial, if eligible { "eligible" } else { "ineligible; evidence retained" }); }
            if wave.attempts.iter().any(|a| a.status == Status::Complete && a.sequence.as_ref().is_some_and(|check| !crate::sequence::passed(check))) {
                summary.status = "stopped-after-sequence-check".into(); break;
            }
            let budget_expired = deadline.is_some_and(|deadline| Instant::now() >= deadline);
            let invalid_response = wave.attempts.iter().any(irrevocable_invalid);
            if plan.version >= 3 && (invalid_response || (!eligible && wave.attempts.iter().all(|a| a.status == Status::Complete))) {
                summary.status = "stopped-after-ineligible-response".into(); break;
            }
            if network_failed { summary.status = "stopped-after-response-failure".into(); break; }
            if interrupted() { summary.status = "interrupted".into(); break; }
            if budget_expired { summary.status = "budget-exhausted".into(); break; }
            if *cancellation.borrow() { summary.status = "interrupted".into(); break; }
        }
        Ok::<(), String>(())
    });
    if let Err(error) = operation {
        summary.status = "local-failure".into();
        let _ = evidence::publish(&session_dir, "run.json", &summary);
        if session == 0 {
            let _ = evidence::publish(root, "run.json", &summary);
        }
        return Err(error);
    }
    if summary.status == "completed"
        && (summary.measured_waves != summary.eligible_measured_waves
            || summary.ineligible_warmup_waves != 0)
    {
        summary.status = "completed-with-ineligible-measurements".into();
    }
    if session != 0 && summary.status == "completed" {
        summary.status = "completed-with-ineligible-measurements".into();
    }
    evidence::publish(&session_dir, "run.json", &summary)?;
    // The first receipt remains byte-frozen; later outcomes live in their sessions.
    if session == 0 {
        evidence::publish(root, "run.json", &summary)?;
    }
    Ok(summary)
}

pub(crate) fn irrevocable_invalid(attempt: &Attempt) -> bool {
    let terminal = attempt.terminal_offset.is_some() || attempt.status == Status::Complete;
    attempt.eligibility_errors.iter().any(|error| {
        if terminal {
            error != "response_not_complete"
        } else {
            matches!(
                error.as_str(),
                "reported_output_exceeds_cap"
                    | "provider_reported_prefix_cache_nonzero"
                    | "inconsistent_provider_cache_usage"
                    | "inconsistent_provider_reasoning_usage"
            )
        }
    })
}

#[cfg(test)]
#[path = "../tests/support/measurement_bounded.rs"]
mod measurement_bounded_tests;
