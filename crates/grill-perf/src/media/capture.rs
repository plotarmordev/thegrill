pub(super) mod compare;
use super::cache::{self, Observation};
use super::*;
use crate::{
    model::Deployment,
    process::{self, program_signals::Guard},
};
use std::{
    fs,
    time::{Duration, Instant},
};

const CLAIM: &str = "collector-observed-fresh-process-wall-time-and-container-checked-outputs-not-attested-execution-or-quality";
const TOP_FILES: [&str; 7] = [
    "workload.json",
    "runner.json",
    "deployment.json",
    "receipt.json",
    "cache-before.json",
    "cache-after-setup.json",
    "cache-after.json",
];

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Collector {
    version: String,
    source: String,
    sha256: String,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Platform {
    os: String,
    arch: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    kind: String,
    claim: String,
    support: String,
    slot: Slot,
    workload_sha256: String,
    workload_typed_sha256: String,
    runner_sha256: String,
    deployment_sha256: String,
    program: Program,
    setup_program: Option<Program>,
    execution: Execution,
    markers: Option<markers::Transport>,
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    cache_declared_state: CacheState,
    cache_paths: Vec<PathBuf>,
    collector: Collector,
    clock: String,
    poll_interval_us: u64,
    platform: Platform,
    started_unix_ms: u64,
    projection_root: PathBuf,
}
#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Reservation {
    cell: String,
    phase: Phase,
    seed: u64,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}
