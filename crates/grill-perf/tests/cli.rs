use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "grill-perf-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Server {
    endpoint: String,
    stop: Arc<AtomicBool>,
    seen: std::sync::mpsc::Receiver<Value>,
    count: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    join: Option<thread::JoinHandle<()>>,
}
type Handler = dyn Fn(TcpStream, usize, Value) + Send + Sync;
impl Server {
    fn new(handler: impl Fn(TcpStream, usize, Value) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (send, seen) = std::sync::mpsc::sync_channel(4096);
        let count = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let handler: Arc<Handler> = Arc::new(handler);
        let (s, count2, peak2) = (stop.clone(), count.clone(), peak.clone());
        let join = thread::spawn(move || {
            let mut workers = Vec::new();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let (send, count, active, peak, handler) = (
                            send.clone(),
                            count2.clone(),
                            active.clone(),
                            peak2.clone(),
                            handler.clone(),
                        );
                        workers.push(thread::spawn(move || {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(5)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(5)))
                                .unwrap();
                            stream.set_nodelay(true).unwrap();
                            let request = read_request(&mut stream);
                            send.try_send(request.clone()).unwrap();
                            let index = count.fetch_add(1, Ordering::SeqCst);
                            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                            peak.fetch_max(now, Ordering::SeqCst);
                            handler(stream, index, request);
                            active.fetch_sub(1, Ordering::SeqCst);
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("fixture listener: {e}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            endpoint,
            stop,
            seen,
            count,
            peak,
            join: Some(join),
        }
    }

    fn wait_for_request(&self, child: &mut Child) {
        // Request deadlines start at dispatch, not process spawn. Fingerprinting
        // a debug executable with software SHA can take several seconds first.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match self.seen.recv_timeout(Duration::from_millis(20)) {
                Ok(_) => return,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    if Instant::now() < deadline && child.try_wait().unwrap().is_none() => {}
                Err(error) => {
                    let _ = child.kill();
                    let status = child.wait().unwrap();
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stdout.take() {
                        pipe.read_to_string(&mut stdout).unwrap();
                    }
                    if let Some(mut pipe) = child.stderr.take() {
                        pipe.read_to_string(&mut stderr).unwrap();
                    }
                    panic!(
                        "collector did not dispatch a request: {error}; status={status}; stdout={stdout}; stderr={stderr}"
                    );
                }
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Err(error) = self.join.take().unwrap().join()
            && !thread::panicking()
        {
            std::panic::resume_unwind(error);
        }
    }
}
fn read_request(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 64 * 1024);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap())
        })
        .unwrap();
    assert!(length <= 2 * 1024 * 1024);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}
