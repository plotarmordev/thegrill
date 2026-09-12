use crate::{evidence, model::*, run::Summary};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SESSION_CAP: usize = 2048;

// Lock the directory inode itself: no stale PID guesses or removable lock file.
// The descriptor lives through validation, collection and final publication.
pub fn ownership(root: &Path) -> Result<File> {
    evidence::directory(root)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|e| format!("open run ownership: {e}"))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(
            "run already has an active collector/resumer (or filesystem locking failed)".into(),
        );
    }
    Ok(file)
}

// A separate session-directory lock serializes pause publication with reservation.
// It is released before network collection; control requests never fsync evidence.
fn admission_file(session: &Path) -> Result<File> {
    evidence::directory(session)?;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(session)
        .map_err(|e| format!("open wave admission: {e}"))
}

pub fn admission(session: &Path) -> Result<File> {
    let file = admission_file(session)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err("cannot acquire wave admission lock".into());
    }
    Ok(file)
}

pub fn admission_bounded(session: &Path, deadline: Instant) -> Result<Option<File>> {
    let file = admission_file(session)?;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok((Instant::now() < deadline).then_some(file));
        }
        let error = std::io::Error::last_os_error();
        match error.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => (),
            _ => return Err(format!("cannot acquire wave admission lock: {error}")),
        }
        std::thread::sleep(
            Duration::from_millis(2).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub version: u32,
    pub index: usize,
    pub first_wave: usize,
    pub plan_sha256: String,
    pub collector_sha256: String,
    pub started_unix_ms: u64,
}

pub struct History {
    pub count: usize,
    pub next_wave: usize,
    pub last_status: Option<String>,
    pub open: bool,
    pub evidence_sha256: String,
    pub lineage_sha256: String,
}

pub fn dir(root: &Path, index: usize) -> PathBuf {
    root.join(format!("session-{index:06}"))
}

pub fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