#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SetupReservation {
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Complete,
    NonzeroExit,
    DeadlineExceeded,
    SpawnFailed,
    Cancelled,
    OutputMissing,
    OutputInvalid,
    CaptureFailed,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CaptureStatus {
    Complete,
    Cancelled,
    SetupFailed,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Reason {
    OutputMissing,
    OutputInvalid,
    OutputCount,
    OutputEntryType,
    OutputNonUtf8,
    OutputOversized,
    OutputUnreadable,
    NonzeroExit,
    DeadlineExceeded,
    SpawnFailed,
    Cancelled,
    CancelledBeforeLaunch,
    CaptureFailed,
    CacheMutatedDuringMeasurement,
    CacheObservationIncomplete,
}
#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ValidationLabel {
    Invalid,
    ContainerChecked,
}
#[derive(Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ValidationReason {
    Container { failure: crate::imagefile::Failure },
    OutputDimensions,
    Audio { failure: crate::wavfile::Failure },
    Video { failure: crate::mp4file::Failure },
    OutputExpectations,
}
#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Validation {
    label: ValidationLabel,
    container: Container,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    reason: Option<ValidationReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_us: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_duration_us: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mdhd_duration_us: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_width: Option<crate::mp4file::Rational>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_height: Option<crate::mp4file::Rational>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channels: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fps: Option<crate::mp4file::Rational>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio: Option<bool>,
}
impl Validation {
    fn valid(&self) -> bool {
        self.label == ValidationLabel::ContainerChecked && self.reason.is_none()
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedOutput {
    path: String,
    original_name: String,
    declaration: usize,
    sha256: String,
    bytes: u64,
    validation: Validation,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stream {
    sha256: String,
    bytes: usize,
    truncated: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessRecord {
    end_to_end_us: u64,
    exit_status: Option<i32>,
    status: Status,
    spawned: bool,
    failure: Option<Reason>,
    detail: Option<String>,
    stdout: Stream,
    stderr: Stream,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetupRecord {
    process: ProcessRecord,
    outputs: Vec<RetainedOutput>,
    reasons: Vec<Reason>,
    details: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Run {
    receipt_sha256: String,
    cell: String,
    phase: Phase,
    seed: u64,
    process: ProcessRecord,
    markers: Option<markers::Markers>,
    eligible: bool,
    outputs: Vec<RetainedOutput>,
    reasons: Vec<Reason>,
    details: Vec<String>,
    cache_after: Vec<Observation>,
    cache_mutated_during_measurement: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    kind: String,
    status: CaptureStatus,
    finished_unix_ms: u64,
    elapsed_ms: u64,
    runs: usize,
    files: BTreeMap<String, String>,
    evidence_sha256: String,
}
impl Manifest {
    fn digest(&self) -> Result<String> {
        // The digest covers the complete manifest body, excluding only itself.
        json_digest(&(
            self.version,
            &self.kind,
            self.status,
            self.finished_unix_ms,
            self.elapsed_ms,
            self.runs,
            &self.files,
        ))
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Summary {
    n: usize,
    min: u64,
    median: f64,
    mean: f64,
    max: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Determinism {
    seed: u64,
    n: usize,
    deterministic: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CellSummary {
    cell: String,
    warmup_runs: usize,
    measured_runs: usize,
    eligible: usize,
    ineligible_by_reason: BTreeMap<Reason, usize>,
    end_to_end_us: Option<Summary>,
    markers_by_status: BTreeMap<markers::Status, usize>,
    generation_us: Option<Summary>,
    load_to_generation_us: Option<Summary>,
    determinism_by_seed: Vec<Determinism>,
}
#[derive(Serialize)]
pub(super) struct Inspection {
    version: u32,
    kind: String,
    claim: String,
    support: String,
    status: CaptureStatus,
    execution: Execution,
    slot: Slot,
    cells: Vec<CellSummary>,
    runs: Vec<ReportedRun>,
    setup: Option<SetupRecord>,
    cache_mutated_during_measurement: Option<bool>,
    evidence_sha256: String,
}
impl Inspection {
    pub(super) fn exit_code(&self) -> u8 {
        u8::from(self.status != CaptureStatus::Complete)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    parse(&evidence::read(path, FILE_CAP)?)
}
fn json_digest(value: &impl Serialize) -> Result<String> {
    Ok(evidence::digest(
        &serde_json::to_vec(value).map_err(|error| error.to_string())?,
    ))
}
fn reservation(sample: &Scheduled<'_>, plan: &Plan, root: &Path) -> Result<Reservation> {
    Ok(Reservation {
        cell: sample.cell.id.clone(),
        phase: sample.phase,
        seed: sample.seed,
        args: render(
            &plan.runner.args,
            sample.request,
            sample.seed,
            &root.join(sample.directory()),
            &plan.workload,
        )?,
        env: plan.runner.env.clone(),
    })
}
#[derive(Serialize)]
struct ReportedRun {
    #[serde(flatten)]
    run: Run,
    #[serde(flatten)]
    rates: Rates,
}

#[derive(Serialize)]
struct Rates {
    realtime_factor: Option<f64>,
    frames_per_second_end_to_end: Option<f64>,
}

fn produced(run: &Run) -> (u128, u128) {
    let mut audio_us = 0;
    let mut frames = 0;
    for output in &run.outputs {
        let validation = &output.validation;
        if validation.container == Container::Wav {
            audio_us += u128::from(validation.duration_us.unwrap_or(0));
        }
        frames += u128::from(validation.frames.unwrap_or(0));
    }
    (audio_us, frames)
}

fn rates<'a>(runs: impl Iterator<Item = &'a Run>) -> Rates {
    let (mut elapsed_us, mut audio_us, mut frames) = (0u128, 0u128, 0u128);
    for run in runs.filter(|run| run.eligible) {
        let (audio, video) = produced(run);
        elapsed_us += u128::from(run.process.end_to_end_us);
        audio_us += audio;
        frames += video;
    }
    Rates {
        realtime_factor: (audio_us > 0 && elapsed_us > 0)
            .then(|| elapsed_us as f64 / audio_us as f64),
        frames_per_second_end_to_end: (frames > 0 && elapsed_us > 0)
            .then(|| frames as f64 * 1_000_000.0 / elapsed_us as f64),
    }
}

fn validate_output(bytes: &[u8], declaration: &Output) -> Validation {
    let mut result = Validation {
        label: ValidationLabel::Invalid,
        container: declaration.container,
        width: None,
        height: None,
        reason: None,
        duration_us: None,
        media_duration_us: None,
        mdhd_duration_us: None,
        display_width: None,
        display_height: None,
        sample_rate: None,
        channels: None,
        frames: None,
        fps: None,
        audio: None,
    };
    match declaration.container {
        Container::Png | Container::Jpeg => {
            let checked = if declaration.container == Container::Png {
                crate::imagefile::png(bytes)
            } else {
                crate::imagefile::jpeg(bytes)
            };
            match checked {
                Ok(checked) => {
                    result.width = Some(checked.width);
                    result.height = Some(checked.height);
                }
                Err(failure) => result.reason = Some(ValidationReason::Container { failure }),
            }
        }
        Container::Wav => match crate::wavfile::check(bytes) {
            Ok(checked) => {
                result.duration_us = Some(checked.duration_us);
                result.sample_rate = Some(checked.sample_rate);
                result.channels = Some(checked.channels);
            }
            Err(failure) => result.reason = Some(ValidationReason::Audio { failure }),
        },
        Container::Mp4 => validate_video(bytes, &mut result),
    }
    if result.reason.is_none() && !expectations_match(&declaration.expect, &result) {
        result.reason = Some(
            if matches!(declaration.container, Container::Png | Container::Jpeg) {
                ValidationReason::OutputDimensions
            } else {
                ValidationReason::OutputExpectations
            },
        );
    }
    if result.reason.is_none() {
        result.label = ValidationLabel::ContainerChecked;
    }
    result
}

fn validate_video(bytes: &[u8], result: &mut Validation) {
    let checked = match crate::mp4file::check(bytes) {
        Ok(checked) => checked,
        Err(failure) => {
            result.reason = Some(ValidationReason::Video { failure });
            return;
        }
    };
    let Some(video) = checked
        .tracks
        .iter()
        .find(|track| track.kind == crate::mp4file::TrackKind::Video)
    else {
        result.reason = Some(ValidationReason::OutputExpectations);
        return;
    };
    result.width = video.width;
    result.height = video.height;
    result.frames = Some(video.sample_count);
    result.duration_us = Some(video.duration_us);
    result.media_duration_us = Some(video.media_duration_us);
    result.mdhd_duration_us = Some(video.mdhd_duration_us);
    result.display_width = video.display_width;
    result.display_height = video.display_height;
    result.fps = video.fps;
    result.audio = Some(
        checked
            .tracks
            .iter()
            .any(|track| track.kind == crate::mp4file::TrackKind::Audio),
    );
}

fn expectations_match(expect: &Expect, observed: &Validation) -> bool {
    [
        (expect.width.map(u64::from), observed.width.map(u64::from)),
        (expect.height.map(u64::from), observed.height.map(u64::from)),
        (expect.frames.map(u64::from), observed.frames),
        (
            expect.duration_us,
            observed.media_duration_us.or(observed.duration_us),
        ),
        (
            expect.sample_rate.map(u64::from),
            observed.sample_rate.map(u64::from),
        ),
        (
            expect.channels.map(u64::from),
            observed.channels.map(u64::from),
        ),
        (expect.audio.map(u64::from), observed.audio.map(u64::from)),
    ]
    .into_iter()
    .all(|(expected, actual)| expected.is_none_or(|value| actual == Some(value)))
        && expect.fps.is_none_or(|expected| {
            observed.fps.is_some_and(|actual| {
                u128::from(actual.numerator)
                    == u128::from(expected) * u128::from(actual.denominator)
            })
        })
}

fn output_sources(
    dir: &Path,
    declaration: &Output,
    count: u32,
) -> std::result::Result<Vec<PathBuf>, Reason> {
    if declaration.placeholder == OutputPlaceholder::Output {
        return Ok(vec![
            dir.join(format!("output-00.{}", declaration.container.extension())),
        ]);
    }
    let directory = dir.join("out");
    evidence::directory(&directory).map_err(|_| Reason::OutputEntryType)?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| Reason::OutputUnreadable)? {
        let path = entry.map_err(|_| Reason::OutputUnreadable)?.path();
        if path.file_name().and_then(|name| name.to_str()).is_none() {
            return Err(Reason::OutputNonUtf8);
        }
        let metadata = fs::symlink_metadata(&path).map_err(|_| Reason::OutputUnreadable)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Reason::OutputEntryType);
        }
        paths.push(path);
        if paths.len() > count as usize {
            return Err(Reason::OutputCount);
        }
    }
    if paths.len() != count as usize {
        return Err(Reason::OutputCount);
    }
    paths.sort();
    Ok(paths)
}

fn retain_output(
    source: &Path,
    dir: &Path,
    declaration: &Output,
    index: usize,
    declaration_index: usize,
    cap: usize,
) -> std::result::Result<RetainedOutput, (Reason, Option<String>)> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        (
            if error.kind() == std::io::ErrorKind::NotFound {
                Reason::OutputMissing
            } else {
                Reason::OutputUnreadable
            },
            Some(error.to_string()),
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err((Reason::OutputEntryType, None));
    }
    if metadata.len() > cap as u64 {
        return Err((Reason::OutputOversized, None));
    }
    let original_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or((Reason::OutputNonUtf8, None))?
        .to_string();
    let bytes = evidence::read(source, cap).map_err(|_| (Reason::OutputUnreadable, None))?;
    let path = format!(
        "retained/output-{index:02}.{}",
        declaration.container.extension()
    );
    fs::rename(source, dir.join(&path))
        .map_err(|error| (Reason::OutputUnreadable, Some(error.to_string())))?;
    Ok(RetainedOutput {
        path,
        original_name,
        declaration: declaration_index,
        sha256: evidence::digest(&bytes),
        bytes: bytes.len() as u64,
        validation: validate_output(&bytes, declaration),
    })
}

#[derive(Default)]
struct Outputs {
    retained: Vec<RetainedOutput>,
    reasons: Vec<Reason>,
    details: Vec<String>,
}
fn expected_outputs(output: &Output, request: &Request, args: &[String], setup: bool) -> u32 {
    if setup
        && !args
            .iter()
            .any(|arg| arg.contains(&format!("{{{}}}", output.placeholder.name())))
    {
        0
    } else if output.placeholder == OutputPlaceholder::Output {
        1
    } else {
        request.images_per_run.unwrap_or(1)
    }
}
fn collect_outputs(
    dir: &Path,
    workload: &Workload,
    request: &Request,
    args: &[String],
    setup: bool,
) -> Result<Outputs> {
    let cap = usize::try_from(workload.limits.output_bytes)
        .map_err(|_| "output_bytes exceeds platform size")?;
    let mut result = Outputs::default();
    evidence::directory(&dir.join("retained"))?;
    for (declaration_index, declaration) in workload.outputs.iter().enumerate() {
        let count = expected_outputs(declaration, request, args, setup);
        if count == 0 {
            continue;
        }
        let sources = match output_sources(dir, declaration, count) {
            Ok(paths) => paths,
            Err(reason) => {
                result.reasons.push(reason);
                continue;
            }
        };
        for source in sources {
            match retain_output(
                &source,
                dir,
                declaration,
                result.retained.len(),
                declaration_index,
                cap,
            ) {
                Ok(output) => {
                    if !output.validation.valid() {
                        result.reasons.push(Reason::OutputInvalid);
                    }
                    result.retained.push(output);
                }
                Err((reason, detail)) => {
                    result.reasons.push(reason);
                    if let Some(detail) = detail {
                        result.details.push(detail);
                    }
                }
            }
        }
    }
    result.reasons.sort();
    result.reasons.dedup();
    evidence::sync(dir)?;
    Ok(result)
}

fn process_status(output: &process::Output, deadline_ms: u32) -> Status {
    if output.cancelled {
        Status::Cancelled
    } else if !output.spawned {
        Status::SpawnFailed
    } else if output.timed_out || output.exit_observed_us >= u64::from(deadline_ms) * 1000 {
        Status::DeadlineExceeded
    } else if output.status.is_none() {
        Status::CaptureFailed
    } else if !output.status.is_some_and(|status| status.success()) {
        Status::NonzeroExit
    } else if output.failure.is_some() {
        Status::CaptureFailed
    } else {
        Status::Complete
    }
}
fn reason_for(status: Status) -> Option<Reason> {
    Some(match status {
        Status::Complete => return None,
        Status::Cancelled => Reason::Cancelled,
        Status::DeadlineExceeded => Reason::DeadlineExceeded,
        Status::SpawnFailed => Reason::SpawnFailed,
        Status::NonzeroExit => Reason::NonzeroExit,
        Status::CaptureFailed => Reason::CaptureFailed,
        Status::OutputMissing => Reason::OutputMissing,
        Status::OutputInvalid => Reason::OutputInvalid,
    })
}

struct Session<'a> {
    plan: &'a Plan,
    receipt: &'a Receipt,
    receipt_sha256: &'a str,
    guard: &'a Guard,
}
impl Session<'_> {
    fn launch(
        &self,
        program: &Program,
        args: &[String],
        deadline_ms: u32,
        dir: &Path,
    ) -> Result<ProcessRecord> {
        let argv = args
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let env = self
            .plan
            .runner
            .env
            .iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<Vec<_>>();
        let limits = &self.plan.workload.limits;
        let output = process::run(
            self.guard,
            &process::Request {
                program: &program.declared_path,
                argv: &argv,
                env: &env,
                cwd: self.receipt.cwd.as_deref(),
                deadline: Duration::from_millis(u64::from(deadline_ms)),
                stdout_cap: limits.stdout_bytes,
                stderr_cap: limits.stderr_bytes,
                truncation_fatal: false,
            },
        );
        let status = process_status(&output, deadline_ms);
        evidence::write(&dir.join("stdout.bin"), &output.stdout)?;
        evidence::write(&dir.join("stderr.bin"), &output.stderr)?;
        let failure = if status == Status::Cancelled && !output.spawned {
            Some(Reason::CancelledBeforeLaunch)
        } else {
            reason_for(status)
        };
        Ok(ProcessRecord {
            end_to_end_us: output.exit_observed_us,
            exit_status: output.status.and_then(|status| status.code()),
            status,
            spawned: output.spawned,
            failure,
            detail: output.failure,
            stdout: Stream {
                sha256: evidence::digest(&output.stdout),
                bytes: output.stdout.len(),
                truncated: output.stdout_truncated,
            },
            stderr: Stream {
                sha256: evidence::digest(&output.stderr),
                bytes: output.stderr.len(),
                truncated: output.stderr_truncated,
            },
        })
    }
    fn setup(&self, root: &Path, declaration: &Setup) -> Result<SetupRecord> {
        let dir = root.join("setup");
        prepare_directory(&dir, &declaration.args)?;
        let workload = &self.plan.workload;
        let args = render(
            &declaration.args,
            &workload.request,
            workload.request.seeds[0],
            &dir,
            workload,
        )?;
        evidence::publish(
            &dir,
            "reservation.json",
            &SetupReservation {
                args: args.clone(),
                env: self.plan.runner.env.clone(),
            },
        )?;
        let mut process = self.launch(
            self.receipt
                .setup_program
                .as_ref()
                .ok_or("missing setup program")?,
            &args,
            declaration
                .deadline_ms
                .unwrap_or(workload.limits.setup_deadline_ms),
            &dir,
        )?;
        let outputs = collect_outputs(&dir, workload, &workload.request, &declaration.args, true)?;
        demote_outputs(&mut process, &outputs.reasons);
        let record = SetupRecord {
            process,
            outputs: outputs.retained,
            reasons: outputs.reasons,
            details: outputs.details,
        };
        evidence::publish(&dir, "setup.json", &record)?;
        Ok(record)
    }
    fn sample(&self, root: &Path, sample: &Scheduled<'_>, previous: &[Observation]) -> Result<Run> {
        let dir = root.join(sample.directory());
        prepare_directory(&dir, &self.plan.runner.args)?;
        let reservation = reservation(sample, self.plan, root)?;
        evidence::publish(&dir, "reservation.json", &reservation)?;
        let mut process = self.launch(
            &self.receipt.program,
            &reservation.args,
            self.plan.workload.limits.run_deadline_ms,
            &dir,
        )?;
        let mut outputs = collect_outputs(
            &dir,
            &self.plan.workload,
            sample.request,
            &self.plan.runner.args,
            false,
        )?;
        demote_outputs(&mut process, &outputs.reasons);
        if let Some(reason) = reason_for(process.status) {
            outputs.reasons.push(reason);
        }
        let cache_after = cache::observe(&self.receipt.cache_paths);
        let cache_mutated_during_measurement =
            measurement_growth(sample.phase, previous, &cache_after);
        if let Some(reason) = cache_reason(cache_mutated_during_measurement) {
            outputs.reasons.push(reason);
        }
        outputs.reasons.sort();
        outputs.reasons.dedup();
        let record = Run {
            receipt_sha256: self.receipt_sha256.into(),
            cell: sample.cell.id.clone(),
            phase: sample.phase,
            seed: sample.seed,
            eligible: process.status == Status::Complete
                && cache_mutated_during_measurement == Some(false),
            markers: self
                .plan
                .runner
                .markers
                .map(|_| markers::collect(&dir, process.end_to_end_us)),
            process,
            outputs: outputs.retained,
            reasons: outputs.reasons,
            details: outputs.details,
            cache_mutated_during_measurement,
            cache_after,
        };
        evidence::publish(&dir, "run.json", &record)?;
        Ok(record)
    }
}
fn prepare_directory(dir: &Path, args: &[String]) -> Result<()> {
    evidence::fresh(dir)?;
    evidence::fresh(&dir.join("retained"))?;
    if args.iter().any(|arg| arg.contains("{output_dir}")) {
        evidence::fresh(&dir.join("out"))?;
    }
    Ok(())
}
fn demote_outputs(process: &mut ProcessRecord, reasons: &[Reason]) {
    if process.status == Status::Complete && !reasons.is_empty() {
        process.status = if reasons.contains(&Reason::OutputMissing) {
            Status::OutputMissing
        } else {
            Status::OutputInvalid
        };
    }
}
fn pause(ms: u32) {
    let deadline = Instant::now() + Duration::from_millis(u64::from(ms));
    while !process::program_signals::cancelled() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(process::PROGRAM_POLL));
    }
}

fn make_receipt(
    plan: &Plan,
    admitted: Preflight,
    deployment: &[u8],
    slot: Slot,
    root: &Path,
    started_unix_ms: u64,
) -> Result<Receipt> {
    Ok(Receipt {
        version: 1,
        kind: "media-capture-v1".into(),
        claim: CLAIM.into(),
        support: SUPPORT.into(),
        slot,
        workload_sha256: admitted.workload_sha256,
        workload_typed_sha256: admitted.workload_typed_sha256,
        runner_sha256: admitted.runner_sha256,
        deployment_sha256: evidence::digest(deployment),
        program: admitted.program,
        setup_program: admitted.setup_program,
        execution: plan.runner.execution,
        markers: plan.runner.markers,
        env: admitted.env,
        cwd: admitted.cwd,
        cache_declared_state: plan.runner.cache.declared_state,
        cache_paths: admitted.cache_paths,
        collector: Collector {
            version: env!("CARGO_PKG_VERSION").into(),
            source: env!("GRILL_PERF_SOURCE").into(),
            sha256: evidence::binary_digest()?,
        },
        clock: "grill-monotonic".into(),
        poll_interval_us: process::PROGRAM_POLL.as_micros() as u64,
        platform: Platform {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        },
        started_unix_ms,
        projection_root: root.into(),
    })
}

pub(super) fn capture(
    workload: &Path,
    runner: &Path,
    deployment: &Path,
    slot: &str,
    out: &Path,
) -> Result<Inspection> {
    let plan = load_plan(workload, runner)?;
    let admitted = preflight(&plan)?;
    let deployment_bytes = evidence::read(deployment, FILE_CAP)?;
    let declaration: Deployment = parse(&deployment_bytes)?;
    declaration.validate()?;
    let slot = plan
        .workload
        .slots
        .iter()
        .find(|selected| selected.id == slot)
        .ok_or("undeclared capture slot")?
        .clone();
    let guard = Guard::install()?;
    evidence::fresh(out)?;
    let root = fs::canonicalize(out).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let receipt = make_receipt(
        &plan,
        admitted,
        &deployment_bytes,
        slot,
        &root,
        crate::metrics::unix_ms(),
    )?;
    evidence::write(&root.join("workload.json"), &plan.workload_bytes)?;
    evidence::write(&root.join("runner.json"), &plan.runner_bytes)?;
    evidence::write(&root.join("deployment.json"), &deployment_bytes)?;
    evidence::publish(&root, "receipt.json", &receipt)?;
    let receipt_sha256 = evidence::hash_file(&root.join("receipt.json"), FILE_CAP)?;
    evidence::publish(
        &root,
        "cache-before.json",
        &cache::observe(&receipt.cache_paths),
    )?;
    let session = Session {
        plan: &plan,
        receipt: &receipt,
        receipt_sha256: &receipt_sha256,
        guard: &guard,
    };
    let setup = plan
        .runner
        .setup
        .as_ref()
        .map(|setup| session.setup(&root, setup))
        .transpose()?;
    let after_setup = cache::observe(&receipt.cache_paths);
    evidence::publish(&root, "cache-after-setup.json", &after_setup)?;
    let setup_failed = setup
        .as_ref()
        .is_some_and(|setup| setup.process.status != Status::Complete);
    let mut runs: Vec<Run> = Vec::new();
    if !setup_failed {
        for sample in schedule(&plan.workload) {
            let previous = runs
                .last()
                .map_or(after_setup.as_slice(), |run| &run.cache_after);
            runs.push(session.sample(&root, &sample, previous)?);
            pause(sample.cell.pause_between_runs_ms);
        }
    }
    let status = if process::program_signals::cancelled() {
        CaptureStatus::Cancelled
    } else if setup_failed {
        CaptureStatus::SetupFailed
    } else {
        CaptureStatus::Complete
    };
    evidence::publish(
        &root,
        "cache-after.json",
        &cache::observe(&receipt.cache_paths),
    )?;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis())
        .map_err(|_| "capture interval exceeds timestamp range")?;
    let digest = publish_manifest(
        &root,
        status,
        runs.len(),
        setup.is_some(),
        crate::metrics::unix_ms(),
        elapsed_ms,
    )?;
    Ok(inspection(&plan, &receipt, status, runs, setup, digest))
}

fn manifest_names(runs: usize, setup: bool) -> Vec<String> {
    let mut names = TOP_FILES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if setup {
        names.extend(["setup/reservation.json".into(), "setup/setup.json".into()]);
    }
    for index in 0..runs {
        names.push(format!("run-{index:06}/reservation.json"));
        names.push(format!("run-{index:06}/run.json"));
    }
    names
}
fn publish_manifest(
    root: &Path,
    status: CaptureStatus,
    runs: usize,
    setup: bool,
    finished_unix_ms: u64,
    elapsed_ms: u64,
) -> Result<String> {
    let mut files = BTreeMap::new();
    for name in manifest_names(runs, setup) {
        files.insert(
            name.clone(),
            evidence::hash_file(&root.join(name), FILE_CAP)?,
        );
    }
    let mut manifest = Manifest {
        version: 1,
        kind: "media-capture-complete-v1".into(),
        status,
        finished_unix_ms,
        elapsed_ms,
        runs,
        files,
        evidence_sha256: String::new(),
    };
    manifest.evidence_sha256 = manifest.digest()?;
    evidence::publish(root, "capture.json", &manifest)?;
    Ok(manifest.evidence_sha256)
}
fn check_manifest(root: &Path) -> Result<Manifest> {
    evidence::directory(root)?;
    let manifest: Manifest = read_json(&root.join("capture.json"))?;
    if manifest.kind != "media-capture-complete-v1" || manifest.version != 1 {
        return Err("invalid media capture manifest".into());
    }
    if manifest.evidence_sha256 != manifest.digest()? {
        return Err("capture manifest digest mismatch".into());
    }
    Ok(manifest)
}
fn verified_bytes(root: &Path, name: &str, manifest: &Manifest) -> Result<Vec<u8>> {
    let expected = manifest.files.get(name).ok_or("incomplete manifest")?;
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("invalid evidence path".into());
    }
    let mut ancestor = root.to_path_buf();
    for part in path.parent().into_iter().flat_map(Path::components) {
        ancestor.push(part);
        evidence::directory(&ancestor)?;
    }
    let bytes = evidence::read(&root.join(path), FILE_CAP)?;
    if evidence::digest(&bytes) != *expected {
        return Err(format!("retained evidence hash mismatch: {name}"));
    }
    Ok(bytes)
}
fn verified_json<T: serde::de::DeserializeOwned>(
    root: &Path,
    name: &str,
    manifest: &Manifest,
) -> Result<T> {
    parse(&verified_bytes(root, name, manifest)?)
}