fn header(stream: &mut TcpStream, kind: &str) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
}
fn frame(stream: &mut TcpStream, value: Value) {
    write!(stream, "data: {value}\n\n").unwrap();
    stream.flush().unwrap();
}
fn finish(stream: &mut TcpStream, tokens: Option<u64>, cached: Option<u64>) {
    frame(
        stream,
        json!({"id":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"length"}]}),
    );
    if let Some(tokens) = tokens {
        let mut usage =
            json!({"prompt_tokens":4,"completion_tokens":tokens,"total_tokens":tokens+4});
        if let Some(cached) = cached {
            usage["prompt_tokens_details"] = json!({"cached_tokens":cached});
        }
        frame(stream, json!({"id":"fixture","choices":[],"usage":usage}));
    }
    stream.write_all(b"data: [DONE]\n\n").unwrap();
}
fn normal(mut stream: TcpStream, _: usize, _: Value) {
    header(&mut stream, "text/event-stream");
    frame(
        &mut stream,
        json!({"id":"fixture","choices":[{"index":0,"delta":{"role":"assistant"}}]}),
    );
    thread::sleep(Duration::from_millis(30));
    frame(
        &mut stream,
        json!({"id":"fixture","choices":[{"index":0,"delta":{"reasoning_content":"thinking"}}]}),
    );
    thread::sleep(Duration::from_millis(40));
    let text=b"data: {\"id\":\"fixture\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"caf\xc3\xa9\"}}]}\r\n\r\n";
    for part in text.chunks(3) {
        stream.write_all(part).unwrap();
    }
    thread::sleep(Duration::from_millis(30));
    finish(&mut stream, Some(8), Some(0));
}
fn workload(concurrency: u32, warmup: u32, trials: u32) -> Value {
    json!({"version":1,"name":"fixture-v1","request":{"profile":"portable-chat-v1","stream":true,"output":{"tokens":8,"mode":"cap"},"cache":"observe","temperature_milli":0,"top_p_milli":1000,"seed":42},"limits":{"total_ms":3000,"idle_ms":1000,"response_bytes":65536,"wave_buffer_bytes":33554432},"cases":[{"id":"one","messages":[{"role":"user","content":"Say cafe."}]}],"cells":[{"id":"cell","case":"one","concurrency":concurrency,"warmup_trials":warmup,"trials":trials}]})
}
fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_grill-perf"))
}
fn run(temp: &Temp, server: &Server, name: &str, workload: &Value) -> Output {
    let input = temp.path(&format!("{name}.json"));
    fs::write(&input, serde_json::to_vec(workload).unwrap()).unwrap();
    cli()
        .args(["run"])
        .arg(input)
        .args([
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture-model",
            "--local-http",
            "--json",
            "--out",
        ])
        .arg(temp.path(name))
        .output()
        .unwrap()
}
fn value(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn wave(temp: &Temp, name: &str, index: usize) -> Value {
    value(temp.path(name).join(format!("wave-{index:06}/wave.json")))
}
fn successful(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// Release one whole wave at a time, with bounded waits even if an assertion fails.
#[test]
fn pause_drains_wave_resume_is_exclusive_and_preserves_evidence() {
    let temp = Temp::new();
    let (ready, releases) = std::sync::mpsc::sync_channel(6);
    let server = Server::new(move |stream, index, request| {
        let (release, wait) = std::sync::mpsc::sync_channel(0);
        ready.send(release).unwrap();
        wait.recv_timeout(Duration::from_secs(10)).unwrap();
        normal(stream, index, request);
    });
    let input = temp.path("input.json");
    let mut work = workload(2, 1, 2);
    work["limits"]["total_ms"] = json!(15000);
    work["limits"]["idle_ms"] = json!(12000);
    fs::write(&input, serde_json::to_vec(&work).unwrap()).unwrap();
    let mut child = cli()
        .arg("run")
        .arg(&input)
        .args([
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture-model",
            "--local-http",
            "--json",
            "--out",
        ])
        .arg(temp.path("run"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    for _ in 0..2 {
        server.wait_for_request(&mut child);
    }
    successful(&cli().arg("pause").arg(temp.path("run")).output().unwrap());
    assert!(child.try_wait().unwrap().is_none());
    assert!(!temp.path("run/wave-000000/wave.json").exists());
    assert!(!temp.path("run/wave-000001").exists());
    for _ in 0..2 {
        releases
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .send(())
            .unwrap();
    }
    let paused = child.wait_with_output().unwrap();
    assert_eq!(paused.status.code(), Some(2));
    let summary: Value = serde_json::from_slice(&paused.stdout).unwrap();
    assert_eq!(summary["status"], "paused");
    assert_eq!(
        wave(&temp, "run", 0)["attempts"].as_array().unwrap().len(),
        2
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    assert!(!temp.path("run/wave-000001").exists());
    let retained = [
        "plan.json",
        "workload.json",
        "run.json",
        "session-000000/session.json",
        "session-000000/run.json",
        "wave-000000/reservation.json",
        "wave-000000/wave.json",
        "wave-000000/response-0000.bin",
        "wave-000000/response-0001.bin",
    ]
    .map(|name| (name, fs::read(temp.path("run").join(name)).unwrap()));
    // The ordinary verifier must block damaged bytes before a new session or request.
    fs::write(temp.path("run/wave-000000/response-0000.bin"), b"damaged").unwrap();
    let damaged = cli().arg("resume").arg(temp.path("run")).output().unwrap();
    assert_eq!(damaged.status.code(), Some(1));
    assert!(!temp.path("run/session-000001").exists());
    fs::write(
        temp.path("run/wave-000000/response-0000.bin"),
        &retained[7].1,
    )
    .unwrap();
    // A retained root receipt is mandatory for continuation, even when the
    // independently settled session and wave bytes are intact.
    let mut rewritten = retained[2].1.clone();
    rewritten.push(b'\n');
    for replacement in [None, Some(b"damaged".to_vec()), Some(rewritten)] {
        let receipt = temp.path("run/run.json");
        fs::remove_file(&receipt).unwrap();
        if let Some(bytes) = replacement {
            fs::write(&receipt, bytes).unwrap();
        }
        if !receipt.exists() {
            let compared = cli()
                .arg("compare")
                .arg(temp.path("run"))
                .arg(temp.path("run"))
                .arg("--json")
                .output()
                .unwrap();
            assert_eq!(compared.status.code(), Some(2));
            let report: Value = serde_json::from_slice(&compared.stdout).unwrap();
            assert!(
                report["baseline"][0]["issues"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s
                        .as_str()
                        .unwrap()
                        .starts_with("execution_session_unsettled:"))
            );
            assert!(report["baseline"][0]["median_wave_latency_us"].is_null());
            assert!(!receipt.exists());
        }
        let refused = cli().arg("resume").arg(temp.path("run")).output().unwrap();
        assert_eq!(refused.status.code(), Some(1));
        assert!(!temp.path("run/session-000001").exists());
        assert!(!temp.path("run/wave-000001").exists());
        assert_eq!(server.count.load(Ordering::SeqCst), 2);
        assert_eq!(
            fs::read(temp.path("run/session-000000/run.json")).unwrap(),
            retained[4].1
        );
        fs::write(receipt, &retained[2].1).unwrap();
    }
    let mut resumed = cli()
        .arg("resume")
        .arg(temp.path("run"))
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    for _ in 0..2 {
        server.wait_for_request(&mut resumed);
    }
    let concurrent = cli().arg("resume").arg(temp.path("run")).output().unwrap();
    assert_eq!(concurrent.status.code(), Some(1));
    assert!(!temp.path("run/session-000002").exists());
    for _ in 0..4 {
        releases
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .send(())
            .unwrap();
    }
    let resumed = resumed.wait_with_output().unwrap();
    assert_eq!(resumed.status.code(), Some(2));
    let summary: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(summary["published_waves"], 2);
    assert_eq!(summary["first_wave"], 1);
    assert_eq!(server.count.load(Ordering::SeqCst), 6);
    for (name, bytes) in retained {
        assert_eq!(
            fs::read(temp.path("run").join(name)).unwrap(),
            bytes,
            "{name}"
        );
    }
    assert_eq!(wave(&temp, "run", 1)["spec"]["phase"], "measured");
    assert_eq!(wave(&temp, "run", 2)["spec"]["index"], 2);
    assert_eq!(
        cli()
            .arg("resume")
            .arg(temp.path("run"))
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
    drop(server);
    let compared = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(compared.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&compared.stdout).unwrap();
    assert_eq!(report["baseline"][0]["all_declared_warmups_complete"], true);
    assert!(report["baseline"][0]["median_wave_latency_us"].is_null());
    assert_eq!(report["changes"][0]["eligible"], false);
    assert!(
        report["changes"][0]["ineligibility_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s
                .as_str()
                .unwrap()
                .starts_with("baseline: continued_execution_sessions:"))
    );
}

#[test]
fn local_failure_reserved_suffix_remains_inspectable_but_not_resumable() {
    let temp = Temp::new();
    let staging = temp.path("run/wave-000001/.wave.json.pending");
    let server = Server::new(move |stream, index, request| {
        if index == 1 {
            // Inject an exclusive-publication failure after reservation, not a
            // fabricated successful outcome or a model/transport failure.
            fs::create_dir(&staging).unwrap();
        }
        normal(stream, index, request);
    });
    let output = run(&temp, &server, "run", &workload(1, 0, 3));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        value(temp.path("run/session-000000/run.json"))["status"],
        "local-failure"
    );
    assert_eq!(
        value(temp.path("run/session-000000/run.json"))["published_waves"],
        1
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    let retained = [
        "run.json",
        "session-000000/run.json",
        "wave-000000/wave.json",
        "wave-000000/response-0000.bin",
        "wave-000001/reservation.json",
    ]
    .map(|name| (name, fs::read(temp.path("run").join(name)).unwrap()));
    let compared = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(compared.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&compared.stdout).unwrap();
    assert_eq!(
        report["baseline"][0]["trial_states"],
        json!(["published", "reserved_unsettled", "not_started"])
    );
    assert_eq!(report["baseline"][0]["observed_trials"], 1);
    assert!(report["baseline"][0]["median_wave_latency_us"].is_null());
    assert!(report["baseline"][0]["median_achieved_completion_tokens_per_second"].is_null());
    assert_eq!(report["changes"][0]["eligible"], false);
    assert!(report["changes"][0]["wave_latency_change_percent"].is_null());
    assert!(
        report["baseline"][0]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s
                .as_str()
                .unwrap()
                .starts_with("execution_session_unsettled:"))
    );
    let refused = cli().arg("resume").arg(temp.path("run")).output().unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(!temp.path("run/session-000001").exists());
    assert!(!temp.path("run/wave-000002").exists());
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    for (name, bytes) in retained {
        assert_eq!(fs::read(temp.path("run").join(name)).unwrap(), bytes);
    }
}

// Linux flock waiters are observable without a timing-only negative assertion.
fn wait_for_admission_lock(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let pid = pid.to_string();
    loop {
        if fs::read_to_string("/proc/locks")
            .unwrap()
            .lines()
            .any(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                fields.get(1) == Some(&"->") && fields.get(5) == Some(&pid.as_str())
            })
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "child did not block on admission lock"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn pause_and_wave_admission_share_serialization() {
    use std::os::fd::AsRawFd;
    for pause_writer in [true, false] {
        let temp = Temp::new();
        let (ready, releases) = std::sync::mpsc::sync_channel(1);
        let server = Server::new(move |stream, index, request| {
            let (release, wait) = std::sync::mpsc::sync_channel(0);
            ready.send(release).unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
            normal(stream, index, request);
        });
        let input = temp.path("input.json");
        let mut work = workload(1, 0, 2);
        work["limits"]["total_ms"] = json!(15000);
        work["limits"]["idle_ms"] = json!(12000);
        fs::write(&input, serde_json::to_vec(&work).unwrap()).unwrap();
        let mut child = cli()
            .arg("run")
            .arg(input)
            .args([
                "--endpoint",
                &server.endpoint,
                "--model",
                "fixture-model",
                "--local-http",
                "--json",
                "--out",
            ])
            .arg(temp.path("run"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        server.wait_for_request(&mut child);
        let session = temp.path("run/session-000000");
        let gate = fs::File::open(&session).unwrap();
        assert_eq!(unsafe { libc::flock(gate.as_raw_fd(), libc::LOCK_EX) }, 0);
        let release = releases.recv_timeout(Duration::from_secs(3)).unwrap();
        if pause_writer {
            let pause = cli()
                .arg("pause")
                .arg(temp.path("run"))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            wait_for_admission_lock(pause.id());
            assert!(!session.join("pause.request").exists());
            assert!(!temp.path("run/wave-000000/wave.json").exists());
            drop(gate);
            successful(&pause.wait_with_output().unwrap());
            assert!(child.try_wait().unwrap().is_none());
            release.send(()).unwrap();
        } else {
            release.send(()).unwrap();
            wait_for_admission_lock(child.id());
            assert!(temp.path("run/wave-000000/wave.json").exists());
            assert!(!temp.path("run/wave-000001").exists());
            // Model the control writer publishing while it owns the same gate.
            fs::create_dir(session.join("pause.request")).unwrap();
            drop(gate);
        }
        let paused = child.wait_with_output().unwrap();
        assert_eq!(paused.status.code(), Some(2));
        let summary: Value = serde_json::from_slice(&paused.stdout).unwrap();
        assert_eq!(summary["status"], "paused");
        assert_eq!(summary["published_waves"], 1);
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
        assert!(!temp.path("run/wave-000001").exists());
    }
}

#[test]
fn concurrent_waves_exclude_warmup_and_measure_answer_not_first_bytes() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let output = run(&temp, &server, "run", &workload(2, 1, 2));
    successful(&output);
    assert_eq!(server.count.load(Ordering::SeqCst), 6);
    assert_eq!(server.peak.load(Ordering::SeqCst), 2);
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["measured_waves"], 2);
    let w = wave(&temp, "run", 1);
    assert_eq!(w["spec"]["phase"], "measured");
    assert_eq!(w["completion_tokens"], 16);
    for a in w["attempts"].as_array().unwrap() {
        let t = &a["timing"];
        assert!(t["first_generated_text_us"].as_u64().unwrap() >= 20_000);
        assert!(t["first_answer_text_us"].as_u64().unwrap() >= 50_000);
        assert!(
            t["first_body_us"].as_u64().unwrap() < t["first_generated_text_us"].as_u64().unwrap()
        );
        assert!(
            t["first_generated_text_us"].as_u64().unwrap()
                < t["first_answer_text_us"].as_u64().unwrap()
        );
    }
    let summed: u64 = w["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["timing"]["settle_us"].as_u64().unwrap())
        .sum();
    assert!(w["elapsed_us"].as_u64().unwrap() < summed);
    let compared = cli()
        .args(["compare"])
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&compared);
    let c: Value = serde_json::from_slice(&compared.stdout).unwrap();
    assert_eq!(
        c["changes"][0].get("wave_latency_change_percent"),
        Some(&Value::Null)
    );
    assert!(
        c["changes"][0]["withheld"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| {
                reason
                    .as_str()
                    .unwrap()
                    .starts_with("wave latency: ranges overlap")
            })
    );
    assert_eq!(c["baseline"][0]["planned_trials"], 2);
}

#[test]
fn evidence_is_not_published_while_a_peer_request_is_active() {
    let temp = Temp::new();
    let dir = temp.path("run/wave-000000");
    let barrier = Arc::new(AtomicBool::new(false));
    let checked = barrier.clone();
    let server = Server::new(move |mut s, i, _| {
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        if i == 0 {
            finish(&mut s, Some(8), None);
            drop(s);
            thread::sleep(Duration::from_millis(70));
            let names: Vec<_> = fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert!(
                names
                    .iter()
                    .all(|n| !n.to_string_lossy().starts_with("response-") && n != "wave.json")
            );
            checked.store(true, Ordering::SeqCst);
        } else {
            thread::sleep(Duration::from_millis(180));
            finish(&mut s, Some(8), None);
        }
    });
    successful(&run(&temp, &server, "run", &workload(2, 0, 1)));
    assert!(barrier.load(Ordering::SeqCst));
    assert!(temp.path("run/wave-000000/response-0000.bin").exists());
}

#[test]
fn absent_usage_is_unknown_and_never_a_zero_throughput() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        finish(&mut s, None, None);
    });
    let output = run(&temp, &server, "run", &workload(1, 0, 1));
    assert_eq!(output.status.code(), Some(2));
    let w = wave(&temp, "run", 0);
    assert!(w["achieved_completion_tokens_per_second"].is_null());
    assert!(w["attempts"][0]["usage"]["completion_tokens"].is_null());
    assert_eq!(w["attempts"][0]["status"], "complete");
}

#[test]
fn explicit_fixed_cold_controls_are_sent_and_provider_evidence_is_checked() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        finish(&mut s, Some(8), Some(0));
    });
    let mut w = workload(2, 1, 1);
    w["request"]["profile"] = json!("vllm-fixed-v1");
    w["request"]["output"]["mode"] = json!("exact");
    w["request"]["cache"] = json!("reported-prefix-zero");
    successful(&run(&temp, &server, "run", &w));
    let seen: Vec<_> = server.seen.try_iter().collect();
    let mut salts = std::collections::HashSet::new();
    for body in seen.iter() {
        assert_eq!(body["min_tokens"], 8);
        assert_eq!(body["ignore_eos"], true);
        assert!(salts.insert(body["cache_salt"].as_str().unwrap()));
    }
}

#[test]
fn explicit_thinking_false_is_sent_on_every_request() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(2, 1, 1);
    w["request"]["profile"] = json!("vllm-fixed-v1");
    w["request"]["thinking"] = json!(false);
    successful(&run(&temp, &server, "run", &w));
    let seen: Vec<_> = server.seen.try_iter().collect();
    assert_eq!(seen.len(), 4);
    for body in seen {
        assert_eq!(body["chat_template_kwargs"]["thinking"], false);
    }
}

