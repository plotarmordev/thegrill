use crate::contract::*;
use crate::report::{Verified, VerifiedCase};
use crate::{grade, identity, pack, transport};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn io_error(operation: &str, path: &Path, error: std::io::Error) -> String {
    format!("{operation} {}: {error}", path.display())
}

pub(crate) fn read(path: &Path, cap: usize) -> Result<Vec<u8>> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, cap);
        Err("evidence file handling is supported on Linux only".into())
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let metadata = fs::symlink_metadata(path).map_err(|e| io_error("inspect", path, e))?;
        if !metadata.file_type().is_file() {
            return Err(format!(
                "required input is not a regular file: {}",
                path.display()
            ));
        }
        // Linux O_NOFOLLOW | O_NONBLOCK: reject symlinks, do not hang if a FIFO
        // is substituted between metadata and open. Parent races are not a sandbox.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| io_error("open", path, e))?;
        let metadata = file.metadata().map_err(|e| io_error("stat", path, e))?;
        if !metadata.is_file() || metadata.len() > cap as u64 {
            return Err(format!("nonregular or oversized input: {}", path.display()));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        let limit = cap.checked_add(1).ok_or("read limit overflow")?;
        file.take(limit as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| io_error("read", path, e))?;
        if bytes.len() > cap {
            return Err(format!("input exceeds {cap} bytes: {}", path.display()));
        }
        Ok(bytes)
    }
}

fn directory(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| io_error("inspect directory", path, e))?;
    if !metadata.file_type().is_dir() {
        return Err(format!("not a nonsymlink directory: {}", path.display()));
    }
    Ok(())
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_error("sync directory", path, e))
}

fn parent_of(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Read-only destination admission; exclusive creation still arbitrates races.
pub(crate) fn fresh_destination(path: &Path) -> Result<()> {
    directory(parent_of(path))?;
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error("inspect destination", path, e)),
        Ok(_) => Err(format!("destination already exists: {}", path.display())),
    }
}

/// Exclusively create one fresh owner-only directory; never reuse an existing one.
pub(crate) fn create_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|e| io_error("create fresh directory", path, e))
}

/// Exclusively create an owner-only file for serial evidence capture.
pub(crate) fn open_new(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|e| io_error("create", path, e))
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = open_new(path)?;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|e| io_error("write/sync", path, e))
}

/// Make already-synced staging bytes visible under a never-existing final name
/// without clobbering: hard link, then drop the staging name.
pub(crate) fn publish_link(staging: &Path, target: &Path) -> Result<()> {
    fs::hard_link(staging, target).map_err(|e| io_error("publish without clobber", target, e))?;
    fs::remove_file(staging).map_err(|e| io_error("remove staging link", staging, e))
}

/// Write a receipt through `.<name>.pending`, publish it as `name`, sync the directory.
pub(crate) fn publish_receipt(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let staging = dir.join(format!(".{name}.pending"));
    write_new(&staging, bytes)?;
    publish_link(&staging, &dir.join(name))?;
    sync_directory(dir)
}

pub(crate) fn compute(
    pack: &Pack,
    pack_bytes: &[u8],
    submission: &Submission,
    submission_bytes: &[u8],
) -> Result<View> {
    let answers: HashMap<_, _> = submission
        .answers
        .iter()
        .map(|a| (a.case_id.as_str(), a.artifact.as_deref()))
        .collect();
    let (pack_identity, inputs) = identity::pack(pack, pack_bytes)?;
    let cases = pack
        .cases
        .iter()
        .zip(inputs)
        .map(|(case, input)| {
            let artifact = answers.get(case.id.as_str()).copied().flatten();
            CaseGrade {
                case_id: case.id.clone(),
                input,
                artifact: artifact.map(|a| identity::bytes("artifact", a.as_bytes())),
                evidence: if artifact.is_some() {
                    Evidence::Submitted
                } else {
                    Evidence::Missing
                },
                outcome: artifact.map_or(Outcome::Unknown, |a| {
                    grade::grade(a.as_bytes(), &case.acceptance)
                }),
            }
        })
        .collect();
    Ok(View {
        version: 1,
        claim: ViewClaim::SubmittedArtifactsNotVerifiedExecution,
        identities: identity::all(pack_identity, submission, submission_bytes)?,
        cases,
    })
}

