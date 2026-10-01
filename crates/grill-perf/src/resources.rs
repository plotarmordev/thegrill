//! Bounded, explicitly selected resource observations. No discovery or service control.
//! Native observations are unauthenticated; imports never acquire native provenance.
use crate::{
    envelope::{self, Rational},
    evidence, metrics,
    model::Result,
    policy::Outcome,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

mod nvml;

const ARTIFACT_CAP: usize = 64 * 1024 * 1024;
const PLAN_CAP: usize = 128 * 1024;
const ADAPTER: &str = "linux-proc-cgroup-resource-v1";
const MACOS_ADAPTER: &str = "macos-libproc-sysctl-resource-v1";
// sizeof(struct rusage_info_v4) in <sys/resource.h>; asserted against libc on macOS.
const RUSAGE_INFO_V4_BYTES: usize = 296;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    pub id: String,
    pub kind: ClockKind,
    pub unit: ClockUnit,
    pub resolution_ns: u64,
    pub synchronization: Synchronization,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClockKind {
    LinuxMonotonic,
    SourceMonotonic,
    MacosUptimeRaw,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClockUnit {
    Microseconds,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Synchronization {
    LocalOrigin,
    ExplicitCommonOrigin,
    Unrelated,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    NativeObserved,
    Imported,
    OperatorDeclared,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ownership {
    ProcessAddressSpace,
    CgroupMembers,
    HostShared,
    DedicatedDevice,
    SharedMemory { group: String },
    Unknown,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Process {
        pid: u32,
    },
    CgroupV2 {
        path: PathBuf,
    },
    HostCpu,
    HostMemory,
    NvidiaMemory {
        uuid: String,
        rank: Option<u32>,
    },
    NvidiaPower {
        uuid: String,
        rank: Option<u32>,
    },
    Imported {
        adapter: String,
        device: Option<String>,
        rank: Option<u32>,
    },
    MacosProcess {
        pid: u32,
    },
    MacosHostMemory,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub target: Target,
    pub ownership: Ownership,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourcesConfig {
    pub version: u32,
    pub sources: Vec<Source>,
    pub cadence_us: u64,
    pub max_gap_us: u64,
    pub max_read_us: u64,
    pub max_samples: u32,
    pub deadline_us: u64,
    pub per_sample_bytes: u32,
    pub raw_total_bytes: u64,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    CpuTime,
    ResidentMemory,
    LifetimePeakMemory,
    MemoryUsed,
    MemoryFree,
    MemoryAllocated,
    MemoryReserved,
    MemoryLimit,
    KvUsed,
    KvCapacity,
    Power,
    MemoryPressureLevel,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Bytes,
    ClockTicks,
    Microseconds,
    Microwatts,
    DispatchMemorypressureLevel,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Missing,
    Permission,
    Io,
    Malformed,
    Overflow,
    ByteBudget,
    SampleBudget,
    Deadline,
    Cancelled,
    Gap,
    SourceChanged,
    CounterReset,
    Clock,
    IncompleteExposure,
    UnsupportedBoundary,
    UnsupportedMetric,
    Ownership,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Value {
    Observed { value: u64 },
    Unlimited,
    Unavailable { reason: Failure },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reading {
    pub metric: Metric,
    pub unit: Unit,
    pub value: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Raw {
    pub name: String,
    pub bytes: Vec<u8>,
    pub error: Option<Failure>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub source: String,
    pub clock: String,
    pub started_us: u64,
    pub observed_us: u64,
    pub incarnation: Option<String>,
    pub raw: Vec<Raw>,
    pub readings: Vec<Reading>,
    pub failure: Option<Failure>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeasuredInterval {
    pub clock: String,
    pub started_us: u64,
    pub settled_us: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub version: u32,
    pub kind: String,
    pub provenance: Provenance,
    pub adapter: String,
    pub binary_sha256: String,
    pub config: ResourcesConfig,
    pub clock: Clock,
    pub clk_tck: u64,
    pub observer_started_us: u64,
    pub observer_settled_us: u64,
    pub measured: MeasuredInterval,
    pub snapshots: Vec<Snapshot>,
    pub failures: Vec<Failure>,
    pub raw_bytes: u64,
    pub read_overhead_us: u64,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Statistic {
    SampledMaximum,
    CpuTime,
    CpuUtilizationOneCpu,
    SampledEnergyEstimate,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub source: String,
    pub metric: Metric,
    pub statistic: Statistic,
    pub max_regression_bps: u32,
    pub max_reference_spread_bps: u32,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    A,
    B,
    A2,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Warmup,
    Measured,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Arm {
    pub role: Role,
    pub deployment_pin: String,
    pub config: ResourcesConfig,
    pub warmup_ids: Vec<String>,
    pub measured_ids: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Study {
    pub version: u32,
    pub id: String,
    pub exposure_pin: String,
    pub collector_sha256: String,
    pub candidate_change: String,
    pub duration_us: u64,
    pub arms: Vec<Arm>,
    pub gates: Vec<Gate>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub version: u32,
    pub study_sha256: String,
    pub acquisition_id: String,
    pub role: Role,
    pub phase: Phase,
    pub index: u32,
    pub started_unix_ms: u64,
    pub settled_unix_ms: u64,
    pub observation: Observation,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    kind: String,
    sha256: String,
    provenance: Provenance,
    imported_sha256: Option<String>,
}
#[derive(Serialize)]
pub struct Summary {
    pub source: String,
    pub statistic: Statistic,
    pub metric: Metric,
    pub value: Option<Rational>,
    pub unit: &'static str,
    pub unavailable: Option<Failure>,
}

fn label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn number(s: &str) -> std::result::Result<u64, Failure> {
    s.parse::<u64>().map_err(|e| {
        if matches!(e.kind(), std::num::IntErrorKind::PosOverflow) {
            Failure::Overflow
        } else {
            Failure::Malformed
        }
    })
}
fn checked(n: Option<u64>) -> std::result::Result<u64, Failure> {
    n.ok_or(Failure::Overflow)
}

fn macos_target(target: &Target) -> bool {
    matches!(
        target,
        Target::MacosProcess { .. } | Target::MacosHostMemory
    )
}
// ResourcesConfig::validate admits no mix of macOS and Linux targets.
fn native_clock(config: &ResourcesConfig) -> ClockKind {
    if config.sources.iter().any(|s| macos_target(&s.target)) {
        ClockKind::MacosUptimeRaw
    } else {
        ClockKind::LinuxMonotonic
    }
}
fn native_adapter(config: &ResourcesConfig) -> &'static str {
    if native_clock(config) == ClockKind::MacosUptimeRaw {
        return MACOS_ADAPTER;
    }
    let gpu = config
        .sources
        .iter()
        .filter(|s| nvml::selected(&s.target).is_some())
        .count();
    if gpu == 0 {
        ADAPTER
    } else if gpu == config.sources.len() {
        nvml::ADAPTER
    } else {
        "linux-proc-cgroup-nvml-resource-v1"
    }
}

impl ResourcesConfig {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.sources.is_empty()
            || self.sources.len() > 16
            || !(1_000..=10_000_000).contains(&self.cadence_us)
            || self.max_gap_us < self.cadence_us
            || self.max_gap_us > 60_000_000
            || u64::from(self.max_samples) * self.sources.len() as u64 > 10_000
            || self.max_read_us == 0
            || self.max_read_us > self.max_gap_us
            || !(2..=10_000).contains(&self.max_samples)
            || self.deadline_us == 0
            || self.deadline_us > 3_600_000_000
            || self.per_sample_bytes == 0
            || self.per_sample_bytes > 1024 * 1024
            || self.raw_total_bytes == 0
            || self.raw_total_bytes > 8 * 1024 * 1024
        {
            return Err("invalid finite resource configuration".into());
        }
        let mut ids = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for source in &self.sources {
            if !label(&source.id)
                || !ids.insert(&source.id)
                || !targets
                    .insert(serde_json::to_string(&source.target).map_err(|e| e.to_string())?)
            {
                return Err("duplicate or invalid resource source".into());
            }
            let ownership_ok = match &source.target {
                Target::Process { pid } | Target::MacosProcess { pid } => {
                    *pid > 0
                        && *pid <= i32::MAX as u32
                        && source.ownership == Ownership::ProcessAddressSpace
                }
                Target::CgroupV2 { path } => {
                    path.starts_with("/sys/fs/cgroup")
                        && path.as_os_str().len() <= 4096
                        && path
                            .components()
                            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
                        && source.ownership == Ownership::CgroupMembers
                }
                Target::HostCpu | Target::HostMemory | Target::MacosHostMemory => {
                    source.ownership == Ownership::HostShared
                }
                Target::NvidiaMemory { uuid, .. } | Target::NvidiaPower { uuid, .. } => {
                    nvml::valid_uuid(uuid)
                        && match &source.ownership {
                            Ownership::DedicatedDevice | Ownership::Unknown => true,
                            Ownership::SharedMemory { group } => label(group),
                            _ => false,
                        }
                }
                Target::Imported {
                    adapter, device, ..
                } => {
                    label(adapter)
                        && device.as_ref().is_none_or(|s| label(s))
                        && match &source.ownership {
                            Ownership::SharedMemory { group } => label(group),
                            _ => true,
                        }
                }
            };
            if !ownership_ok {
                return Err("source scope or ownership mismatch".into());
            }
        }
        let macos = self
            .sources
            .iter()
            .filter(|s| macos_target(&s.target))
            .count();
        if macos != 0 && macos != self.sources.len() {
            return Err("macOS and Linux resource sources cannot share one observer".into());
        }
        Ok(())
    }
}

// Closed source parser; process comm can contain spaces and closing parentheses.
fn proc_stat(bytes: &[u8], expected_pid: u32) -> std::result::Result<(u64, u64), Failure> {
    let close = bytes
        .iter()
        .rposition(|b| *b == b')')
        .ok_or(Failure::Malformed)?;
    let open = bytes
        .iter()
        .position(|b| *b == b'(')
        .ok_or(Failure::Malformed)?;
    if open >= close {
        return Err(Failure::Malformed);
    }
    let pid = std::str::from_utf8(&bytes[..open])
        .map_err(|_| Failure::Malformed)?
        .trim();
    if number(pid)? != u64::from(expected_pid) {
        return Err(Failure::SourceChanged);
    }
    let fields: Vec<&str> = std::str::from_utf8(&bytes[close + 1..])
        .map_err(|_| Failure::Malformed)?
        .split_whitespace()
        .collect();
    if fields.len() < 20 {
        return Err(Failure::Malformed);
    }
    // fields[0] is kernel field 3 (state). utime already includes guest_time.
    let cpu = checked(number(fields[11])?.checked_add(number(fields[12])?))?;
    Ok((number(fields[19])?, cpu))
}
fn field(bytes: &[u8], name: &str, kb: bool) -> std::result::Result<u64, Failure> {
    let text = std::str::from_utf8(bytes).map_err(|_| Failure::Malformed)?;
    let mut found = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if parts.next() != Some(name) {
            continue;
        }
        if found.is_some() {
            return Err(Failure::Malformed);
        }
        let n = number(parts.next().ok_or(Failure::Malformed)?)?;
        if kb && parts.next() != Some("kB") || parts.next().is_some() {
            return Err(Failure::Malformed);
        }
        found = Some(if kb { checked(n.checked_mul(1024))? } else { n });
    }
    found.ok_or(Failure::Missing)
}
fn reading(metric: Metric, unit: Unit, value: std::result::Result<u64, Failure>) -> Reading {
    Reading {
        metric,
        unit,
        value: match value {
            Ok(value) => Value::Observed { value },
            Err(reason) => Value::Unavailable { reason },
        },
    }
}
fn raw_bytes<'a>(raw: &'a [Raw], name: &str) -> std::result::Result<&'a [u8], Failure> {
    let entry = raw
        .iter()
        .find(|r| r.name == name)
        .ok_or(Failure::Missing)?;
    if let Some(error) = entry.error {
        return Err(error);
    }
    Ok(&entry.bytes)
}
// Kernel integers as retained native bytes; every macOS target is little-endian.
fn little_endian(bytes: &[u8]) -> std::result::Result<u64, Failure> {
    match *bytes {
        [a, b, c, d] => Ok(u32::from_le_bytes([a, b, c, d]).into()),
        _ => bytes
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| Failure::Malformed),
    }
}
fn parse_macos(
    target: &Target,
    raw: &[Raw],
) -> std::result::Result<(Option<String>, Vec<Reading>), Failure> {
    let get = |name| raw_bytes(raw, name);
    if let Target::MacosProcess { pid } = target {
        let info = get("rusage_info_v4")?;
        if info.len() != RUSAGE_INFO_V4_BYTES {
            return Err(Failure::Malformed);
        }
        // rusage_info_v4 offsets: ri_phys_footprint 72, ri_proc_start_abstime 80,
        // ri_proc_exit_abstime 88, ri_lifetime_max_phys_footprint 240.
        let field = |offset: usize| little_endian(&info[offset..offset + 8]);
        let start = field(80)?;
        if start == 0 {
            return Err(Failure::Malformed);
        }
        // An exited, unreaped process keeps its PID and final accounting, not live memory.
        if field(88)? != 0 {
            return Err(Failure::Missing);
        }
        return Ok((
            Some(format!("pid:{pid}:start_abstime:{start}")),
            vec![
                reading(Metric::MemoryUsed, Unit::Bytes, field(72)),
                reading(Metric::LifetimePeakMemory, Unit::Bytes, field(240)),
            ],
        ));
    }
    let value = |name| get(name).and_then(little_endian);
    // vm.pagesize is the kernel page that counts free pages; Rosetta changes hw.pagesize.
    let page = value("vm.pagesize").and_then(|n| {
        if n.is_power_of_two() {
            Ok(n)
        } else {
            Err(Failure::Malformed)
        }
    });
    // DISPATCH_MEMORYPRESSURE_NORMAL, _WARN and _CRITICAL; nothing else has a meaning.
    let pressure = value("kern.memorystatus_vm_pressure_level").and_then(|n| {
        if matches!(n, 1 | 2 | 4) {
            Ok(n)
        } else {
            Err(Failure::Malformed)
        }
    });
    Ok((
        Some("host-memory".into()),
        vec![
            reading(
                Metric::MemoryFree,
                Unit::Bytes,
                value("vm.page_free_count").and_then(|free| checked(free.checked_mul(page?))),
            ),
            reading(Metric::MemoryLimit, Unit::Bytes, value("hw.memsize")),
            reading(
                Metric::MemoryPressureLevel,
                Unit::DispatchMemorypressureLevel,
                pressure,
            ),
        ],
    ))
}
fn expected_files(target: &Target) -> &'static [&'static str] {
    match target {
        Target::Process { .. } => &["stat", "status", "stat_after"],
        Target::CgroupV2 { .. } => &["memory.current", "memory.peak", "memory.max", "cpu.stat"],
        Target::HostCpu => &["stat"],
        Target::HostMemory => &["meminfo"],
        Target::NvidiaMemory { .. } | Target::NvidiaPower { .. } => &["nvml.json"],
        Target::Imported { .. } => &["readings.json"],
        Target::MacosProcess { .. } => &["rusage_info_v4"],
        Target::MacosHostMemory => &[
            "hw.memsize",
            "vm.pagesize",
            "vm.page_free_count",
            "kern.memorystatus_vm_pressure_level",
        ],
    }
}
fn parse_snapshot(
    source: &Source,
    raw: &[Raw],
    directory_identity: Option<&str>,
) -> std::result::Result<(Option<String>, Vec<Reading>), Failure> {
    let get = |name| raw_bytes(raw, name);
    let scalar = |name| {
        number(
            std::str::from_utf8(get(name)?)
                .map_err(|_| Failure::Malformed)?
                .trim(),
        )
    };
    match source.target {
        Target::Process { pid } => {
            let (start, cpu) = proc_stat(get("stat")?, pid)?;
            let (after, after_cpu) = proc_stat(get("stat_after")?, pid)?;
            if start != after {
                return Err(Failure::SourceChanged);
            }
            if after_cpu < cpu {
                return Err(Failure::CounterReset);
            }
            let memory = |name| field(get("status")?, name, true);
            Ok((
                Some(format!("pid:{pid}:start:{start}")),
                vec![
                    reading(Metric::CpuTime, Unit::ClockTicks, Ok(cpu)),
                    reading(Metric::ResidentMemory, Unit::Bytes, memory("VmRSS:")),
                    reading(Metric::LifetimePeakMemory, Unit::Bytes, memory("VmHWM:")),
                ],
            ))
        }
        Target::CgroupV2 { .. } => {
            let limit = match get("memory.max") {
                Ok(b) if b == b"max\n" || b == b"max" => Value::Unlimited,
                _ => reading(Metric::MemoryLimit, Unit::Bytes, scalar("memory.max")).value,
            };
            Ok((
                directory_identity.map(str::to_owned),
                vec![
                    reading(Metric::MemoryUsed, Unit::Bytes, scalar("memory.current")),
                    reading(
                        Metric::LifetimePeakMemory,
                        Unit::Bytes,
                        scalar("memory.peak"),
                    ),
                    Reading {
                        metric: Metric::MemoryLimit,
                        unit: Unit::Bytes,
                        value: limit,
                    },
                    reading(
                        Metric::CpuTime,
                        Unit::Microseconds,
                        get("cpu.stat").and_then(|b| field(b, "usage_usec", false)),
                    ),
                ],
            ))
        }
        Target::HostMemory => Ok((
            Some("host-memory".into()),
            vec![
                reading(
                    Metric::MemoryFree,
                    Unit::Bytes,
                    get("meminfo").and_then(|b| field(b, "MemFree:", true)),
                ),
                reading(
                    Metric::MemoryLimit,
                    Unit::Bytes,
                    get("meminfo").and_then(|b| field(b, "MemTotal:", true)),
                ),
            ],
        )),
        Target::HostCpu => {
            let bytes = get("stat")?;
            let text = std::str::from_utf8(bytes).map_err(|_| Failure::Malformed)?;
            let mut lines = text.lines().filter(|l| l.starts_with("cpu "));
            let fields: Vec<&str> = lines
                .next()
                .ok_or(Failure::Missing)?
                .split_whitespace()
                .skip(1)
                .collect();
            if lines.next().is_some() || fields.len() < 8 {
                return Err(Failure::Malformed);
            }
            // Busy CPU only: user/nice/system/irq/softirq/steal. Exclude idle/iowait;
            // guest and guest_nice (fields 8/9) already occur in user/nice.
            let cpu = [0, 1, 2, 5, 6, 7]
                .into_iter()
                .try_fold(0u64, |sum, i| checked(sum.checked_add(number(fields[i])?)))?;
            Ok((
                Some(format!("host-boot:{}", field(bytes, "btime", false)?)),
                vec![reading(Metric::CpuTime, Unit::ClockTicks, Ok(cpu))],
            ))
        }
        Target::NvidiaMemory { ref uuid, .. } => nvml::parse(get("nvml.json")?, uuid, true),
        Target::NvidiaPower { ref uuid, .. } => nvml::parse(get("nvml.json")?, uuid, false),
        Target::Imported { .. } => {
            let readings: Vec<Reading> =
                serde_json::from_slice(get("readings.json")?).map_err(|_| Failure::Malformed)?;
            Ok((directory_identity.map(str::to_owned), readings))
        }
        Target::MacosProcess { .. } | Target::MacosHostMemory => parse_macos(&source.target, raw),
    }
}

struct OpenSource {
    directory: Option<File>,
    identity: Option<(u64, u64)>,
    error: Option<Failure>,
}
fn io_failure(error: &std::io::Error) -> Failure {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => Failure::Permission,
        std::io::ErrorKind::NotFound => Failure::Missing,
        _ => Failure::Io,
    }
}
fn source_directory(target: &Target) -> PathBuf {
    match target {
        Target::Process { pid } => PathBuf::from(format!("/proc/{pid}")),
        Target::CgroupV2 { path } => path.clone(),
        _ => PathBuf::from("/proc"),
    }
}
fn open_source(source: &Source) -> OpenSource {
    if nvml::selected(&source.target).is_some() || macos_target(&source.target) {
        return OpenSource {
            directory: None,
            identity: None,
            error: None,
        };
    }
    let result = (|| {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(source_directory(&source.target))
            .map_err(|e| io_failure(&e))?;
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: live owned fd and correctly sized output storage.
        if unsafe { libc::fstatfs(directory.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(Failure::Io);
        }
        // SAFETY: fstatfs returned 0, so it initialized the buffer.
        let filesystem = unsafe { filesystem.assume_init() };
        let expected = if matches!(source.target, Target::CgroupV2 { .. }) {
            0x63677270
        } else {
            0x9fa0
        };
        if filesystem.f_type != expected {
            return Err(Failure::SourceChanged);
        }
        let meta = directory.metadata().map_err(|e| io_failure(&e))?;
        Ok((directory, (meta.dev(), meta.ino())))
    })();
    match result {
        Ok((directory, identity)) => OpenSource {
            directory: Some(directory),
            identity: Some(identity),
            error: None,
        },
        Err(error) => OpenSource {
            directory: None,
            identity: None,
            error: Some(error),
        },
    }
}
fn read_source(open: &OpenSource, name: &str, cap: usize) -> Raw {
    let mut raw = Raw {
        name: name.into(),
        bytes: Vec::new(),
        error: open.error,
    };
    let Some(directory) = &open.directory else {
        return raw;
    };
    if cap == 0 {
        raw.error = Some(Failure::ByteBudget);
        return raw;
    }
    // Names come only from expected_files, never from evidence-controlled paths.
    let filename = std::ffi::CString::new(if name == "stat_after" { "stat" } else { name })
        .expect("fixed filename");
    // SAFETY: the directory fd is live and `filename` is NUL-terminated.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            filename.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        raw.error = Some(io_failure(&std::io::Error::last_os_error()));
        return raw;
    }
    // SAFETY: `fd` is a fresh descriptor from openat that nothing else owns.
    let file = unsafe { File::from_raw_fd(fd) };
    match file.metadata() {
        Ok(meta) if meta.is_file() => {}
        _ => {
            raw.error = Some(Failure::Io);
            return raw;
        }
    }
    if let Err(error) = file.take(cap as u64 + 1).read_to_end(&mut raw.bytes) {
        raw.error = Some(io_failure(&error));
    }
    if raw.bytes.len() > cap {
        raw.bytes.truncate(cap);
        raw.error = Some(Failure::ByteBudget);
    }
    raw
}
#[cfg(target_os = "macos")]
fn macos_value(target: &Target, name: &str) -> std::result::Result<Vec<u8>, Failure> {
    const _: () = assert!(size_of::<libc::rusage_info_v4>() == RUSAGE_INFO_V4_BYTES);
    let failure = || {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Failure::Missing
        } else {
            io_failure(&error)
        }
    };
    if let Target::MacosProcess { pid } = target {
        let mut info = vec![0u8; RUSAGE_INFO_V4_BYTES];
        // SAFETY: validate() bounds pid to i32; the kernel writes one rusage_info_v4,
        // whose size is asserted above, into the owned buffer.
        let status = unsafe {
            libc::proc_pid_rusage(*pid as i32, libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast())
        };
        return if status == 0 {
            Ok(info)
        } else {
            Err(failure())
        };
    }
    let name = std::ffi::CString::new(name).expect("fixed sysctl name");
    let mut value = [0u8; 8];
    let mut len = value.len();
    // SAFETY: NUL-terminated fixed name; the kernel writes at most `len` bytes into the
    // owned buffer and no new value is supplied.
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return Err(failure());
    }
    Ok(value[..len].to_vec())
}
#[cfg(not(target_os = "macos"))]
fn macos_value(_: &Target, _: &str) -> std::result::Result<Vec<u8>, Failure> {
    Err(Failure::Missing)
}
// Names come only from expected_files; they are libproc/sysctl identities, not paths.
fn read_macos(target: &Target, name: &str, cap: usize) -> Raw {
    let mut raw = Raw {
        name: name.into(),
        bytes: Vec::new(),
        error: Some(Failure::ByteBudget),
    };
    if cap == 0 {
        return raw;
    }
    match macos_value(target, name) {
        Ok(bytes) => {
            raw.bytes = bytes;
            raw.error = None;
        }
        Err(error) => raw.error = Some(error),
    }
    if raw.bytes.len() > cap {
        raw.bytes.truncate(cap);
        raw.error = Some(Failure::ByteBudget);
    }
    raw
}
fn elapsed(origin: Instant) -> Result<u64> {
    u64::try_from(origin.elapsed().as_micros()).map_err(|_| "resource clock overflow".into())
}

/// One caller-owned synchronous observer. Main calls sample at its admitted cadence;
/// the origin MUST be the existing capture Instant, not a freshly created observer clock.
/// finish accepts the explicit first-required-start .. last-required-settlement interval.
pub struct Observer {
    origin: Instant,
    observation: Observation,
    opened: Vec<OpenSource>,
    cycles: u32,
    nvml: nvml::NvmlSource,
}
impl Observer {
    pub fn start(config: ResourcesConfig, origin: Instant, clock: Clock) -> Result<Self> {
        config.validate()?;
        if config
            .sources
            .iter()
            .any(|s| matches!(s.target, Target::Imported { .. }))
        {
            return Err("native observer cannot execute imported sources".into());
        }
        validate_clock(&clock)?;
        let host = host_clock(clock.id.clone())?;
        if clock.resolution_ns != host.resolution_ns {
            return Err("host resource clock resolution mismatch".into());
        }
        if native_clock(&config) != host.kind {
            return Err("selected native resource sources are not observable on this host".into());
        }
        if clock.kind != host.kind || clock.synchronization != Synchronization::LocalOrigin {
            return Err("host observer requires the shared local monotonic clock".into());
        }
        let observer_started_us = elapsed(origin)?;
        // SAFETY: sysconf/clock_getres are read-only host queries, not device probes.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if ticks <= 0 {
            return Err("CLK_TCK unavailable".into());
        }
        let opened = config.sources.iter().map(open_source).collect();
        Ok(Self {
            origin,
            opened,
            cycles: 0,
            nvml: nvml::NvmlSource::default(),
            observation: Observation {
                version: 1,
                kind: "resource-observation-v1".into(),
                provenance: Provenance::NativeObserved,
                adapter: native_adapter(&config).into(),
                binary_sha256: evidence::binary_digest()?,
                config,
                clock: clock.clone(),
                clk_tck: ticks as u64,
                observer_started_us,
                observer_settled_us: observer_started_us,
                measured: MeasuredInterval {
                    clock: clock.id,
                    started_us: observer_started_us,
                    settled_us: observer_started_us,
                },
                snapshots: Vec::new(),
                failures: Vec::new(),
                raw_bytes: 0,
                read_overhead_us: 0,
            },
        })
    }
    fn fail(&mut self, failure: Failure) {
        if !self.observation.failures.contains(&failure) {
            self.observation.failures.push(failure);
        }
    }
    /// Returns false after a finite budget expires; no replacement samples or retries.
    pub fn sample(&mut self) -> Result<bool> {
        let now = elapsed(self.origin)?;
        if self.cycles >= self.observation.config.max_samples {
            self.fail(Failure::SampleBudget);
            return Ok(false);
        }
        if now - self.observation.observer_started_us >= self.observation.config.deadline_us {
            self.fail(Failure::Deadline);
            return Ok(false);
        }
        self.cycles += 1;
        let mut cycle_bytes = 0u64;
        for i in 0..self.observation.config.sources.len() {
            let started_us = elapsed(self.origin)?;
            let source = &self.observation.config.sources[i];
            let open = &self.opened[i];
            let mut failure = open.error;
            // Holding the directory fd prevents path reuse from silently selecting a new cgroup.
            if matches!(source.target, Target::CgroupV2 { .. }) {
                match std::fs::symlink_metadata(source_directory(&source.target)) {
                    Ok(m)
                        if Some((m.dev(), m.ino())) == open.identity
                            && !m.file_type().is_symlink() => {}
                    Ok(_) => failure = Some(Failure::SourceChanged),
                    Err(e) => failure = Some(io_failure(&e)),
                }
            }
            let mut raw = Vec::new();
            for name in expected_files(&source.target) {
                let remaining = (u64::from(self.observation.config.per_sample_bytes) - cycle_bytes)
                    .min(self.observation.config.raw_total_bytes - self.observation.raw_bytes);
                let late = elapsed(self.origin)? - self.observation.observer_started_us
                    >= self.observation.config.deadline_us;
                let entry = if late {
                    Raw {
                        name: (*name).into(),
                        bytes: Vec::new(),
                        error: Some(Failure::Deadline),
                    }
                } else if let Some((uuid, memory)) = nvml::selected(&source.target) {
                    self.nvml.read(uuid, memory, remaining as usize)
                } else if macos_target(&source.target) {
                    read_macos(&source.target, name, remaining as usize)
                } else {
                    read_source(open, name, remaining as usize)
                };
                cycle_bytes += entry.bytes.len() as u64;
                self.observation.raw_bytes += entry.bytes.len() as u64;
                raw.push(entry);
            }
            let directory_identity = open
                .identity
                .map(|(device, inode)| format!("directory:{device}:{inode}"));
            let (incarnation, readings) =
                match parse_snapshot(source, &raw, directory_identity.as_deref()) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        failure = Some(error);
                        (None, Vec::new())
                    }
                };
            let observed_us = elapsed(self.origin)?;
            self.observation.read_overhead_us = self
                .observation
                .read_overhead_us
                .checked_add(observed_us - started_us)
                .ok_or("resource overhead overflow")?;
            if failure.is_none() && observed_us - started_us > self.observation.config.max_read_us {
                failure = Some(Failure::Gap);
            }
            self.observation.snapshots.push(Snapshot {
                source: source.id.clone(),
                clock: self.observation.clock.id.clone(),
                started_us,
                observed_us,
                incarnation,
                raw,
                readings,
                failure,
            });
        }
        if elapsed(self.origin)? - self.observation.observer_started_us
            > self.observation.config.deadline_us
        {
            self.fail(Failure::Deadline);
        }
        Ok(true)
    }
    pub fn finish(mut self, measured: MeasuredInterval, cancelled: bool) -> Result<Observation> {
        if cancelled {
            self.fail(Failure::Cancelled);
        }
        self.observation.observer_settled_us = elapsed(self.origin)?;
        self.observation.measured = measured;
        validate_observation(&self.observation)?;
        Ok(self.observation)
    }
    pub fn last_observed_us(&self) -> Option<u64> {
        self.observation.snapshots.last().map(|s| s.observed_us)
    }
    fn covered_end_us(&self) -> Option<u64> {
        // A complete cycle appends every source in time order. Its first completion
        // bounds the interval covered by all sources, not just the final reader.
        let first = self
            .observation
            .snapshots
            .len()
            .checked_sub(self.observation.config.sources.len())?;
        self.observation.snapshots.get(first).map(|s| s.observed_us)
    }
}