#[test]
fn explicit_thinking_true_is_sent_on_every_request() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(2, 1, 1);
    w["request"]["profile"] = json!("vllm-fixed-v1");
    w["request"]["thinking"] = json!(true);
    successful(&run(&temp, &server, "run", &w));
    let seen: Vec<_> = server.seen.try_iter().collect();
    assert_eq!(seen.len(), 4);
    for body in seen {
        assert_eq!(body["chat_template_kwargs"]["thinking"], true);
    }
}

#[test]
fn omitted_or_null_thinking_preserves_provider_defaults() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(1, 0, 1);
    w["request"]["profile"] = json!("vllm-fixed-v1");
    successful(&run(&temp, &server, "omitted", &w));
    w["request"]["thinking"] = Value::Null;
    successful(&run(&temp, &server, "null", &w));
    let seen: Vec<_> = server.seen.try_iter().collect();
    assert_eq!(seen.len(), 2);
    for body in seen {
        assert!(body.get("chat_template_kwargs").is_none());
    }
    // Plans recorded before the field existed must keep their workload digest.
    for name in ["omitted", "null"] {
        let plan = value(temp.path(name).join("plan.json"));
        assert!(plan["workload"]["request"].get("thinking").is_none());
    }
}

#[test]
fn portable_thinking_is_rejected_before_dispatch() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(1, 0, 1);
    w["request"]["thinking"] = json!(false);
    let output = run(&temp, &server, "portable", &w);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
    assert!(!temp.path("portable").exists());
    // Same declaration under the explicit profile is admitted, so the refusal
    // above is the profile rule, not unknown-field rejection.
    w["request"]["profile"] = json!("vllm-fixed-v1");
    successful(&run(&temp, &server, "fixed", &w));
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}

#[test]
fn fill_renders_distinct_lane_and_trial_salts_and_observe_plans_load() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(2, 1, 2);
    w["cases"][0]["messages"][0]["content"] = json!("header {salt}\n{fill}\nfooter");
    w["cases"][0]["fill"] = json!({"unit":" the","repeat":3});
    successful(&run(&temp, &server, "run", &w));
    let plan = value(temp.path("run/plan.json"));
    let namespace = plan["cache_namespace"].as_str().unwrap();
    assert_eq!(namespace.len(), 64);
    assert!(namespace.bytes().all(|b| b.is_ascii_hexdigit()));
    let expected: std::collections::HashSet<_> = (0..3)
        .flat_map(|index| {
            (0..2).map(move |lane| {
                format!(
                    "header {}-{index}-{lane}\n the the the\nfooter",
                    &namespace[..16]
                )
            })
        })
        .collect();
    let seen: Vec<_> = server.seen.try_iter().collect();
    assert_eq!(seen.len(), 6);
    let actual: std::collections::HashSet<_> = seen
        .iter()
        .map(|body| {
            assert!(body.get("cache_salt").is_none());
            body["messages"][0]["content"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(actual, expected);
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["changes"][0].get("wave_latency_change_percent"),
        Some(&Value::Null)
    );
    assert!(
        report["changes"][0]["withheld"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| {
                reason
                    .as_str()
                    .unwrap()
                    .starts_with("wave latency: ranges overlap")
            })
    );
}

#[test]
fn fill_request_cap_admits_large_complete_bodies_and_loads_them() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(1, 0, 1);
    w["cases"][0]["messages"][0]["content"] = json!("header{fill}footer");
    w["cases"][0]["fill"] = json!({"unit":" the","repeat":70000});
    successful(&run(&temp, &server, "large", &w));
    let seen = server.seen.try_recv().unwrap();
    assert_eq!(
        seen["messages"][0]["content"],
        format!("header{}footer", " the".repeat(70000))
    );
    let reservation = value(temp.path("large/wave-000000/reservation.json"));
    let body = reservation["requests"][0].as_str().unwrap();
    assert!(body.len() > 256 * 1024 && body.len() < 2 * 1024 * 1024);
    assert_eq!(serde_json::from_str::<Value>(body).unwrap(), seen);
    let output = cli()
        .arg("compare")
        .arg(temp.path("large"))
        .arg(temp.path("large"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    for (name, fill) in [
        ("rendered", json!({"unit":" the","repeat":524288})),
        ("escaped", json!({"unit":"\"\"","repeat":600000})),
    ] {
        w["cases"][0]["fill"] = fill;
        assert_eq!(run(&temp, &server, name, &w).status.code(), Some(1));
        assert!(!temp.path(name).exists());
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}

#[test]
fn fill_requires_bounded_controls_and_unique_placeholders() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(1, 0, 1);
    w["cases"][0]["messages"] = json!([
        {"role":"system","content":"header {salt}"},
        {"role":"user","content":"{fill}footer"}
    ]);
    for (name, fill) in [
        ("unit-bound", json!({"unit":"é".repeat(32),"repeat":1})),
        ("repeat-bound", json!({"unit":"x","repeat":1000000})),
    ] {
        w["cases"][0]["fill"] = fill;
        successful(&run(&temp, &server, name, &w));
    }
    for (name, fill, first, second) in [
        (
            "missing",
            json!({"unit":"x","repeat":1}),
            "header {salt}",
            "footer",
        ),
        (
            "duplicate-fill",
            json!({"unit":"x","repeat":1}),
            "{fill}",
            "{fill}",
        ),
        (
            "duplicate-salt",
            json!({"unit":"x","repeat":1}),
            "{salt}",
            "{fill}{salt}",
        ),
        (
            "empty-unit",
            json!({"unit":"","repeat":1}),
            "header",
            "{fill}",
        ),
        (
            "long-unit",
            json!({"unit":"é".repeat(33),"repeat":1}),
            "header",
            "{fill}",
        ),
        (
            "zero-repeat",
            json!({"unit":"x","repeat":0}),
            "header",
            "{fill}",
        ),
        (
            "large-repeat",
            json!({"unit":"x","repeat":1000001}),
            "header",
            "{fill}",
        ),
        (
            "unknown-field",
            json!({"unit":"x","repeat":1,"extra":true}),
            "header",
            "{fill}",
        ),
    ] {
        w["cases"][0]["fill"] = fill;
        w["cases"][0]["messages"][0]["content"] = json!(first);
        w["cases"][0]["messages"][1]["content"] = json!(second);
        assert_eq!(run(&temp, &server, name, &w).status.code(), Some(1));
        assert!(!temp.path(name).exists());
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
}

#[test]
fn absent_fill_preserves_literal_placeholders_and_plan_identity() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(1, 0, 1);
    w["cases"][0]["messages"][0]["content"] = json!("{salt}{fill}{fill}{salt}");
    successful(&run(&temp, &server, "omitted", &w));
    w["cases"][0]["fill"] = Value::Null;
    successful(&run(&temp, &server, "null", &w));
    let seen: Vec<_> = server.seen.try_iter().collect();
    assert_eq!(seen.len(), 2);
    for body in seen {
        assert_eq!(body["messages"][0]["content"], "{salt}{fill}{fill}{salt}");
    }
    let omitted = value(temp.path("omitted/plan.json"));
    let null = value(temp.path("null/plan.json"));
    for plan in [&omitted, &null] {
        assert!(plan["workload"]["cases"][0].get("fill").is_none());
        assert!(plan["cache_namespace"].is_null());
    }
    assert_eq!(omitted["workload_sha256"], null["workload_sha256"]);
    successful(
        &cli()
            .arg("compare")
            .arg(temp.path("omitted"))
            .arg(temp.path("null"))
            .arg("--json")
            .output()
            .unwrap(),
    );
}

#[test]
fn fill_bytes_count_toward_each_cells_wave_buffer_bound() {
    let temp = Temp::new();
    let server = Server::new(|mut s, index, request| {
        if request["stream"] == true {
            normal(s, index, request);
        } else {
            header(&mut s, "application/json");
            write!(s, "{}", json!({"choices":[{"message":{"content":"x"},"finish_reason":"stop"}],"usage":{"completion_tokens":8}})).unwrap();
        }
    });
    let mut w = workload(2, 0, 1);
    w["cases"][0]["messages"][0]["content"] = json!("{fill}");
    w["cases"][0]["fill"] = json!({"unit":" the","repeat":100});
    for (name, stream, base) in [
        ("stream", true, 2 * 65536 + 6 * 256 * 1024 + 512 * 1024),
        ("body", false, 6 * 65536 + 512 * 1024),
    ] {
        w["request"]["stream"] = json!(stream);
        w["limits"]["wave_buffer_bytes"] = json!(2 * (base + 400) - 1);
        assert_eq!(run(&temp, &server, name, &w).status.code(), Some(1));
        assert!(!temp.path(name).exists());
        w["limits"]["wave_buffer_bytes"] = json!(2 * (base + 400));
        successful(&run(&temp, &server, name, &w));
        assert_eq!(wave(&temp, name, 0)["attempts"][0]["status"], "complete");
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 4);
}

#[test]
fn fill_wave_reservation_bound_counts_escaped_request_bytes() {
    let temp = Temp::new();
    let server = Server::new(normal);
    // A quote unit doubles when the body is embedded as a JSON string in the receipt.
    for (name, unit, rejected, admitted) in [("plain", "x", 40, 1), ("escaped", "\"", 9, 8)] {
        let mut w = workload(rejected, 0, 1);
        w["limits"]["wave_buffer_bytes"] = json!(128 * 1024 * 1024);
        w["cases"][0]["messages"][0]["content"] = json!("{fill}");
        w["cases"][0]["fill"] = json!({"unit":unit,"repeat":1000000});
        let sent = server.count.load(Ordering::SeqCst);
        assert_eq!(run(&temp, &server, name, &w).status.code(), Some(1));
        assert!(!temp.path(name).exists());
        assert_eq!(server.count.load(Ordering::SeqCst), sent);
        w["cells"][0]["concurrency"] = json!(admitted);
        successful(&run(&temp, &server, name, &w));
        let reservation = value(temp.path(name).join("wave-000000/reservation.json"));
        let body = reservation["requests"][0].as_str().unwrap();
        assert!(body.len() < 2 * 1024 * 1024);
        let escaped = serde_json::to_string(body).unwrap().len();
        assert!(escaped * rejected as usize > 32 * 1024 * 1024);
        assert!(escaped * admitted as usize <= 32 * 1024 * 1024);
        let output = cli()
            .arg("compare")
            .arg(temp.path(name))
            .arg(temp.path(name))
            .output()
            .unwrap();
        successful(&output);
    }
}

#[test]
fn fill_body_at_the_request_cap_is_refused_before_any_run_directory() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(1, 0, 1);
    w["request"]["seed"] = Value::Null;
    w["limits"]["wave_buffer_bytes"] = json!(8 * 1024 * 1024);
    w["cases"][0]["messages"][0]["content"] = json!("{fill}");
    w["cases"][0]["fill"] = json!({"unit":"xxx","repeat":1000});
    successful(&run(&temp, &server, "probe", &w));
    let reservation = value(temp.path("probe").join("wave-000000/reservation.json"));
    let base = reservation["requests"][0].as_str().unwrap().len() - 3000;
    // Without seed or salt the admission sample equals the sent body; admission
    // keeps two bytes of lane-digit slack, so a sample at the cap is refused.
    let cap = 2 * 1024 * 1024;
    let repeat = (cap - base) / 3;
    let pad = "y".repeat(cap - base - 3 * repeat);
    w["cases"][0]["fill"]["repeat"] = json!(repeat);
    w["cases"][0]["messages"][0]["content"] = json!(format!("{{fill}}{pad}"));
    assert_eq!(run(&temp, &server, "cap", &w).status.code(), Some(1));
    assert!(!temp.path("cap").exists());
    w["cases"][0]["messages"][0]["content"] = json!(format!("{{fill}}{pad}y"));
    w["cases"][0]["fill"]["repeat"] = json!(repeat - 1);
    successful(&run(&temp, &server, "under", &w));
    let reservation = value(temp.path("under").join("wave-000000/reservation.json"));
    assert_eq!(reservation["requests"][0].as_str().unwrap().len(), cap - 2);
}

#[test]
fn warm_prefix_protocol_reuses_primed_lane_salts_but_not_other_lanes() {
    let temp = Temp::new();
    let server = Server::new(|mut s, i, _| {
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        finish(&mut s, Some(8), Some(if i < 2 { 0 } else { 4 }));
    });
    let mut w = workload(2, 1, 1);
    w["request"]["profile"] = json!("vllm-fixed-v1");
    w["request"]["cache"] = json!("reported-prefix-hit");
    successful(&run(&temp, &server, "run", &w));
    let seen: Vec<_> = server.seen.try_iter().collect();
    let a: std::collections::HashSet<_> = seen[..2]
        .iter()
        .map(|r| r["cache_salt"].as_str().unwrap())
        .collect();
    let b: std::collections::HashSet<_> = seen[2..]
        .iter()
        .map(|r| r["cache_salt"].as_str().unwrap())
        .collect();
    assert_eq!(a.len(), 2);
    assert_eq!(a, b);
}

#[test]
fn malformed_or_incomplete_streams_remain_failed_with_raw_bytes() {
    for (name, body, status) in [
        (
            "eof",
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}]}\n\n",
            "incomplete",
        ),
        ("earlydone", "data: [DONE]\n\n", "malformed"),
        ("badjson", "data: {broken}\n\n", "malformed"),
        (
            "afterfinish",
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"more\"}}]}\n\n",
            "malformed",
        ),
    ] {
        let temp = Temp::new();
        let server = Server::new(move |mut s, _, _| {
            header(&mut s, "text/event-stream");
            s.write_all(body.as_bytes()).unwrap();
        });
        let output = run(&temp, &server, name, &workload(1, 0, 2));
        assert_eq!(output.status.code(), Some(2));
        let w = wave(&temp, name, 0);
        assert_eq!(w["attempts"][0]["status"], status);
        assert!(
            !fs::read(temp.path(name).join("wave-000000/response-0000.bin"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn http_errors_are_not_retried_or_hidden_by_successful_subset_rates() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        s.write_all(
            b"HTTP/1.1 503 Unavailable\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops",
        )
        .unwrap();
    });
    let output = run(&temp, &server, "run", &workload(2, 0, 3));
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    let w = wave(&temp, "run", 0);
    assert!(w["achieved_completion_tokens_per_second"].is_null());
    assert_eq!(w["attempts"][0]["status"], "http_error");
    assert!(!temp.path("run/wave-000001").exists());
}

#[test]
fn timeout_settles_partial_evidence_without_starting_more_waves() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "text/event-stream");
        s.write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
            .unwrap();
        thread::sleep(Duration::from_millis(200));
    });
    let mut w = workload(1, 0, 2);
    w["limits"]["idle_ms"] = json!(50);
    let output = run(&temp, &server, "run", &w);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        wave(&temp, "run", 0)["attempts"][0]["status"],
        "idle_timeout"
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}

