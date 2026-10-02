use crate::{evidence, lifecycle, metrics, model::*, wire};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio::task::JoinSet;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn latch(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}
static LATCH_INSTALLATION: LazyLock<Result<()>> = LazyLock::new(|| {
    // Later acquisitions must not replace Tokio's process-wide chained handler.
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `latch` only performs an atomic store, so it is async-signal-safe.
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
#[derive(clap::Args)]
pub struct CommonArgs {
    pub workload: PathBuf,
    #[arg(long)]
    pub endpoint: String,
    #[arg(long)]
    pub model: String,
    /// Optional JSON declarations of model revision, runtime, hardware and settings.
    #[arg(long)]
    pub deployment: Option<PathBuf>,
    /// Capture an exact observed-envelope declaration before dispatch.
    #[arg(long)]
    pub policy: Option<PathBuf>,
    /// Optional bounded server-wide diagnostics; can perturb between-wave cadence.
    #[arg(long)]
    pub metrics_url: Option<String>,
    /// Metrics diagnostics protocol: 1 keeps the frozen before/after allowlist,
    /// 2 selects the explicit accounting protocol.
    #[arg(long, default_value_t = 1)]
    pub metrics_version: u32,
    /// Environment variable *name* holding a metrics-only credential. Never the
    /// model credential, and never retained as a value.
    #[arg(long)]
    pub metrics_auth_env: Option<String>,
    /// Explicit operator declaration; this does not authenticate isolation.
    #[arg(long, value_parser = [metrics::v2::ISOLATION_SHARED, metrics::v2::ISOLATION_EXCLUSIVE])]
    pub metrics_isolation: Option<String>,
    #[arg(long)]
    pub auth_env: Option<String>,
    #[arg(long)]
    pub local_http: bool,
}
#[derive(clap::Args)]
pub struct Options {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long)]
    pub json: bool,
}

struct Admitted {
    source: Vec<u8>,
    source_sha256: String,
    workload: Workload,
    workload_sha256: String,
    endpoint: String,
    deployment: Option<Deployment>,
    waves: Vec<WaveSpec>,
    metrics: Option<metrics::Protocol>,
    policy_bytes: Option<Vec<u8>>,
    policy_sha256: Option<String>,
    schedule_budget: Option<ScheduleBudget>,
    acquisition_budget: Option<crate::acquisition::Budget>,
}

#[derive(Serialize)]
pub struct DeploymentBytes {
    model_revision: Option<usize>,
    runtime: Option<usize>,
    hardware: Option<usize>,
    settings: Option<usize>,
}

#[derive(Serialize)]
pub struct PreflightReport<'a> {
    claim: &'static str,
    source_sha256: String,
    workload_sha256: String,
    name: String,
    request: RequestSettings,
    limits: Limits,
    warmup_requests: usize,
    measured_requests: usize,
    total_output_token_ceiling: usize,
    endpoint: String,
    model: &'a str,
    local_http: bool,
    auth_env: Option<&'a str>,
    deployment_bytes: Option<DeploymentBytes>,
    metrics_url: Option<String>,
    policy_sha256: Option<String>,
    planned_waves: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    schedule: Option<Vec<crate::schedule::Scenario>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schedule_budget: Option<ScheduleBudget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    acquisition_budget: Option<crate::acquisition::Budget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    acquisition: Option<crate::acquisition::Protocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resources: Option<crate::serving_resources::Config>,
}