pub fn host_clock(id: String) -> Result<Clock> {
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = id;
        Err("native resource observation is supported on Linux and macOS only".into())
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        // The clock std::time::Instant reads, which is the observer origin's clock.
        #[cfg(target_os = "linux")]
        let (source, kind) = (libc::CLOCK_MONOTONIC, ClockKind::LinuxMonotonic);
        #[cfg(target_os = "macos")]
        let (source, kind) = (libc::CLOCK_UPTIME_RAW, ClockKind::MacosUptimeRaw);
        let mut resolution = std::mem::MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: clock_getres only writes into the owned timespec buffer.
        if unsafe { libc::clock_getres(source, resolution.as_mut_ptr()) } != 0 {
            return Err("monotonic resolution unavailable".into());
        }
        // SAFETY: clock_getres returned 0, so it initialized the buffer.
        let resolution = unsafe { resolution.assume_init() };
        let ns = u64::try_from(resolution.tv_sec)
            .ok()
            .and_then(|n| n.checked_mul(1_000_000_000))
            .and_then(|n| {
                u64::try_from(resolution.tv_nsec)
                    .ok()
                    .and_then(|r| n.checked_add(r))
            })
            .ok_or("monotonic resolution overflow")?;
        // Retained offsets are truncated to microseconds even when the host clock is finer.
        Ok(Clock {
            id,
            kind,
            unit: ClockUnit::Microseconds,
            resolution_ns: ns.max(1000),
            synchronization: Synchronization::LocalOrigin,
        })
    }
}
fn validate_clock(clock: &Clock) -> Result<()> {
    if !label(&clock.id) || clock.resolution_ns == 0 || clock.resolution_ns > 1_000_000_000 {
        return Err("invalid clock identity/resolution".into());
    }
    Ok(())
}
fn compatible_clocks(a: &Clock, b: &Clock) -> bool {
    a.kind == b.kind
        && a.unit == b.unit
        && a.resolution_ns == b.resolution_ns
        && a.synchronization == b.synchronization
        && a.synchronization != Synchronization::Unrelated
}
fn valid_reading(reading: &Reading, imported: bool) -> bool {
    let unit_ok = match reading.metric {
        Metric::CpuTime => matches!(reading.unit, Unit::ClockTicks | Unit::Microseconds),
        Metric::Power => reading.unit == Unit::Microwatts && imported,
        Metric::MemoryPressureLevel => reading.unit == Unit::DispatchMemorypressureLevel,
        _ => reading.unit == Unit::Bytes,
    };
    unit_ok && (!matches!(reading.value, Value::Unlimited) || reading.metric == Metric::MemoryLimit)
}
pub fn validate_observation(observation: &Observation) -> Result<()> {
    let o = observation;
    o.config.validate()?;
    validate_clock(&o.clock)?;
    if o.version != 1
        || o.kind != "resource-observation-v1"
        || !label(&o.adapter)
        || !evidence::is_digest(&o.binary_sha256)
        || o.clk_tck == 0
        || o.clk_tck > 1_000_000_000
        || o.observer_started_us > o.observer_settled_us
        || o.measured.clock != o.clock.id
        || o.measured.started_us > o.measured.settled_us
        || o.measured.started_us < o.observer_started_us
        || o.measured.settled_us > o.observer_settled_us
        || o.failures.len() > 16
        || o.snapshots.len() > o.config.max_samples as usize * o.config.sources.len()
        || !o.snapshots.len().is_multiple_of(o.config.sources.len())
    {
        return Err("invalid resource observation identity/clock/bounds".into());
    }
    if o.provenance == Provenance::NativeObserved
        && (o.adapter != native_adapter(&o.config)
            || o.clock.kind != native_clock(&o.config)
            || o.clock.synchronization != Synchronization::LocalOrigin
            || o.config
                .sources
                .iter()
                .any(|s| matches!(s.target, Target::Imported { .. })))
    {
        return Err("native resource source/clock mismatch".into());
    }
    let mut raw_bytes = 0u64;
    let mut overhead = 0u64;
    let mut last = o.observer_started_us;
    for cycle in o.snapshots.chunks(o.config.sources.len()) {
        let mut cycle_bytes = 0u64;
        for (snapshot, source) in cycle.iter().zip(&o.config.sources) {
            if snapshot.source != source.id
                || snapshot.clock != o.clock.id
                || snapshot.started_us < last
                || snapshot.observed_us < snapshot.started_us
                || snapshot.observed_us > o.observer_settled_us
                || snapshot.incarnation.as_ref().is_some_and(|s| !label(s))
                || snapshot.raw.len() != expected_files(&source.target).len()
                || snapshot.readings.len() > 12
            {
                return Err("invalid resource sample membership/clock/bounds".into());
            }
            for (raw, expected) in snapshot.raw.iter().zip(expected_files(&source.target)) {
                if raw.name != *expected {
                    return Err("resource raw source list mismatch".into());
                }
                cycle_bytes = cycle_bytes
                    .checked_add(raw.bytes.len() as u64)
                    .ok_or("resource raw overflow")?;
            }
            let mut metrics = BTreeSet::new();
            if snapshot.readings.iter().any(|r| {
                !metrics.insert(r.metric)
                    || !valid_reading(
                        r,
                        matches!(
                            source.target,
                            Target::Imported { .. } | Target::NvidiaPower { .. }
                        ),
                    )
            }) {
                return Err("duplicate resource metric or fabricated unit".into());
            }
            match parse_snapshot(source, &snapshot.raw, snapshot.incarnation.as_deref()) {
                Ok((identity, readings))
                    if identity == snapshot.incarnation && readings == snapshot.readings => {}
                Err(error)
                    if snapshot.failure == Some(error)
                        && snapshot.readings.is_empty()
                        && snapshot.incarnation.is_none() => {}
                _ => return Err("resource raw replay mismatch".into()),
            }
            overhead = overhead
                .checked_add(snapshot.observed_us - snapshot.started_us)
                .ok_or("resource overhead overflow")?;
            last = snapshot.observed_us;
        }
        if cycle_bytes > u64::from(o.config.per_sample_bytes) {
            return Err("resource sample byte budget exceeded".into());
        }
        raw_bytes = raw_bytes
            .checked_add(cycle_bytes)
            .ok_or("resource byte count overflow")?;
    }
    if raw_bytes != o.raw_bytes
        || raw_bytes > o.config.raw_total_bytes
        || overhead != o.read_overhead_us
    {
        return Err("resource retained byte/overhead mismatch".into());
    }
    Ok(())
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
fn rational(n: u128, d: u128) -> std::result::Result<Rational, Failure> {
    if d == 0 {
        return Err(Failure::Overflow);
    }
    let g = gcd(n, d);
    Ok(Rational {
        numerator: u64::try_from(n / g).map_err(|_| Failure::Overflow)?,
        denominator: u64::try_from(d / g).map_err(|_| Failure::Overflow)?,
    })
}
fn add(a: Rational, b: Rational) -> std::result::Result<Rational, Failure> {
    let g = gcd(u128::from(a.denominator), u128::from(b.denominator));
    let x = u128::from(b.denominator) / g;
    let y = u128::from(a.denominator) / g;
    rational(
        u128::from(a.numerator)
            .checked_mul(x)
            .and_then(|n| {
                u128::from(b.numerator)
                    .checked_mul(y)
                    .and_then(|m| n.checked_add(m))
            })
            .ok_or(Failure::Overflow)?,
        u128::from(a.denominator)
            .checked_mul(x)
            .ok_or(Failure::Overflow)?,
    )
}
fn numeric(value: &Value) -> std::result::Result<u64, Failure> {
    match value {
        Value::Observed { value } => Ok(*value),
        Value::Unavailable { reason } => Err(*reason),
        Value::Unlimited => Err(Failure::UnsupportedMetric),
    }
}
fn gate_supported(gate: &Gate) -> bool {
    match gate.statistic {
        Statistic::SampledMaximum => matches!(
            gate.metric,
            Metric::ResidentMemory
                | Metric::MemoryUsed
                | Metric::MemoryAllocated
                | Metric::MemoryReserved
                | Metric::KvUsed
                | Metric::Power
        ),
        Statistic::CpuTime | Statistic::CpuUtilizationOneCpu => gate.metric == Metric::CpuTime,
        Statistic::SampledEnergyEstimate => gate.metric == Metric::Power,
    }
}
fn source_supports(source: &Source, metric: Metric) -> bool {
    match source.target {
        Target::Process { .. } => matches!(
            metric,
            Metric::CpuTime | Metric::ResidentMemory | Metric::LifetimePeakMemory
        ),
        Target::CgroupV2 { .. } => matches!(
            metric,
            Metric::CpuTime | Metric::MemoryUsed | Metric::LifetimePeakMemory | Metric::MemoryLimit
        ),
        Target::HostCpu => metric == Metric::CpuTime,
        Target::HostMemory => matches!(metric, Metric::MemoryFree | Metric::MemoryLimit),
        Target::NvidiaMemory { .. } => matches!(
            metric,
            Metric::MemoryUsed | Metric::MemoryFree | Metric::MemoryLimit
        ),
        Target::NvidiaPower { .. } => metric == Metric::Power,
        Target::Imported { .. } => true,
        Target::MacosProcess { .. } => {
            matches!(metric, Metric::MemoryUsed | Metric::LifetimePeakMemory)
        }
        Target::MacosHostMemory => matches!(
            metric,
            Metric::MemoryFree | Metric::MemoryLimit | Metric::MemoryPressureLevel
        ),
    }
}
#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
fn summary_value(o: &Observation, gate: &Gate) -> std::result::Result<Rational, Failure> {
    if !gate_supported(gate) {
        return Err(Failure::UnsupportedMetric);
    }
    if o.provenance == Provenance::OperatorDeclared {
        return Err(Failure::IncompleteExposure);
    }
    if let Some(failure) = o.failures.first() {
        return Err(*failure);
    }
    if o.clock.synchronization == Synchronization::Unrelated {
        return Err(Failure::Clock);
    }
    if o.observer_settled_us - o.observer_started_us > o.config.deadline_us {
        return Err(Failure::Deadline);
    }
    let source = o
        .config
        .sources
        .iter()
        .find(|s| s.id == gate.source)
        .ok_or(Failure::Missing)?;
    if source.ownership == Ownership::Unknown {
        return Err(Failure::Ownership);
    }
    // No aggregation across scopes, ranks, unified device/host memory or shared owners.
    let snapshots: Vec<&Snapshot> = o
        .snapshots
        .iter()
        .filter(|s| s.source == gate.source)
        .collect();
    let start = o.measured.started_us;
    let end = o.measured.settled_us;
    if start >= end || snapshots.len() < 2 {
        return Err(Failure::IncompleteExposure);
    }
    if snapshots[0].observed_us > start
        || snapshots.last().ok_or(Failure::Missing)?.observed_us < end
    {
        return Err(Failure::IncompleteExposure);
    }
    if nvml::selected(&source.target).is_some()
        && let Some(failure) = snapshots.iter().find_map(|s| s.failure)
    {
        return Err(failure);
    }
    let identity = snapshots[0]
        .incarnation
        .as_ref()
        .ok_or(Failure::SourceChanged)?;
    let mut values = Vec::with_capacity(snapshots.len());
    let mut previous: Option<(&Snapshot, u64)> = None;
    let mut unit = None;
    for snapshot in snapshots {
        if let Some(failure) = snapshot.failure {
            return Err(failure);
        }
        if snapshot.clock != o.clock.id {
            return Err(Failure::Clock);
        }
        if snapshot.incarnation.as_ref() != Some(identity) {
            return Err(Failure::SourceChanged);
        }
        if snapshot.observed_us - snapshot.started_us > o.config.max_read_us {
            return Err(Failure::Gap);
        }
        if let Some(error) = snapshot.raw.iter().find_map(|r| r.error) {
            return Err(error);
        }
        if let Some(reason) = snapshot.readings.iter().find_map(|r| match r.value {
            Value::Unavailable { reason } => Some(reason),
            _ => None,
        }) {
            return Err(reason);
        }
        let reading = snapshot
            .readings
            .iter()
            .find(|r| r.metric == gate.metric)
            .ok_or(Failure::Missing)?;
        if unit.is_some_and(|u| u != reading.unit) {
            return Err(Failure::SourceChanged);
        }
        unit = Some(reading.unit);
        let value = numeric(&reading.value)?;
        if let Some((old, old_value)) = previous {
            let gap = snapshot
                .observed_us
                .checked_sub(old.observed_us)
                .ok_or(Failure::Clock)?;
            if gap == 0 || gap > o.config.max_gap_us {
                return Err(Failure::Gap);
            }
            if gate.metric == Metric::CpuTime && value < old_value {
                return Err(Failure::CounterReset);
            }
        }
        // Resets of the same source's cumulative CPU or lifetime high-water mark
        // also invalidate an otherwise plausible memory/power observation.
        if let Some((old, _)) = previous {
            for metric in [Metric::CpuTime, Metric::LifetimePeakMemory] {
                let counter = |s: &Snapshot| {
                    s.readings
                        .iter()
                        .find(|r| r.metric == metric)
                        .and_then(|r| numeric(&r.value).ok())
                };
                if matches!((counter(old), counter(snapshot)), (Some(a), Some(b)) if b < a) {
                    return Err(Failure::CounterReset);
                }
            }
        }
        values.push((snapshot.observed_us, value));
        previous = Some((snapshot, value));
    }
    match gate.statistic {
        Statistic::SampledMaximum => values
            .iter()
            .filter(|(t, _)| *t >= start && *t <= end)
            .map(|(_, v)| *v)
            .max()
            .map(|n| Rational::from((n, 1)))
            .ok_or(Failure::IncompleteExposure),
        Statistic::CpuTime | Statistic::CpuUtilizationOneCpu => {
            // No interpolation of cumulative CPU counters at unknown measured boundaries.
            let first = values
                .iter()
                .find(|(t, _)| *t == start)
                .ok_or(Failure::UnsupportedBoundary)?
                .1;
            let last = values
                .iter()
                .find(|(t, _)| *t == end)
                .ok_or(Failure::UnsupportedBoundary)?
                .1;
            let delta = last.checked_sub(first).ok_or(Failure::CounterReset)?;
            let scale = match unit {
                Some(Unit::ClockTicks) => o.clk_tck,
                Some(Unit::Microseconds) => 1_000_000,
                _ => return Err(Failure::UnsupportedMetric),
            };
            if gate.statistic == Statistic::CpuTime {
                rational(u128::from(delta) * 1_000_000, u128::from(scale))
            } else {
                rational(
                    u128::from(delta) * 1_000_000,
                    u128::from(scale) * u128::from(end - start),
                )
            }
        }
        Statistic::SampledEnergyEstimate => {
            if unit != Some(Unit::Microwatts) {
                return Err(Failure::UnsupportedMetric);
            }
            let mut energy = Rational::from((0, 1));
            for pair in values.windows(2) {
                let [(t0, p0), (t1, p1)] = [pair[0], pair[1]];
                let left = t0.max(start);
                let right = t1.min(end);
                if right <= left {
                    continue;
                }
                let dt = u128::from(t1 - t0);
                let offsets = u128::from(left - t0) + u128::from(right - t0);
                // Clipped trapezoid of the piecewise-linear power curve, in microjoules.
                // All positive weights; no signed cancellation or floating arithmetic.
                let weighted = u128::from(p0)
                    .checked_mul(2 * dt - offsets)
                    .and_then(|n| {
                        u128::from(p1)
                            .checked_mul(offsets)
                            .and_then(|m| n.checked_add(m))
                    })
                    .ok_or(Failure::Overflow)?;
                let n = weighted
                    .checked_mul(u128::from(right - left))
                    .ok_or(Failure::Overflow)?;
                let d = dt.checked_mul(2_000_000).ok_or(Failure::Overflow)?;
                energy = add(energy, rational(n, d)?)?;
            }
            Ok(energy)
        }
    }
}
pub fn summarize(observation: &Observation, gate: &Gate) -> Summary {
    let unit = match gate.statistic {
        Statistic::CpuTime => "microseconds_cpu_time",
        Statistic::CpuUtilizationOneCpu => "logical_cpu_ratio",
        Statistic::SampledEnergyEstimate => "microjoules_sampled_trapezoidal_estimate",
        Statistic::SampledMaximum if gate.metric == Metric::Power => "microwatts_sampled_maximum",
        Statistic::SampledMaximum => "bytes_sampled_maximum",
    };
    let value = if validate_observation(observation).is_ok() {
        summary_value(observation, gate)
    } else {
        Err(Failure::Malformed)
    };
    Summary {
        source: gate.source.clone(),
        statistic: gate.statistic,
        metric: gate.metric,
        value: value.as_ref().ok().copied(),
        unit,
        unavailable: value.err(),
    }
}

fn compatible_configs(a: &ResourcesConfig, b: &ResourcesConfig) -> bool {
    let normalize = |c: &ResourcesConfig| {
        let mut c = c.clone();
        for source in &mut c.sources {
            match &mut source.target {
                Target::Process { pid } | Target::MacosProcess { pid } => *pid = 1,
                Target::CgroupV2 { path } => *path = PathBuf::from("/sys/fs/cgroup"),
                _ => {}
            }
        }
        c
    };
    normalize(a) == normalize(b)
}
impl Study {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || !label(&self.id)
            || !evidence::is_digest(&self.exposure_pin)
            || !evidence::is_digest(&self.collector_sha256)
            || !label(&self.candidate_change)
            || self.duration_us == 0
            || self.duration_us > 3_600_000_000
            || self.arms.len() != 3
            || self.gates.is_empty()
            || self.gates.len() > 32
        {
            return Err("invalid resource study contract".into());
        }
        let mut ids = BTreeSet::new();
        for (arm, role) in self.arms.iter().zip([Role::A, Role::B, Role::A2]) {
            arm.config.validate()?;
            if arm.role != role
                || !evidence::is_digest(&arm.deployment_pin)
                || arm.warmup_ids.is_empty()
                || arm.warmup_ids.len() > 20
                || !(3..=1000).contains(&arm.measured_ids.len())
                || self
                    .duration_us
                    .checked_add(arm.config.max_gap_us)
                    .is_none_or(|n| n >= arm.config.deadline_us)
                || self.duration_us / arm.config.cadence_us + 2 > u64::from(arm.config.max_samples)
                || !compatible_configs(&self.arms[0].config, &arm.config)
                || arm.warmup_ids.len() != self.arms[0].warmup_ids.len()
                || arm.measured_ids.len() != self.arms[0].measured_ids.len()
            {
                return Err("resource arm count/exposure/source/budget mismatch".into());
            }
            for id in arm.warmup_ids.iter().chain(&arm.measured_ids) {
                if !label(id) || !ids.insert(id) {
                    return Err("duplicate/invalid acquisition identity".into());
                }
            }
        }
        if self.arms[0].deployment_pin != self.arms[2].deployment_pin {
            return Err("resource A/A2 deployment pin mismatch".into());
        }
        let mut gates = BTreeSet::new();
        for gate in &self.gates {
            if !gate_supported(gate)
                || !self.arms[0]
                    .config
                    .sources
                    .iter()
                    .any(|s| s.id == gate.source && source_supports(s, gate.metric))
                || gate.max_regression_bps > 9999
                || gate.max_reference_spread_bps > 1_000_000
                || !gates.insert(
                    serde_json::to_string(&(gate.source.as_str(), gate.metric, gate.statistic))
                        .map_err(|e| e.to_string())?,
                )
            {
                return Err("unsupported/duplicate resource gate".into());
            }
        }
        Ok(())
    }
    fn arm(&self, role: Role) -> &Arm {
        &self.arms[match role {
            Role::A => 0,
            Role::B => 1,
            Role::A2 => 2,
        }]
    }
    fn acquisition(&self, role: Role, phase: Phase, index: u32) -> Result<&str> {
        let arm = self.arm(role);
        let ids = match phase {
            Phase::Warmup => &arm.warmup_ids,
            Phase::Measured => &arm.measured_ids,
        };
        ids.get(index as usize)
            .map(String::as_str)
            .ok_or("acquisition index outside prospective plan".into())
    }
}
fn study(path: &Path) -> Result<(Study, String)> {
    let bytes = evidence::read(path, PLAN_CAP)?;
    let study: Study = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    study.validate()?;
    Ok((study, evidence::digest(&bytes)))
}
fn save(root: &Path, capture: &Capture, imported: Option<&[u8]>) -> Result<()> {
    evidence::fresh(root)?;
    publish_capture(root, capture, imported)
}
fn publish_capture(root: &Path, capture: &Capture, imported: Option<&[u8]>) -> Result<()> {
    let bytes = serde_json::to_vec(capture).map_err(|e| e.to_string())?;
    if bytes.len() > ARTIFACT_CAP {
        return Err("resource artifact exceeds bound".into());
    }
    evidence::write(&root.join("observation.json"), &bytes)?;
    if let Some(bytes) = imported {
        evidence::write(&root.join("imported.json"), bytes)?;
    }
    evidence::publish(
        root,
        "resource.json",
        &Receipt {
            version: 1,
            kind: "resource-capture-v1".into(),
            sha256: evidence::digest(&bytes),
            provenance: capture.observation.provenance,
            imported_sha256: imported.map(evidence::digest),
        },
    )?;
    evidence::sync(
        root.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
}
pub fn load(root: &Path) -> Result<Capture> {
    evidence::directory(root)?;
    let receipt: Receipt =
        serde_json::from_slice(&evidence::read(&root.join("resource.json"), PLAN_CAP)?)
            .map_err(|e| e.to_string())?;
    let bytes = evidence::read(&root.join("observation.json"), ARTIFACT_CAP)?;
    if receipt.version != 1
        || receipt.kind != "resource-capture-v1"
        || receipt.sha256 != evidence::digest(&bytes)
    {
        return Err("resource receipt digest/version mismatch".into());
    }
    let capture: Capture = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    validate_capture(&capture)?;
    if capture.observation.provenance != receipt.provenance {
        return Err("resource provenance mismatch".into());
    }
    match (receipt.provenance, receipt.imported_sha256) {
        (Provenance::Imported, Some(hash)) => {
            let original = evidence::read(&root.join("imported.json"), ARTIFACT_CAP)?;
            if evidence::digest(&original) != hash {
                return Err("imported resource hash mismatch".into());
            }
            let mut imported: Capture =
                serde_json::from_slice(&original).map_err(|e| e.to_string())?;
            imported.observation.provenance = Provenance::Imported;
            if imported != capture {
                return Err("imported resource provenance projection mismatch".into());
            }
        }
        (Provenance::NativeObserved, None) => {
            let (plan, hash) = study(&root.join("study.json"))?;
            if hash != capture.study_sha256
                || plan.collector_sha256 != capture.observation.binary_sha256
                || plan.arm(capture.role).config != capture.observation.config
                || plan.acquisition(capture.role, capture.phase, capture.index)?
                    != capture.acquisition_id
            {
                return Err("native resource retained study mismatch".into());
            }
        }
        (Provenance::OperatorDeclared, None) => {}
        _ => return Err("resource import provenance missing".into()),
    }
    Ok(capture)
}
fn validate_capture(capture: &Capture) -> Result<()> {
    if capture.version != 1
        || !evidence::is_digest(&capture.study_sha256)
        || !label(&capture.acquisition_id)
        || capture.index >= 1000
        || capture.started_unix_ms > capture.settled_unix_ms
    {
        return Err("invalid resource capture identity".into());
    }
    validate_observation(&capture.observation)
}
pub fn import(input: &Path, out: &Path) -> Result<Capture> {
    let bytes = evidence::read(input, ARTIFACT_CAP)?;
    let mut capture: Capture = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    // Override before validation: a file claiming to be native is still just imported bytes.
    capture.observation.provenance = Provenance::Imported;
    validate_capture(&capture)?;
    save(out, &capture, Some(&bytes))?;
    Ok(capture)
}

pub fn capture(plan: &Path, role: Role, phase: Phase, index: u32, out: &Path) -> Result<Capture> {
    let plan_bytes = evidence::read(plan, PLAN_CAP)?;
    let study: Study = serde_json::from_slice(&plan_bytes).map_err(|e| e.to_string())?;
    study.validate()?;
    let study_sha256 = evidence::digest(&plan_bytes);
    if evidence::binary_digest()? != study.collector_sha256 {
        return Err("resource collector binary pin mismatch".into());
    }
    let acquisition_id = study.acquisition(role, phase, index)?.to_owned();
    let config = study.arm(role).config.clone();
    if config
        .sources
        .iter()
        .any(|s| matches!(s.target, Target::Imported { .. }))
    {
        return Err("native capture cannot execute imported sources; use resource import".into());
    }
    evidence::fresh(out)?;
    evidence::write(&out.join("study.json"), &plan_bytes)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let started_unix_ms = metrics::unix_ms_checked()?;
    let clock = host_clock(acquisition_id.clone())?;
    let origin = Instant::now();
    let mut observer = Observer::start(config.clone(), origin, clock.clone())?;
    observer.sample()?;
    let start = observer
        .last_observed_us()
        .ok_or("resource initial sample missing")?;
    let target = start
        .checked_add(study.duration_us)
        .ok_or("resource interval overflow")?;
    let cancelled = runtime.block_on(async {
        let signal = tokio::signal::ctrl_c();
        tokio::pin!(signal);
        let mut cancelled = false;
        loop {
            let now = elapsed(origin)?;
            if now >= target { observer.sample()?; break; }
            let wake = now.checked_add(config.cadence_us).ok_or("resource cadence overflow")?.min(target);
            tokio::select! {
                result = &mut signal => { result.map_err(|e| e.to_string())?; cancelled = true; break; }
                _ = tokio::time::sleep(Duration::from_micros(wake - now)) => {}
            }
            if !observer.sample()? { break; }
            if observer.covered_end_us().is_some_and(|t| t >= target) { break; }
        }
        Ok::<bool, String>(cancelled)
    })?;
    let end = observer.covered_end_us().unwrap_or(start).max(start);
    let observation = observer.finish(
        MeasuredInterval {
            clock: clock.id,
            started_us: start,
            settled_us: end,
        },
        cancelled,
    )?;
    let capture = Capture {
        version: 1,
        study_sha256,
        acquisition_id,
        role,
        phase,
        index,
        started_unix_ms,
        settled_unix_ms: metrics::unix_ms_checked()?,
        observation,
    };
    publish_capture(out, &capture, None)?;
    Ok(capture)
}

#[derive(Serialize)]
pub struct GateDecision {
    pub gate: Gate,
    pub decision: Outcome,
    pub counts: [usize; 3],
    pub warmup_counts: [usize; 3],
    pub reference: Option<[Rational; 2]>,
    pub candidate: Option<[Rational; 2]>,
    pub adverse_bounds: Option<[f64; 2]>,
    pub reasons: Vec<String>,
}
#[derive(Serialize)]
pub struct Decision {
    pub version: u32,
    pub kind: &'static str,
    pub claim: &'static str,
    pub decision: Outcome,
    pub gates: Vec<GateDecision>,
    pub errors: Vec<String>,
}
fn failure_outcome(failure: Failure) -> Outcome {
    if matches!(
        failure,
        Failure::Overflow | Failure::Malformed | Failure::Clock
    ) {
        Outcome::Error
    } else {
        Outcome::Inconclusive
    }
}
#[expect(clippy::too_many_lines, reason = "predates the function-length limit")]
fn compare_inner(plan: &Path, groups: [&[PathBuf]; 3], result: &mut Decision) -> Result<()> {
    let (study, hash) = study(plan)?;
    result.gates = study
        .gates
        .iter()
        .map(|gate| GateDecision {
            gate: gate.clone(),
            decision: Outcome::Pass,
            counts: [0; 3],
            warmup_counts: [0; 3],
            reference: None,
            candidate: None,
            adverse_bounds: None,
            reasons: Vec::new(),
        })
        .collect();
    let mut samples: [Vec<Vec<Rational>>; 3] =
        std::array::from_fn(|_| vec![Vec::new(); study.gates.len()]);
    let mut first_clock: Option<Clock> = None;
    let mut binary: Option<String> = None;
    let mut last_unix = 0;
    let mut provenance = None;
    let mut gpu_identities: Vec<Option<String>> = vec![None; study.gates.len()];
    for (arm_index, (arm, roots)) in study.arms.iter().zip(groups).enumerate() {
        let expected = arm.warmup_ids.len() + arm.measured_ids.len();
        if roots.len() > expected {
            return Err("extra resource acquisitions are not replacements".into());
        }
        if roots.len() != expected {
            for gate in &mut result.gates {
                gate.decision = gate.decision.max(Outcome::Inconclusive);
                gate.reasons
                    .push(format!("{:?}: missing declared acquisitions", arm.role));
            }
        }
        for (ordinal, root) in roots.iter().enumerate() {
            let capture = load(root)?;
            let (phase, index) = if ordinal < arm.warmup_ids.len() {
                (Phase::Warmup, ordinal)
            } else {
                (Phase::Measured, ordinal - arm.warmup_ids.len())
            };
            let o = &capture.observation;
            if capture.study_sha256 != hash
                || capture.role != arm.role
                || capture.phase != phase
                || capture.index as usize != index
                || capture.acquisition_id != study.acquisition(arm.role, phase, index as u32)?
                || o.config != arm.config
                || o.binary_sha256 != study.collector_sha256
                || capture.started_unix_ms < last_unix
                || capture.settled_unix_ms < capture.started_unix_ms
            {
                return Err("resource acquisition pin/order/exposure mismatch".into());
            }
            // Unix timestamps establish declared acquisition ordering only, never durations.
            last_unix = capture.settled_unix_ms;
            if first_clock
                .as_ref()
                .is_some_and(|clock| !compatible_clocks(clock, &o.clock))
                || binary.as_ref().is_some_and(|pin| pin != &o.binary_sha256)
                || provenance.is_some_and(|p| p != o.provenance)
            {
                return Err("resource clock/collector/provenance contracts differ".into());
            }
            first_clock.get_or_insert_with(|| o.clock.clone());
            binary.get_or_insert_with(|| o.binary_sha256.clone());
            provenance.get_or_insert(o.provenance);
            let duration = o.measured.settled_us - o.measured.started_us;
            let duration_ok = duration >= study.duration_us
                && duration
                    <= study
                        .duration_us
                        .checked_add(o.config.max_gap_us)
                        .ok_or("resource exposure overflow")?;
            for (gate_index, gate) in result.gates.iter_mut().enumerate() {
                let mut value = if duration_ok {
                    summary_value(o, &gate.gate)
                } else {
                    Err(Failure::IncompleteExposure)
                };
                if value.is_ok()
                    && arm
                        .config
                        .sources
                        .iter()
                        .any(|s| s.id == gate.gate.source && nvml::selected(&s.target).is_some())
                {
                    let identity = o
                        .snapshots
                        .iter()
                        .find(|s| s.source == gate.gate.source)
                        .and_then(|s| s.incarnation.as_ref())
                        .ok_or("NVML source identity missing")?;
                    if gpu_identities[gate_index]
                        .as_ref()
                        .is_some_and(|old| old != identity)
                    {
                        value = Err(Failure::SourceChanged);
                    } else {
                        gpu_identities[gate_index].get_or_insert_with(|| identity.clone());
                    }
                }
                match value {
                    Ok(value) if phase == Phase::Measured => {
                        samples[arm_index][gate_index].push(value);
                        gate.counts[arm_index] += 1;
                    }
                    Ok(_) => gate.warmup_counts[arm_index] += 1,
                    Err(reason) => {
                        gate.decision = gate.decision.max(failure_outcome(reason));
                        gate.reasons
                            .push(format!("{}: {reason:?}", capture.acquisition_id));
                    }
                }
            }
        }
    }
    result.claim = if provenance == Some(Provenance::NativeObserved) {
        if study.arms[0]
            .config
            .sources
            .iter()
            .any(|s| nvml::selected(&s.target).is_some())
        {
            "native-resource-observed-envelope-not-causal-or-live-qualified"
        } else {
            "native-host-observed-envelope-not-causal-or-live-qualified"
        }
    } else {
        "imported-or-declared-resource-comparison-not-native-execution"
    };
    for (i, gate) in result.gates.iter_mut().enumerate() {
        if gate.decision != Outcome::Pass {
            continue;
        }
        let reference: Vec<Rational> = samples[0][i]
            .iter()
            .chain(&samples[2][i])
            .copied()
            .collect();
        let range = |samples: &[Rational]| {
            envelope::range(samples)
                .map_err(|e| format!("{e:?}"))?
                .ok_or_else(|| "resource samples missing".to_owned())
        };
        let reference = range(&reference)?;
        let candidate = range(&samples[1][i])?;
        gate.reference = Some(reference);
        gate.candidate = Some(candidate);
        match envelope::assess(
            reference,
            candidate,
            gate.gate.max_regression_bps,
            gate.gate.max_reference_spread_bps,
            envelope::Direction::LowerBetter,
        ) {
            Ok(assessment) => {
                gate.decision = match assessment.decision {
                    envelope::EnvelopeDecision::Pass => Outcome::Pass,
                    envelope::EnvelopeDecision::Regression => Outcome::Regression,
                };
                gate.adverse_bounds = Some(assessment.adverse_bounds);
            }
            Err(reason) => {
                gate.decision = match reason {
                    envelope::EnvelopeReason::ArithmeticOverflow { .. }
                    | envelope::EnvelopeReason::InvalidRational
                    | envelope::EnvelopeReason::InvalidBounds => Outcome::Error,
                    _ => Outcome::Inconclusive,
                };
                gate.reasons.push(format!("{reason:?}"));
            }
        }
    }
    result.decision = result
        .gates
        .iter()
        .map(|g| g.decision)
        .max()
        .unwrap_or(Outcome::Error);
    Ok(())
}
pub fn compare(plan: &Path, groups: [&[PathBuf]; 3]) -> Decision {
    let mut result = Decision {
        version: 1,
        kind: "resource-decision-v1",
        claim: "resource-comparison-not-qualified",
        decision: Outcome::Error,
        gates: Vec::new(),
        errors: Vec::new(),
    };
    if let Err(error) = compare_inner(plan, groups, &mut result) {
        result.decision = Outcome::Error;
        result.errors.push(error);
    }
    result
}
fn inspection(capture: &Capture, plan: Option<&Path>) -> Result<serde_json::Value> {
    let mut summaries = Vec::new();
    if let Some(path) = plan {
        let (study, hash) = study(path)?;
        if hash != capture.study_sha256
            || study.arm(capture.role).config != capture.observation.config
            || study.collector_sha256 != capture.observation.binary_sha256
            || study.acquisition(capture.role, capture.phase, capture.index)?
                != capture.acquisition_id
        {
            return Err("resource inspection plan identity mismatch".into());
        }
        summaries.extend(
            study
                .gates
                .iter()
                .map(|g| summarize(&capture.observation, g)),
        );
    }
    let o = &capture.observation;
    Ok(serde_json::json!({
        "kind": "resource-inspection-v1", "provenance": o.provenance,
        "claim": "resource-observation-not-capacity-or-retention-or-live-qualification",
        "acquisition_id": capture.acquisition_id, "clock": o.clock, "measured": o.measured,
        "sources": o.config.sources, "snapshots": o.snapshots.len(), "failures": o.failures,
        "raw_bytes": o.raw_bytes, "read_overhead_us": o.read_overhead_us, "summaries": summaries,
        "cpu_denominator": "one_logical_cpu_may_exceed_one",
        "source_attribution": "declared_scope_not_model_causality_no_cross_source_sums"
    }))
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Sample only prospectively selected ordinary Linux or macOS host sources; no process launch.
    Capture {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long, value_enum)]
        role: Role,
        #[arg(long, value_enum)]
        phase: Phase,
        #[arg(long)]
        index: u32,
        #[arg(long)]
        out: PathBuf,
    },
    /// Retain bounded source bytes as imported evidence, regardless of claimed provenance.
    Import {
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Replay raw bytes and expose source-specific summaries without live access.
    Inspect {
        capture: PathBuf,
        #[arg(long)]
        plan: Option<PathBuf>,
    },
    /// Exact prospective A/B/A2 envelope; list warmups then measured acquisitions in order.
    Compare {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long, num_args = 1..)]
        a: Vec<PathBuf>,
        #[arg(long, num_args = 1..)]
        b: Vec<PathBuf>,
        #[arg(long, num_args = 1..)]
        a2: Vec<PathBuf>,
    },
}
pub fn execute(command: Command) -> Result<u8> {
    match command {
        Command::Capture {
            plan,
            role,
            phase,
            index,
            out,
        } => {
            let capture = capture(&plan, role, phase, index, &out)?;
            let result = inspection(&capture, Some(&plan))?;
            crate::print_json(&result)?;
            let (study, _) = study(&plan)?;
            Ok(
                if study
                    .gates
                    .iter()
                    .all(|g| summary_value(&capture.observation, g).is_ok())
                {
                    0
                } else {
                    2
                },
            )
        }
        Command::Import { input, out } => {
            let capture = import(&input, &out)?;
            crate::print_json(&inspection(&capture, None)?)?;
            Ok(0)
        }
        Command::Inspect { capture, plan } => {
            crate::print_json(&inspection(&load(&capture)?, plan.as_deref())?)?;
            Ok(0)
        }
        Command::Compare { plan, a, b, a2 } => {
            let decision = compare(&plan, [&a, &b, &a2]);
            crate::print_json(&decision)?;
            Ok(decision.decision.exit())
        }
    }
}

#[cfg(test)]
#[path = "../tests/support/resources.rs"]
mod tests;