fn verified_observations(root: &Path, name: &str, manifest: &Manifest) -> Result<Vec<Observation>> {
    let Strict(value) = serde_json::from_slice(&verified_bytes(root, name, manifest)?)
        .map_err(|error| error.to_string())?;
    for observation in value
        .as_array()
        .ok_or("cache observations must be an array")?
    {
        if !observation.is_object() {
            return Err("cache observations must be JSON objects".into());
        }
    }
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn verify_process(
    dir: &Path,
    record: &ProcessRecord,
    limits: &Limits,
    deadline_ms: u32,
) -> Result<()> {
    for (name, stream, cap) in [
        ("stdout.bin", &record.stdout, limits.stdout_bytes),
        ("stderr.bin", &record.stderr, limits.stderr_bytes),
    ] {
        let bytes = evidence::read(&dir.join(name), cap)?;
        if bytes.len() != stream.bytes || evidence::digest(&bytes) != stream.sha256 {
            return Err("stream hash mismatch".into());
        }
    }
    if record.status == Status::Complete
        && (!record.spawned
            || record.exit_status != Some(0)
            || record.failure.is_some()
            || record.end_to_end_us >= u64::from(deadline_ms) * 1000)
    {
        return Err("process eligibility mismatch".into());
    }
    if record.status == Status::SpawnFailed && record.spawned {
        return Err("spawn status mismatch".into());
    }
    if !record.spawned && record.exit_status.is_some() {
        return Err("unstarted process has exit status".into());
    }
    Ok(())
}
fn verify_outputs(dir: &Path, outputs: &[RetainedOutput], workload: &Workload) -> Result<Vec<u32>> {
    let mut counts = vec![0; workload.outputs.len()];
    if !outputs.is_empty() {
        evidence::directory(&dir.join("retained"))?;
    }
    for (index, output) in outputs.iter().enumerate() {
        let declaration = workload
            .outputs
            .get(output.declaration)
            .ok_or("invalid output declaration index")?;
        if output.path
            != format!(
                "retained/output-{index:02}.{}",
                declaration.container.extension()
            )
            || output.original_name.is_empty()
            || Path::new(&output.original_name).components().count() != 1
            || !matches!(
                Path::new(&output.original_name).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err("invalid retained output name".into());
        }
        let cap = usize::try_from(workload.limits.output_bytes)
            .map_err(|_| "output cap exceeds platform")?;
        let bytes = evidence::read(&dir.join(&output.path), cap)?;
        if bytes.len() as u64 != output.bytes || evidence::digest(&bytes) != output.sha256 {
            return Err("retained output hash mismatch".into());
        }
        if validate_output(&bytes, declaration) != output.validation {
            return Err("retained output validation mismatch".into());
        }
        counts[output.declaration] += 1;
    }
    Ok(counts)
}
fn output_counts_match(
    counts: &[u32],
    workload: &Workload,
    request: &Request,
    args: &[String],
    setup: bool,
) -> bool {
    counts
        .iter()
        .zip(&workload.outputs)
        .all(|(count, output)| *count == expected_outputs(output, request, args, setup))
}
fn verify_run(dir: &Path, run: &Run, sample: &Scheduled<'_>, plan: &Plan) -> Result<()> {
    if run.markers.is_some() != plan.runner.markers.is_some() {
        return Err("marker record declaration mismatch".into());
    }
    if run
        .markers
        .as_ref()
        .is_some_and(|record| *record != markers::collect(dir, run.process.end_to_end_us))
    {
        return Err("retained evidence hash mismatch: markers".into());
    }
    let workload = &plan.workload;
    verify_process(
        dir,
        &run.process,
        &workload.limits,
        workload.limits.run_deadline_ms,
    )?;
    let counts = verify_outputs(dir, &run.outputs, workload)?;
    let eligible = run.process.status == Status::Complete
        && run.reasons.is_empty()
        && output_counts_match(&counts, workload, sample.request, &plan.runner.args, false)
        && run.outputs.iter().all(|output| output.validation.valid());
    if run.eligible != eligible {
        return Err("sample eligibility mismatch".into());
    }
    for reason in [
        Reason::CacheMutatedDuringMeasurement,
        Reason::CacheObservationIncomplete,
    ] {
        if run.reasons.contains(&reason)
            != (cache_reason(run.cache_mutated_during_measurement) == Some(reason))
        {
            return Err("cache eligibility reason mismatch".into());
        }
    }
    if run.cell != sample.cell.id || run.phase != sample.phase || run.seed != sample.seed {
        return Err("sample schedule mismatch".into());
    }
    Ok(())
}

fn verify_receipt(receipt: &Receipt, plan: &Plan, deployment: &[u8]) -> Result<Deployment> {
    if receipt.kind != "media-capture-v1"
        || receipt.version != 1
        || receipt.support != SUPPORT
        || receipt.claim != CLAIM
        || receipt.workload_sha256 != evidence::digest(&plan.workload_bytes)
        || receipt.workload_typed_sha256 != json_digest(&plan.workload)?
        || receipt.runner_sha256 != evidence::digest(&plan.runner_bytes)
        || receipt.deployment_sha256 != evidence::digest(deployment)
        || !plan.workload.slots.contains(&receipt.slot)
        || receipt.execution != plan.runner.execution
        || receipt.markers != plan.runner.markers
        || receipt.env != plan.runner.env
        || receipt.cache_declared_state != plan.runner.cache.declared_state
        || receipt.setup_program.is_some() != plan.runner.setup.is_some()
    {
        return Err("media receipt declaration mismatch".into());
    }
    let declaration: Deployment = parse(deployment)?;
    declaration.validate()?;
    Ok(declaration)
}
fn inspect_setup(
    root: &Path,
    plan: &Plan,
    receipt: &Receipt,
    manifest: &Manifest,
) -> Result<Option<SetupRecord>> {
    let Some(setup) = &plan.runner.setup else {
        return Ok(None);
    };
    let reservation: SetupReservation = verified_json(root, "setup/reservation.json", manifest)?;
    let args = render(
        &setup.args,
        &plan.workload.request,
        plan.workload.request.seeds[0],
        &receipt.projection_root.join("setup"),
        &plan.workload,
    )?;
    if reservation
        != (SetupReservation {
            args,
            env: plan.runner.env.clone(),
        })
    {
        return Err("setup reservation projection mismatch".into());
    }
    let record: SetupRecord = verified_json(root, "setup/setup.json", manifest)?;
    let dir = root.join("setup");
    verify_process(
        &dir,
        &record.process,
        &plan.workload.limits,
        setup
            .deadline_ms
            .unwrap_or(plan.workload.limits.setup_deadline_ms),
    )?;
    let counts = verify_outputs(&dir, &record.outputs, &plan.workload)?;
    if record.process.status == Status::Complete
        && (!record.reasons.is_empty()
            || !output_counts_match(
                &counts,
                &plan.workload,
                &plan.workload.request,
                &setup.args,
                true,
            )
            || record
                .outputs
                .iter()
                .any(|output| !output.validation.valid()))
    {
        return Err("setup output eligibility mismatch".into());
    }
    Ok(Some(record))
}

pub(super) fn inspect(root: &Path) -> Result<Inspection> {
    Ok(load(root)?.inspection)
}

struct Loaded {
    plan: Plan,
    receipt: Receipt,
    deployment: Deployment,
    finished_unix_ms: u64,
    inspection: Inspection,
}

fn load(root: &Path) -> Result<Loaded> {
    let manifest = check_manifest(root)?;
    let plan = plan_from_bytes(
        verified_bytes(root, "workload.json", &manifest)?,
        verified_bytes(root, "runner.json", &manifest)?,
    )?;
    let receipt: Receipt = verified_json(root, "receipt.json", &manifest)?;
    let receipt_sha256 = manifest
        .files
        .get("receipt.json")
        .expect("verified receipt entry");
    let deployment = verify_receipt(
        &receipt,
        &plan,
        &verified_bytes(root, "deployment.json", &manifest)?,
    )?;
    if manifest.finished_unix_ms < receipt.started_unix_ms {
        return Err("capture completion precedes its start".into());
    }
    if manifest.runs > schedule(&plan.workload).count() {
        return Err("capture run count exceeds plan".into());
    }
    let expected_names = manifest_names(manifest.runs, plan.runner.setup.is_some())
        .into_iter()
        .collect::<BTreeSet<_>>();
    if manifest.files.keys().cloned().collect::<BTreeSet<_>>() != expected_names {
        return Err("manifest completeness mismatch".into());
    }
    verified_observations(root, "cache-before.json", &manifest)?;
    let after_setup = verified_observations(root, "cache-after-setup.json", &manifest)?;
    verified_observations(root, "cache-after.json", &manifest)?;
    let setup = inspect_setup(root, &plan, &receipt, &manifest)?;
    let setup_failed = setup
        .as_ref()
        .is_some_and(|setup| setup.process.status != Status::Complete);
    let expected_runs = if setup_failed {
        0
    } else {
        schedule(&plan.workload).count()
    };
    if manifest.runs != expected_runs {
        return Err("capture run count mismatch".into());
    }
    let mut runs: Vec<Run> = Vec::with_capacity(expected_runs);
    for sample in schedule(&plan.workload).take(expected_runs) {
        let prefix = sample.directory();
        let reserved: Reservation =
            verified_json(root, &format!("{prefix}/reservation.json"), &manifest)?;
        if reserved != reservation(&sample, &plan, &receipt.projection_root)? {
            return Err("sample reservation projection mismatch".into());
        }
        let run: Run = verified_json(root, &format!("{prefix}/run.json"), &manifest)?;
        if &run.receipt_sha256 != receipt_sha256 {
            return Err("run record is bound to a different capture receipt".into());
        }
        verify_run(&root.join(prefix), &run, &sample, &plan)?;
        let previous = runs
            .last()
            .map_or(after_setup.as_slice(), |run| &run.cache_after);
        if run.cache_mutated_during_measurement
            != measurement_growth(sample.phase, previous, &run.cache_after)
        {
            return Err("cache growth observation mismatch".into());
        }
        runs.push(run);
    }
    let cancelled = runs
        .iter()
        .any(|run| run.process.status == Status::Cancelled)
        || setup
            .as_ref()
            .is_some_and(|setup| setup.process.status == Status::Cancelled);
    if (setup_failed && manifest.status == CaptureStatus::Complete)
        || (cancelled && manifest.status != CaptureStatus::Cancelled)
        || (!setup_failed && manifest.status == CaptureStatus::SetupFailed)
    {
        return Err("capture status mismatch".into());
    }
    let process_us = runs
        .iter()
        .map(|run| u128::from(run.process.end_to_end_us))
        .sum::<u128>()
        + setup
            .as_ref()
            .map_or(0, |setup| u128::from(setup.process.end_to_end_us));
    if process_us > u128::from(manifest.elapsed_ms) * 1000 {
        return Err("capture elapsed time is shorter than its process durations".into());
    }
    // Allow clock drift and slew without treating time spent suspended as overlap.
    let tolerance_ms = 1000 + manifest.elapsed_ms / 1000;
    if manifest.finished_unix_ms - receipt.started_unix_ms
        < manifest.elapsed_ms.saturating_sub(tolerance_ms)
    {
        return Err(
            "capture wall-clock interval is shorter than elapsed time beyond tolerance".into(),
        );
    }
    let inspection = inspection(
        &plan,
        &receipt,
        manifest.status,
        runs,
        setup,
        manifest.evidence_sha256,
    );
    Ok(Loaded {
        plan,
        receipt,
        deployment,
        finished_unix_ms: manifest.finished_unix_ms,
        inspection,
    })
}

fn inspection(
    plan: &Plan,
    receipt: &Receipt,
    status: CaptureStatus,
    runs: Vec<Run>,
    setup: Option<SetupRecord>,
    evidence_sha256: String,
) -> Inspection {
    let cells = plan
        .workload
        .cells
        .iter()
        .map(|cell| summarize(cell, runs.iter()))
        .collect();
    Inspection {
        version: 1,
        kind: "media-inspection-v1".into(),
        claim: CLAIM.into(),
        support: SUPPORT.into(),
        status,
        execution: receipt.execution,
        slot: receipt.slot.clone(),
        cells,
        cache_mutated_during_measurement: if runs
            .iter()
            .any(|run| run.cache_mutated_during_measurement == Some(true))
        {
            Some(true)
        } else if runs
            .iter()
            .any(|run| run.cache_mutated_during_measurement.is_none())
        {
            None
        } else {
            Some(false)
        },
        runs: runs
            .into_iter()
            .map(|run| ReportedRun {
                rates: rates(std::iter::once(&run)),
                run,
            })
            .collect(),
        setup,
        evidence_sha256,
    }
}

fn measurement_growth(phase: Phase, before: &[Observation], after: &[Observation]) -> Option<bool> {
    if phase == Phase::Measured {
        cache::grew(before, after)
    } else {
        Some(false)
    }
}

fn cache_reason(growth: Option<bool>) -> Option<Reason> {
    match growth {
        Some(false) => None,
        Some(true) => Some(Reason::CacheMutatedDuringMeasurement),
        None => Some(Reason::CacheObservationIncomplete),
    }
}

fn hashes_by_seed<'a>(runs: impl Iterator<Item = &'a Run>) -> BTreeMap<u64, Vec<Vec<&'a str>>> {
    let mut seeds = BTreeMap::<u64, Vec<Vec<&str>>>::new();
    for run in runs.filter(|run| run.phase == Phase::Measured && run.eligible) {
        seeds.entry(run.seed).or_default().push(
            run.outputs
                .iter()
                .map(|output| output.sha256.as_str())
                .collect(),
        );
    }
    seeds
}