// Called by the ordinary offline evidence loader, not a competing verifier.
pub fn history(
    root: &Path,
    plan: &Plan,
    plan_hash: &str,
    states: &[&str],
    waves: &[Option<Wave>],
) -> Result<History> {
    let mut indices = Vec::new();
    for (entry_count, entry) in fs::read_dir(root).map_err(|e| e.to_string())?.enumerate() {
        if entry_count > SESSION_CAP + 1024 + 16 {
            return Err("run directory entry count exceeds bound".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("session-") {
            let index = name
                .strip_prefix("session-")
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|i| name == format!("session-{i:06}"))
                .ok_or("invalid session directory name")?;
            indices.push(index);
            if indices.len() > SESSION_CAP {
                return Err("session count exceeds bound".into());
            }
        }
        if name.starts_with("wave-")
            && !name
                .strip_prefix("wave-")
                .and_then(|s| s.parse::<usize>().ok())
                .is_some_and(|i| i < plan.waves.len() && name == format!("wave-{i:06}"))
        {
            return Err("wave outside frozen schedule".into());
        }
    }
    indices.sort_unstable();
    if plan.version == 1 {
        if !indices.is_empty() {
            return Err("legacy plan cannot carry continuation sessions".into());
        }
        return Ok(History {
            count: 0,
            next_wave: 0,
            last_status: None,
            open: false,
            evidence_sha256: evidence::digest(b"legacy-session-absent"),
            lineage_sha256: evidence::digest(b"legacy-session-absent"),
        });
    }
    if indices.is_empty() {
        return Err("lifecycle plan has no execution session".into());
    }
    let mut next_wave = 0;
    let mut last_status = None;
    let mut open = false;
    let mut publication_uncertain = false;
    let mut fingerprint = Sha256::new();
    let mut lineage = Sha256::new();
    for (expected, index) in indices.iter().copied().enumerate() {
        if index != expected || open {
            return Err("noncontiguous or overlapping execution sessions".into());
        }
        let path = dir(root, index);
        evidence::directory(&path)?;
        let session_bytes = evidence::read(&path.join("session.json"), FILE_CAP)?;
        fingerprint.update(evidence::digest(&session_bytes).as_bytes());
        let session: Session =
            serde_json::from_slice(&session_bytes).map_err(|e| format!("invalid session: {e}"))?;
        lineage.update(session.started_unix_ms.to_le_bytes());
        lineage.update((session.first_wave as u64).to_le_bytes());
        if session.version != 1
            || session.index != index
            || session.first_wave != next_wave
            || session.plan_sha256 != plan_hash
            || session.collector_sha256 != plan.collector_sha256
        {
            return Err("execution session lineage mismatch".into());
        }
        if index != 0 && last_status.as_deref() != Some("paused") {
            return Err("continuation does not follow a cooperative pause".into());
        }
        if exists(&path.join("run.json"))? {
            let end_bytes = evidence::read(&path.join("run.json"), FILE_CAP)?;
            fingerprint.update(b"settled\0");
            fingerprint.update(evidence::digest(&end_bytes).as_bytes());
            lineage.update(evidence::digest(&end_bytes).as_bytes());
            let end: Summary = serde_json::from_slice(&end_bytes)
                .map_err(|e| format!("invalid session outcome: {e}"))?;
            if index == 0 {
                let receipt = root.join("run.json");
                if exists(&receipt)? {
                    fingerprint.update(b"root-present\0");
                    if evidence::read(&receipt, FILE_CAP)? != end_bytes {
                        return Err("first root receipt differs from session zero outcome".into());
                    }
                } else {
                    // Session publication precedes root publication. Do not repair or
                    // mistake this crash window (or later loss) for settled evidence.
                    publication_uncertain = true;
                    fingerprint.update(b"root-missing\0");
                }
            }
            if end.version != 1
                || end.session != index
                || end.first_wave != next_wave
                || end.planned_waves != plan.waves.len()
                || end.published_waves > plan.waves.len() - next_wave
            {
                return Err("execution session outcome mismatch".into());
            }
            let end_wave = next_wave + end.published_waves;
            if states[next_wave..end_wave]
                .iter()
                .any(|s| *s != "published")
            {
                return Err("session claims unpublished wave evidence".into());
            }
            let observed = &waves[next_wave..end_wave];
            let measured = observed
                .iter()
                .flatten()
                .filter(|w| w.spec.phase == Phase::Measured)
                .count();
            let eligible = observed
                .iter()
                .flatten()
                .filter(|w| w.spec.phase == Phase::Measured && w.eligible)
                .count();
            let bad_warmup = observed
                .iter()
                .flatten()
                .filter(|w| w.spec.phase == Phase::Warmup && !w.eligible)
                .count();
            if end.measured_waves != measured
                || end.eligible_measured_waves != eligible
                || end.ineligible_warmup_waves != bad_warmup
                || (end.status == "paused"
                    && observed
                        .iter()
                        .flatten()
                        .any(|w| w.attempts.iter().any(|a| a.status != Status::Complete)))
            {
                return Err("session outcome contradicts retained wave evidence".into());
            }
            if !matches!(
                end.status.as_str(),
                "completed"
                    | "completed-with-ineligible-measurements"
                    | "paused"
                    | "interrupted"
                    | "stopped-after-response-failure"
                    | "local-failure"
                    | "budget-exhausted"
                    | "stopped-after-ineligible-response"
                    | "stopped-after-sequence-check"
            ) || (end.status.starts_with("completed") && end_wave != plan.waves.len())
                || (plan.version < 3
                    && matches!(
                        end.status.as_str(),
                        "budget-exhausted" | "stopped-after-ineligible-response"
                    ))
                || (end.status == "stopped-after-sequence-check" && plan.workload.version != 2)
            {
                return Err("invalid session terminal status".into());
            }
            next_wave = end_wave;
            last_status = Some(end.status);
        } else {
            fingerprint.update(b"unsettled\0");
            open = true;
            last_status = None;
        }
    }
    if !open && states[next_wave..].iter().any(|s| *s != "not_started") {
        let suffix = &states[next_wave..];
        if last_status.as_deref() == Some("local-failure")
            && suffix.first() == Some(&"reserved_unsettled")
            && suffix[1..].iter().all(|s| *s == "not_started")
        {
            open = true;
        } else {
            return Err("evidence outside settled execution sessions".into());
        }
    }
    open |= publication_uncertain;
    Ok(History {
        count: indices.len(),
        next_wave,
        last_status,
        open,
        evidence_sha256: evidence::hex(&fingerprint.finalize()),
        lineage_sha256: evidence::hex(&lineage.finalize()),
    })
}

pub fn start(root: &Path, session: &Session) -> Result<PathBuf> {
    if session.index >= SESSION_CAP {
        return Err("session count exceeds bound".into());
    }
    let path = dir(root, session.index);
    evidence::fresh(&path)?;
    evidence::json(&path.join("session.json"), session)?;
    evidence::sync(&path)?;
    evidence::sync(root)?;
    Ok(path)
}

pub fn pause(root: &Path) -> Result<()> {
    // Do not hash evidence or fsync while collectors are active. The immutable
    // plan and bounded session inventory suffice to address this control marker.
    evidence::directory(root)?;
    let plan: Plan =
        serde_json::from_slice(&evidence::read(&root.join("plan.json"), 8 * 1024 * 1024)?)
            .map_err(|e| format!("invalid plan: {e}"))?;
    if !matches!(plan.version, 2 | 3) {
        return Err("pause requires a lifecycle-enabled performance run".into());
    }
    let mut active = None;
    for index in 0..SESSION_CAP {
        let path = dir(root, index);
        if !exists(&path)? {
            break;
        }
        evidence::directory(&path)?;
        active = Some(path);
    }
    let path = active.ok_or("no execution session to pause")?;
    if let Ok(_owner) = ownership(root) {
        return Err(
            "run has no active collector; inspect the retained session before resuming".into(),
        );
    }
    let _admission = admission(&path)?;
    if exists(&path.join("run.json"))? {
        return Err("session already settled; inspect its run.json outcome".into());
    }
    // A pause request does not claim liveness or completion. A crashed collector
    // remains uncertain; only its published session outcome establishes pause.
    match fs::create_dir(path.join("pause.request")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            evidence::directory(&path.join("pause.request"))
        }
        Err(e) => Err(format!("request pause: {e}")),
    }
}

pub fn requested(session: &Path) -> Result<bool> {
    let path = session.join("pause.request");
    if !exists(&path)? {
        return Ok(false);
    }
    evidence::directory(&path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_deadline_does_not_wait_for_a_held_lock() {
        let path = std::env::temp_dir().join(format!(
            "grill-perf-admission-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let held = admission(&path).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let other = path.clone();
        let waiter = std::thread::spawn(move || {
            let result = admission_bounded(&other, Instant::now() + Duration::from_millis(5));
            send.send(result.map(|file| file.is_none())).unwrap();
        });
        let result = receive.recv_timeout(Duration::from_secs(5));
        drop(held);
        waiter.join().unwrap();
        assert!(result.unwrap().unwrap());
        assert!(admission_bounded(&path, Instant::now()).unwrap().is_none());
        let available = admission_bounded(&path, Instant::now() + Duration::from_secs(1))
            .unwrap()
            .unwrap();
        drop(available);
        fs::remove_dir(path).unwrap();
    }
}