fn aggregate(pack: usize, submission: usize, receipt: usize) -> Result<()> {
    let total = pack
        .checked_add(submission)
        .and_then(|n| n.checked_add(receipt))
        .ok_or("view size overflow")?;
    if pack > PACK_CAP || submission > SUBMISSION_CAP || receipt > RECEIPT_CAP || total > VIEW_CAP {
        return Err("view exceeds required-data byte limits".into());
    }
    Ok(())
}

pub(crate) fn publish(pack_path: &Path, submission_path: &Path, out: &Path) -> Result<View> {
    let pack_bytes = read(pack_path, PACK_CAP)?;
    let pack = pack::admit(&pack_bytes)?;
    let submission_bytes = read(submission_path, SUBMISSION_CAP)?;
    let submission = pack::submission(&submission_bytes, &pack)?;
    let view = compute(&pack, &pack_bytes, &submission, &submission_bytes)?;
    // Receipt output is bounded by CASE_CAP and bounded case IDs/digests, then
    // explicitly checked before any directory is created.
    let receipt = serde_json::to_vec_pretty(&view).map_err(|e| format!("view encoding: {e}"))?;
    aggregate(pack_bytes.len(), submission_bytes.len(), receipt.len())?;
    let parent = parent_of(out);
    directory(parent)?;
    create_directory(out)?;
    // On failure leave only our partial directory for inspection; never recursively
    // delete it or overwrite a destination. A receipt may exist if final sync fails.
    sync_directory(parent)?;
    write_new(&out.join("pack.json"), &pack_bytes)?;
    write_new(&out.join("submission.json"), &submission_bytes)?;
    sync_directory(out)?;
    publish_receipt(out, "view.json", &receipt)?;
    sync_directory(parent)?;
    Ok(view)
}

fn verify_submitted(path: &Path) -> Result<Verified> {
    let pack_bytes = read(&path.join("pack.json"), PACK_CAP)?;
    let pack = pack::admit(&pack_bytes)?;
    let submission_bytes = read(&path.join("submission.json"), SUBMISSION_CAP)?;
    let submission = pack::submission(&submission_bytes, &pack)?;
    let receipt = read(&path.join("view.json"), RECEIPT_CAP)?;
    aggregate(pack_bytes.len(), submission_bytes.len(), receipt.len())?;
    let saved: View = crate::record::parse(&receipt).map_err(|e| format!("view JSON: {e}"))?;
    let expected = compute(&pack, &pack_bytes, &submission, &submission_bytes)?;
    if saved != expected {
        return Err("view receipt does not match recomputed identities/evidence/outcomes".into());
    }
    let i = expected.identities;
    Ok(Verified {
        claim: "submitted-artifacts-not-verified-execution",
        source: i.pack.source,
        tasks: i.pack.tasks,
        target: i.pack.target,
        protocol: i.protocol,
        grading: i.pack.grading,
        qualification: i.pack.qualification,
        system: submission.system,
        rendering_unknown: submission.protocol.rendering.status == RenderingStatus::Unknown,
        cases: expected
            .cases
            .into_iter()
            .map(|c| VerifiedCase {
                case_id: c.case_id,
                input: c.input,
                outcome: c.outcome,
                result: crate::report::ResultKind::graded(c.outcome),
            })
            .collect(),
    })
}

// ----- Run evidence: fixed names, generated ordinals, validated reads -----

pub(crate) const INITIAL_VIEW: &str = "grades/initial";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Submitted,
    Run,
    RunView,
}

pub(crate) fn kind(path: &Path) -> Result<Kind> {
    directory(path)?;
    if exists(&path.join("submission.json")) {
        Ok(Kind::Submitted)
    } else if exists(&path.join("plan.json")) {
        if exists(&path.join("view.json")) {
            Ok(Kind::RunView)
        } else {
            Ok(Kind::Run)
        }
    } else {
        Err(format!(
            "not a grading view or run directory: {}",
            path.display()
        ))
    }
}

pub(crate) fn attempt_name(attempt: u32) -> String {
    format!("{attempt:06}")
}

pub(crate) struct RunDir {
    pub root: PathBuf,
    pub attempts: PathBuf,
}

impl RunDir {
    pub(crate) fn attempt(&self, attempt: u32) -> PathBuf {
        self.attempts.join(attempt_name(attempt))
    }
}