pub fn preflight(o: &CommonArgs) -> Result<PreflightReport<'_>> {
    let admitted = admit(o)?;
    let (warmup, measured, tokens) = crate::selection::budgets(&admitted.workload, 1)?;
    Ok(PreflightReport {
        claim: "offline-declared-run-admission-not-backend-qualification",
        source_sha256: admitted.source_sha256,
        workload_sha256: admitted.workload_sha256,
        name: admitted.workload.name,
        request: admitted.workload.request,
        limits: admitted.workload.limits,
        warmup_requests: warmup,
        measured_requests: measured,
        total_output_token_ceiling: tokens,
        endpoint: admitted.endpoint,
        model: &o.model,
        local_http: o.local_http,
        auth_env: o.auth_env.as_deref(),
        deployment_bytes: admitted.deployment.as_ref().map(|value| DeploymentBytes {
            model_revision: value.model_revision.as_ref().map(String::len),
            runtime: value.runtime.as_ref().map(String::len),
            hardware: value.hardware.as_ref().map(String::len),
            settings: value.settings.as_ref().map(String::len),
        }),
        metrics_url: admitted.metrics.map(|config| config.endpoint().to_owned()),
        policy_sha256: admitted.policy_sha256,
        planned_waves: admitted.waves.len(),
        schedule: admitted.workload.schedule,
        schedule_budget: admitted.schedule_budget,
        acquisition_budget: admitted.acquisition_budget,
        acquisition: admitted.workload.acquisition,
        resources: admitted.workload.resources,
    })
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
    if o.common.metrics_url.is_some() {
        return Err("metrics diagnostics are unsupported in bounded capture; use the advanced unbounded run path".into());
    }
    execute_inner(o, Some(deadline))
}
/// Capacity admits a finite workload6 study, including the existing bounded
/// metrics2 observer. Its global deadline never authorizes another request.
pub fn execute_capacity_bounded(o: &Options, deadline: Instant) -> Result<Summary> {
    let source = evidence::read(&o.common.workload, FILE_CAP)?;
    let workload: Workload = serde_json::from_slice(&source).map_err(|e| e.to_string())?;
    if workload.version != 6 {
        return Err("capacity requires workload6".into());
    }
    execute_inner(o, Some(deadline))
}

