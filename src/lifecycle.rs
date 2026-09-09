//! Cooperative admission control and immutable continuation sessions (Linux local filesystems).
use crate::contract::*;
use crate::store::{self, AttemptState, Loaded};
use crate::{identity, record};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const SESSION_CAP: usize = CASE_CAP + 1;
const META_CAP: usize = 8192;

pub(crate) struct Lock(File);
impl Lock {
    pub fn run(root: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(root)
            .map_err(|e| format!("open run ownership: {e}"))?;
        Self::acquire(file, true)
    }
    fn gate(root: &Path) -> Result<Self> {
        directory(&root.join("lifecycle"))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(root.join("lifecycle/admission.lock"))
            .map_err(|e| format!("open admission lock: {e}"))?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("admission lock is not a regular file".into());
        }
        Self::acquire(file, false)
    }
    fn acquire(file: File, nonblocking: bool) -> Result<Self> {
        // SAFETY: file owns a valid descriptor; flock does not retain a pointer.
        if unsafe {
            libc::flock(
                file.as_raw_fd(),
                libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 },
            )
        } != 0
        {
            return Err(
                "run is active or admission is in progress; retry the control command".into(),
            );
        }
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: the descriptor remains owned until after this call.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    version: u32,
    plan: String,
    previous: Option<String>,
    first_attempt: u32,
    started_unix_ms: u64,
    pid: u32,
    grill: String,
    lockfile: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct End {
    version: u32,
    start: String,
    next_attempt: u32,
    state: String,
    view: String,
    pause_requested: bool,
}
#[derive(Debug, Serialize)]
pub(crate) struct Status {
    pub state: String,
    pub sessions: usize,
    pub next_attempt: usize,
    pub detail: Option<String>,
}
struct History {
    next: usize,
    previous: Option<String>,
    count: usize,
    state: String,
    first_view: Option<Vec<u8>>,
}

fn present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}
fn directory(path: &Path) -> Result<()> {
    if !std::fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Err(format!("unsafe lifecycle directory: {}", path.display()));
    }
    Ok(())
}
fn encoded(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(|e| e.to_string())
}
fn prefix(loaded: &Loaded) -> Result<usize> {
    let mut next = 0;
    for a in &loaded.attempts {
        match &a.state {
            AttemptState::Terminal(_) if a.attempt as usize == next => next += 1,
            AttemptState::NotStarted => {}
            AttemptState::Unresolved => {
                return Err("uncertain reserved attempt; resume refuses replay".into());
            }
            AttemptState::Invalid(why) => return Err(format!("damaged attempt evidence: {why}")),
            _ => return Err("noncontiguous attempt evidence".into()),
        }
    }
    Ok(next)
}
fn snapshot_matches(saved: &RunView, current: &RunView, next: usize) -> bool {
    saved.version == current.version
        && saved.claim == current.claim
        && saved.identities == current.identities
        && saved.cases.len() == current.cases.len()
        && saved
            .cases
            .iter()
            .zip(&current.cases)
            .enumerate()
            .all(|(i, (old, now))| {
                if i < next {
                    old == now
                } else {
                    old.case_id == now.case_id
                        && old.input == now.input
                        && old.attempt == now.attempt
                        && old.terminal.is_none()
                        && old.reason.is_none()
                        && old.artifact.is_none()
                        && old.evidence == RunEvidence::NotStarted
                        && old.outcome == Outcome::Unknown
                }
            })
}
fn check_initial(root: &Path, first: Option<&[u8]>) -> Result<()> {
    if let Some(first) = first {
        directory(&root.join("grades"))?;
        directory(&root.join(store::INITIAL_VIEW))?;
        if store::read(
            &root.join(store::INITIAL_VIEW).join("view.json"),
            RECEIPT_CAP,
        )? != first
        {
            return Err("historical initial grade receipt changed".into());
        }
    }
    Ok(())
}