/// Freeze input snapshots before any attempt exists: fresh run directory,
/// exact pack and plan bytes, empty attempts directory, all synced.
pub(crate) fn create_run(out: &Path, pack_bytes: &[u8], plan_bytes: &[u8]) -> Result<RunDir> {
    let parent = parent_of(out);
    directory(parent)?;
    create_directory(out)?;
    sync_directory(parent)?;
    write_new(&out.join("pack.json"), pack_bytes)?;
    write_new(&out.join("plan.json"), plan_bytes)?;
    let attempts = out.join("attempts");
    create_directory(&attempts)?;
    sync_directory(out)?;
    Ok(RunDir {
        root: out.to_path_buf(),
        attempts,
    })
}

pub(crate) fn publish_initial_view(run: &RunDir, receipt: &[u8]) -> Result<()> {
    let grades = run.root.join("grades");
    create_directory(&grades)?;
    let initial = run.root.join(INITIAL_VIEW);
    create_directory(&initial)?;
    sync_directory(&grades)?;
    sync_directory(&run.root)?;
    publish_receipt(&initial, "view.json", receipt)?;
    sync_directory(&grades)
}

pub(crate) enum AttemptState {
    NotStarted,
    Unresolved,
    Terminal(Box<Terminal>),
    Invalid(&'static str),
}

pub(crate) struct CompletionMetadata {
    pub stop: Finish,
    pub usage: Option<Usage>,
}

pub(crate) struct LoadedAttempt {
    pub attempt: u32,
    pub state: AttemptState,
    pub reservation: Option<String>,
    pub terminal: Option<String>,
    pub artifact: Option<String>,
    pub outcome: Outcome,
    pub recovered_metadata: Option<CompletionMetadata>,
}

pub(crate) struct Loaded {
    pub kind: Kind,
    pub pack: Pack,
    pub pack_bytes: Vec<u8>,
    pub identities: PackIdentities,
    pub inputs: Vec<String>,
    pub plan: Plan,
    pub plan_bytes: Vec<u8>,
    pub plan_digest: String,
    pub attempts: Vec<LoadedAttempt>,
}

fn check_file(path: &Path, cap: usize, bytes: u64, sha256: &str) -> Result<Vec<u8>, &'static str> {
    let data = read(path, cap).map_err(|_| "missing, unreadable, nonregular or oversized file")?;
    if data.len() as u64 != bytes || identity::sha256(&data) != sha256 {
        return Err("size or hash mismatch");
    }
    Ok(data)
}

