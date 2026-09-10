use crate::{evidence, model::Result, wire};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NAMES: [&str; 4] = [
    "vllm:spec_decode_num_draft_tokens_total",
    "vllm:spec_decode_num_accepted_tokens_total",
    "vllm:num_requests_running",
    "vllm:num_requests_waiting",
];
const RECEIPT_CAP: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub endpoint: String,
    pub allowlist: [String; 4],
    pub scope: String,
    pub cadence: String,
    pub deadline_us: u64,
    pub body_bytes: usize,
    pub line_bytes: usize,
    pub series: usize,
    pub labels_per_series: usize,
    pub label_bytes_per_series: usize,
    pub run_budget_us: u64,
    pub retained_raw_bytes: usize,
    pub max_requests: usize,
}
impl Config {
    pub fn new(endpoint: String, waves: usize) -> Self {
        Self {
            version: 1,
            endpoint,
            allowlist: NAMES.map(str::to_owned),
            scope: "server-wide-full-labels-not-workload-attributed".into(),
            cadence: "reserved-before-origin-and-after-all-lanes-settle".into(),
            deadline_us: 2_000_000,
            body_bytes: 1024 * 1024,
            line_bytes: 64 * 1024,
            series: 256,
            labels_per_series: 16,
            label_bytes_per_series: 4096,
            run_budget_us: 30_000_000,
            retained_raw_bytes: 16 * 1024 * 1024,
            max_requests: waves * 2,
        }
    }
    pub fn validate(&self, waves: usize, local_http: bool) -> Result<()> {
        let url = wire::endpoint(&self.endpoint, local_http)?;
        if *self != Self::new(url.to_string(), waves) {
            return Err("unsupported metrics protocol or bounds".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub file: String,
    pub sha256: String,
    pub overhead_us: u64,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Complete,
    ParseError,
    TransportError,
    HttpError,
    Unsupported,
    Deadline,
    BodyLimit,
    SkippedBudget,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub started_unix_ms: u64,
    pub scrape_us: u64,
    pub duration_us: u64,
    pub charged_us: u64,
    pub allowance_us: u64,
    pub status: Status,
    pub error: Option<String>,
    pub http_status: Option<u16>,
    pub raw_bytes: usize,
    pub raw_sha256: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub version: u32,
    pub plan_sha256: String,
    pub wave: u32,
    pub before: Snapshot,
    pub after: Snapshot,
    pub measured_origin_unix_ms: u64,
    pub measured_duration_us: u64,
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Budget {
    pub requests: usize,
    pub charged_us: u64,
    pub duration_us: u64,
    pub raw_bytes: usize,
}
impl Budget {
    fn exhausted(&self, config: &Config) -> bool {
        self.requests >= config.max_requests
            || self.charged_us >= config.run_budget_us
            || config.retained_raw_bytes.saturating_sub(self.raw_bytes) < config.body_bytes
    }
    fn retain(&mut self, config: &Config, snapshot: &Snapshot) -> Result<()> {
        let skipped = snapshot.status == Status::SkippedBudget;
        if skipped != self.exhausted(config)
            || snapshot.charged_us != snapshot.duration_us
            || snapshot.allowance_us
                != if skipped {
                    0
                } else {
                    config
                        .deadline_us
                        .min(config.run_budget_us.saturating_sub(self.charged_us))
                }
            || (snapshot.status == Status::Deadline && snapshot.scrape_us < snapshot.allowance_us)
            || snapshot.scrape_us > snapshot.duration_us
            || snapshot
                .http_status
                .is_some_and(|s| !(100..=999).contains(&s))
            || (!skipped && (snapshot.duration_us == 0 || snapshot.scrape_us == 0))
            || (snapshot.raw_bytes > 0 && snapshot.http_status.is_none())
            || (snapshot.status == Status::Unsupported && snapshot.http_status != Some(200))
            || snapshot.raw_bytes > config.body_bytes
            || (skipped
                && (snapshot.duration_us != 0
                    || snapshot.scrape_us != 0
                    || snapshot.raw_bytes != 0
                    || snapshot.http_status.is_some()))
            || snapshot.error.as_ref().is_some_and(|s| s.len() > 256)
            || (snapshot.status == Status::Complete) != snapshot.error.is_none()
        {
            return Err("invalid metrics snapshot budget or status".into());
        }
        self.requests += usize::from(!skipped);
        self.charged_us = self
            .charged_us
            .checked_add(snapshot.charged_us)
            .ok_or("metrics budget overflow")?;
        self.duration_us = self
            .duration_us
            .checked_add(snapshot.duration_us)
            .ok_or("metrics duration overflow")?;
        self.raw_bytes += snapshot.raw_bytes;
        Ok(())
    }
    pub fn finish_wave(&mut self, overhead_us: u64, snapshots_us: u64) -> Result<()> {
        let publication_us = overhead_us
            .checked_sub(snapshots_us)
            .ok_or("metrics overhead contradicts scrape duration")?;
        self.charged_us = self
            .charged_us
            .checked_add(publication_us)
            .ok_or("metrics budget overflow")?;
        self.duration_us = self
            .duration_us
            .checked_add(publication_us)
            .ok_or("metrics duration overflow")?;
        Ok(())
    }
}
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

pub async fn scrape(
    client: &reqwest::Client,
    config: &Config,
    budget: &mut Budget,
    dir: &Path,
    side: &str,
) -> Result<Snapshot> {
    let start = Instant::now();
    let mut snapshot = Snapshot {
        started_unix_ms: unix_ms(),
        scrape_us: 0,
        duration_us: 0,
        charged_us: 0,
        allowance_us: 0,
        status: Status::SkippedBudget,
        error: Some("whole-run telemetry budget exhausted".into()),
        http_status: None,
        raw_bytes: 0,
        raw_sha256: String::new(),
    };
    let mut raw = Vec::new();
    if !budget.exhausted(config) {
        let allowance = config
            .deadline_us
            .min(config.run_budget_us - budget.charged_us);
        snapshot.allowance_us = allowance;
        let operation = async {
            let mut response = client
                .get(&config.endpoint)
                .header("accept", "text/plain; version=0.0.4")
                .header("accept-encoding", "identity")
                .send()
                .await
                .map_err(|_| (Status::TransportError, "metrics request failed".to_owned()))?;
            snapshot.http_status = Some(response.status().as_u16());
            let encoded = response
                .headers()
                .get_all("content-encoding")
                .iter()
                .any(|v| match v.to_str() {
                    Ok(s) => s
                        .split(',')
                        .any(|s| !s.trim().eq_ignore_ascii_case("identity")),
                    Err(_) => true,
                });
            while let Some(chunk) = response.chunk().await.map_err(|_| {
                (
                    Status::TransportError,
                    "metrics body transfer failed".to_owned(),
                )
            })? {
                if wire::us(start) >= allowance {
                    return Err((Status::Deadline, "metrics scrape deadline expired".into()));
                }
                let remaining = config.body_bytes - raw.len();
                raw.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if chunk.len() > remaining {
                    return Err((Status::BodyLimit, "metrics body exceeds bound".into()));
                }
            }
            if snapshot.http_status != Some(200) {
                return Err((
                    Status::HttpError,
                    "metrics HTTP status is not successful".into(),
                ));
            }
            if encoded {
                return Err((
                    Status::Unsupported,
                    "metrics content encoding is unsupported".into(),
                ));
            }
            parse(&raw, config).map_err(|e| (Status::ParseError, e))?;
            if wire::us(start) >= allowance {
                return Err((Status::Deadline, "metrics scrape deadline expired".into()));
            }
            Ok::<(), (Status, String)>(())
        };
        match tokio::time::timeout(Duration::from_micros(allowance), operation).await {
            Ok(Ok(())) => {
                snapshot.status = Status::Complete;
                snapshot.error = None;
            }
            Ok(Err((status, error))) => {
                snapshot.status = status;
                snapshot.error = Some(error);
            }
            Err(_) => {
                snapshot.status = Status::Deadline;
                snapshot.error = Some("metrics scrape deadline expired".into());
            }
        }
        snapshot.scrape_us = wire::us(start).max(1);
    }
    snapshot.raw_bytes = raw.len();
    snapshot.raw_sha256 = evidence::digest(&raw);
    evidence::write(&dir.join(format!("metrics-{side}.bin")), &raw)?;
    if snapshot.status != Status::SkippedBudget {
        snapshot.duration_us = wire::us(start).max(snapshot.scrape_us);
        snapshot.charged_us = snapshot.duration_us;
    }
    budget.retain(config, &snapshot)?;
    Ok(snapshot)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Identity {
    pub name: String,
    pub labels: BTreeMap<String, String>,
}
#[derive(Debug, Serialize)]
pub struct Series {
    #[serde(flatten)]
    pub identity: Identity,
    pub value: f64,
}
fn identifier(text: &str, metric: bool) -> bool {
    let mut bytes = text.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || (metric && b == b':'))
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || (metric && b == b':'))
}
fn labels<'a>(mut text: &'a str, config: &Config) -> Result<(BTreeMap<String, String>, &'a str)> {
    let original = text.len();
    let mut labels = BTreeMap::new();
    loop {
        text = text.trim_start_matches([' ', '\t']);
        if let Some(rest) = text.strip_prefix('}') {
            if original - rest.len() + 1 > config.label_bytes_per_series {
                return Err("metrics label bytes exceed bound".into());
            }
            return Ok((labels, rest));
        }
        let (name, rest) = text.split_once('=').ok_or("invalid metrics label")?;
        let name = name.trim_end_matches([' ', '\t']);
        if !identifier(name, false) || labels.len() >= config.labels_per_series {
            return Err("invalid or excessive metrics labels".into());
        }
        text = rest
            .trim_start_matches([' ', '\t'])
            .strip_prefix('"')
            .ok_or("metrics label must be quoted")?;
        let mut value = String::new();
        let mut chars = text.char_indices();
        let end = loop {
            let (index, c) = chars.next().ok_or("unterminated metrics label")?;
            match c {
                '"' => break index + 1,
                '\\' => value.push(match chars.next().map(|(_, c)| c) {
                    Some('n') => '\n',
                    Some('"') => '"',
                    Some('\\') => '\\',
                    _ => return Err("invalid metrics label escape".into()),
                }),
                c if c.is_control() => return Err("invalid metrics label control".into()),
                c => value.push(c),
            }
            if value.len() > config.label_bytes_per_series {
                return Err("metrics label bytes exceed bound".into());
            }
        };
        if labels.insert(name.to_owned(), value).is_some() {
            return Err("duplicate metrics label".into());
        }
        text = text[end..].trim_start_matches([' ', '\t']);
        if let Some(rest) = text.strip_prefix(',') {
            text = rest;
        } else if !text.starts_with('}') {
            return Err("invalid metrics label separator".into());
        }
    }
}
pub fn parse(raw: &[u8], config: &Config) -> Result<Vec<Series>> {
    if raw.len() > config.body_bytes {
        return Err("metrics body exceeds bound".into());
    }
    let text = std::str::from_utf8(raw).map_err(|_| "metrics body is not UTF-8")?;
    let mut seen = BTreeSet::new();
    let mut series = Vec::new();
    for line in text.lines() {
        if line.len() > config.line_bytes {
            return Err("metrics line exceeds bound".into());
        }
        let line = line.trim_matches([' ', '\t', '\r']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let end = line.find(['{', ' ', '\t']).unwrap_or(line.len());
        let name = &line[..end];
        if !NAMES.contains(&name) {
            continue;
        }
        if seen.len() >= config.series {
            return Err("metrics selected series exceed bound".into());
        }
        let (labels, value) = if let Some(rest) = line[end..].strip_prefix('{') {
            labels(rest, config)?
        } else {
            (BTreeMap::new(), &line[end..])
        };
        if !value.starts_with([' ', '\t']) {
            return Err("missing metrics sample separator".into());
        }
        let mut fields = value.split_whitespace();
        let value = fields
            .next()
            .ok_or("missing metrics sample value")?
            .parse::<f64>()
            .map_err(|_| "invalid metrics number")?;
        if !value.is_finite() {
            return Err("nonfinite metrics number".into());
        }
        if let Some(timestamp) = fields.next() {
            timestamp
                .parse::<i64>()
                .map_err(|_| "invalid metrics timestamp")?;
        }
        if fields.next().is_some() {
            return Err("unsupported metrics sample suffix".into());
        }
        let identity = Identity {
            name: name.into(),
            labels,
        };
        if !seen.insert(identity.clone()) {
            return Err("duplicate metrics series".into());
        }
        if NAMES[..2].contains(&name) && value < 0.0 {
            return Err("negative metrics counter".into());
        }
        series.push(Series { identity, value });
    }
    series.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(series)
}

#[derive(Serialize)]
pub struct CounterDelta {
    #[serde(flatten)]
    pub identity: Identity,
    pub before: Option<f64>,
    pub after: Option<f64>,
    pub delta: Option<f64>,
    pub status: &'static str,
}
#[derive(Serialize)]
pub struct Acceptance {
    pub labels: BTreeMap<String, String>,
    pub draft_delta: Option<f64>,
    pub accepted_delta: Option<f64>,
    pub ratio: Option<f64>,
    pub status: &'static str,
}
#[derive(Serialize)]
pub struct WaveSummary {
    pub receipt: Receipt,
    pub overhead_us: u64,
    pub before_series: Vec<Series>,
    pub after_series: Vec<Series>,
    pub counters: Vec<CounterDelta>,
    pub acceptance: Vec<Acceptance>,
}
#[derive(Serialize)]
pub struct Summary {
    pub config: Config,
    pub budget: Budget,
    pub waves: Vec<WaveSummary>,
}
pub fn publish(dir: &Path, receipt: &Receipt) -> Result<Reference> {
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|e| e.to_string())?;
    if bytes.len() > RECEIPT_CAP {
        return Err("metrics receipt exceeds bound".into());
    }
    evidence::write(&dir.join("metrics.json"), &bytes)?;
    evidence::sync(dir)?;
    Ok(Reference {
        file: "metrics.json".into(),
        sha256: evidence::digest(&bytes),
        overhead_us: 0,
    })
}
pub fn load(
    dir: &Path,
    reference: &Reference,
    config: &Config,
    plan_hash: &str,
    wave: u32,
    budget: &mut Budget,
) -> Result<WaveSummary> {
    if reference.file != "metrics.json" {
        return Err("invalid metrics companion path".into());
    }
    let bytes = evidence::read(&dir.join("metrics.json"), RECEIPT_CAP)?;
    if evidence::digest(&bytes) != reference.sha256 {
        return Err("metrics companion hash mismatch".into());
    }
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid metrics receipt: {e}"))?;
    if receipt.version != 1 || receipt.plan_sha256 != plan_hash || receipt.wave != wave {
        return Err("metrics companion lineage mismatch".into());
    }
    let snapshots_us = receipt
        .before
        .duration_us
        .checked_add(receipt.after.duration_us)
        .ok_or("metrics duration overflow")?;
    let mut parsed = Vec::with_capacity(2);
    for (side, snapshot) in [("before", &receipt.before), ("after", &receipt.after)] {
        budget.retain(config, snapshot)?;
        let raw = evidence::read(&dir.join(format!("metrics-{side}.bin")), config.body_bytes)?;
        if raw.len() != snapshot.raw_bytes || evidence::digest(&raw) != snapshot.raw_sha256 {
            return Err("metrics raw evidence hash mismatch".into());
        }
        let samples = parse(&raw, config);
        match snapshot.status {
            Status::Complete if snapshot.http_status == Some(200) => parsed.push(samples?),
            Status::ParseError if snapshot.http_status == Some(200) && samples.is_err() => {
                parsed.push(Vec::new())
            }
            Status::Complete | Status::ParseError => {
                return Err("metrics parser status contradicts raw evidence".into());
            }
            Status::HttpError if snapshot.http_status.is_none_or(|s| s == 200) => {
                return Err("invalid metrics HTTP failure".into());
            }
            Status::BodyLimit if raw.len() != config.body_bytes => {
                return Err("invalid metrics body limit".into());
            }
            _ => parsed.push(Vec::new()),
        }
    }
    budget.finish_wave(reference.overhead_us, snapshots_us)?;
    let after_series = parsed.pop().ok_or("missing after metrics")?;
    let before_series = parsed.pop().ok_or("missing before metrics")?;
    let before: BTreeMap<_, _> = before_series
        .iter()
        .map(|s| (&s.identity, s.value))
        .collect();
    let after: BTreeMap<_, _> = after_series
        .iter()
        .map(|s| (&s.identity, s.value))
        .collect();
    let identities: BTreeSet<_> = before
        .keys()
        .chain(after.keys())
        .copied()
        .filter(|id| NAMES[..2].contains(&id.name.as_str()))
        .collect();
    let counters: Vec<_> = identities
        .into_iter()
        .map(|identity| {
            let a = before.get(identity).copied();
            let b = after.get(identity).copied();
            let (delta, status) = match a.zip(b) {
                Some((a, b)) if b < a => (None, "reset_observed"),
                Some((a, b)) if (b - a).is_finite() => {
                    (Some(b - a), "nondecreasing_restart_unverified")
                }
                Some(_) => (None, "nonfinite_delta"),
                None => (None, "missing"),
            };
            CounterDelta {
                identity: identity.clone(),
                before: a,
                after: b,
                delta,
                status,
            }
        })
        .collect();
    let mut pairs = BTreeMap::new();
    for counter in &counters {
        let pair = pairs
            .entry(&counter.identity.labels)
            .or_insert((None, None));
        if counter.identity.name == NAMES[0] {
            pair.0 = counter.delta;
        } else {
            pair.1 = counter.delta;
        }
    }
    let acceptance = pairs
        .into_iter()
        .map(|(labels, (draft_delta, accepted_delta))| {
            let (ratio, status) = match draft_delta.zip(accepted_delta) {
                Some((draft, accepted)) if draft > 0.0 && accepted <= draft => (
                    Some(accepted / draft),
                    "compatible_deltas_restart_unverified",
                ),
                Some((0.0, _)) => (None, "zero_denominator"),
                Some(_) => (None, "inconsistent_counters"),
                None => (None, "missing_or_reset"),
            };
            Acceptance {
                labels: labels.clone(),
                draft_delta,
                accepted_delta,
                ratio,
                status,
            }
        })
        .collect();
    Ok(WaveSummary {
        receipt,
        overhead_us: reference.overhead_us,
        before_series,
        after_series,
        counters,
        acceptance,
    })
}