fn identical_hashes(outputs: &[Vec<&str>]) -> bool {
    outputs.windows(2).all(|pair| pair[0] == pair[1])
}

fn summarize<'a>(cell: &Cell, runs: impl Iterator<Item = &'a Run>) -> CellSummary {
    let matching = runs.filter(|run| run.cell == cell.id).collect::<Vec<_>>();
    let measured = matching
        .iter()
        .copied()
        .filter(|run| run.phase == Phase::Measured)
        .collect::<Vec<_>>();
    let times = measured
        .iter()
        .filter(|run| run.eligible)
        .map(|run| run.process.end_to_end_us)
        .collect::<Vec<_>>();
    let end_to_end_us = summary(&times);
    let marker_summary = |metric| {
        summary(
            &measured
                .iter()
                .filter(|run| run.eligible)
                .filter_map(|run| run.markers.as_ref()?.delta_ns(metric).map(|ns| ns / 1000))
                .collect::<Vec<_>>(),
        )
    };
    let generation_us = marker_summary(Metric::Generation);
    let load_to_generation_us = marker_summary(Metric::LoadToGeneration);
    let mut markers_by_status = BTreeMap::new();
    for markers in measured.iter().filter_map(|run| run.markers.as_ref()) {
        *markers_by_status.entry(markers.status).or_default() += 1;
    }
    let mut reasons = BTreeMap::new();
    let seeds = hashes_by_seed(measured.iter().copied());
    for run in &measured {
        if !run.eligible {
            for reason in &run.reasons {
                *reasons.entry(*reason).or_default() += 1;
            }
        }
    }
    let determinism_by_seed = seeds
        .into_iter()
        .map(|(seed, outputs)| Determinism {
            seed,
            n: outputs.len(),
            deterministic: (outputs.len() > 1).then(|| identical_hashes(&outputs)),
        })
        .collect();
    CellSummary {
        cell: cell.id.clone(),
        warmup_runs: matching.len() - measured.len(),
        measured_runs: measured.len(),
        eligible: times.len(),
        ineligible_by_reason: reasons,
        end_to_end_us,
        markers_by_status,
        generation_us,
        load_to_generation_us,
        determinism_by_seed,
    }
}