#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
fn admit(o: &CommonArgs) -> Result<Admitted> {
    let source = evidence::read(&o.workload, FILE_CAP)?;
    let workload: Workload =
        serde_json::from_slice(&source).map_err(|e| format!("invalid workload: {e}"))?;
    workload.validate()?;
    if matches!(workload.version, 2 | 5) && o.policy.is_some() {
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
    let waves = workload.waves();
    if o.metrics_url.is_none()
        && (o.metrics_version != 1 || o.metrics_auth_env.is_some() || o.metrics_isolation.is_some())
    {
        return Err("metrics protocol controls require --metrics-url".into());
    }
    let metrics = o
        .metrics_url
        .as_ref()
        .map(|endpoint| {
            wire::endpoint(endpoint, o.local_http).and_then(|url| {
                let config = metrics::Protocol::new(
                    url.to_string(),
                    waves.len(),
                    o.metrics_version,
                    o.metrics_auth_env.clone(),
                    o.metrics_isolation.clone(),
                    o.auth_env.as_deref(),
                )?;
                config.validate(waves.len(), o.local_http, o.auth_env.as_deref())?;
                Ok(config)
            })
        })
        .transpose()?;
    let source_sha256 = evidence::digest(&source);
    let workload_sha256 =
        evidence::digest(&serde_json::to_vec(&workload).map_err(|e| e.to_string())?);
    let policy_bytes = o
        .policy
        .as_ref()
        .map(|path| evidence::read(path, crate::policy::CAP))
        .transpose()?;
    // Execution salts are hex digests; this bound has their exact encoded width
    // without generating or publishing an execution identity.
    let context = wire::BodyContext {
        workload: &workload,
        model: &o.model,
        cache_namespace: workload
            .cache_namespace_required()
            .then_some("0000000000000000000000000000000000000000000000000000000000000000"),
    };
    if matches!(workload.version, 2 | 5) {
        for (case, spec) in workload.cases.iter().zip(&waves) {
            if case.step.as_ref().is_some_and(|step| step.parent.is_none()) {
                let mut root = spec.clone();
                root.index = 0;
                crate::sequence::State::default().request(&context, &root, 0)?;
            }
        }
    }
    let mut request_bytes = 0u64;
    if workload.version == 4 {
        for spec in &waves {
            let mut escaped_total = 0usize;
            for lane in 0..spec.concurrency {
                let body = wire::request_body(&context, spec, lane)?;
                request_bytes = request_bytes
                    .checked_add(body.len() as u64)
                    .ok_or("schedule input byte ceiling overflows")?;
                escaped_total = escaped_total
                    .checked_add(
                        serde_json::to_string(&body)
                            .map_err(|e| e.to_string())?
                            .len(),
                    )
                    .ok_or("schedule reservation size overflow")?;
            }
            if escaped_total > 32 * 1024 * 1024 {
                return Err(format!(
                    "scenario {} exceeds the reservation receipt bound",
                    spec.cell
                ));
            }
        }
    }
    let schedule_budget = if workload.version == 4 {
        // Same pretty-printed nesting depth as plan.workload/plan.waves. The
        // remaining bounded envelope plus its actual variable strings fits the
        // separately reserved 64 KiB; no source or lane data is omitted.
        let plan_bound = json_size(&(&workload, &waves))?
            .checked_add(json_size(&(
                &o.model,
                url.as_str(),
                &deployment,
                &o.auth_env,
                &metrics,
            ))?)
            .and_then(|n| n.checked_add(64 * 1024))
            .ok_or("schedule plan size overflows")?;
        if plan_bound > 8 * 1024 * 1024 {
            return Err("schedule exceeds the 8 MiB native plan bound".into());
        }
        let (warmup, measured, _) = crate::selection::budgets(&workload, 1)?;
        Some(ScheduleBudget {
            request_body_bytes: request_bytes,
            response_byte_ceiling: (warmup as u64 + measured as u64)
                * workload.limits.response_bytes as u64,
            scenario_time_ceiling_ms: waves.len() as u64 * u64::from(workload.limits.total_ms),
            maximum_admitted_lanes: waves.iter().map(|s| s.concurrency).max().unwrap_or(0),
        })
    } else {
        None
    };
    if let Some(bytes) = &policy_bytes {
        crate::policy::parse(
            bytes,
            &evidence::binary_digest()?,
            &source_sha256,
            &workload,
            metrics.as_ref(),
        )
        .map_err(|e| e.as_str().to_owned())?;
    }
    let acquisition_budget = if workload.version == 6 {
        crate::acquisition::preflight(&context)?;
        let plan_bound = json_size(&(&workload, &waves))?
            .checked_add(json_size(&(
                &o.model,
                url.as_str(),
                &deployment,
                &o.auth_env,
                &metrics,
            ))?)
            .and_then(|n| n.checked_add(64 * 1024))
            .ok_or("acquisition plan size overflow")?;
        if plan_bound > 8 * 1024 * 1024 {
            return Err("acquisition exceeds native plan bound".into());
        }
        Some(crate::acquisition::budget(&workload)?)
    } else {
        None
    };
    // Seed magnitude peaks at a corner of the trial/lane range and index 1023 is
    // the widest index, but lane 0 renders one digit narrower than lanes 10..63 in
    // each of the cache salt and the text salt, so an interior body can exceed a
    // corner sample by two bytes. Bound with that slack rather than serializing
    // every prompt. Generated prose is ASCII without JSON escapes and has an
    // identity-independent length, so these bounds also cover every prose body.
    // Identical output controls give warmup the same encoded bound as measured.
    for cell in workload.cells.iter().filter(|_| workload.version != 6) {
        for phase in [Phase::Measured, Phase::Warmup] {
            if phase == Phase::Warmup
                && (cell.warmup_trials == 0
                    || workload.request.effective_output(phase)
                        == workload.request.effective_output(Phase::Measured))
            {
                continue;
            }
            for (trial, lane) in [(0, 0), (100, 63)] {
                let bound = WaveSpec {
                    index: 1023,
                    phase,
                    cell: cell.id.clone(),
                    case: Some(cell.case.clone()),
                    trial,
                    concurrency: cell.concurrency,
                    lanes: None,
                    acquisition: None,
                };
                let body = wire::request_body(&context, &bound, lane)?;
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
    }
    Ok(Admitted {
        source,
        source_sha256,
        workload,
        workload_sha256,
        endpoint: url.to_string(),
        deployment,
        waves,
        metrics,
        policy_sha256: policy_bytes.as_deref().map(evidence::digest),
        policy_bytes,
        schedule_budget,
        acquisition_budget,
    })
}

#[derive(Serialize)]
struct ScheduleBudget {
    request_body_bytes: u64,
    response_byte_ceiling: u64,
    scenario_time_ceiling_ms: u64,
    maximum_admitted_lanes: u32,
}

fn json_size(value: &impl Serialize) -> Result<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("JSON size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer_pretty(&mut counter, value).map_err(|e| e.to_string())?;
    Ok(counter.0)
}

fn execute_inner(o: &Options, deadline: Option<Instant>) -> Result<Summary> {
    let admitted = admit(&o.common)?;
    let cache_namespace = if !admitted.workload.cache_namespace_required() {
        None
    } else {
        let mut random = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut random))
            .map_err(|_| "cannot obtain cache-salt entropy")?;
        Some(evidence::digest(&random))
    };
    let cache_mechanism = admitted
        .workload
        .cache_mechanism_required()
        .then(|| "declared-vllm-prefix-cache".into());
    let pool = admitted
        .waves
        .iter()
        .map(|s| s.concurrency as usize)
        .max()
        .unwrap_or(1);
    let plan = Plan {
        version: if admitted.workload.version == 6 {
            5
        } else if matches!(admitted.workload.version, 4 | 5) {
            4
        } else {
            3
        },
        metric_contract: Some(METRIC_CONTRACT.into()),
        kind: "performance-run-v1".into(),
        tool_version: env!("CARGO_PKG_VERSION").into(),
        collector_sha256: evidence::binary_digest()?,
        workload: admitted.workload,
        workload_sha256: admitted.workload_sha256,
        source_sha256: admitted.source_sha256,
        model: o.common.model.clone(),
        deployment: admitted.deployment,
        cache_mechanism,
        cache_evidence_source: "provider-reported:usage.prompt_tokens_details.cached_tokens".into(),
        endpoint: admitted.endpoint,
        auth_env: o.common.auth_env.clone(),
        local_http: o.common.local_http,
        pool_max_idle_per_host: pool,
        started_unix_ms: metrics::unix_ms(),
        cache_namespace,
        waves: admitted.waves,
        metrics: admitted.metrics,
        policy_sha256: admitted.policy_sha256,
    };
    evidence::fresh(&o.out)?;
    let _owner = lifecycle::ownership(&o.out)?;
    evidence::write(&o.out.join("workload.json"), &admitted.source)?;
    if let Some(bytes) = &admitted.policy_bytes {
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
    if matches!(loaded.plan.workload.version, 2 | 5 | 6) {
        return Err("bounded conversation sequences cannot resume; retain the partial sequence and start a new explicitly budgeted capture".into());
    }
    if !matches!(loaded.plan.version, 2..=4) {
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

#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
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
            started_unix_ms: metrics::unix_ms(),
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
        let metrics_auth = plan
            .metrics
            .as_ref()
            .and_then(|config| config.auth_env())
            .map(|name| wire::credential(Some(name)))
            .transpose()?
            .flatten();
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).map_err(|_| "cannot subscribe to interrupt signal")?;
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|_| "cannot subscribe to termination signal")?;
        let mut sequence = crate::sequence::State::default();
        let capture_origin = Instant::now();
        let mut resource_session = crate::serving_resources::Session::new(root, plan, plan_hash, capture_origin);
        let deadline = if plan.version == 5 {
            let allowance = crate::acquisition::budget(&plan.workload)?.wall_time_ceiling_us;
            let acquisition_deadline = capture_origin.checked_add(Duration::from_micros(allowance)).ok_or("acquisition deadline overflow")?;
            Some(deadline.map_or(acquisition_deadline, |d| d.min(acquisition_deadline)))
        } else { deadline };
        let body_context = wire::BodyContext::from(plan);
        let acquisition_operation = async {
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
            resource_session.begin(spec.index).await?;
            let preparation = Instant::now();
            let requests: Vec<_> = match (0..spec.concurrency).map(|lane| sequence.request(&body_context, spec, lane)).collect::<Result<_>>() {
                Ok(requests) => requests,
                Err(detail) if plan.version == 5 => {
                    evidence::publish(root, "acquisition-failure.json", &crate::acquisition::AdmissionFailure { version: 1, wave: spec.clone(), detail })?;
                    summary.status = "local-failure".into();
                    break;
                }
                Err(detail) => return Err(detail),
            };
            let hashes = requests.iter().map(|r| evidence::digest(r.as_bytes())).collect();
            let reservation = Reservation { version: if matches!(plan.version, 4 | 5) { 2 } else { 1 }, plan_sha256: plan_hash.into(), wave: spec.clone(), requests, request_sha256: hashes };
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
            let settings = (0..spec.concurrency).map(|lane| crate::schedule::settings(&plan.workload, spec, lane)).collect::<Result<Vec<_>>>()?;
            let prepared: Vec<_> = reservation.requests.into_iter().zip(&settings).map(|(body, settings)| wire::request(&client, &url, auth.as_ref(), body, settings.stream)).collect::<Result<_>>()?;
            let telemetry_start = plan.metrics.as_ref().map(|_| Instant::now());
            let before = match (&plan.metrics, &metrics_client, telemetry_start) {
                (Some(config), Some(client), Some(telemetry_origin)) => Some(metrics::capture(client, config, &mut metrics_budget, &dir, "before", &metrics::CaptureCtx { origin: telemetry_origin, auth: metrics_auth.as_ref(), cancelled: Some(&INTERRUPTED) }).await?),
                _ => None,
            };
            let before_overhead_us = telemetry_start.map_or(0, wire::us);
            let (stop, cancellation) = watch::channel(interrupted());
            let mut tasks = JoinSet::new();
            let mut tool_expectation = sequence.tool_expectation(&plan.workload, spec)?;
            let preparation_us = wire::us(preparation).saturating_sub(before_overhead_us);
            let measured_origin_unix_ms = plan.metrics.as_ref().map(|_| metrics::unix_ms());
            let origin = Instant::now();
            let (mut settled, schedule_fatal) = if spec.lanes.is_some() {
                let scenario_deadline = origin + Duration::from_millis(u64::from(plan.workload.limits.total_ms));
                let scenario_deadline = deadline.map_or(scenario_deadline, |d| d.min(scenario_deadline));
                dispatch_schedule(&client, prepared, (&settings, &plan.workload.limits), spec, (origin, scenario_deadline), (&mut signal, &mut terminate), (&stop, &cancellation)).await?
            } else {
                for (lane, request) in prepared.into_iter().enumerate() {
                    tasks.spawn(wire::collect(client.clone(), request, plan.workload.limits.clone(), settings[lane].clone(), wire::CollectContext {
                        lane: lane as u32, origin, deadline, cancel: cancellation.clone(),
                        first_generated: None, tool_expectation: tool_expectation.take(),
                    }));
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
                (settled, None)
            };
            let measured_duration_us = wire::us(origin);
            // Bind timeout classification to the retained barrier clock, including
            // scheduler delay after the final admission-loop deadline check.
            let schedule_fatal = schedule_fatal.or_else(|| {
                (spec.lanes.is_some()
                    && measured_duration_us >= u64::from(plan.workload.limits.total_ms) * 1000)
                    .then_some(crate::schedule::Reason::Deadline)
            });
            let acquisition_clock = if plan.version == 5 {
                let started_offset_us = metrics::offset_us(capture_origin, origin)?;
                Some(crate::acquisition::StepClock {
                    clock_id: plan_hash.into(), kind: crate::acquisition::ClockKind::StdInstantMonotonic,
                    units: crate::acquisition::ClockUnits::Microseconds,
                    started_offset_us,
                    settled_offset_us: started_offset_us.checked_add(crate::acquisition::settlement(settled.iter().map(|a| &a.attempt))?).ok_or("acquisition clock overflow")?,
                })
            } else { None };
            let metrics = match (&plan.metrics, &metrics_client, before, measured_origin_unix_ms, telemetry_start) {
                (Some(config), Some(client), Some(before), Some(measured_origin_unix_ms), Some(telemetry_origin)) => {
                    let after_start = Instant::now();
                    let measured = metrics::Measured {
                        origin_offset_us: metrics::offset_us(telemetry_origin, origin)?,
                        origin_unix_ms: measured_origin_unix_ms,
                        duration_us: measured_duration_us,
                    };
                    let after = metrics::capture(client, config, &mut metrics_budget, &dir, "after", &metrics::CaptureCtx { origin: telemetry_origin, auth: metrics_auth.as_ref(), cancelled: Some(&INTERRUPTED) }).await?;
                    let snapshots_us = before.duration_us() + after.duration_us();
                    let mut reference = metrics::publish_wave(&dir, config, plan_hash, spec.index, before, after, measured)?;
                    reference.overhead_us = before_overhead_us + wire::us(after_start);
                    metrics_budget.finish_wave(reference.overhead_us, snapshots_us)?;
                    Some(reference)
                }
                _ => None,
            };
            // No hashing, filesystem writes or receipt publication while any peer is active.
            let publication = Instant::now();
            settled.sort_by_key(|a| a.attempt.lane);
            let (elapsed_us, dispatch_spread_us) = crate::schedule::bounds(settled.iter().map(|a| &a.attempt), spec.lanes.as_ref().map(|_| measured_duration_us))?;
            let mut attempts = Vec::with_capacity(settled.len());
            for collected in settled {
                let mut a = collected.attempt;
                if plan.version < 3 {
                    a.timing.last_generated_text_us = None;
                    a.timing.terminal_us = None;
                }
                a.response_sha256 = evidence::digest(&collected.body);
                a.sequence = sequence.observe(plan, spec, &a, &collected.body)?;
                a.eligibility_errors = eligibility(&a, &settings[a.lane as usize], crate::acquisition::eligibility_phase(&plan.workload, spec.phase));
                evidence::write(&dir.join(format!("response-{:04}.bin", a.lane)), &collected.body)?;
                attempts.push(a);
            }
            let network_failed = attempts.iter().any(|a| {
                !matches!(a.status, Status::Complete | Status::Interrupted)
                    || a.http_status.is_some_and(|status| status != 200)
            });
            let schedule = if spec.lanes.is_some() {
                let reasons: Vec<_> = attempts.iter().map(|a| (!a.dispatched).then_some(schedule_fatal.unwrap_or(crate::schedule::Reason::Cancelled))).collect();
                Some(crate::schedule::observation(spec, &attempts, &reasons, schedule_fatal, measured_duration_us)?)
            } else { None };
            let (eligible, tokens, rate) = crate::schedule::throughput(&attempts, elapsed_us, schedule.as_ref());
            let wave = Wave { version: if matches!(plan.version, 4 | 5) { 2 } else { 1 }, plan_sha256: plan_hash.into(), reservation_sha256, spec: spec.clone(), attempts, elapsed_us, dispatch_spread_us, preparation_us, reservation_publication_us, body_publication_us: wire::us(publication), completion_tokens: tokens, achieved_completion_tokens_per_second: rate, eligible, metrics, schedule, acquisition_clock };
            evidence::publish(&dir, "wave.json", &wave)?;
            resource_session.member(&wave);
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
            if let Some(reason) = schedule_fatal {
                summary.status = match reason {
                    crate::schedule::Reason::Deadline => "budget-exhausted",
                    crate::schedule::Reason::Cancelled => "interrupted",
                    _ => "stopped-after-response-failure",
                }.into();
                break;
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
        }.await;
        // Always drain the caller-owned observer before the terminal run receipt,
        // including errors, cancellation, local publication and incomplete groups.
        let resource_result = resource_session.finish(acquisition_operation.is_err() || summary.status != "completed").await;
        acquisition_operation?;
        resource_result?;
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

/// Admission inside the existing wave collector: no second client, retry or publication path.
async fn dispatch_schedule(
    client: &reqwest::Client,
    prepared: Vec<reqwest::Request>,
    controls: (&[RequestSettings], &Limits),
    spec: &WaveSpec,
    window: (Instant, Instant),
    signals: (
        &mut tokio::signal::unix::Signal,
        &mut tokio::signal::unix::Signal,
    ),
    cancellation: (&watch::Sender<bool>, &watch::Receiver<bool>),
) -> Result<(Vec<wire::Collected>, Option<crate::schedule::Reason>)> {
    use crate::schedule::{Reason, State};
    let (settings, limits) = controls;
    let (origin, deadline) = window;
    let (signal, terminate) = signals;
    let (stop, cancellation) = cancellation;
    let lanes = spec.lanes.as_deref().ok_or("missing schedule lanes")?;
    let mut state = State::new(lanes);
    let (notify, mut notifications) = tokio::sync::mpsc::channel(lanes.len());
    let mut pending: Vec<_> = prepared.into_iter().map(Some).collect();
    let mut settled = Vec::with_capacity(lanes.len());
    let mut tasks = JoinSet::<wire::Collected>::new();
    let mut task_failed = false;
    loop {
        if interrupted() || *cancellation.borrow() {
            state.fail(Reason::Cancelled);
        }
        if Instant::now() >= deadline {
            state.fail(Reason::Deadline);
        }
        // Notifications and already settled failures take precedence over arrivals.
        while let Ok(event) = notifications.try_recv() {
            state.notify(event);
        }
        while let Some(result) = tasks.try_join_next() {
            match result {
                Ok(collected) => {
                    state.settle(
                        &collected.attempt,
                        &settings[collected.attempt.lane as usize],
                        spec.phase,
                    );
                    settled.push(collected);
                }
                Err(_) => {
                    task_failed = true;
                    state.fail(Reason::LaneFailed);
                }
            }
        }
        if interrupted() || *cancellation.borrow() {
            state.fail(Reason::Cancelled);
        }
        if Instant::now() >= deadline {
            state.fail(Reason::Deadline);
        }
        if state.fatal.is_some() {
            let _ = stop.send(true);
        }
        while let Some(lane) = state.ready(wire::us(origin)) {
            // Recheck between admissions, not only at the start of this iteration.
            if interrupted() || *cancellation.borrow() {
                state.fail(Reason::Cancelled);
                break;
            }
            if Instant::now() >= deadline {
                state.fail(Reason::Deadline);
                break;
            }
            let request = pending[lane]
                .take()
                .ok_or("scheduled lane admitted twice")?;
            tasks.spawn(wire::collect(
                client.clone(),
                request,
                limits.clone(),
                settings[lane].clone(),
                wire::CollectContext {
                    lane: lane as u32,
                    origin,
                    deadline: Some(deadline),
                    cancel: cancellation.clone(),
                    first_generated: Some(notify.clone()),
                    tool_expectation: None,
                },
            ));
        }
        if state.fatal.is_some() {
            let _ = stop.send(true);
        }
        if tasks.is_empty() && (!state.pending() || state.fatal.is_some()) {
            break;
        }
        let next = state
            .next_due()
            .and_then(|us| origin.checked_add(Duration::from_micros(us)))
            .unwrap_or(deadline)
            .min(deadline);
        tokio::select! {
            biased;
            _ = signal.recv(), if state.fatal.is_none() => { state.fail(Reason::Cancelled); },
            _ = terminate.recv(), if state.fatal.is_none() => { state.fail(Reason::Cancelled); },
            event = notifications.recv() => {
                match event { Some(event) => state.notify(event), None => state.fail(Reason::NotificationFailed) }
            },
            result = tasks.join_next(), if !tasks.is_empty() => {
                if interrupted() || *cancellation.borrow() { state.fail(Reason::Cancelled); }
                if Instant::now() >= deadline { state.fail(Reason::Deadline); }
                while let Ok(event) = notifications.try_recv() { state.notify(event); }
                match result {
                    Some(Ok(collected)) => {
                        state.settle(&collected.attempt, &settings[collected.attempt.lane as usize], spec.phase);
                        settled.push(collected);
                    }
                    Some(Err(_)) => { task_failed = true; state.fail(Reason::LaneFailed); }
                    None => (),
                }
            },
            _ = tokio::time::sleep_until(next.into()), if state.fatal.is_none() => (),
        }
    }
    // A panicked task has unknown transport state. Drain every other admitted
    // peer before returning; reservation retains all identities without success.
    if task_failed {
        return Err(
            "collector task failed after admitted peers settled; reservation retained".into(),
        );
    }
    for (lane, request) in pending.into_iter().enumerate() {
        if request.is_some() {
            settled.push(crate::schedule::undispatched(
                lane as u32,
                state.fatal.unwrap_or(Reason::Cancelled),
            ));
        }
    }
    Ok((settled, state.fatal))
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
                    | "reasoning_reported_with_thinking_disabled"
            )
        }
    })
}

#[cfg(test)]
#[path = "../tests/support/measurement_bounded.rs"]
mod measurement_bounded_tests;