/// Physical/semantic consistency of a terminal beyond hash lineage: a receipt
/// cannot claim commitment while describing an exchange that could not have
/// produced it. This is internal consistency, not execution authentication.
fn terminal_consistent(t: &Terminal, stream: bool, protocol: &Protocol) -> bool {
    let committed = t.commitment == Commitment::Committed;
    let body = &t.body;
    // A DONE boundary is evidence whether or not the finalized stream qualified:
    // committed/refused attempts and streams whose only defect is an unsupported
    // answer channel at settlement carry the offset; nothing else may.
    let offsets = match body.terminal_offset {
        Some(offset) => {
            stream
                && body.complete
                && (committed || t.reason == Reason::UnsupportedResponse)
                && offset <= body.bytes
                && body.surplus_retained == body.bytes - offset
                && body.surplus_observed >= body.surplus_retained
        }
        None => body.surplus_retained == 0 && body.surplus_observed == 0,
    };
    // Stored HTTP facts must describe an exchange this HTTP/1 profile could
    // have parsed: a parsed 200 carries the mode's media type, and a declared
    // length agrees with a completely retained body or bounds a partial one.
    let http_ok = match &t.http {
        Some(http) => {
            t.dispatched
                && matches!(http.version.as_str(), "HTTP/1.0" | "HTTP/1.1")
                // A local capture failure records whatever HTTP/1 facts were
                // actually observed; every other reason implies the gate outcome.
                && match t.reason {
                    Reason::LocalIo => true,
                    Reason::HttpStatus => http.status != 200,
                    _ => http.status == 200,
                }
                && http.content_type.as_ref().is_none_or(|c| c.len() <= 128)
                && (matches!(t.reason, Reason::HttpStatus | Reason::HttpEntity | Reason::LocalIo)
                    || http
                        .content_type
                        .as_deref()
                        .is_some_and(|c| transport::media_type_matches(c, stream)))
                && http.content_length.is_none_or(|n| {
                    if body.complete && !body.truncated && body.terminal_offset.is_none() {
                        n == body.bytes
                    } else {
                        n >= body.bytes
                    }
                })
        }
        None => {
            body.bytes == 0
                && !committed
                && matches!(
                    t.reason,
                    Reason::TransportFailure
                        | Reason::TotalDeadline
                        | Reason::IdleDeadline
                        | Reason::Interrupted
                )
        }
    };
    let reason_ok = match t.reason {
        Reason::Committed => {
            committed
                && body.complete
                && matches!(t.stop, Some(Finish::Stop | Finish::Length))
                && t.final_artifact.is_some()
                && (stream == body.terminal_offset.is_some())
                && (stream || !body.truncated)
        }
        Reason::Refused => {
            committed
                && body.complete
                && t.stop.is_some()
                && t.final_artifact.is_none()
                && (stream == body.terminal_offset.is_some())
                && (stream || !body.truncated)
        }
        Reason::UnsupportedResponse if t.version == 2 && t.stop.is_some() => {
            !committed
                && body.complete
                && matches!(t.stop, Some(Finish::Stop | Finish::Length))
                && t.final_artifact.is_none()
                && (stream == body.terminal_offset.is_some())
                && (stream || !body.truncated)
        }
        Reason::ResponseCap => {
            !committed
                && t.stop.is_none()
                && t.final_artifact.is_none()
                && (body.truncated || body.bytes == 0)
        }
        _ => !committed && t.stop.is_none() && t.final_artifact.is_none(),
    };
    matches!(t.version, 1 | 2)
        && (t.dispatched
            || (t.http.is_none()
                && body.bytes == 0
                && matches!(t.reason, Reason::TransportFailure | Reason::Interrupted)))
        && t.detail.len() <= DETAIL_CAP
        && body.bytes <= u64::from(protocol.collection.response_bytes)
        && (!body.truncated || body.bytes == u64::from(protocol.collection.response_bytes))
        && t.final_artifact
            .as_ref()
            .is_none_or(|f| f.bytes <= u64::from(protocol.collection.artifact_bytes))
        && offsets
        && http_ok
        && reason_ok
}

// V1 discarded facts when a complete response lacked the final content channel.
// Recover only that archival case, from bytes already checked against its receipt.
// A failed parse, mismatched DONE boundary, or another delivery never supplies facts.
fn legacy_completion_metadata(
    terminal: &Terminal,
    bytes: &[u8],
    protocol: &Protocol,
) -> Option<CompletionMetadata> {
    if terminal.version != 1
        || terminal.reason != Reason::UnsupportedResponse
        || !terminal.body.complete
    {
        return None;
    }
    let mut collector = transport::Collector::new(protocol.collection.artifact_bytes as usize);
    let settled = if protocol.stream {
        let consumed = collector.feed(bytes).ok()??;
        if terminal.body.terminal_offset != Some(consumed as u64) {
            return None;
        }
        collector.settle()
    } else {
        if terminal.body.truncated {
            return None;
        }
        collector.entity(bytes).ok()?
    };
    if !matches!(settled.delivery, transport::Delivery::Absent)
        || terminal
            .usage
            .as_ref()
            .is_some_and(|usage| Some(usage) != settled.usage.as_ref())
    {
        return None;
    }
    Some(CompletionMetadata {
        stop: settled.stop,
        usage: settled.usage,
    })
}