fn history(root: &Path, loaded: &Loaded) -> Result<History> {
    let base = root.join("lifecycle");
    directory(&base)?;
    let current = store::run_view(loaded)?;
    let mut h = History {
        next: 0,
        previous: None,
        count: 0,
        state: "ready".into(),
        first_view: None,
    };
    // Fixed names, a fixed ceiling, and rejection of unknown entries prevent hidden sessions.
    let entries = std::fs::read_dir(&base).map_err(|e| e.to_string())?;
    let mut names = std::collections::BTreeSet::new();
    for entry in entries {
        if names.len() > SESSION_CAP {
            return Err("too many lifecycle entries".into());
        }
        names.insert(entry.map_err(|e| e.to_string())?.file_name());
    }
    names.remove(std::ffi::OsStr::new("admission.lock"));
    for index in 0..SESSION_CAP {
        let name = format!("{index:06}");
        if !names.remove(std::ffi::OsStr::new(&name)) {
            break;
        }
        let dir = base.join(name);
        directory(&dir)?;
        let bytes = store::read(&dir.join("start.json"), META_CAP)?;
        let start: Start = record::parse(&bytes).map_err(|e| e.to_string())?;
        if start.version != 1
            || start.plan != loaded.plan_digest
            || start.previous != h.previous
            || start.first_attempt as usize != h.next
        {
            return Err("changed lifecycle start or plan".into());
        }
        h.count += 1;
        if !present(&dir.join("end.json")) {
            h.state = "uncertain".into();
            if !names.is_empty() {
                return Err("session after unsettled session".into());
            }
            check_initial(root, h.first_view.as_deref())?;
            return Ok(h);
        }
        let end_bytes = store::read(&dir.join("end.json"), META_CAP)?;
        let end: End = record::parse(&end_bytes).map_err(|e| e.to_string())?;
        let view_bytes = store::read(&dir.join("view.json"), RECEIPT_CAP)?;
        let view: RunView = record::parse(&view_bytes).map_err(|e| e.to_string())?;
        let next = end.next_attempt as usize;
        let pause = dir.join("pause.request");
        if present(&pause) != end.pause_requested
            || (end.pause_requested && store::read(&pause, META_CAP)? != end.start.as_bytes())
            || (end.state == "paused" && !end.pause_requested)
        {
            return Err("changed historical pause request".into());
        }
        if end.version != 1
            || end.start != identity::bytes("session-start", &bytes)
            || end.view != identity::bytes("session-view", &view_bytes)
            || next < h.next
            || next > loaded.attempts.len()
            || !matches!(
                end.state.as_str(),
                "paused" | "interrupted" | "blocked" | "completed"
            )
            || (end.state == "completed" && next != loaded.attempts.len())
            || (end.state == "paused" && next == loaded.attempts.len())
            || !snapshot_matches(&view, &current, next)
        {
            return Err("changed lifecycle settlement or saved evidence".into());
        }
        if index == 0 {
            h.first_view = Some(view_bytes);
        }
        h.next = next;
        h.previous = Some(identity::bytes("session-end", &end_bytes));
        h.state = end.state;
    }
    if !names.is_empty() {
        return Err("unknown or noncontiguous lifecycle entries".into());
    }
    if h.count == 0 {
        return Err("missing lifecycle session; legacy runs are inspection-only".into());
    }
    check_initial(root, h.first_view.as_deref())?;
    Ok(h)
}

