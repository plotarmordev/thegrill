use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Polling cadence and cleanup allowance, not real-time guarantees.
pub(crate) const PROGRAM_POLL: Duration = Duration::from_millis(2);
const PROGRAM_CLEANUP: Duration = Duration::from_millis(100);

pub struct Request<'a> {
    pub program: &'a Path,
    pub argv: &'a [OsString],
    pub env: &'a [(OsString, OsString)],
    pub cwd: Option<&'a Path>,
    pub deadline: Duration,
    pub stdout_cap: usize,
    pub stderr_cap: usize,
    pub truncation_fatal: bool,
}

#[derive(Default)]
pub struct Output {
    pub status: Option<ExitStatus>,
    pub spawned: bool,
    /// Stops at observed leader exit, excluding subsequent pipe drainage.
    pub exit_observed_us: u64,
    /// Includes cleanup to preserve the microbench receipt's timing contract.
    pub elapsed_with_cleanup_ms: u64,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub failure: Option<String>,
}

impl Output {
    pub(crate) fn fail(&mut self, detail: impl AsRef<str>) {
        if let Some(failure) = &mut self.failure {
            failure.push_str("; ");
            failure.push_str(detail.as_ref());
        } else {
            self.failure = Some(detail.as_ref().to_string());
        }
    }
}

/// Scoped to the synchronous external-capture command, including publication.
/// Existing serving collectors own their separate process-wide signal latch.
pub mod program_signals {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ACTIVE: Mutex<()> = Mutex::new(());
    static CANCELLED: AtomicBool = AtomicBool::new(false);

    extern "C" fn cancel(_: libc::c_int) {
        CANCELLED.store(true, Ordering::SeqCst);
    }

    pub fn cancelled() -> bool {
        CANCELLED.load(Ordering::SeqCst)
    }