fn summary(times: &[u64]) -> Option<Summary> {
    evidence::median(times.iter().map(|value| *value as f64).collect()).map(|median| Summary {
        n: times.len(),
        min: *times.iter().min().expect("nonempty median"),
        median,
        mean: times.iter().map(|value| u128::from(*value)).sum::<u128>() as f64
            / times.len() as f64,
        max: *times.iter().max().expect("nonempty median"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximal_schedule_manifest_fits_and_round_trips() {
        let runs = MAX_CELLS * (MAX_WARMUPS + MAX_MEASURED) as usize;
        let files = manifest_names(runs, true)
            .into_iter()
            .map(|name| (name, "0".repeat(64)))
            .collect();
        let mut manifest = Manifest {
            version: 1,
            kind: "media-capture-complete-v1".into(),
            status: CaptureStatus::Complete,
            finished_unix_ms: crate::metrics::unix_ms(),
            elapsed_ms: 0,
            runs,
            evidence_sha256: String::new(),
            files,
        };
        manifest.evidence_sha256 = manifest.digest().unwrap();
        let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        assert!(bytes.len() <= FILE_CAP);
        let root = std::env::temp_dir().join(format!(
            "grill-media-max-{}-{}",
            std::process::id(),
            crate::metrics::unix_ms()
        ));
        evidence::fresh(&root).unwrap();
        evidence::write(&root.join("capture.json"), &bytes).unwrap();
        let read = check_manifest(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(read.runs, runs);
        assert_eq!(read.files.len(), runs * 2 + TOP_FILES.len() + 2);
        assert_eq!(read.evidence_sha256, read.digest().unwrap());
    }

    #[test]
    fn deadline_and_spawn_status_use_observed_facts() {
        use std::os::unix::process::ExitStatusExt;
        let mut output = process::Output {
            spawned: true,
            status: Some(std::process::ExitStatus::from_raw(0)),
            exit_observed_us: 100_000,
            ..process::Output::default()
        };
        assert!(process_status(&output, 100) == Status::DeadlineExceeded);
        output.exit_observed_us = 99_999;
        assert!(process_status(&output, 100) == Status::Complete);
        output.status = None;
        assert!(process_status(&output, 100) == Status::CaptureFailed);
        output.spawned = false;
        assert!(process_status(&output, 100) == Status::SpawnFailed);
    }
}