pub(crate) struct Session {
    root: PathBuf,
    dir: PathBuf,
    start: String,
    pub index: usize,
}
impl Session {
    pub fn create(root: &Path, loaded: &Loaded, fresh: bool, now: u64) -> Result<Self> {
        let (index, previous, first) = if fresh {
            store::create_directory(&root.join("lifecycle"))?;
            store::write_new(&root.join("lifecycle/admission.lock"), b"")?;
            store::sync_directory(root)?;
            (0, None, 0)
        } else {
            let h = history(root, loaded)?;
            if h.state == "uncertain" {
                return Err(
                    "unsettled session; crash uncertainty requires offline review, not replay"
                        .into(),
                );
            }
            let next = prefix(loaded)?;
            if next != h.next {
                return Err("attempt evidence changed after session settlement".into());
            }
            if h.count >= SESSION_CAP {
                return Err("continuation session ceiling reached".into());
            }
            (h.count, h.previous, next)
        };
        let dir = root.join("lifecycle").join(format!("{index:06}"));
        store::create_directory(&dir)?;
        let bytes = encoded(&Start {
            version: 1,
            plan: loaded.plan_digest.clone(),
            previous,
            first_attempt: first as u32,
            started_unix_ms: now,
            pid: std::process::id(),
            grill: env!("CARGO_PKG_VERSION").into(),
            lockfile: identity::bytes("lockfile", include_bytes!("../Cargo.lock")),
        })?;
        store::publish_receipt(&dir, "start.json", &bytes)?;
        store::sync_directory(&root.join("lifecycle"))?;
        Ok(Self {
            root: root.to_path_buf(),
            dir,
            start: identity::bytes("session-start", &bytes),
            index,
        })
    }
    pub fn admission(&self) -> Result<(Lock, bool)> {
        let lock = Lock::gate(&self.root)?;
        let paused = if present(&self.dir.join("pause.request")) {
            let bytes = store::read(&self.dir.join("pause.request"), META_CAP)?;
            if bytes != self.start.as_bytes() {
                return Err("invalid pause request".into());
            }
            true
        } else {
            false
        };
        Ok((lock, paused))
    }
    pub fn finish(&self, loaded: &Loaded, state: &str) -> Result<()> {
        let (_gate, pause_requested) = self.admission()?;
        let next = prefix(loaded)?;
        let view = store::receipt(&store::run_view(loaded)?)?;
        store::publish_receipt(&self.dir, "view.json", &view)?;
        let end = End {
            version: 1,
            start: self.start.clone(),
            next_attempt: next as u32,
            state: state.into(),
            view: identity::bytes("session-view", &view),
            pause_requested,
        };
        store::publish_receipt(&self.dir, "end.json", &encoded(&end)?)
    }
}
pub(crate) fn resumable(root: &Path, loaded: &Loaded) -> Result<usize> {
    if loaded.kind != store::Kind::Run {
        return Err("resume requires original raw run evidence".into());
    }
    for entry in std::fs::read_dir(root.join("attempts")).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        let Some(name) = name.to_str() else {
            return Err("unexpected attempt filename".into());
        };
        let ordinal = name
            .parse::<usize>()
            .map_err(|_| "unexpected attempt filename")?;
        if ordinal >= loaded.attempts.len() || name != format!("{ordinal:06}") {
            return Err("unexpected attempt outside the frozen plan".into());
        }
        if matches!(&loaded.attempts[ordinal].state, AttemptState::NotStarted) {
            return Err(
                "partial reservation directory; refusing to reuse an ambiguous attempt".into(),
            );
        }
    }
    let h = history(root, loaded)?;
    if h.state == "uncertain" {
        return Err("unsettled session; crash uncertainty remains explicit".into());
    }
    let next = prefix(loaded)?;
    if next != h.next {
        return Err("attempt evidence changed after settlement".into());
    }
    if next == loaded.attempts.len() && matches!(h.state.as_str(), "interrupted" | "blocked") {
        return Err(format!(
            "run exhausted with {} execution; no attempts remain to resume",
            h.state
        ));
    }
    Ok(next)
}
pub(crate) fn pause(root: &Path) -> Result<&'static str> {
    if !present(&root.join("lifecycle")) {
        return Err("run has no lifecycle evidence; archival runs are not pause-capable".into());
    }
    let _gate = Lock::gate(root)?;
    // An idle run needs no mutation. A failed ownership acquisition means a live collector.
    if let Ok(_owner) = Lock::run(root) {
        let loaded = store::load(root)?;
        let h = history(root, &loaded)?;
        return Ok(if h.state == "completed" {
            "completed; no pause needed"
        } else {
            "inactive; inspect before resuming"
        });
    }
    let mut last = None;
    for index in 0..SESSION_CAP {
        let dir = root.join("lifecycle").join(format!("{index:06}"));
        if !present(&dir) {
            break;
        }
        directory(&dir)?;
        last = Some(dir);
    }
    let dir = last.ok_or("missing active session")?;
    if present(&dir.join("end.json")) {
        return Ok("session already drained; inspect run");
    }
    let start = store::read(&dir.join("start.json"), META_CAP)?;
    let token = identity::bytes("session-start", &start);
    if present(&dir.join("pause.request")) {
        if store::read(&dir.join("pause.request"), META_CAP)? != token.as_bytes() {
            return Err("invalid pause request".into());
        }
    } else {
        store::publish_receipt(&dir, "pause.request", token.as_bytes())?;
    }
    Ok("pause requested; active attempt drains before paused settlement; inspect run")
}
pub(crate) fn inspect(root: &Path, loaded: &Loaded) -> Status {
    if loaded.kind != store::Kind::Run || !present(&root.join("lifecycle")) {
        return Status {
            state: "legacy_inspection_only".into(),
            sessions: 0,
            next_attempt: 0,
            detail: None,
        };
    }
    match history(root, loaded) {
        Ok(h) => {
            let active = Lock::run(root).is_err();
            let pause = h.count > 0
                && present(
                    &root
                        .join("lifecycle")
                        .join(format!("{:06}", h.count - 1))
                        .join("pause.request"),
                );
            Status {
                state: if active {
                    if pause { "pause_requested" } else { "running" }.into()
                } else {
                    h.state
                },
                sessions: h.count,
                next_attempt: loaded
                    .attempts
                    .iter()
                    .take_while(|a| matches!(&a.state, AttemptState::Terminal(_)))
                    .count(),
                detail: None,
            }
        }
        Err(e) => Status {
            state: "blocked".into(),
            sessions: 0,
            next_attempt: 0,
            detail: Some(e),
        },
    }
}
pub(crate) fn historical_initial(root: &Path, loaded: &Loaded) -> bool {
    history(root, loaded).is_ok_and(|h| h.count > 1 && h.first_view.is_some())
}