fn load_attempt(
    dir: &Path,
    attempt: u32,
    case: &Case,
    input: &str,
    plan: &Plan,
    plan_digest: &str,
    raw: bool,
) -> LoadedAttempt {
    let mut loaded = LoadedAttempt {
        attempt,
        state: AttemptState::NotStarted,
        reservation: None,
        terminal: None,
        artifact: None,
        outcome: Outcome::Unknown,
        recovered_metadata: None,
    };
    let invalid = |mut loaded: LoadedAttempt, what: &'static str| {
        loaded.state = AttemptState::Invalid(what);
        loaded
    };
    if !exists(dir) {
        return loaded;
    }
    if directory(dir).is_err() {
        return invalid(loaded, "attempt entry is not a nonsymlink directory");
    }
    let reservation_path = dir.join("reservation.json");
    if !exists(&reservation_path) {
        // Only an empty pre-reservation crash directory is honestly not started;
        // any surviving evidence without its reservation is corruption.
        return match fs::read_dir(dir).map(|mut entries| entries.next().is_none()) {
            Ok(true) => loaded,
            _ => invalid(loaded, "evidence without a reservation"),
        };
    }
    let Ok(reservation_bytes) = read(&reservation_path, RESERVATION_CAP) else {
        return invalid(loaded, "reservation unreadable, nonregular or oversized");
    };
    let Ok(reservation) = crate::record::parse::<Reservation>(&reservation_bytes) else {
        return invalid(loaded, "reservation is not a closed record");
    };
    // The request projection is recomputed from the frozen plan and case, never
    // trusted from the receipt.
    let expected_body =
        transport::request_body(&plan.system.model, &case.messages, &plan.protocol).ok();
    if reservation.version != 1
        || reservation.attempt != attempt
        || reservation.case_id != case.id
        || reservation.input != input
        || reservation.plan != plan_digest
        || Some(&reservation.endpoint) != plan.system.endpoint.as_ref()
        || reservation.method != "POST"
        || reservation.headers != transport::headers(plan.protocol.stream)
        || reservation.auth_env != plan.transport.auth_env
        || reservation.body.len() > REQUEST_CAP
        || expected_body.as_ref() != Some(&reservation.body)
        || reservation.request != identity::bytes("request", reservation.body.as_bytes())
    {
        return invalid(
            loaded,
            "reservation does not match the plan, case or request projection",
        );
    }
    let reservation_digest = identity::bytes("reservation-source", &reservation_bytes);
    loaded.reservation = Some(reservation_digest.clone());
    let terminal_path = dir.join("terminal.json");
    if !exists(&terminal_path) {
        loaded.state = AttemptState::Unresolved;
        return loaded;
    }
    let Ok(terminal_bytes) = read(&terminal_path, TERMINAL_CAP) else {
        return invalid(loaded, "terminal unreadable, nonregular or oversized");
    };
    let Ok(terminal) = crate::record::parse::<Terminal>(&terminal_bytes) else {
        return invalid(loaded, "terminal is not a closed record");
    };
    if terminal.attempt != attempt
        || terminal.case_id != case.id
        || terminal.input != input
        || terminal.reservation != reservation_digest
        || terminal.request != reservation.request
    {
        return invalid(loaded, "terminal lineage does not match its reservation");
    }
    if !terminal_consistent(&terminal, plan.protocol.stream, &plan.protocol) {
        return invalid(
            loaded,
            "terminal is physically or semantically contradictory",
        );
    }
    if raw {
        match check_file(
            &dir.join("body.bin"),
            RESPONSE_CAP,
            terminal.body.bytes,
            &terminal.body.sha256,
        ) {
            Ok(bytes) => {
                loaded.recovered_metadata =
                    legacy_completion_metadata(&terminal, &bytes, &plan.protocol);
            }
            Err(_) => return invalid(loaded, "raw body evidence missing or mismatched"),
        }
    }
    if let Some(final_artifact) = &terminal.final_artifact {
        match check_file(
            &dir.join("final.txt"),
            ARTIFACT_CAP,
            final_artifact.bytes,
            &final_artifact.sha256,
        ) {
            Ok(artifact) => {
                loaded.artifact = Some(identity::bytes("artifact", &artifact));
                loaded.outcome = grade::grade(&artifact, &case.acceptance);
            }
            Err(_) => return invalid(loaded, "final artifact missing or mismatched"),
        }
    } else if terminal.reason == Reason::Refused {
        loaded.outcome = Outcome::Refused;
    }
    loaded.terminal = Some(identity::bytes("terminal-source", &terminal_bytes));
    loaded.state = AttemptState::Terminal(Box::new(terminal));
    loaded
}