fn cancellation_case(signal: i32) {
    let temp = Temp::new();
    let (ready, releases) = std::sync::mpsc::sync_channel(2);
    let server = Server::new(move |mut s, _, _| {
        header(&mut s, "text/event-stream");
        s.write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
            .unwrap();
        let (release, wait) = std::sync::mpsc::sync_channel(0);
        ready.send(release).unwrap();
        wait.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    let input = temp.path("input.json");
    fs::write(&input, serde_json::to_vec(&workload(2, 0, 3)).unwrap()).unwrap();
    let mut child = cli()
        .arg("run")
        .arg(input)
        .args([
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture-model",
            "--local-http",
            "--json",
            "--out",
        ])
        .arg(temp.path("run"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    for _ in 0..2 {
        server.wait_for_request(&mut child);
    }
    let releases = [
        releases.recv_timeout(Duration::from_secs(3)).unwrap(),
        releases.recv_timeout(Duration::from_secs(3)).unwrap(),
    ];
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    let output = child.wait_with_output().unwrap();
    for release in releases {
        release.send(()).unwrap();
    }
    assert_eq!(output.status.code(), Some(2));
    let w = wave(&temp, "run", 0);
    assert!(
        w["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["status"] == "interrupted")
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    assert!(!temp.path("run/wave-000001").exists());
    let refused = cli().arg("resume").arg(temp.path("run")).output().unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
}

#[test]
fn evidence_write_collision_is_not_overwritten_or_reported_successful() {
    let temp = Temp::new();
    let path = temp.path("run/wave-000000/response-0000.bin");
    let server = Server::new(move |mut s, _, _| {
        fs::write(&path, b"sentinel").unwrap();
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        finish(&mut s, Some(8), None);
    });
    let output = run(&temp, &server, "run", &workload(1, 0, 2));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        fs::read(temp.path("run/wave-000000/response-0000.bin")).unwrap(),
        b"sentinel"
    );
    assert!(!temp.path("run/wave-000000/wave.json").exists());
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}

#[test]
fn comparison_rejects_changed_raw_evidence_and_incompatible_controls() {
    let temp = Temp::new();
    let server = Server::new(normal);
    successful(&run(&temp, &server, "a", &workload(1, 0, 1)));
    let mut w = workload(1, 0, 1);
    w["request"]["temperature_milli"] = json!(600);
    successful(&run(&temp, &server, "b", &w));
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    fs::write(temp.path("a/wave-000000/response-0000.bin"), b"changed").unwrap();
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("a"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn admission_rejects_unsafe_endpoint_or_memory_bound_without_dispatch() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(64, 0, 1);
    w["limits"]["wave_buffer_bytes"] = json!(1024);
    let output = run(&temp, &server, "memory", &w);
    assert_eq!(output.status.code(), Some(1));
    assert!(!temp.path("memory").exists());
    let input = temp.path("safe.json");
    fs::write(&input, serde_json::to_vec(&workload(1, 0, 1)).unwrap()).unwrap();
    let output = cli()
        .arg("run")
        .arg(input)
        .args([
            "--endpoint",
            "http://example.invalid/v1/chat/completions",
            "--model",
            "fixture",
            "--local-http",
            "--out",
        ])
        .arg(temp.path("unsafe"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!temp.path("unsafe").exists());
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}

#[test]
fn nonstreaming_results_do_not_invent_first_text_timing() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, request| {
        assert_eq!(request["stream"], false);
        header(&mut s, "application/json");
        write!(s, "{}", json!({"choices":[{"index":0,"message":{"content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":8,"total_tokens":12}})).unwrap();
    });
    let mut w = workload(1, 0, 1);
    w["request"]["stream"] = json!(false);
    successful(&run(&temp, &server, "run", &w));
    let result = wave(&temp, "run", 0);
    assert!(result["attempts"][0]["timing"]["first_generated_text_us"].is_null());
    assert!(result["attempts"][0]["timing"]["first_answer_text_us"].is_null());
    let compared = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .output()
        .unwrap();
    successful(&compared);
}

#[test]
fn output_and_prefix_requirements_are_observed_not_assumed() {
    for (tokens, cached, expected) in [
        (4, Some(0), "reported_output_not_exact"),
        (8, Some(1), "provider_reported_prefix_cache_nonzero"),
        (8, None, "provider_prefix_cache_usage_unavailable"),
        (9, Some(0), "reported_output_exceeds_cap"),
    ] {
        let temp = Temp::new();
        let server = Server::new(move |mut s, _, _| {
            header(&mut s, "text/event-stream");
            frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
            finish(&mut s, Some(tokens), cached);
        });
        let mut w = workload(1, 0, 1);
        w["request"]["profile"] = json!("vllm-fixed-v1");
        w["request"]["output"]["mode"] = json!("exact");
        w["request"]["cache"] = json!("reported-prefix-zero");
        assert_eq!(run(&temp, &server, "run", &w).status.code(), Some(2));
        let result = wave(&temp, "run", 0);
        assert!(
            result["attempts"][0]["eligibility_errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == expected)
        );
        assert!(result["achieved_completion_tokens_per_second"].is_null());
        assert_eq!(result["completion_tokens"], tokens);
    }
}

#[test]
fn response_records_reject_array_shapes_and_excessive_nesting() {
    let deep = format!(
        "{{\"choices\":[],\"extra\":{}{}}}",
        "[".repeat(65),
        "]".repeat(65)
    );
    for body in [
        "[null,[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}],{\"completion_tokens\":8},null]".to_owned(),
        "{\"choices\":[[0,{\"content\":\"x\"},null,\"stop\"]],\"usage\":{\"completion_tokens\":8}}".to_owned(),
        "{\"choices\":[{\"delta\":[\"x\"],\"finish_reason\":\"stop\"}],\"usage\":{\"completion_tokens\":8}}".to_owned(),
        deep,
    ] {
        let temp = Temp::new();
        let server = Server::new(move |mut s, _, _| { header(&mut s, "text/event-stream"); let _ = write!(s, "data: {body}\n\ndata: [DONE]\n\n"); });
        assert_eq!(run(&temp, &server, "run", &workload(1, 0, 1)).status.code(), Some(2));
        assert_eq!(wave(&temp, "run", 0)["attempts"][0]["status"], "malformed");
    }
}

#[test]
fn total_deadline_fires_despite_regular_body_activity() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "text/event-stream");
        for _ in 0..50 {
            if s.write_all(b": heartbeat\n\n").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    let mut w = workload(1, 0, 1);
    w["limits"]["total_ms"] = json!(80);
    w["limits"]["idle_ms"] = json!(40);
    assert_eq!(run(&temp, &server, "run", &w).status.code(), Some(2));
    assert_eq!(
        wave(&temp, "run", 0)["attempts"][0]["status"],
        "total_timeout"
    );
}

#[test]
fn semantic_completion_ignores_surplus_but_not_missing_done_boundaries() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        let mut bytes = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}],\"usage\":{\"completion_tokens\":8}}\n\ndata: [DONE]\n\n".to_vec();
        bytes.extend_from_slice(&[255; 4096]);
        let _ = s.write_all(&bytes);
    });
    let mut w = workload(1, 0, 1);
    w["limits"]["response_bytes"] = json!(1024);
    successful(&run(&temp, &server, "run", &w));
    let result = wave(&temp, "run", 0);
    assert_eq!(result["attempts"][0]["status"], "complete");
    assert!(result["attempts"][0]["response_bytes"].as_u64().unwrap() <= 1024);
    successful(
        &cli()
            .arg("compare")
            .arg(temp.path("run"))
            .arg(temp.path("run"))
            .output()
            .unwrap(),
    );
}

#[test]
fn offline_comparison_rechecks_reported_usage_and_wave_duration() {
    let temp = Temp::new();
    let server = Server::new(normal);
    successful(&run(&temp, &server, "run", &workload(1, 0, 1)));
    let path = temp.path("run/wave-000000/wave.json");
    let original = value(&path);
    let mut changed = original.clone();
    changed["attempts"][0]["usage"]["completion_tokens"] = json!(7);
    changed["completion_tokens"] = json!(7);
    changed["achieved_completion_tokens_per_second"] =
        json!(7_000_000.0 / changed["elapsed_us"].as_u64().unwrap() as f64);
    fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert_eq!(
        cli()
            .arg("compare")
            .arg(temp.path("run"))
            .arg(temp.path("run"))
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
    let mut changed = original;
    changed["elapsed_us"] = json!(1);
    changed["achieved_completion_tokens_per_second"] = json!(8_000_000.0);
    fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert_eq!(
        cli()
            .arg("compare")
            .arg(temp.path("run"))
            .arg(temp.path("run"))
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn shorter_outputs_are_not_reported_as_a_matched_speedup() {
    let temp = Temp::new();
    let server = Server::new(|mut s, index, _| {
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        finish(&mut s, Some(if index == 0 { 8 } else { 4 }), None);
    });
    successful(&run(&temp, &server, "a", &workload(1, 0, 1)));
    successful(&run(&temp, &server, "b", &workload(1, 0, 1)));
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["changes"][0]["observed_output_amounts_match"], false);
    assert!(result["changes"][0]["wave_latency_change_percent"].is_null());
    assert_eq!(result["baseline"][0]["reported_completion_tokens"][0], 8);
    assert_eq!(result["candidate"][0]["reported_completion_tokens"][0], 4);
}

#[test]
#[ignore = "explicit release-mode collection-overhead experiment"]
fn paced_collection_overhead() {
    fn med(mut values: Vec<u64>) -> u64 {
        values.sort_unstable();
        values[values.len() / 2]
    }
    for concurrency in [1, 6] {
        let temp = Temp::new();
        let server = Server::new(|mut stream, _, _| {
            header(&mut stream, "text/event-stream");
            for _ in 0..512 {
                frame(&mut stream, json!({"choices":[{"delta":{"content":"x"}}]}));
                thread::sleep(Duration::from_millis(1));
            }
            finish(&mut stream, Some(512), None);
        });
        let address = server
            .endpoint
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .to_owned();
        let request_body = serde_json::to_vec(&json!({"model":"fixture-model","messages":[{"role":"user","content":"Say cafe."}],"stream":true,"max_tokens":512})).unwrap();
        let mut baseline = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            thread::scope(|scope| {
                for _ in 0..concurrency {
                    let address = &address;
                    let body = &request_body;
                    scope.spawn(move || {
                        let mut socket = TcpStream::connect(address).unwrap();
                        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                        write!(socket, "POST /v1/chat/completions HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                        socket.write_all(body).unwrap();
                        std::io::copy(&mut socket, &mut std::io::sink()).unwrap();
                    });
                }
            });
            baseline.push(start.elapsed().as_micros() as u64);
        }
        let mut w = workload(concurrency, 1, 3);
        w["request"]["output"]["tokens"] = json!(512);
        w["limits"]["total_ms"] = json!(5000);
        w["limits"]["idle_ms"] = json!(2000);
        let output = run(&temp, &server, "profile", &w);
        successful(&output);
        let measured: Vec<_> = (1..4).map(|i| wave(&temp, "profile", i)).collect();
        let durations: Vec<_> = measured
            .iter()
            .map(|w| w["elapsed_us"].as_u64().unwrap())
            .collect();
        let capture: u64 = measured
            .iter()
            .flat_map(|w| w["attempts"].as_array().unwrap())
            .map(|a| a["timing"]["capture_parse_us"].as_u64().unwrap())
            .sum();
        let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
        let ratio = med(durations.clone()) as f64 / med(baseline.clone()) as f64;
        println!(
            "OVERHEAD {}",
            json!({"concurrency":concurrency,"events_per_request":512,"pause_per_event_ms":1,"drain_control_wave_us":baseline,"collector_wave_us":durations,"median_latency_ratio":ratio,"capture_parse_wall_us_sum":capture,"all_wave_preparation_us":summary["wave_preparation_us"],"reservation_publication_us":summary["reservation_publication_us"],"all_wave_publication_us":summary["wave_publication_us"],"prespecified_fixture_margin":0.05,"claim":"paced-loopback-calibration-not-server-capacity"})
        );
        assert!(
            ratio <= 1.05,
            "collector exceeds the prespecified 5% paced-fixture margin"
        );
    }
}

#[test]
fn sigint_preserves_active_attempts_and_stops_admission() {
    cancellation_case(libc::SIGINT);
}

#[test]
fn sigterm_preserves_active_attempts_and_stops_admission() {
    cancellation_case(libc::SIGTERM);
}

#[test]
fn connection_is_reused_when_the_server_keeps_it_alive() {
    let temp = Temp::new();
    let server = Server::new(|mut socket, _, first| {
        for i in 0..3 {
            let request = if i == 0 {
                first.clone()
            } else {
                read_request(&mut socket)
            };
            assert_eq!(request["model"], "fixture-model");
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}],\"usage\":{\"completion_tokens\":8}}\n\ndata: [DONE]\n\n";
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}", body.len()).unwrap();
            socket.flush().unwrap();
        }
    });
    successful(&run(&temp, &server, "run", &workload(1, 1, 2)));
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}

#[test]
fn crashed_and_unstarted_waves_remain_distinguishable() {
    let temp = Temp::new();
    let server = Server::new(normal);
    successful(&run(&temp, &server, "run", &workload(1, 0, 2)));
    fs::remove_file(temp.path("run/session-000000/run.json")).unwrap();
    fs::remove_file(temp.path("run/wave-000000/wave.json")).unwrap();
    fs::remove_dir_all(temp.path("run/wave-000001")).unwrap();
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["baseline"][0]["trial_states"],
        json!(["reserved_unsettled", "not_started"])
    );
    assert_eq!(
        report["baseline"][0]["first_answer_text_us"],
        json!([null, null])
    );
    assert!(report["changes"][0]["wave_latency_change_percent"].is_null());
    let refused = cli().arg("resume").arg(temp.path("run")).output().unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(!temp.path("run/session-000001").exists());
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    let inactive_pause = cli().arg("pause").arg(temp.path("run")).output().unwrap();
    assert_eq!(inactive_pause.status.code(), Some(1));
    assert!(!temp.path("run/session-000000/pause.request").exists());
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
}