    pub struct Guard {
        previous: Vec<(libc::c_int, libc::sigaction)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Guard {
        pub fn install() -> crate::model::Result<Self> {
            let lock = ACTIVE
                .try_lock()
                .map_err(|_| "external capture already active")?;
            CANCELLED.store(false, Ordering::SeqCst);
            let mut guard = Self {
                previous: Vec::new(),
                _lock: lock,
            };
            // Auto-reaping or a competing SIGCHLD handler can release the PID/PGID.
            // SAFETY: sigaction is plain C data; all-zero is a valid value.
            let mut chld: libc::sigaction = unsafe { std::mem::zeroed() };
            // SAFETY: a null new action only queries into the owned out-parameter.
            if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut chld) } != 0
                || chld.sa_sigaction != libc::SIG_DFL
                || chld.sa_flags & libc::SA_NOCLDWAIT != 0
            {
                return Err("external capture requires a waitable child leader".into());
            }
            for signal in [libc::SIGINT, libc::SIGTERM] {
                // SAFETY: sigaction is plain C data; all-zero is a valid value.
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                action.sa_sigaction = cancel as *const () as libc::sighandler_t;
                // SAFETY: initializes the mask of the owned action.
                unsafe { libc::sigemptyset(&mut action.sa_mask) };
                // SAFETY: sigaction is plain C data; all-zero is a valid value.
                let mut previous = unsafe { std::mem::zeroed() };
                // SAFETY: `cancel` only performs an atomic store, so it is
                // async-signal-safe; both pointers are live locals.
                if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
                    return Err(format!(
                        "install capture signal handler: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                guard.previous.push((signal, previous));
            }
            Ok(guard)
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            for (signal, previous) in self.previous.iter().rev() {
                // SAFETY: restores the action saved when this signal was installed.
                unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
            }
        }
    }
}

fn nonblocking(stream: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let fd = stream.as_raw_fd();
    // SAFETY: fcntl only operates on the fd borrowed from `stream` for this call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: as above; the new flags extend the flags just read.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// At most 16 reads per stream per tick, so a busy writer cannot starve the timer.
fn drain(
    stream: &mut impl std::io::Read,
    bytes: &mut Vec<u8>,
    cap: usize,
    truncated: &mut bool,
    done: &mut bool,
) -> std::io::Result<()> {
    if *done {
        return Ok(());
    }
    let mut buffer = [0_u8; 8192];
    for _ in 0..16 {
        match stream.read(&mut buffer) {
            Ok(0) => {
                *done = true;
                break;
            }
            Ok(count) => {
                let retained = count.min(cap - bytes.len());
                bytes.extend_from_slice(&buffer[..retained]);
                *truncated |= retained < count;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => break,
            Err(error) => {
                *truncated = true;
                *done = true;
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Observe exit without reaping: the owned leader reserves its PID and PGID.
fn leader_exited(pid: u32) -> std::io::Result<bool> {
    // SAFETY: siginfo_t is plain C data; all-zero is a valid value.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: WNOWAIT observes without reaping; `info` is an owned out-parameter.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(exited_pid(&info) != 0)
}

#[cfg(target_os = "linux")]
fn exited_pid(info: &libc::siginfo_t) -> libc::pid_t {
    // SAFETY: waitid initialized si_pid, including zero when no change occurred.
    unsafe { info.si_pid() }
}

#[cfg(not(target_os = "linux"))]
fn exited_pid(info: &libc::siginfo_t) -> libc::pid_t {
    info.si_pid
}

/// The caller holds a program_signals::Guard through capture and publication.
/// No wait/reader joins: normal-exit drainage uses the remaining run deadline.
pub fn run(_guard: &program_signals::Guard, request: &Request<'_>) -> Output {
    use std::os::unix::process::CommandExt;
    let started = Instant::now();
    let mut run = Output::default();
    if program_signals::cancelled() {
        run.cancelled = true;
        run.fail("collector cancelled before program launch");
        return run;
    }
    let mut command = Command::new(request.program);
    command
        .args(request.argv)
        .envs(request.env.iter().map(|(key, value)| (key, value)));
    if let Some(cwd) = request.cwd {
        command.current_dir(cwd);
    }
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            run.fail(format!("spawn {}: {error}", request.program.display()));
            run.elapsed_with_cleanup_ms = started.elapsed().as_millis() as u64;
            run.exit_observed_us = started.elapsed().as_micros() as u64;
            return run;
        }
    };
    run.spawned = true;
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out_done = false;
    let mut err_done = false;
    if let Err(error) = nonblocking(&stdout) {
        run.fail(format!("stdout nonblocking setup: {error}"));
        run.stdout_truncated = true;
        out_done = true; // Never read a descriptor whose nonblocking setup failed.
    }
    if let Err(error) = nonblocking(&stderr) {
        run.fail(format!("stderr nonblocking setup: {error}"));
        run.stderr_truncated = true;
        err_done = true;
    }
    let mut cleanup_started = None;
    loop {
        if let Err(error) = drain(
            &mut stdout,
            &mut run.stdout,
            request.stdout_cap,
            &mut run.stdout_truncated,
            &mut out_done,
        ) {
            run.fail(format!("stdout read failed: {error}"));
        }
        if let Err(error) = drain(
            &mut stderr,
            &mut run.stderr,
            request.stderr_cap,
            &mut run.stderr_truncated,
            &mut err_done,
        ) {
            run.fail(format!("stderr read failed: {error}"));
        }
        if program_signals::cancelled() && !run.cancelled {
            run.fail("collector cancelled by SIGINT or SIGTERM");
            run.cancelled = true;
        }
        if cleanup_started.is_none() {
            if request.truncation_fatal && (run.stdout_truncated || run.stderr_truncated) {
                run.fail("program stdout/stderr capture truncated (cap or read/setup failure)");
            }
            if started.elapsed() >= request.deadline {
                run.timed_out = true;
                run.fail(format!(
                    "program exceeded the declared {} ms deadline",
                    request.deadline.as_millis()
                ));
            }
            let exited = match leader_exited(pid) {
                Ok(exited) => exited,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => false,
                Err(error) => {
                    // In particular ECHILD means identity is no longer reserved.
                    // Never signal a numeric PID/PGID after that loss of ownership.
                    run.fail(format!(
                        "observe owned leader failed; group not signalled: {error}"
                    ));
                    break;
                }
            };
            if exited {
                run.exit_observed_us = started.elapsed().as_micros() as u64;
            }
            if exited || run.failure.is_some() {
                // This is the ONLY group signal. WNOWAIT above kept the leader
                // unreaped even on normal exit; no try_wait occurs before it.
                // SAFETY: kill takes plain integers; the unreaped leader keeps
                // its PGID reserved, so the signal cannot reach a reused group.
                if unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) } != 0 {
                    let error = std::io::Error::last_os_error();
                    // XNU skips exited members: an owned zombie-only group returns EPERM.
                    if !(cfg!(target_os = "macos")
                        && exited
                        && error.raw_os_error() == Some(libc::EPERM))
                    {
                        run.fail(format!("owned group cleanup signal failed: {error}"));
                    }
                }
                let cleanup = Instant::now();
                let drain_until = if run.failure.is_none() {
                    started + request.deadline
                } else {
                    cleanup + PROGRAM_CLEANUP
                };
                cleanup_started = Some((cleanup, drain_until));
            }
        }
        if let Some((cleanup, drain_until)) = cleanup_started {
            // No further group signals are permitted after this reap.
            if run.status.is_none() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if run.exit_observed_us == 0 {
                            run.exit_observed_us = started.elapsed().as_micros() as u64;
                        }
                        run.status = Some(status);
                    }
                    Ok(None) => (),
                    Err(error) => {
                        run.fail(format!("owned leader reap failed: {error}"));
                        break;
                    }
                }
            }
            if run.status.is_some() && out_done && err_done {
                break;
            }
            if Instant::now() >= drain_until
                || (cleanup.elapsed() >= PROGRAM_CLEANUP && (run.status.is_none() || run.cancelled))
            {
                if run.status.is_none() {
                    run.fail("owned leader not reaped within 100 ms cleanup allowance");
                }
                break;
            }
        }
        std::thread::sleep(PROGRAM_POLL);
    }
    finish(&mut run, out_done, err_done, request.truncation_fatal);
    if run.exit_observed_us == 0 {
        run.exit_observed_us = started.elapsed().as_micros() as u64;
    }
    run.elapsed_with_cleanup_ms = started.elapsed().as_millis() as u64;
    run
}

