use crate::model::*;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 15) as usize] as char);
    }
    output
}
pub fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
pub fn read(path: &Path, cap: usize) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > cap as u64 {
        return Err(format!("not a bounded regular file: {}", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > cap {
        return Err("evidence grew past its byte bound".into());
    }
    Ok(bytes)
}
pub fn directory(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("directory {}: {e}", path.display()))?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err(format!("not a nonsymlink directory: {}", path.display()));
    }
    Ok(())
}
pub fn fresh(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    directory(parent).map_err(|e| {
        format!("{e}; create the output parent or choose an existing nonsymlink directory")
    })?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!(
                    "create fresh directory {}: {e}; choose a new output directory; existing evidence is never overwritten",
                    path.display()
                )
            } else {
                format!("create fresh directory {}: {e}", path.display())
            }
        })
}
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("publish {}: {e}", path.display()))
}
pub fn sync(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("sync directory {}: {e}", path.display()))
}
pub fn json<T: Serialize>(path: &Path, value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    write(path, &bytes)?;
    Ok(bytes)
}
pub fn publish<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<()> {
    let staging = dir.join(format!(".{name}.pending"));
    json(&staging, value)?;
    fs::hard_link(&staging, dir.join(name)).map_err(|e| format!("publish receipt: {e}"))?;
    fs::remove_file(&staging).map_err(|e| e.to_string())?;
    sync(dir)
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid performance evidence: {e}"))
}
pub fn wave_dir(root: &Path, index: u32) -> PathBuf {
    root.join(format!("wave-{index:06}"))
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}
pub fn binary_digest() -> Result<String> {
    static DIGEST: std::sync::LazyLock<Result<String>> = std::sync::LazyLock::new(hash_binary);
    DIGEST.clone()
}
fn hash_binary() -> Result<String> {
    let mut file =
        File::open("/proc/self/exe").map_err(|e| format!("read collector binary: {e}"))?;
    let mut buf = [0u8; 65536];
    let mut hash = Sha256::new();
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hex(&hash.finalize()))
}
pub fn throughput(attempts: &[Attempt], elapsed_us: u64) -> (bool, Option<u64>, Option<f64>) {
    let eligible = attempts
        .iter()
        .all(|a| a.status == Status::Complete && a.eligibility_errors.is_empty());
    let tokens = if attempts.iter().all(|a| a.status == Status::Complete) {
        attempts
            .iter()
            .try_fold(0u64, |sum, a| sum.checked_add(a.usage.completion_tokens?))
    } else {
        None
    };
    let rate = tokens
        .filter(|_| eligible && elapsed_us > 0)
        .map(|n| n as f64 * 1_000_000.0 / elapsed_us as f64);
    (eligible && rate.is_some(), tokens, rate)
}
pub(crate) fn decode_sample(attempt: &Attempt) -> Option<(u64, u64)> {
    if attempt.status != Status::Complete {
        return None;
    }
    let tokens = attempt.usage.completion_tokens?;
    let first = attempt.timing.first_generated_text_us?;
    let settle = attempt.timing.settle_us;
    if tokens < 2 || settle <= first {
        return None;
    }
    Some((tokens - 1, settle - first))
}
pub(crate) fn prefill_sample(attempt: &Attempt) -> Option<(u64, u64)> {
    if attempt.status != Status::Complete
        || attempt.usage.cached_prompt_tokens.is_some_and(|n| n > 0)
    {
        return None;
    }
    let tokens = attempt.usage.prompt_tokens?;
    let first = attempt.timing.first_generated_text_us?;
    if tokens == 0 || first == 0 {
        return None;
    }
    Some((tokens, first))
}
fn decode_rate(attempt: &Attempt) -> Option<f64> {
    decode_sample(attempt).map(|(n, d)| n as f64 * 1_000_000.0 / d as f64)
}
pub(crate) fn text_decode_rate(attempt: &Attempt) -> Option<f64> {
    if attempt.status != Status::Complete {
        return None;
    }
    let tokens = attempt.usage.completion_tokens?;
    let first = attempt.timing.first_generated_text_us?;
    let last = attempt.timing.last_generated_text_us?;
    (tokens >= 2 && last > first).then(|| (tokens - 1) as f64 * 1_000_000.0 / (last - first) as f64)
}
fn prefill_rate(attempt: &Attempt) -> Option<f64> {
    prefill_sample(attempt).map(|(n, d)| n as f64 * 1_000_000.0 / d as f64)
}
pub struct Loaded {
    pub plan: Plan,
    pub waves: Vec<Option<Wave>>,
    pub states: Vec<&'static str>,
    pub history: crate::lifecycle::History,
    pub metrics: Option<crate::metrics::Summary>,
    pub policy: Option<crate::policy::Policy>,
    pub plan_sha256: String,
    pub evidence_sha256: String,
    pub lineage_sha256: String,
}
pub fn load(root: &Path) -> Result<Loaded> {
    load_verified(root).map_err(|error| error.detail)
}
pub(crate) struct LoadError {
    pub reason: crate::policy::Reason,
    pub detail: String,
}
impl From<String> for LoadError {
    fn from(detail: String) -> Self {
        Self {
            reason: crate::policy::Reason::InvalidEvidence,
            detail,
        }
    }
}
impl From<&str> for LoadError {
    fn from(detail: &str) -> Self {
        detail.to_owned().into()
    }
}
pub(crate) fn load_verified(root: &Path) -> Result<Loaded, LoadError> {
    directory(root)?;
    let plan_bytes = read(&root.join("plan.json"), 8 * 1024 * 1024)?;
    let plan: Plan = decode(&plan_bytes)?;
    if !matches!(plan.version, 1..=3) || plan.kind != "performance-run-v1" {
        return Err("unsupported performance plan".into());
    }
    if match plan.version {
        3 => plan.metric_contract.as_deref() != Some(METRIC_CONTRACT),
        _ => plan.metric_contract.is_some(),
    } {
        return Err("metric contract does not match plan version".into());
    }
    plan.workload.validate()?;
    if let Some(deployment) = &plan.deployment {
        deployment.validate()?;
    }
    if let Some(config) = &plan.metrics {
        if plan.version < 2 {
            return Err("metrics require execution-session provenance".into());
        }
        config.validate(plan.waves.len(), plan.local_http)?;
    }
    let expected_mechanism = (plan.workload.request.profile != Profile::PortableChatV1)
        .then_some("declared-vllm-prefix-cache");
    if plan.cache_mechanism.as_deref() != expected_mechanism
        || plan.cache_evidence_source
            != "provider-reported:usage.prompt_tokens_details.cached_tokens"
    {
        return Err("cache observation source does not match the request profile".into());
    }
    let expected_pool = plan
        .workload
        .cells
        .iter()
        .map(|c| c.concurrency as usize)
        .max()
        .unwrap_or(1);
    if plan.pool_max_idle_per_host != expected_pool || plan.collector_sha256.len() != 64 {
        return Err("invalid declared collector or connection-pool policy".into());
    }
    match (
        &plan.cache_namespace,
        plan.workload.request.cache != Cache::Observe || plan.workload.salted(),
    ) {
        (None, false) => (),
        (Some(n), true) if n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()) => {}
        _ => return Err("cache namespace does not match the declared mechanism".into()),
    }
    let source = read(&root.join("workload.json"), FILE_CAP)?;
    let workload: Workload = decode(&source)?;
    if workload != plan.workload
        || digest(&source) != plan.source_sha256
        || digest(&serde_json::to_vec(&workload).map_err(|e| e.to_string())?)
            != plan.workload_sha256
        || plan.waves != workload.waves()
    {
        return Err("workload or schedule identity mismatch".into());
    }
    let policy = plan
        .policy_sha256
        .as_ref()
        .map(|hash| {
            let bytes = read(&root.join("policy.json"), crate::policy::CAP).map_err(|detail| {
                LoadError {
                    reason: crate::policy::Reason::InvalidPolicy,
                    detail,
                }
            })?;
            if digest(&bytes) != *hash {
                return Err(LoadError {
                    reason: crate::policy::Reason::PolicyHashMismatch,
                    detail: "policy sidecar hash mismatch".into(),
                });
            }
            crate::policy::parse(&bytes, &plan).map_err(|reason| LoadError {
                detail: reason.as_str().into(),
                reason,
            })
        })
        .transpose()?;
    crate::wire::endpoint(&plan.endpoint, plan.local_http)?;
    let plan_hash = digest(&plan_bytes);
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"grill-perf-evidence-v1\0");
    fingerprint.update(plan_hash.as_bytes());
    fingerprint.update(plan.source_sha256.as_bytes());
    let mut lineage = Sha256::new();
    lineage.update(b"grill-perf-acquisition-lineage-v1\0");
    let mut waves = Vec::with_capacity(plan.waves.len());
    let mut states = Vec::with_capacity(plan.waves.len());
    let mut metrics_budget = crate::metrics::Budget::default();
    let mut metrics_waves = Vec::new();
    let mut sequence = crate::sequence::State::default();
    for spec in &plan.waves {
        let dir = wave_dir(root, spec.index);
        if !exists(&dir)? {
            fingerprint.update(b"not_started\0");
            waves.push(None);
            states.push("not_started");
            continue;
        }
        directory(&dir)?;
        let reservation_bytes = read(&dir.join("reservation.json"), 40 * 1024 * 1024)?;
        let reservation: Reservation = decode(&reservation_bytes)?;
        let reservation_hash = digest(&reservation_bytes);
        fingerprint.update(reservation_hash.as_bytes());
        if reservation.version != 1
            || reservation.plan_sha256 != plan_hash
            || reservation.wave != *spec
            || reservation.requests.len() != spec.concurrency as usize
            || reservation.request_sha256.len() != reservation.requests.len()
        {
            return Err("reservation lineage mismatch".into());
        }
        for (lane, (body, hash)) in reservation
            .requests
            .iter()
            .zip(&reservation.request_sha256)
            .enumerate()
        {
            if body.len() > REQUEST_CAP
                || digest(body.as_bytes()) != *hash
                || *body != sequence.request(&plan, spec, lane as u32)?
            {
                return Err("request evidence does not match declared controls".into());
            }
        }
        let path = dir.join("wave.json");
        if !exists(&path)? {
            fingerprint.update(b"reserved_unsettled\0");
            waves.push(None);
            states.push("reserved_unsettled");
            continue;
        }
        let wave_bytes = read(&path, FILE_CAP)?;
        fingerprint.update(b"published\0");
        fingerprint.update(digest(&wave_bytes).as_bytes());
        let wave: Wave = decode(&wave_bytes)?;
        lineage.update(
            digest(
                &serde_json::to_vec(&(
                    &wave.spec,
                    &wave.attempts,
                    wave.elapsed_us,
                    wave.dispatch_spread_us,
                    wave.preparation_us,
                    wave.reservation_publication_us,
                    wave.body_publication_us,
                ))
                .map_err(|e| e.to_string())?,
            )
            .as_bytes(),
        );
        if wave.version != 1
            || wave.plan_sha256 != plan_hash
            || wave.reservation_sha256 != reservation_hash
            || wave.spec != *spec
            || wave.attempts.len() != spec.concurrency as usize
        {
            return Err("wave lineage mismatch".into());
        }
        match (&plan.metrics, &wave.metrics) {
            (Some(config), Some(reference)) => {
                let summary = crate::metrics::load(
                    &dir,
                    reference,
                    config,
                    &plan_hash,
                    spec.index,
                    &mut metrics_budget,
                )?;
                if wave.attempts.iter().any(|a| {
                    a.timing
                        .dispatch_offset_us
                        .checked_add(a.timing.settle_us)
                        .is_none_or(|n| n > summary.receipt.measured_duration_us)
                }) {
                    return Err("metrics measurement boundary contradicts wave settlement".into());
                }
                metrics_waves.push(summary);
            }
            (None, None) => (),
            _ => return Err("missing or undeclared metrics companion evidence".into()),
        }
        let settings = crate::sequence::settings(&workload, spec);
        for (lane, a) in wave.attempts.iter().enumerate() {
            if a.lane as usize != lane
                || a.response_bytes > workload.limits.response_bytes
                || a.terminal_offset.is_some_and(|n| n > a.response_bytes)
                || a.eligibility_errors != eligibility(a, &settings, spec.phase)
            {
                return Err("invalid attempt facts".into());
            }
            let body = read(
                &dir.join(format!("response-{lane:04}.bin")),
                workload.limits.response_bytes,
            )?;
            if body.len() != a.response_bytes || digest(&body) != a.response_sha256 {
                return Err("response evidence hash mismatch".into());
            }
            if sequence.observe(&plan, spec, a, &body)? != a.sequence {
                return Err(
                    "sequence check or lineage differs from retained response evidence".into(),
                );
            }
            if a.status == Status::Complete {
                if !a.dispatched || a.http_status != Some(200) {
                    return Err("complete response was not a successful dispatch".into());
                }
                crate::wire::verify_complete(
                    a,
                    &body,
                    settings.stream,
                    plan.version == 3,
                    settings.profile,
                )?;
            }
            a.timing.validate(settings.stream)?;
            if plan.version == 3 {
                if a.status != Status::Complete {
                    crate::wire::verify_partial_arrivals(
                        a,
                        &body,
                        settings.stream,
                        settings.profile,
                    )?;
                }
                if a.timing.last_generated_text_us.is_some()
                    != a.timing.first_generated_text_us.is_some()
                    || a.timing.terminal_us.is_some() != a.terminal_offset.is_some()
                {
                    return Err("generated-text arrival observations are incomplete".into());
                }
            } else if a.timing.last_generated_text_us.is_some() || a.timing.terminal_us.is_some() {
                return Err(
                    "legacy evidence cannot declare generated-text arrival observations".into(),
                );
            }
            if a.http_status.is_some() != a.timing.headers_us.is_some()
                || (a.response_bytes != 0) != a.timing.first_body_us.is_some()
                || (a.status == Status::Complete
                    && a.timing.settle_us >= u64::from(workload.limits.total_ms) * 1000)
            {
                return Err(
                    "response observations contradict header, body or deadline facts".into(),
                );
            }
            if !a.dispatched
                && (a.status != Status::Interrupted
                    || a.http_status.is_some()
                    || a.response_bytes != 0
                    || a.timing.headers_us.is_some())
            {
                return Err("undispatched attempt contains response facts".into());
            }
        }
        let first = wave
            .attempts
            .iter()
            .map(|a| a.timing.dispatch_offset_us)
            .min()
            .unwrap_or(0);
        let last_dispatch = wave
            .attempts
            .iter()
            .map(|a| a.timing.dispatch_offset_us)
            .max()
            .unwrap_or(0);
        let mut last = first;
        for a in &wave.attempts {
            last = last.max(
                a.timing
                    .dispatch_offset_us
                    .checked_add(a.timing.settle_us)
                    .ok_or("timing overflow")?,
            );
        }
        if wave.dispatch_spread_us != last_dispatch - first
            || wave.elapsed_us != last - first
            || throughput(&wave.attempts, wave.elapsed_us)
                != (
                    wave.eligible,
                    wave.completion_tokens,
                    wave.achieved_completion_tokens_per_second,
                )
        {
            return Err("wave metric derivation mismatch".into());
        }
        waves.push(Some(wave));
        states.push("published");
    }
    let history = crate::lifecycle::history(root, &plan, &plan_hash, &states, &waves)?;
    fingerprint.update(history.evidence_sha256.as_bytes());
    lineage.update(history.lineage_sha256.as_bytes());
    Ok(Loaded {
        metrics: plan.metrics.as_ref().map(|config| crate::metrics::Summary {
            config: config.clone(),
            budget: metrics_budget,
            waves: metrics_waves,
        }),
        plan,
        waves,
        states,
        history,
        policy,
        plan_sha256: plan_hash,
        evidence_sha256: hex(&fingerprint.finalize()),
        lineage_sha256: hex(&lineage.finalize()),
    })
}
#[derive(Serialize)]
pub struct CellSummary {
    pub cell: String,
    pub planned_trials: u32,
    pub observed_trials: usize,
    pub eligible_trials: usize,
    pub wave_latency_us: Vec<Option<u64>>,
    pub achieved_completion_tokens_per_second: Vec<Option<f64>>,
    pub median_wave_latency_us: Option<f64>,
    pub median_achieved_completion_tokens_per_second: Option<f64>,
    pub median_decode_tokens_per_second: Option<f64>,
    pub median_text_decode_tokens_per_second: Option<f64>,
    pub median_prefill_tokens_per_second: Option<f64>,
    pub wave_latency_us_range: Option<[f64; 2]>,
    pub achieved_completion_tokens_per_second_range: Option<[f64; 2]>,
    pub decode_tokens_per_second_range: Option<[f64; 2]>,
    pub text_decode_tokens_per_second_range: Option<[f64; 2]>,
    pub prefill_tokens_per_second_range: Option<[f64; 2]>,
    pub first_answer_text_us: Vec<Option<u64>>,
    pub trial_states: Vec<&'static str>,
    pub reported_completion_tokens: Vec<Option<u64>>,
    pub issues: Vec<String>,
    pub warmup_states: Vec<String>,
    pub all_declared_warmups_complete: bool,
    pub lane_completion_tokens: Vec<Vec<Option<u64>>>,
    pub lane_decode_tokens_per_second: Vec<Vec<Option<f64>>>,
    pub lane_text_decode_tokens_per_second: Vec<Vec<Option<f64>>>,
    pub lane_prompt_tokens: Vec<Vec<Option<u64>>>,
    pub lane_prefill_tokens_per_second: Vec<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_check: Option<crate::sequence::Check>,
}
fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}
fn range(values: &[f64]) -> Option<[f64; 2]> {
    let first = *values.first()?;
    Some(values.iter().fold([first, first], |[min, max], &value| {
        [min.min(value), max.max(value)]
    }))
}
pub fn summarize(run: &Loaded) -> Vec<CellSummary> {
    let warmups_complete = run
        .plan
        .waves
        .iter()
        .zip(&run.waves)
        .filter(|(spec, _)| spec.phase == Phase::Warmup)
        .all(|(_, wave)| wave.as_ref().is_some_and(|w| w.eligible));
    run.plan
        .workload
        .cells
        .iter()
        .map(|cell| {
            let pairs: Vec<_> = run
                .plan
                .waves
                .iter()
                .zip(&run.waves)
                .filter(|(spec, _)| spec.phase == Phase::Measured && spec.cell == cell.id)
                .collect();
            let mut issues = Vec::new();
            if run.history.count > 1 {
                issues.push("continued_execution_sessions: client pool reset and cache/warmup continuity unverified; no uninterrupted timing comparison".into());
            }
            if run.history.open {
                issues.push("execution_session_unsettled: interruption or publication outcome uncertain".into());
            }
            if !warmups_complete {
                issues.push("declared_warmup_evidence_incomplete".into());
            }
            let mut first_answer = Vec::new();
            for (spec, wave) in &pairs {
                match wave {
                    None => {
                        issues.push(run.states[spec.index as usize].into());
                        first_answer.extend(std::iter::repeat_n(None, spec.concurrency as usize));
                    }
                    Some(w) => {
                        for a in &w.attempts {
                            issues.extend(a.eligibility_errors.clone());
                            first_answer.push(a.timing.first_answer_text_us);
                        }
                    }
                }
            }
            issues.sort();
            issues.dedup();
            let observed = pairs.iter().filter(|(_, w)| w.is_some()).count();
            let eligible = pairs
                .iter()
                .filter(|(_, w)| w.as_ref().is_some_and(|w| w.eligible))
                .count();
            let latencies: Vec<_> = pairs
                .iter()
                .map(|(_, w)| w.as_ref().map(|w| w.elapsed_us))
                .collect();
            let rates: Vec<_> = pairs
                .iter()
                .map(|(_, w)| {
                    w.as_ref()
                        .and_then(|w| w.achieved_completion_tokens_per_second)
                })
                .collect();
            let decode_rates: Vec<Vec<_>> = pairs
                .iter()
                .map(|(spec, wave)| match wave {
                    Some(w) => w
                        .attempts
                        .iter()
                        .map(decode_rate)
                        .collect(),
                    None => vec![None; spec.concurrency as usize],
                })
                .collect();
            let text_decode_rates: Vec<Vec<_>> = pairs
                .iter()
                .map(|(spec, wave)| match wave {
                    Some(w) => w.attempts.iter().map(text_decode_rate).collect(),
                    None => vec![None; spec.concurrency as usize],
                })
                .collect();
            let prefill_rates: Vec<Vec<_>> = pairs
                .iter()
                .map(|(spec, wave)| match wave {
                    Some(w) => w.attempts.iter().map(prefill_rate).collect(),
                    None => vec![None; spec.concurrency as usize],
                })
                .collect();
            let complete = warmups_complete && eligible == cell.trials as usize
                && run.history.count <= 1 && !run.history.open;
            let (latency_values, rate_values, decode_values, prefill_values) = if complete {
                (
                    latencies.iter().flatten().map(|n| *n as f64).collect::<Vec<_>>(),
                    rates.iter().flatten().copied().collect::<Vec<_>>(),
                    decode_rates.iter().flatten().flatten().copied().collect::<Vec<_>>(),
                    prefill_rates.iter().flatten().flatten().copied().collect::<Vec<_>>(),
                )
            } else {
                (Vec::new(), Vec::new(), Vec::new(), Vec::new())
            };
            let text_decode_values = if complete {
                text_decode_rates.iter().flatten().flatten().copied().collect()
            } else {
                Vec::new()
            };
            CellSummary {
                sequence_check: pairs.first().and_then(|(_, wave)| wave.as_ref()).and_then(|wave| wave.attempts.first()).and_then(|a| a.sequence.clone()),
                cell: cell.id.clone(),
                planned_trials: cell.trials,
                observed_trials: observed,
                eligible_trials: eligible,
                wave_latency_us_range: range(&latency_values),
                achieved_completion_tokens_per_second_range: range(&rate_values),
                decode_tokens_per_second_range: range(&decode_values),
                prefill_tokens_per_second_range: range(&prefill_values),
                median_wave_latency_us: median(latency_values),
                median_achieved_completion_tokens_per_second: median(rate_values),
                median_decode_tokens_per_second: median(decode_values),
                lane_decode_tokens_per_second: decode_rates,
                text_decode_tokens_per_second_range: range(&text_decode_values),
                median_text_decode_tokens_per_second: median(text_decode_values),
                lane_text_decode_tokens_per_second: text_decode_rates,
                median_prefill_tokens_per_second: median(prefill_values),
                lane_prefill_tokens_per_second: prefill_rates,
                wave_latency_us: latencies,
                achieved_completion_tokens_per_second: rates,
                reported_completion_tokens: pairs
                    .iter()
                    .map(|(_, w)| w.as_ref().and_then(|w| w.completion_tokens))
                    .collect(),
                first_answer_text_us: first_answer,
                trial_states: pairs
                    .iter()
                    .map(|(spec, _)| run.states[spec.index as usize])
                    .collect(),
                issues,
                warmup_states: run
                    .plan
                    .waves
                    .iter()
                    .zip(&run.waves)
                    .filter(|(spec, _)| spec.phase == Phase::Warmup && spec.cell == cell.id)
                    .map(|(spec, wave)| match wave {
                        None => run.states[spec.index as usize].to_owned(),
                        Some(w) => if w.eligible { "eligible" } else { "ineligible" }.to_owned(),
                    })
                    .collect(),
                all_declared_warmups_complete: warmups_complete,
                lane_completion_tokens: pairs
                    .iter()
                    .map(|(spec, wave)| match wave {
                        Some(w) => w
                            .attempts
                            .iter()
                            .map(|a| a.usage.completion_tokens)
                            .collect(),
                        None => vec![None; spec.concurrency as usize],
                    })
                    .collect(),
                lane_prompt_tokens: pairs
                    .iter()
                    .map(|(spec, wave)| match wave {
                        Some(w) => w.attempts.iter().map(|a| a.usage.prompt_tokens).collect(),
                        None => vec![None; spec.concurrency as usize],
                    })
                    .collect(),
            }
        })
        .collect()
}
#[derive(Serialize)]
pub struct CellChange {
    pub cell: String,
    pub eligible: bool,
    pub observed_output_amounts_match: bool,
    pub reference_output_amounts_match: Option<bool>,
    pub ineligibility_reasons: Vec<String>,
    pub withheld: Vec<String>,
    pub wave_latency_change_percent: Option<f64>,
    pub achieved_throughput_change_percent: Option<f64>,
    pub decode_rate_change_percent: Option<f64>,
    pub prefill_rate_change_percent: Option<f64>,
}
#[derive(Serialize)]
pub struct Drift {
    pub cell: String,
    pub withheld: Vec<String>,
    pub wave_latency_percent: Option<f64>,
    pub achieved_throughput_percent: Option<f64>,
    pub decode_rate_percent: Option<f64>,
    pub prefill_rate_percent: Option<f64>,
}
#[derive(Serialize)]
pub struct Comparison {
    pub version: u32,
    pub claim: &'static str,
    pub baseline_model: String,
    pub candidate_model: String,
    pub baseline_deployment: Option<Deployment>,
    pub candidate_deployment: Option<Deployment>,
    pub reference_model: Option<String>,
    pub reference_deployment: Option<Deployment>,
    pub reference_identity: Option<ReferenceIdentity>,
    pub baseline: Vec<CellSummary>,
    pub candidate: Vec<CellSummary>,
    pub reference: Option<Vec<CellSummary>>,
    pub drift: Option<Vec<Drift>>,
    pub changes: Vec<CellChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_metrics: Option<crate::metrics::Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_metrics: Option<crate::metrics::Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_metrics: Option<crate::metrics::Summary>,
}
fn change(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    a.zip(b)
        .filter(|(a, _)| *a > 0.0)
        .map(|(a, b)| 100.0 * (b / a - 1.0))
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceIdentityStatus {
    DeclaredMatch,
    DeclaredMismatch,
    Unavailable,
}
impl ReferenceIdentityStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeclaredMatch => "declared_match",
            Self::DeclaredMismatch => "declared_mismatch",
            Self::Unavailable => "unavailable",
        }
    }
}
#[derive(Serialize)]
pub struct ReferenceIdentity {
    pub status: ReferenceIdentityStatus,
    pub reasons: Vec<String>,
    pub scope: &'static str,
}
pub(crate) fn reference_identity(a: &Plan, a2: &Plan) -> ReferenceIdentity {
    let mut reasons = Vec::new();
    let mut mismatch = false;
    let mut declaration = |name: &str, a: Option<&str>, a2: Option<&str>| match (
        a.filter(|s| !s.is_empty()),
        a2.filter(|s| !s.is_empty()),
    ) {
        (Some(a), Some(a2)) if a != a2 => {
            mismatch = true;
            reasons.push(format!(
                "reference: {name} declaration differs from baseline"
            ));
        }
        (Some(_), Some(_)) => {}
        (a, a2) => {
            if a.is_none() {
                reasons.push(format!(
                    "reference: baseline {name} declaration unavailable"
                ));
            }
            if a2.is_none() {
                reasons.push(format!("reference: {name} declaration unavailable"));
            }
        }
    };
    declaration("model", Some(&a.model), Some(&a2.model));
    declaration("endpoint", Some(&a.endpoint), Some(&a2.endpoint));
    let a = a.deployment.as_ref();
    let a2 = a2.deployment.as_ref();
    declaration(
        "model_revision",
        a.and_then(|d| d.model_revision.as_deref()),
        a2.and_then(|d| d.model_revision.as_deref()),
    );
    declaration(
        "runtime",
        a.and_then(|d| d.runtime.as_deref()),
        a2.and_then(|d| d.runtime.as_deref()),
    );
    declaration(
        "hardware",
        a.and_then(|d| d.hardware.as_deref()),
        a2.and_then(|d| d.hardware.as_deref()),
    );
    declaration(
        "settings",
        a.and_then(|d| d.settings.as_deref()),
        a2.and_then(|d| d.settings.as_deref()),
    );
    ReferenceIdentity {
        status: if mismatch {
            ReferenceIdentityStatus::DeclaredMismatch
        } else if reasons.is_empty() {
            ReferenceIdentityStatus::DeclaredMatch
        } else {
            ReferenceIdentityStatus::Unavailable
        },
        reasons,
        scope: "Matching declarations do not verify server restoration, cache state, timing order or causal effect.",
    }
}
fn output_amounts_match(a: &CellSummary, b: &CellSummary) -> bool {
    a.lane_completion_tokens
        .iter()
        .flatten()
        .all(Option::is_some)
        && a.lane_completion_tokens == b.lane_completion_tokens
}
fn complete_lane_observations(values: &[Vec<Option<f64>>]) -> bool {
    values.iter().flatten().all(Option::is_some)
}