#[test]
fn equal_wave_totals_do_not_hide_different_lane_lengths() {
    let temp = Temp::new();
    let server = Server::new(|mut s, i, request| {
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        let n = if i < 2 {
            if request["seed"] == 42 { 2 } else { 8 }
        } else {
            5
        };
        finish(&mut s, Some(n), None);
    });
    successful(&run(&temp, &server, "a", &workload(2, 0, 1)));
    successful(&run(&temp, &server, "b", &workload(2, 0, 1)));
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["changes"][0]["observed_output_amounts_match"], false);
    assert!(report["changes"][0]["wave_latency_change_percent"].is_null());
}

#[test]
fn missing_warmup_prevents_a_matched_comparison() {
    let temp = Temp::new();
    let server = Server::new(normal);
    successful(&run(&temp, &server, "run", &workload(1, 1, 1)));
    fs::remove_file(temp.path("run/wave-000000/wave.json")).unwrap();
    fs::remove_file(temp.path("run/session-000000/run.json")).unwrap();
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["baseline"][0]["warmup_states"],
        json!(["reserved_unsettled"])
    );
    assert!(report["changes"][0]["wave_latency_change_percent"].is_null());
}

#[test]
fn inconsistent_reasoning_usage_cannot_produce_an_eligible_rate() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "text/event-stream");
        frame(
            &mut s,
            json!({"choices":[{"delta":{"content":"x"},"finish_reason":"stop"}],"usage":{"completion_tokens":8,"completion_tokens_details":{"reasoning_tokens":9}}}),
        );
        s.write_all(b"data: [DONE]\n\n").unwrap();
    });
    assert_eq!(
        run(&temp, &server, "run", &workload(1, 0, 1)).status.code(),
        Some(2)
    );
    assert!(wave(&temp, "run", 0)["achieved_completion_tokens_per_second"].is_null());
}