/// Read-only, integrity-checked load of a run directory or run-derived grade
/// view. Fixed directory components must be nonsymlink directories; per-attempt
/// evidence problems become `Invalid` states rather than aborting, so crash
/// windows can be inspected; grading refuses invalid states.
pub(crate) fn load(path: &Path) -> Result<Loaded> {
    let kind = kind(path)?;
    if kind == Kind::Submitted {
        return Err("expected a run directory or run-derived grade view".into());
    }
    let pack_bytes = read(&path.join("pack.json"), PACK_CAP)?;
    let pack = pack::admit(&pack_bytes)?;
    let (identities, inputs) = identity::pack(&pack, &pack_bytes)?;
    let plan_bytes = read(&path.join("plan.json"), PLAN_CAP)?;
    let plan: Plan = crate::record::parse(&plan_bytes).map_err(|e| format!("plan JSON: {e}"))?;
    if plan.version != 1 || plan.pack != identities || plan.cases as usize != pack.cases.len() {
        return Err("plan does not match the admitted pack snapshot".into());
    }
    pack::system(&plan.system)?;
    pack::protocol(&plan.protocol)?;
    // The bound endpoint and fixed transport declaration are recomputed from
    // the plan's own system/protocol, so a plan cannot describe a client that
    // this binary would not have built.
    let bound = plan
        .system
        .endpoint
        .as_deref()
        .ok_or_else(|| String::from("run plan requires a bound endpoint"))?;
    let endpoint = transport::Endpoint::parse(bound, true)?;
    if endpoint.text() != bound
        || plan.transport
            != transport::plan_transport(
                endpoint.local,
                plan.protocol.stream,
                plan.transport.auth_env.clone(),
            )
    {
        return Err("plan transport declaration does not match the fixed profile".into());
    }
    let plan_digest = identity::bytes("plan-source", &plan_bytes);
    let attempts_dir = path.join("attempts");
    directory(&attempts_dir)
        .map_err(|e| format!("required attempts directory is missing or unsafe: {e}"))?;
    let attempts = pack
        .cases
        .iter()
        .zip(&inputs)
        .enumerate()
        .map(|(i, (case, input))| {
            let attempt = i as u32;
            load_attempt(
                &attempts_dir.join(attempt_name(attempt)),
                attempt,
                case,
                input,
                &plan,
                &plan_digest,
                kind == Kind::Run,
            )
        })
        .collect();
    Ok(Loaded {
        kind,
        pack,
        pack_bytes,
        identities,
        inputs,
        plan,
        plan_bytes,
        plan_digest,
        attempts,
    })
}