pub(crate) fn compatible(a: &Plan, b: &Plan) -> bool {
    a.workload_sha256 == b.workload_sha256
        && a.metric_contract == b.metric_contract
        && a.tool_version == b.tool_version
        && a.collector_sha256 == b.collector_sha256
        && a.local_http == b.local_http
        && a.pool_max_idle_per_host == b.pool_max_idle_per_host
        && a.metrics == b.metrics
}

pub fn compare(a: &Path, b: &Path, reference: Option<&Path>) -> Result<Comparison> {
    let left = load(a)?;
    let right = load(b)?;
    let mut reference = reference
        .map(|path| load(path).map_err(|e| format!("reference: {e}")))
        .transpose()?;
    for (side, other) in
        std::iter::once(("candidate", &right)).chain(reference.iter().map(|run| ("reference", run)))
    {
        if !compatible(&left.plan, &other.plan) {
            return Err(format!(
                "{side}: incompatible workload, metric contract, tool, transport or metrics controls; no matched comparison produced"
            ));
        }
    }
    let baseline = summarize(&left);
    let candidate = summarize(&right);
    let reference_identity = reference
        .as_ref()
        .map(|run| reference_identity(&left.plan, &run.plan));
    let reference_model = reference.as_ref().map(|run| run.plan.model.clone());
    let reference_deployment = reference
        .as_ref()
        .and_then(|run| run.plan.deployment.clone());
    let reference_metrics = reference.as_mut().and_then(|run| run.metrics.take());
    let reference = reference.as_ref().map(summarize);
    let changes: Vec<CellChange> = baseline
        .iter()
        .zip(&candidate)
        .enumerate()
        .map(|(index, (a, b))| {
            let a2 = reference.as_ref().map(|reference| &reference[index]);
            let same = output_amounts_match(a, b);
            let reference_same = a2.map(|a2| output_amounts_match(a, a2));
            let mut reasons: Vec<String> = a
                .issues
                .iter()
                .map(|s| format!("baseline: {s}"))
                .chain(b.issues.iter().map(|s| format!("candidate: {s}")))
                .collect();
            let semantics_qualified = [Some(a), Some(b), a2].into_iter().flatten()
                .all(|cell| cell.sequence_check.as_ref().is_none_or(crate::sequence::passed));
            if !semantics_qualified {
                reasons.push("declared sequence correctness or strict-output check failed; timing observations retained separately".into());
            }
            if !same {
                reasons.push("paired trial/lane completion counts missing or unequal".into());
            }
            if let Some(a2) = a2 {
                reasons.extend(a2.issues.iter().map(|s| format!("reference: {s}")));
                if a2.median_wave_latency_us.is_none() {
                    reasons
                        .push("reference: complete eligible measured evidence unavailable".into());
                }
                if reference_same != Some(true) {
                    reasons.push(
                        "reference: paired trial/lane completion counts missing or unequal".into(),
                    );
                }
            }
            if let Some(identity) = &reference_identity {
                reasons.extend(identity.reasons.iter().cloned());
            }
            let reference_qualified = a2.is_none_or(|a2| {
                reference_same == Some(true)
                    && a2.median_wave_latency_us.is_some()
                    && reference_identity.as_ref().is_some_and(|identity| {
                        identity.status == ReferenceIdentityStatus::DeclaredMatch
                    })
            });
            let eligible = same
                && semantics_qualified
                && a.median_wave_latency_us.is_some()
                && b.median_wave_latency_us.is_some()
                && reference_qualified;
            let mut withheld = Vec::new();
            // A supplied repeat must contribute this metric, never disappear into
            // a baseline-only range when its observations are unavailable.
            let mut matched_change =
                |name: &str,
                 value: Option<f64>,
                 a: Option<[f64; 2]>,
                 a2: Option<[f64; 2]>,
                 b: Option<[f64; 2]>,
                 unit: (f64, usize),
                 observations_complete: bool| {
                    if reference.is_some() && !observations_complete {
                        withheld.push(format!(
                            "{name}: reference comparison has missing lane observations"
                        ));
                        return None;
                    }
                    if a2.is_none() && reference.is_some() {
                        withheld.push(format!("{name}: reference metric range unavailable"));
                        return None;
                    }
                    let value = value.filter(|_| same && reference_qualified && semantics_qualified)?;
                    let (a, b) = a.zip(b)?;
                    let a = match a2 {
                        Some(a2) => [a[0].min(a2[0]), a[1].max(a2[1])],
                        None => a,
                    };
                    if a[1] < b[0] || b[1] < a[0] {
                        return Some(value);
                    }
                    let (divisor, decimals) = unit;
                    let [a0, a1, b0, b1] = [a[0], a[1], b[0], b[1]].map(|v| v / divisor);
                    withheld.push(format!(
                        "{name}: ranges overlap, {a0:.*}-{a1:.*} vs {b0:.*}-{b1:.*}{}",
                        decimals,
                        decimals,
                        decimals,
                        decimals,
                        if a2.is_some() {
                            " (baseline pooled with reference)"
                        } else {
                            ""
                        }
                    ));
                    None
                };
            CellChange {
                cell: a.cell.clone(),
                eligible,
                observed_output_amounts_match: same,
                reference_output_amounts_match: reference_same,
                ineligibility_reasons: reasons,
                wave_latency_change_percent: matched_change(
                    "wave latency",
                    change(a.median_wave_latency_us, b.median_wave_latency_us),
                    a.wave_latency_us_range,
                    a2.and_then(|a2| a2.wave_latency_us_range),
                    b.wave_latency_us_range,
                    (1_000_000.0, 2),
                    true,
                ),
                achieved_throughput_change_percent: matched_change(
                    "achieved throughput",
                    change(
                        a.median_achieved_completion_tokens_per_second,
                        b.median_achieved_completion_tokens_per_second,
                    ),
                    a.achieved_completion_tokens_per_second_range,
                    a2.and_then(|a2| a2.achieved_completion_tokens_per_second_range),
                    b.achieved_completion_tokens_per_second_range,
                    (1.0, 1),
                    true,
                ),
                decode_rate_change_percent: matched_change(
                    "decode rate",
                    change(
                        a.median_decode_tokens_per_second,
                        b.median_decode_tokens_per_second,
                    ),
                    a.decode_tokens_per_second_range,
                    a2.and_then(|a2| a2.decode_tokens_per_second_range),
                    b.decode_tokens_per_second_range,
                    (1.0, 1),
                    std::iter::once(a)
                        .chain(std::iter::once(b))
                        .chain(a2)
                        .all(|s| complete_lane_observations(&s.lane_decode_tokens_per_second)),
                ),
                prefill_rate_change_percent: matched_change(
                    "prefill rate",
                    change(
                        a.median_prefill_tokens_per_second,
                        b.median_prefill_tokens_per_second,
                    ),
                    a.prefill_tokens_per_second_range,
                    a2.and_then(|a2| a2.prefill_tokens_per_second_range),
                    b.prefill_tokens_per_second_range,
                    (1.0, 1),
                    std::iter::once(a)
                        .chain(std::iter::once(b))
                        .chain(a2)
                        .all(|s| complete_lane_observations(&s.lane_prefill_tokens_per_second)),
                ),
                withheld,
            }
        })
        .collect();
    let drift = reference.as_ref().map(|reference| {
        baseline
            .iter()
            .zip(&candidate)
            .zip(reference)
            .zip(&changes)
            .map(|(((a, b), a2), qualification)| {
                let mut withheld = qualification.ineligibility_reasons.clone();
                let mut drift_change =
                    |name: &str, a: Option<f64>, a2: Option<f64>, observations_complete: bool| {
                        if !observations_complete {
                            withheld.push(format!(
                                "{name}: reference comparison has missing lane observations"
                            ));
                            return None;
                        }
                        if a2.is_none() {
                            withheld.push(format!("{name}: reference metric range unavailable"));
                            return None;
                        }
                        if !qualification.eligible {
                            return None;
                        }
                        let value = change(a, a2);
                        if value.is_none() {
                            withheld.push(format!(
                                "{name}: baseline metric unavailable or nonpositive"
                            ));
                        }
                        value
                    };
                Drift {
                    cell: a.cell.clone(),
                    wave_latency_percent: drift_change(
                        "wave latency",
                        a.median_wave_latency_us,
                        a2.median_wave_latency_us,
                        true,
                    ),
                    achieved_throughput_percent: drift_change(
                        "achieved throughput",
                        a.median_achieved_completion_tokens_per_second,
                        a2.median_achieved_completion_tokens_per_second,
                        true,
                    ),
                    decode_rate_percent: drift_change(
                        "decode rate",
                        a.median_decode_tokens_per_second,
                        a2.median_decode_tokens_per_second,
                        [a, b, a2]
                            .into_iter()
                            .all(|s| complete_lane_observations(&s.lane_decode_tokens_per_second)),
                    ),
                    prefill_rate_percent: drift_change(
                        "prefill rate",
                        a.median_prefill_tokens_per_second,
                        a2.median_prefill_tokens_per_second,
                        [a, b, a2]
                            .into_iter()
                            .all(|s| complete_lane_observations(&s.lane_prefill_tokens_per_second)),
                    ),
                    withheld,
                }
            })
            .collect()
    });
    Ok(Comparison {
        version: 3,
        claim: "descriptive-deployment-comparison-not-causal-or-steady-state-capacity",
        baseline_model: left.plan.model,
        candidate_model: right.plan.model,
        baseline_deployment: left.plan.deployment,
        candidate_deployment: right.plan.deployment,
        reference_model,
        reference_deployment,
        reference_identity,
        baseline_metrics: left.metrics,
        candidate_metrics: right.metrics,
        reference_metrics,
        baseline,
        candidate,
        reference,
        drift,
        changes,
    })
}