#[test]
fn nonstreaming_admission_budgets_body_sized_semantic_allocations() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let mut w = workload(2, 0, 1);
    w["request"]["stream"] = json!(false);
    w["limits"]["response_bytes"] = json!(8 * 1024 * 1024);
    w["limits"]["wave_buffer_bytes"] = json!(40 * 1024 * 1024);
    assert_eq!(run(&temp, &server, "run", &w).status.code(), Some(1));
    assert!(!temp.path("run").exists());
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}

#[test]
fn offline_loading_rejects_impossible_nonstreaming_text_times() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "application/json");
        write!(s, "{}", json!({"choices":[{"message":{"content":"x"},"finish_reason":"stop"}],"usage":{"completion_tokens":8}})).unwrap();
    });
    let mut w = workload(1, 0, 1);
    w["request"]["stream"] = json!(false);
    successful(&run(&temp, &server, "run", &w));
    let path = temp.path("run/wave-000000/wave.json");
    let mut result = value(&path);
    result["attempts"][0]["timing"]["first_answer_text_us"] = json!(0);
    fs::write(path, serde_json::to_vec(&result).unwrap()).unwrap();
    assert_eq!(
        cli()
            .arg("compare")
            .arg(temp.path("run"))
            .arg(temp.path("run"))
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn usage_errors_are_distinct_from_retained_ineligible_runs() {
    let output = cli().arg("run").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    successful(&cli().arg("--help").output().unwrap());
    successful(&cli().arg("--version").output().unwrap());
}

#[test]
fn incomplete_usage_is_retained_but_not_a_completed_wave_total() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "text/event-stream");
        frame(
            &mut s,
            json!({"choices":[{"delta":{"content":"partial"}}],"usage":{"completion_tokens":8}}),
        );
    });
    assert_eq!(
        run(&temp, &server, "run", &workload(1, 0, 1)).status.code(),
        Some(2)
    );
    let result = wave(&temp, "run", 0);
    assert_eq!(result["attempts"][0]["usage"]["completion_tokens"], 8);
    assert_eq!(result["attempts"][0]["status"], "incomplete");
    assert!(result["completion_tokens"].is_null());
    assert!(result["achieved_completion_tokens_per_second"].is_null());
}

#[test]
fn nonstreaming_unknown_fields_do_not_bypass_utf8_validation() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "application/json");
        s.write_all(b"{\"choices\":[{\"message\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}],\"usage\":{\"completion_tokens\":8},\"ignored\":\"\xff\"}").unwrap();
    });
    let mut w = workload(1, 0, 1);
    w["request"]["stream"] = json!(false);
    assert_eq!(run(&temp, &server, "run", &w).status.code(), Some(2));
    assert_eq!(wave(&temp, "run", 0)["attempts"][0]["status"], "malformed");
}

#[test]
fn endpoint_size_and_control_characters_fail_before_output_creation() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let input = temp.path("input.json");
    fs::write(&input, serde_json::to_vec(&workload(1, 0, 1)).unwrap()).unwrap();
    for (name, endpoint) in [
        ("long", format!("{}/{}", server.endpoint, "x".repeat(4096))),
        ("control", format!("{}\n", server.endpoint)),
    ] {
        let output = cli()
            .arg("run")
            .arg(&input)
            .args([
                "--endpoint",
                &endpoint,
                "--model",
                "fixture",
                "--local-http",
                "--out",
            ])
            .arg(temp.path(name))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(!temp.path(name).exists());
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}

#[test]
fn streaming_decode_rate_uses_post_first_text_interval_and_measured_lanes() {
    let temp = Temp::new();
    let interval = Duration::from_millis(150);
    let server = Server::new(move |mut s, i, _| {
        header(&mut s, "text/event-stream");
        thread::sleep(Duration::from_millis(100));
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        thread::sleep(if i < 2 {
            Duration::from_millis(30)
        } else {
            interval
        });
        finish(&mut s, Some(8), None);
    });
    successful(&run(&temp, &server, "run", &workload(2, 1, 2)));
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let lanes = report["baseline"][0]["lane_decode_tokens_per_second"]
        .as_array()
        .unwrap();
    assert_eq!(lanes.len(), 2);
    let mut rates = Vec::new();
    for (trial, values) in lanes.iter().enumerate() {
        let receipt = wave(&temp, "run", trial + 1);
        let values = values.as_array().unwrap();
        assert_eq!(values.len(), 2);
        for (lane, value) in values.iter().enumerate() {
            let rate = value.as_f64().unwrap();
            let attempt = &receipt["attempts"][lane];
            let tokens = attempt["usage"]["completion_tokens"].as_u64().unwrap();
            let first = attempt["timing"]["first_generated_text_us"]
                .as_u64()
                .unwrap();
            let settle = attempt["timing"]["settle_us"].as_u64().unwrap();
            let expected = (tokens - 1) as f64 * 1_000_000.0 / (settle - first) as f64;
            assert!((rate - expected).abs() < 1e-9);
            let paced = (tokens - 1) as f64 / interval.as_secs_f64();
            assert!(rate > paced * 0.5 && rate < paced * 1.5, "{rate}");
            rates.push(rate);
        }
    }
    rates.sort_by(f64::total_cmp);
    let median = report["baseline"][0]["median_decode_tokens_per_second"]
        .as_f64()
        .unwrap();
    assert!((median - (rates[1] + rates[2]) / 2.0).abs() < 1e-9);
}

#[test]
fn nonstreaming_decode_rate_is_null() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "application/json");
        write!(s, "{}", json!({"choices":[{"message":{"content":"x"},"finish_reason":"stop"}],"usage":{"completion_tokens":8}})).unwrap();
    });
    let mut work = workload(1, 0, 1);
    work["request"]["stream"] = json!(false);
    successful(&run(&temp, &server, "run", &work));
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["baseline"][0]["lane_decode_tokens_per_second"],
        json!([[null]])
    );
    assert!(
        report["baseline"][0]
            .get("median_decode_tokens_per_second")
            .unwrap()
            .is_null()
    );
    assert!(
        report["changes"][0]
            .get("decode_rate_change_percent")
            .unwrap()
            .is_null()
    );
    let human = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .output()
        .unwrap();
    successful(&human);
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("wave latency withheld"));
    assert!(text.contains("decode rate n/a"));
    assert!(!text.contains('%'));
}

#[test]
fn single_completion_token_decode_rate_is_null() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        thread::sleep(Duration::from_millis(30));
        finish(&mut s, Some(1), None);
    });
    successful(&run(&temp, &server, "run", &workload(1, 0, 1)));
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["baseline"][0]["lane_decode_tokens_per_second"],
        json!([[null]])
    );
    assert!(
        report["baseline"][0]
            .get("median_decode_tokens_per_second")
            .unwrap()
            .is_null()
    );
    assert!(
        report["changes"][0]
            .get("decode_rate_change_percent")
            .unwrap()
            .is_null()
    );
}

#[test]
fn faster_streaming_decode_has_positive_matched_rate_change() {
    let temp = Temp::new();
    let server = Server::new(|mut s, i, _| {
        header(&mut s, "text/event-stream");
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        thread::sleep(Duration::from_millis(if i == 0 { 180 } else { 30 }));
        finish(&mut s, Some(8), None);
    });
    let work = workload(1, 0, 1);
    successful(&run(&temp, &server, "baseline", &work));
    successful(&run(&temp, &server, "candidate", &work));
    let output = cli()
        .arg("compare")
        .arg(temp.path("baseline"))
        .arg(temp.path("candidate"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let change = report["changes"][0]["decode_rate_change_percent"]
        .as_f64()
        .unwrap();
    assert!(change > 0.0, "{change}");
    let baseline = report["baseline"][0]["median_decode_tokens_per_second"]
        .as_f64()
        .unwrap();
    let candidate = report["candidate"][0]["median_decode_tokens_per_second"]
        .as_f64()
        .unwrap();
    assert!((change - 100.0 * (candidate / baseline - 1.0)).abs() < 1e-9);
    assert!(
        report["changes"][0]["achieved_throughput_change_percent"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    let human = cli()
        .arg("compare")
        .arg(temp.path("baseline"))
        .arg(temp.path("candidate"))
        .output()
        .unwrap();
    successful(&human);
    let text = String::from_utf8(human.stdout).unwrap();
    let percentages: Vec<f64> = text
        .split_whitespace()
        .filter_map(|word| word.trim_end_matches(';').strip_suffix('%'))
        .map(|number| number.parse().unwrap())
        .collect();
    assert_eq!(percentages.len(), 4);
    for (actual, field) in percentages.iter().zip([
        "wave_latency_change_percent",
        "achieved_throughput_change_percent",
        "decode_rate_change_percent",
    ]) {
        let expected = report["changes"][0][field].as_f64().unwrap();
        assert!((actual - expected).abs() <= 0.005);
    }
}

#[test]
fn streaming_prefill_rate_uses_prompt_tokens_and_first_text_time() {
    let temp = Temp::new();
    let interval = Duration::from_millis(150);
    let server = Server::new(move |mut s, i, _| {
        header(&mut s, "text/event-stream");
        thread::sleep(if i < 2 {
            Duration::from_millis(30)
        } else {
            interval
        });
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        thread::sleep(Duration::from_millis(30));
        finish(&mut s, Some(8), (i % 2 == 0).then_some(0));
    });
    successful(&run(&temp, &server, "run", &workload(2, 1, 2)));
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["baseline"][0]["lane_prompt_tokens"],
        json!([[4, 4], [4, 4]])
    );
    let lanes = report["baseline"][0]["lane_prefill_tokens_per_second"]
        .as_array()
        .unwrap();
    assert_eq!(lanes.len(), 2);
    let mut rates = Vec::new();
    for (trial, values) in lanes.iter().enumerate() {
        let receipt = wave(&temp, "run", trial + 1);
        let values = values.as_array().unwrap();
        assert_eq!(values.len(), 2);
        for (lane, value) in values.iter().enumerate() {
            let rate = value.as_f64().unwrap();
            let attempt = &receipt["attempts"][lane];
            let tokens = attempt["usage"]["prompt_tokens"].as_u64().unwrap();
            let first = attempt["timing"]["first_generated_text_us"]
                .as_u64()
                .unwrap();
            let expected = tokens as f64 * 1_000_000.0 / first as f64;
            assert!((rate - expected).abs() < 1e-9);
            let paced = tokens as f64 / interval.as_secs_f64();
            assert!(rate > paced * 0.5 && rate < paced * 1.5, "{rate}");
            rates.push(rate);
        }
    }
    rates.sort_by(f64::total_cmp);
    let median = report["baseline"][0]["median_prefill_tokens_per_second"]
        .as_f64()
        .unwrap();
    assert!((median - (rates[1] + rates[2]) / 2.0).abs() < 1e-9);
    assert_eq!(report["baseline"], report["candidate"]);
    assert_eq!(
        report["changes"][0].get("prefill_rate_change_percent"),
        Some(&Value::Null)
    );
    assert!(
        report["changes"][0]["withheld"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| {
                reason
                    .as_str()
                    .unwrap()
                    .starts_with("prefill rate: ranges overlap")
            })
    );
}

#[test]
fn cached_prompt_tokens_withhold_prefill_rate() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "text/event-stream");
        thread::sleep(Duration::from_millis(30));
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        thread::sleep(Duration::from_millis(30));
        finish(&mut s, Some(8), Some(1));
    });
    successful(&run(&temp, &server, "run", &workload(1, 0, 1)));
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["baseline"][0]["lane_prompt_tokens"], json!([[4]]));
    assert_eq!(
        report["baseline"][0]["lane_prefill_tokens_per_second"],
        json!([[null]])
    );
    assert_eq!(
        report["baseline"][0].get("median_prefill_tokens_per_second"),
        Some(&Value::Null)
    );
    assert_eq!(
        report["changes"][0].get("prefill_rate_change_percent"),
        Some(&Value::Null)
    );
    assert_eq!(
        report["changes"][0].get("decode_rate_change_percent"),
        Some(&Value::Null)
    );
    assert!(
        report["changes"][0]["withheld"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| {
                reason
                    .as_str()
                    .unwrap()
                    .starts_with("decode rate: ranges overlap")
            })
    );
}