fn finish(run: &mut Output, out_done: bool, err_done: bool, truncation_fatal: bool) {
    if !out_done {
        run.stdout_truncated = true;
        if truncation_fatal {
            run.fail("stdout drainage incomplete at cleanup boundary");
        }
    }
    if !err_done {
        run.stderr_truncated = true;
        if truncation_fatal {
            run.fail("stderr drainage incomplete at cleanup boundary");
        }
    }
    if truncation_fatal && (run.stdout_truncated || run.stderr_truncated) {
        run.fail("raw output is truncated; retained prefixes only");
    }
    if !run.status.is_some_and(|status| status.success()) {
        run.fail(format!(
            "program exited with status {:?}",
            run.status.and_then(|status| status.code())
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    static ACTIVE_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(target_os = "linux")]
    fn delayed_pipe_close(cancel: bool) -> Output {
        let root =
            std::env::temp_dir().join(format!("grill-drain-{}-{cancel}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let leader = root.join("leader");
        let release = root.join("release");
        let (finished, completion) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(10);
            while !leader.exists() {
                assert!(Instant::now() < until, "leader did not start");
                std::thread::sleep(PROGRAM_POLL);
            }
            let pid = loop {
                let pid = std::fs::read_to_string(&leader).unwrap();
                if !pid.is_empty() {
                    break pid;
                }
                assert!(Instant::now() < until);
                std::thread::sleep(PROGRAM_POLL);
            };
            let proc = std::path::PathBuf::from(format!("/proc/{pid}"));
            // Duplicated writers model a killed child that has not closed its
            // pipes yet. SIGSTOP cannot delay SIGKILL on Linux.
            let stdout = std::fs::OpenOptions::new()
                .write(true)
                .open(proc.join("fd/1"))
                .unwrap();
            let stderr = std::fs::OpenOptions::new()
                .write(true)
                .open(proc.join("fd/2"))
                .unwrap();
            std::fs::write(release, b"").unwrap();
            while proc.exists() {
                assert!(Instant::now() < until, "leader not reaped");
                std::thread::sleep(PROGRAM_POLL);
            }
            // Reaping happens after group SIGKILL, so this exceeds the old
            // window regardless of how long startup took.
            std::thread::sleep(Duration::from_millis(250));
            if cancel {
                // The leader is already reaped, so cancellation must interrupt
                // normal-exit drainage rather than the running-program path.
                // SAFETY: the collector guard has installed the SIGINT handler.
                assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
                let completed = completion.recv_timeout(Duration::from_secs(5)).is_ok();
                drop((stdout, stderr));
                assert!(completed, "cancellation waited for pipe EOF");
                return;
            }
            drop((stdout, stderr));
        });
        let output = shell(
            &format!(
                "printf '%s' $$ > '{0}/leader'; while [ ! -e '{0}/release' ]; do sleep 0.01; done; sleep 30 & printf honest; printf evidence >&2",
                root.display()
            ),
            Duration::from_secs(10),
            1024,
            true,
        );
        let _ = finished.send(());
        holder.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
        output
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn normal_exit_waits_for_delayed_pipe_close() {
        let output = delayed_pipe_close(false);
        assert!(output.failure.is_none(), "{:?}", output.failure);
        assert!(output.status.unwrap().success());
        assert_eq!(output.stdout, b"honest");
        assert_eq!(output.stderr, b"evidence");
        assert!(!output.stdout_truncated && !output.stderr_truncated);
        assert!(!output.timed_out && !output.cancelled);
        assert!(output.elapsed_with_cleanup_ms >= output.exit_observed_us / 1000 + 250);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn cancellation_interrupts_normal_exit_pipe_drain() {
        let output = delayed_pipe_close(true);
        assert!(output.status.unwrap().success());
        assert!(output.cancelled);
        assert!(!output.timed_out);
        assert_eq!(output.stdout, b"honest");
        assert_eq!(output.stderr, b"evidence");
        assert!(output.stdout_truncated && output.stderr_truncated);
        assert_eq!(
            output.failure.as_deref(),
            Some(
                "collector cancelled by SIGINT or SIGTERM; stdout drainage incomplete at cleanup boundary; stderr drainage incomplete at cleanup boundary; raw output is truncated; retained prefixes only"
            )
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn escaped_descendant_pipe_is_bounded_by_deadline() {
        let root = std::env::temp_dir().join(format!("grill-escaped-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let pid_file = root.join("pid");
        let deadline = Duration::from_secs(2);
        let output = shell(
            &format!(
                "setsid sh -c 'printf \"%s\" $$ > \"{}\"; exec sleep 30' & while [ ! -s '{}' ]; do sleep 0.01; done; printf honest",
                pid_file.display(),
                pid_file.display()
            ),
            deadline,
            1024,
            true,
        );
        let pid: libc::pid_t = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        // SAFETY: the fixture's escaped sleeper remains alive beyond the deadline.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        std::fs::remove_dir_all(root).unwrap();
        assert!(output.status.unwrap().success());
        assert!(!output.timed_out && !output.cancelled);
        assert_eq!(output.stdout, b"honest");
        assert!(output.stdout_truncated && output.stderr_truncated);
        assert_eq!(
            output.failure.as_deref(),
            Some(
                "stdout drainage incomplete at cleanup boundary; stderr drainage incomplete at cleanup boundary; raw output is truncated; retained prefixes only"
            )
        );
        assert!(output.elapsed_with_cleanup_ms >= deadline.as_millis() as u64);
        // Scheduling can delay observation, but the escaped sleeper is not awaited.
        assert!(output.elapsed_with_cleanup_ms < deadline.as_millis() as u64 + 5000);
    }

    fn shell(script: &str, deadline: Duration, cap: usize, fatal: bool) -> Output {
        let _lock = ACTIVE_TEST
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let guard = program_signals::Guard::install().unwrap();
        run(
            &guard,
            &Request {
                program: Path::new("/bin/sh"),
                argv: &["-c".into(), script.into()],
                env: &[("GRILL_PROCESS_TEST".into(), "inherited addition".into())],
                cwd: Some(Path::new("/")),
                deadline,
                stdout_cap: cap,
                stderr_cap: cap,
                truncation_fatal: fatal,
            },
        )
    }

    #[test]
    fn normal_exit_captures_streams_environment_and_cwd() {
        let output = shell(
            "printf '%s' \"$GRILL_PROCESS_TEST\"; pwd >&2",
            Duration::from_secs(5),
            1024,
            true,
        );
        assert!(output.spawned);
        assert!(output.status.unwrap().success());
        assert_eq!(output.stdout, b"inherited addition");
        assert_eq!(output.stderr, b"/\n");
        assert!(output.failure.is_none(), "{:?}", output.failure);
        assert!(!output.timed_out && !output.cancelled);
        assert!(!output.stdout_truncated && !output.stderr_truncated);
        assert!(output.exit_observed_us > 0);
        assert!(output.exit_observed_us / 1000 <= output.elapsed_with_cleanup_ms);
    }

    #[test]
    fn nonzero_exit_retains_status_and_output() {
        let output = shell(
            "printf failure >&2; exit 7",
            Duration::from_secs(5),
            1024,
            true,
        );
        assert_eq!(output.status.unwrap().code(), Some(7));
        assert_eq!(output.stderr, b"failure");
        assert!(!output.timed_out && !output.cancelled);
        assert_eq!(
            output.failure.as_deref(),
            Some("program exited with status Some(7)")
        );
    }

    #[test]
    fn deadline_kills_sleeping_descendant() {
        let output = shell(
            "sleep 60 & printf '%s' \"$!\"; wait",
            Duration::from_millis(500),
            1024,
            true,
        );
        assert!(output.timed_out);
        assert!(!output.cancelled);
        assert_eq!(output.status.unwrap().signal(), Some(libc::SIGKILL));
        let pid = std::str::from_utf8(&output.stdout)
            .unwrap()
            .parse::<i32>()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let state = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            let state = String::from_utf8(state.stdout).unwrap();
            // An orphan zombie has exited; only the system reaper can remove it.
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                break;
            }
            assert!(Instant::now() < until, "descendant still alive: {state}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn truncation_policy_controls_termination() {
        for fatal in [true, false] {
            let output = shell(
                "printf abcdef; printf uvwxyz >&2; sleep 0.1; exit 0",
                Duration::from_secs(5),
                3,
                fatal,
            );
            assert_eq!(output.stdout, b"abc");
            assert_eq!(output.stderr, b"uvw");
            assert!(output.stdout_truncated && output.stderr_truncated);
            assert!(!output.timed_out && !output.cancelled);
            if fatal {
                assert_eq!(output.status.unwrap().signal(), Some(libc::SIGKILL));
                assert!(output.failure.unwrap().contains("truncated"));
            } else {
                assert!(output.status.unwrap().success());
                assert!(output.failure.is_none(), "{:?}", output.failure);
            }
        }
    }
}