/// Apply the built-in grader to loaded evidence. Never generates answer text.
pub(crate) fn run_view(loaded: &Loaded) -> Result<RunView> {
    let cases = loaded
        .pack
        .cases
        .iter()
        .zip(&loaded.inputs)
        .zip(&loaded.attempts)
        .map(|((case, input), a)| {
            let (evidence, reason) = match &a.state {
                AttemptState::NotStarted => (RunEvidence::NotStarted, None),
                AttemptState::Unresolved => (RunEvidence::Unresolved, None),
                AttemptState::Invalid(what) => {
                    return Err(format!(
                        "attempt {} for case {}: {what}",
                        a.attempt, case.id
                    ));
                }
                AttemptState::Terminal(t) => (
                    match t.reason {
                        Reason::Committed => RunEvidence::Collected,
                        Reason::Refused => RunEvidence::Refused,
                        _ => RunEvidence::Uncommitted,
                    },
                    Some(t.reason),
                ),
            };
            Ok(RunCaseGrade {
                case_id: case.id.clone(),
                input: input.clone(),
                attempt: a.attempt,
                terminal: a.terminal.clone(),
                reason,
                artifact: a.artifact.clone(),
                evidence,
                outcome: a.outcome,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RunView {
        version: 1,
        claim: RunViewClaim::ClientCollectedArtifactsNotAuthenticatedExecution,
        identities: RunIdentities {
            pack: PackIdentities {
                source: loaded.identities.source.clone(),
                tasks: loaded.identities.tasks.clone(),
                target: loaded.identities.target.clone(),
                qualification: loaded.identities.qualification.clone(),
                grading: loaded.identities.grading.clone(),
            },
            plan: loaded.plan_digest.clone(),
            protocol: identity::protocol(&loaded.plan.protocol)?,
            system: identity::system(&loaded.plan.system)?,
        },
        cases,
    })
}

pub(crate) fn receipt(view: &RunView) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(view).map_err(|e| format!("view encoding: {e}"))?;
    if bytes.len() > RECEIPT_CAP {
        return Err("grade view receipt exceeds its byte limit".into());
    }
    Ok(bytes)
}

fn saved_view(path: &Path) -> Result<RunView> {
    let bytes = read(path, RECEIPT_CAP)?;
    crate::record::parse(&bytes).map_err(|e| format!("view JSON: {e}"))
}

pub(crate) fn view_path(loaded_kind: Kind, base: &Path) -> PathBuf {
    match loaded_kind {
        Kind::Run => base.join(INITIAL_VIEW).join("view.json"),
        _ => base.join("view.json"),
    }
}

/// Verify a saved run-derived receipt against recomputed evidence.
pub(crate) fn verify_run(loaded: &Loaded, path: &Path) -> Result<RunView> {
    let expected = run_view(loaded)?;
    if loaded.kind == Kind::Run {
        directory(&path.join("grades"))?;
        directory(&path.join(INITIAL_VIEW))?;
    }
    let saved = saved_view(&view_path(loaded.kind, path))?;
    if saved != expected {
        if loaded.kind == Kind::Run && crate::lifecycle::historical_initial(path, loaded) {
            return Err("initial grade receipt is historical after continuation; regrade into a new view before comparison".into());
        }
        return Err(
            "run view receipt does not match recomputed identities/evidence/outcomes".into(),
        );
    }
    Ok(expected)
}

pub(crate) fn verify(path: &Path) -> Result<Verified> {
    match kind(path)? {
        Kind::Submitted => verify_submitted(path),
        Kind::Run | Kind::RunView => {
            let loaded = load(path)?;
            let view = verify_run(&loaded, path)?;
            let i = view.identities;
            Ok(Verified {
                claim: "client-collected-artifacts-not-authenticated-execution",
                source: i.pack.source,
                tasks: i.pack.tasks,
                target: i.pack.target,
                protocol: i.protocol,
                grading: i.pack.grading,
                qualification: i.pack.qualification,
                system: loaded.plan.system,
                rendering_unknown: loaded.plan.protocol.rendering.status
                    == RenderingStatus::Unknown,
                cases: view
                    .cases
                    .into_iter()
                    .zip(&loaded.attempts)
                    .map(|(c, attempt)| VerifiedCase {
                        case_id: c.case_id,
                        input: c.input,
                        outcome: c.outcome,
                        result: crate::report::attempt_result(attempt),
                    })
                    .collect(),
            })
        }
    }
}

fn copy_checked(from: &Path, to: &Path, cap: usize, domain: &str, digest: &str) -> Result<()> {
    let bytes = read(from, cap)?;
    if identity::bytes(domain, &bytes) != digest {
        return Err(format!(
            "evidence changed during regrade: {}",
            from.display()
        ));
    }
    write_new(to, &bytes)
}

/// Offline regrade: apply the current grader to a run's saved evidence and
/// publish a fresh self-contained grade view (receipts and final artifacts,
/// never raw bodies). No client, credential or network is involved.
pub(crate) fn regrade(run: &Path, out: &Path) -> Result<RunView> {
    let loaded = load(run)?;
    let view = run_view(&loaded)?;
    let receipt = receipt(&view)?;
    let parent = parent_of(out);
    directory(parent)?;
    create_directory(out)?;
    sync_directory(parent)?;
    write_new(&out.join("pack.json"), &loaded.pack_bytes)?;
    write_new(&out.join("plan.json"), &loaded.plan_bytes)?;
    let attempts = out.join("attempts");
    create_directory(&attempts)?;
    sync_directory(out)?;
    let source_attempts = run.join("attempts");
    for a in &loaded.attempts {
        let Some(reservation) = &a.reservation else {
            continue;
        };
        let name = attempt_name(a.attempt);
        let from = source_attempts.join(&name);
        let to = attempts.join(&name);
        create_directory(&to)?;
        copy_checked(
            &from.join("reservation.json"),
            &to.join("reservation.json"),
            RESERVATION_CAP,
            "reservation-source",
            reservation,
        )?;
        if let (Some(terminal), AttemptState::Terminal(t)) = (&a.terminal, &a.state) {
            copy_checked(
                &from.join("terminal.json"),
                &to.join("terminal.json"),
                TERMINAL_CAP,
                "terminal-source",
                terminal,
            )?;
            if let (Some(_), Some(artifact)) = (&t.final_artifact, &a.artifact) {
                copy_checked(
                    &from.join("final.txt"),
                    &to.join("final.txt"),
                    ARTIFACT_CAP,
                    "artifact",
                    artifact,
                )?;
            }
        }
        sync_directory(&to)?;
    }
    sync_directory(&attempts)?;
    publish_receipt(out, "view.json", &receipt)?;
    sync_directory(parent)?;
    Ok(view)
}