#[test]
fn unequal_prompt_tokens_keep_prefill_rate_change_and_stay_visible() {
    let temp = Temp::new();
    let server = Server::new(|mut s, i, _| {
        header(&mut s, "text/event-stream");
        thread::sleep(Duration::from_millis(30));
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        thread::sleep(Duration::from_millis(30));
        frame(
            &mut s,
            json!({"choices":[{"delta":{},"finish_reason":"length"}],"usage":{"prompt_tokens":if i == 0 { 4 } else { 5 },"completion_tokens":8}}),
        );
        s.write_all(b"data: [DONE]\n\n").unwrap();
    });
    let work = workload(1, 0, 1);
    successful(&run(&temp, &server, "baseline", &work));
    successful(&run(&temp, &server, "candidate", &work));
    let output = cli()
        .arg("compare")
        .arg(temp.path("baseline"))
        .arg(temp.path("candidate"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["baseline"][0]["lane_prompt_tokens"], json!([[4]]));
    assert_eq!(report["candidate"][0]["lane_prompt_tokens"], json!([[5]]));
    for side in ["baseline", "candidate"] {
        assert!(report[side][0]["median_prefill_tokens_per_second"].is_number());
    }
    // Per-run text salts tokenize to different counts; the per-token rate must still compare.
    let change = &report["changes"][0];
    assert!(change["prefill_rate_change_percent"].is_number());
    assert_eq!(change["eligible"], true);
    assert_eq!(change["observed_output_amounts_match"], true);
    assert_eq!(change["ineligibility_reasons"], json!([]));
    for field in [
        "wave_latency_change_percent",
        "achieved_throughput_change_percent",
        "decode_rate_change_percent",
    ] {
        assert!(change[field].is_number(), "{field}");
    }
}

#[test]
fn faster_first_text_has_positive_matched_prefill_rate_change() {
    let temp = Temp::new();
    let server = Server::new(|mut s, i, _| {
        header(&mut s, "text/event-stream");
        thread::sleep(Duration::from_millis(if i == 0 { 180 } else { 30 }));
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        thread::sleep(Duration::from_millis(30));
        finish(&mut s, Some(8), None);
    });
    let work = workload(1, 0, 1);
    successful(&run(&temp, &server, "baseline", &work));
    successful(&run(&temp, &server, "candidate", &work));
    let output = cli()
        .arg("compare")
        .arg(temp.path("baseline"))
        .arg(temp.path("candidate"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let change = report["changes"][0]["prefill_rate_change_percent"]
        .as_f64()
        .unwrap();
    assert!(change > 0.0, "{change}");
    let baseline = report["baseline"][0]["median_prefill_tokens_per_second"]
        .as_f64()
        .unwrap();
    let candidate = report["candidate"][0]["median_prefill_tokens_per_second"]
        .as_f64()
        .unwrap();
    assert!((change - 100.0 * (candidate / baseline - 1.0)).abs() < 1e-9);
    let human = cli()
        .arg("compare")
        .arg(temp.path("baseline"))
        .arg(temp.path("candidate"))
        .output()
        .unwrap();
    successful(&human);
    let text = String::from_utf8(human.stdout).unwrap();
    let percentages: Vec<f64> = text
        .split_whitespace()
        .filter_map(|word| word.trim_end_matches(';').strip_suffix('%'))
        .map(|number| number.parse().unwrap())
        .collect();
    assert_eq!(percentages.len(), 4);
    for (actual, field) in percentages.iter().zip([
        "wave_latency_change_percent",
        "achieved_throughput_change_percent",
        "decode_rate_change_percent",
        "prefill_rate_change_percent",
    ]) {
        let expected = report["changes"][0][field].as_f64().unwrap();
        assert!((actual - expected).abs() <= 0.005);
    }
}

fn spread_server(timings: &'static [(u64, u64, u64)]) -> Server {
    Server::new(move |mut s, i, _| {
        let (prefill_ms, decode_ms, tokens) = timings[i];
        header(&mut s, "text/event-stream");
        thread::sleep(Duration::from_millis(prefill_ms));
        frame(&mut s, json!({"choices":[{"delta":{"content":"x"}}]}));
        thread::sleep(Duration::from_millis(decode_ms));
        finish(&mut s, Some(tokens), Some(0));
    })
}

#[test]
fn comparison_nonoverlapping_ranges_use_measured_trials_and_lanes() {
    let temp = Temp::new();
    let server = spread_server(&[
        (5, 5, 8),
        (5, 5, 8),
        (60, 60, 8),
        (80, 80, 8),
        (100, 100, 8),
        (120, 120, 8),
        (5, 5, 8),
        (5, 5, 8),
        (360, 360, 8),
        (380, 380, 8),
        (400, 400, 8),
        (420, 420, 8),
    ]);
    let work = workload(2, 1, 2);
    successful(&run(&temp, &server, "a", &work));
    successful(&run(&temp, &server, "b", &work));
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    for side in ["baseline", "candidate"] {
        let summary = &report[side][0];
        for (field, values) in [
            ("wave_latency_us_range", "wave_latency_us"),
            (
                "achieved_completion_tokens_per_second_range",
                "achieved_completion_tokens_per_second",
            ),
            (
                "decode_tokens_per_second_range",
                "lane_decode_tokens_per_second",
            ),
            (
                "prefill_tokens_per_second_range",
                "lane_prefill_tokens_per_second",
            ),
        ] {
            let mut samples = Vec::new();
            for value in summary[values].as_array().unwrap() {
                if let Some(lanes) = value.as_array() {
                    samples.extend(lanes.iter().filter_map(Value::as_f64));
                } else {
                    samples.push(value.as_f64().unwrap());
                }
            }
            samples.sort_by(f64::total_cmp);
            assert_eq!(
                summary[field],
                json!([samples[0], samples[samples.len() - 1]])
            );
        }
    }
    let a = &report["baseline"][0];
    let b = &report["candidate"][0];
    assert!(
        a["wave_latency_us_range"][1].as_f64().unwrap()
            < b["wave_latency_us_range"][0].as_f64().unwrap()
    );
    for (field, median) in [
        ("wave_latency_change_percent", "median_wave_latency_us"),
        (
            "achieved_throughput_change_percent",
            "median_achieved_completion_tokens_per_second",
        ),
        (
            "decode_rate_change_percent",
            "median_decode_tokens_per_second",
        ),
        (
            "prefill_rate_change_percent",
            "median_prefill_tokens_per_second",
        ),
    ] {
        let expected = 100.0 * (b[median].as_f64().unwrap() / a[median].as_f64().unwrap() - 1.0);
        assert!((report["changes"][0][field].as_f64().unwrap() - expected).abs() < 1e-9);
    }
    assert_eq!(report["changes"][0]["withheld"], json!([]));
}

#[test]
fn comparison_overlapping_ranges_withhold_without_ineligibility() {
    let temp = Temp::new();
    let server = spread_server(&[(50, 50, 8), (400, 400, 8), (150, 150, 8), (300, 300, 8)]);
    let work = workload(1, 0, 2);
    successful(&run(&temp, &server, "a", &work));
    successful(&run(&temp, &server, "b", &work));
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let human = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .output()
        .unwrap();
    successful(&human);
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("cell: wave latency withheld; achieved throughput withheld; decode rate withheld; prefill rate withheld"));
    let change = &report["changes"][0];
    assert_eq!(change["eligible"], true);
    assert_eq!(change["ineligibility_reasons"], json!([]));
    for (name, field, range) in [
        (
            "wave latency",
            "wave_latency_change_percent",
            "wave_latency_us_range",
        ),
        (
            "achieved throughput",
            "achieved_throughput_change_percent",
            "achieved_completion_tokens_per_second_range",
        ),
        (
            "decode rate",
            "decode_rate_change_percent",
            "decode_tokens_per_second_range",
        ),
        (
            "prefill rate",
            "prefill_rate_change_percent",
            "prefill_tokens_per_second_range",
        ),
    ] {
        assert_eq!(change.get(field), Some(&Value::Null));
        let a = &report["baseline"][0][range];
        let b = &report["candidate"][0][range];
        let (amin, amax, bmin, bmax) = (
            a[0].as_f64().unwrap(),
            a[1].as_f64().unwrap(),
            b[0].as_f64().unwrap(),
            b[1].as_f64().unwrap(),
        );
        assert!(amin <= bmax && bmin <= amax);
        let reason = if name == "wave latency" {
            format!(
                "{name}: ranges overlap, {:.2}-{:.2} vs {:.2}-{:.2}",
                amin / 1_000_000.0,
                amax / 1_000_000.0,
                bmin / 1_000_000.0,
                bmax / 1_000_000.0
            )
        } else {
            format!("{name}: ranges overlap, {amin:.1}-{amax:.1} vs {bmin:.1}-{bmax:.1}")
        };
        assert!(
            change["withheld"]
                .as_array()
                .unwrap()
                .contains(&json!(reason))
        );
        assert!(text.contains(&format!("  {reason}\n")));
    }
}

#[test]
fn comparison_incomplete_cell_ranges_are_explicit_null() {
    let temp = Temp::new();
    let server = Server::new(normal);
    successful(&run(&temp, &server, "run", &workload(1, 1, 1)));
    fs::remove_file(temp.path("run/wave-000000/wave.json")).unwrap();
    fs::remove_file(temp.path("run/session-000000/run.json")).unwrap();
    let output = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    for side in ["baseline", "candidate"] {
        for field in [
            "wave_latency_us_range",
            "achieved_completion_tokens_per_second_range",
            "decode_tokens_per_second_range",
            "prefill_tokens_per_second_range",
        ] {
            assert_eq!(report[side][0].get(field), Some(&Value::Null));
        }
    }
    assert_eq!(report["changes"][0]["eligible"], false);
    assert_eq!(report["changes"][0]["withheld"], json!([]));
    let human = cli()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(2));
    let text = String::from_utf8(human.stdout).unwrap();
    for reason in report["changes"][0]["ineligibility_reasons"]
        .as_array()
        .unwrap()
    {
        assert!(text.contains(&format!("  {}\n", reason.as_str().unwrap())));
    }
}

#[test]
fn comparison_latency_change_survives_overlapping_throughput() {
    let temp = Temp::new();
    let server = spread_server(&[(100, 100, 1), (120, 120, 8), (300, 300, 1), (320, 320, 8)]);
    let work = workload(1, 0, 2);
    successful(&run(&temp, &server, "a", &work));
    successful(&run(&temp, &server, "b", &work));
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let a = &report["baseline"][0];
    let b = &report["candidate"][0];
    assert!(
        a["wave_latency_us_range"][1].as_f64().unwrap()
            < b["wave_latency_us_range"][0].as_f64().unwrap()
    );
    let change = &report["changes"][0];
    assert!(change["wave_latency_change_percent"].as_f64().unwrap() > 0.0);
    assert_eq!(
        change.get("achieved_throughput_change_percent"),
        Some(&Value::Null)
    );
    assert!(change["withheld"].as_array().unwrap().iter().any(|reason| {
        reason
            .as_str()
            .unwrap()
            .starts_with("achieved throughput: ranges overlap")
    }));
    assert_eq!(change["eligible"], true);
    let human = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .output()
        .unwrap();
    successful(&human);
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains(&format!(
        "cell: wave latency {:+.2}%; achieved throughput withheld;",
        change["wave_latency_change_percent"].as_f64().unwrap()
    )));
}

#[test]
fn comparison_reference_drift_withholds_smaller_and_equal_changes() {
    let temp = Temp::new();
    let server = spread_server(&[(100, 100, 8), (150, 150, 8), (350, 350, 8)]);
    let work = workload(1, 0, 1);
    for name in ["a", "b", "a2"] {
        successful(&run(&temp, &server, name, &work));
    }
    let ordinary = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&ordinary);
    let ordinary: Value = serde_json::from_slice(&ordinary.stdout).unwrap();
    assert_eq!(ordinary.get("reference"), Some(&Value::Null));
    assert_eq!(ordinary.get("drift"), Some(&Value::Null));
    for reference in ["a2", "b"] {
        let output = cli()
            .arg("compare")
            .arg(temp.path("a"))
            .arg(temp.path("b"))
            .arg("--reference")
            .arg(temp.path(reference))
            .arg("--json")
            .output()
            .unwrap();
        successful(&output);
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let change = &report["changes"][0];
        assert_eq!(change["eligible"], true);
        assert_eq!(change["ineligibility_reasons"], json!([]));
        let human = cli()
            .arg("compare")
            .arg(temp.path("a"))
            .arg(temp.path("b"))
            .arg("--reference")
            .arg(temp.path(reference))
            .output()
            .unwrap();
        successful(&human);
        let text = String::from_utf8(human.stdout).unwrap();
        let mut drift_parts = Vec::new();
        for (name, field, drift_field, median) in [
            (
                "wave latency",
                "wave_latency_change_percent",
                "wave_latency_percent",
                "median_wave_latency_us",
            ),
            (
                "achieved throughput",
                "achieved_throughput_change_percent",
                "achieved_throughput_percent",
                "median_achieved_completion_tokens_per_second",
            ),
            (
                "decode rate",
                "decode_rate_change_percent",
                "decode_rate_percent",
                "median_decode_tokens_per_second",
            ),
            (
                "prefill rate",
                "prefill_rate_change_percent",
                "prefill_rate_percent",
                "median_prefill_tokens_per_second",
            ),
        ] {
            let c = ordinary["changes"][0][field].as_f64().unwrap();
            let d = report["drift"][0][drift_field].as_f64().unwrap();
            let expected = 100.0
                * (report["reference"][0][median].as_f64().unwrap()
                    / report["baseline"][0][median].as_f64().unwrap()
                    - 1.0);
            assert!((d - expected).abs() < 1e-9);
            assert!(c.abs() <= d.abs());
            if reference == "b" {
                assert_eq!(c, d);
            }
            assert_eq!(change.get(field), Some(&Value::Null));
            let reason = format!("{name}: change {c:+.2}% within reference drift {d:+.2}%");
            assert!(
                change["withheld"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(reason))
            );
            assert!(text.contains(&format!("  {reason}\n")));
            drift_parts.push(format!("{name} {d:+.2}%"));
        }
        assert_eq!(report["drift"][0]["cell"], "cell");
        assert!(text.contains(&format!("  reference drift: {}\n", drift_parts.join("; "))));
    }
}

#[test]
fn comparison_change_exceeding_reference_drift_remains_present() {
    let temp = Temp::new();
    let server = spread_server(&[
        (100, 100, 8),
        (200, 200, 8),
        (400, 400, 8),
        (500, 500, 8),
        (110, 110, 8),
        (190, 190, 8),
    ]);
    let work = workload(1, 0, 2);
    for name in ["a", "b", "a2"] {
        successful(&run(&temp, &server, name, &work));
    }
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--reference")
        .arg(temp.path("a2"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let human = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--reference")
        .arg(temp.path("a2"))
        .output()
        .unwrap();
    successful(&human);
    let text = String::from_utf8(human.stdout).unwrap();
    let mut change_parts = Vec::new();
    let mut drift_parts = Vec::new();
    for (name, field, drift_field, median, range) in [
        (
            "wave latency",
            "wave_latency_change_percent",
            "wave_latency_percent",
            "median_wave_latency_us",
            "wave_latency_us_range",
        ),
        (
            "achieved throughput",
            "achieved_throughput_change_percent",
            "achieved_throughput_percent",
            "median_achieved_completion_tokens_per_second",
            "achieved_completion_tokens_per_second_range",
        ),
        (
            "decode rate",
            "decode_rate_change_percent",
            "decode_rate_percent",
            "median_decode_tokens_per_second",
            "decode_tokens_per_second_range",
        ),
        (
            "prefill rate",
            "prefill_rate_change_percent",
            "prefill_rate_percent",
            "median_prefill_tokens_per_second",
            "prefill_tokens_per_second_range",
        ),
    ] {
        let a = &report["baseline"][0];
        let b = &report["candidate"][0];
        let a2 = &report["reference"][0];
        assert!(
            a[range][0].as_f64().unwrap() <= a2[range][1].as_f64().unwrap()
                && a2[range][0].as_f64().unwrap() <= a[range][1].as_f64().unwrap()
        );
        assert!(
            a[range][1].as_f64().unwrap() < b[range][0].as_f64().unwrap()
                || b[range][1].as_f64().unwrap() < a[range][0].as_f64().unwrap()
        );
        let c = report["changes"][0][field].as_f64().unwrap();
        let d = report["drift"][0][drift_field].as_f64().unwrap();
        assert!(c.abs() > d.abs());
        let baseline = a[median].as_f64().unwrap();
        assert!((c - 100.0 * (b[median].as_f64().unwrap() / baseline - 1.0)).abs() < 1e-9);
        assert!((d - 100.0 * (a2[median].as_f64().unwrap() / baseline - 1.0)).abs() < 1e-9);
        change_parts.push(format!("{name} {c:+.2}%"));
        drift_parts.push(format!("{name} {d:+.2}%"));
    }
    assert_eq!(report["changes"][0]["withheld"], json!([]));
    assert!(text.contains(&format!(
        "cell: {}\n  reference drift: {}\n",
        change_parts.join("; "),
        drift_parts.join("; ")
    )));
}

#[test]
fn comparison_reference_checks_workload_and_reports_absent_drift_metrics() {
    let temp = Temp::new();
    let server = Server::new(|mut s, _, _| {
        header(&mut s, "application/json");
        s.write_all(br#"{"choices":[{"message":{"content":"x"},"finish_reason":"length"}],"usage":{"prompt_tokens":4,"completion_tokens":8}}"#).unwrap();
    });
    let mut work = workload(1, 0, 1);
    work["request"]["stream"] = json!(false);
    successful(&run(&temp, &server, "a", &work));
    let output = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("a"))
        .arg("--reference")
        .arg(temp.path("a"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["drift"][0]["wave_latency_percent"], 0.0);
    assert_eq!(report["drift"][0]["achieved_throughput_percent"], 0.0);
    for field in ["decode_rate_percent", "prefill_rate_percent"] {
        assert_eq!(report["drift"][0].get(field), Some(&Value::Null));
    }
    let human = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("a"))
        .arg("--reference")
        .arg(temp.path("a"))
        .output()
        .unwrap();
    successful(&human);
    assert!(String::from_utf8(human.stdout).unwrap().contains(
        "  reference drift: wave latency +0.00%; achieved throughput +0.00%; decode rate n/a; prefill rate n/a\n"
    ));
    work["cases"][0]["messages"][0]["content"] = json!("Different workload.");
    successful(&run(&temp, &server, "other", &work));
    let refused = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("a"))
        .arg("--reference")
        .arg(temp.path("other"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(refused.stdout.is_empty());
    assert!(
        String::from_utf8(refused.stderr)
            .unwrap()
            .contains("incompatible workload")
    );
}
