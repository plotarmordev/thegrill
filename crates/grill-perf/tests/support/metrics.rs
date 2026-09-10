use super::*;
use sha2::{Digest, Sha256};

const DRAFT: &str = "vllm:spec_decode_num_draft_tokens_total";
const ACCEPTED: &str = "vllm:spec_decode_num_accepted_tokens_total";
const RUNNING: &str = "vllm:num_requests_running";

struct MetricsServer {
    endpoint: String,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl MetricsServer {
    fn new(handler: impl Fn(usize, &str) -> Vec<u8> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/metrics", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (calls, stopped) = (count.clone(), stop.clone());
        let handler = Arc::new(handler);
        let join = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let (handler, calls) = (handler.clone(), calls.clone());
                        workers.push(thread::spawn(move || {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(5)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(5)))
                                .unwrap();
                            let mut bytes = Vec::new();
                            let mut byte = [0];
                            while !bytes.ends_with(b"\r\n\r\n") {
                                stream.read_exact(&mut byte).unwrap();
                                bytes.push(byte[0]);
                                assert!(bytes.len() <= 64 * 1024);
                            }
                            let headers = String::from_utf8(bytes).unwrap();
                            assert!(headers.starts_with("GET /metrics HTTP/1.1\r\n"));
                            assert!(!headers.to_ascii_lowercase().contains("authorization:"));
                            let response = handler(calls.fetch_add(1, Ordering::SeqCst), &headers);
                            // Bounds/deadline fixtures deliberately close before consuming the entity.
                            let _ = stream.write_all(&response);
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("metrics fixture: {e}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            endpoint,
            count,
            stop,
            join: Some(join),
        }
    }
}
impl Drop for MetricsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Err(error) = self.join.take().unwrap().join()
            && !thread::panicking()
        {
            std::panic::resume_unwind(error);
        }
    }
}
fn response(body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
fn command(temp: &Temp, server: &Server, metrics: &str, name: &str, work: Value) -> Command {
    let input = temp.path(&format!("{name}.json"));
    fs::write(&input, serde_json::to_vec(&work).unwrap()).unwrap();
    let mut command = cli();
    command
        .arg("run")
        .arg(input)
        .args([
            "--endpoint",
            &server.endpoint,
            "--metrics-url",
            metrics,
            "--model",
            "fixture-model",
            "--local-http",
            "--json",
            "--auth-env",
            "GRILL_METRICS_TEST_KEY",
            "--out",
        ])
        .arg(temp.path(name))
        .env("GRILL_METRICS_TEST_KEY", "fixture-secret");
    command
}
fn compare(root: &Path) -> Output {
    cli()
        .arg("compare")
        .arg(root)
        .arg(root)
        .arg("--json")
        .output()
        .unwrap()
}
fn report(root: &Path) -> Value {
    let output = compare(root);
    successful(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}
fn fast(mut stream: TcpStream, _: usize, _: Value) {
    header(&mut stream, "text/event-stream");
    frame(
        &mut stream,
        json!({"choices":[{"index":0,"delta":{"content":"ok"}}]}),
    );
    finish(&mut stream, Some(8), Some(0));
}
fn padded() -> Vec<u8> {
    let mut body = Vec::with_capacity(1024 * 1024);
    while body.len() < 1024 * 1024 {
        let n = (1024 * 1024 - body.len()).min(64 * 1024);
        body.push(b'#');
        body.extend(std::iter::repeat_n(b'x', n - 2));
        body.push(b'\n');
    }
    body
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn rewrite_receipt(root: &Path, edit: impl FnOnce(&mut Value)) {
    let dir = root.join("wave-000000");
    let mut receipt = value(dir.join("metrics.json"));
    edit(&mut receipt);
    let bytes = serde_json::to_vec_pretty(&receipt).unwrap();
    fs::write(dir.join("metrics.json"), &bytes).unwrap();
    let mut wave = value(dir.join("wave.json"));
    wave["metrics"]["sha256"] = json!(digest(&bytes));
    fs::write(
        dir.join("wave.json"),
        serde_json::to_vec_pretty(&wave).unwrap(),
    )
    .unwrap();
}

#[test]
fn metrics_opt_out_has_no_calls_or_receipt_fields_and_loads_legacy_evidence() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let metrics = MetricsServer::new(|_, _| panic!("opt-out must not scrape"));
    successful(&run(&temp, &server, "off", &workload(1, 0, 1)));
    let root = temp.path("off");
    let original = fs::read(root.join("wave-000000/wave.json")).unwrap();
    assert!(value(root.join("plan.json")).get("metrics").is_none());
    assert!(wave(&temp, "off", 0).get("metrics").is_none());
    assert!(!root.join("wave-000000/metrics-before.bin").exists());
    assert!(report(&root).get("baseline_metrics").is_none());
    assert_eq!(
        fs::read(root.join("wave-000000/wave.json")).unwrap(),
        original
    );
    assert_eq!(metrics.count.load(Ordering::SeqCst), 0);
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    let legacy = temp.path("legacy");
    fs::create_dir(&legacy).unwrap();
    fs::create_dir(legacy.join("wave-000000")).unwrap();
    for path in ["workload.json", "wave-000000/response-0000.bin"] {
        fs::copy(root.join(path), legacy.join(path)).unwrap();
    }
    let mut plan = value(root.join("plan.json"));
    plan["version"] = json!(1);
    let bytes = serde_json::to_vec_pretty(&plan).unwrap();
    fs::write(legacy.join("plan.json"), &bytes).unwrap();
    let plan_hash = digest(&bytes);
    let mut reservation = value(root.join("wave-000000/reservation.json"));
    reservation["plan_sha256"] = json!(plan_hash);
    let bytes = serde_json::to_vec_pretty(&reservation).unwrap();
    fs::write(legacy.join("wave-000000/reservation.json"), &bytes).unwrap();
    let mut receipt = wave(&temp, "off", 0);
    receipt["plan_sha256"] = json!(plan_hash);
    receipt["reservation_sha256"] = json!(digest(&bytes));
    let bytes = serde_json::to_vec_pretty(&receipt).unwrap();
    fs::write(legacy.join("wave-000000/wave.json"), &bytes).unwrap();
    assert!(report(&legacy).get("baseline_metrics").is_none());
    assert_eq!(
        fs::read(legacy.join("wave-000000/wave.json")).unwrap(),
        bytes
    );
}

#[test]
fn metrics_scrapes_bracket_all_lanes_and_expose_label_matched_deltas_offline() {
    let temp = Temp::new();
    let root = temp.path("on");
    let settled = Arc::new(AtomicUsize::new(0));
    let done = settled.clone();
    let dir = root.clone();
    let server = Server::new(move |mut stream, _, _| {
        assert!(dir.join("wave-000000/metrics-before.bin").exists());
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"index":0,"delta":{"content":"ok"}}]}),
        );
        done.fetch_add(1, Ordering::SeqCst);
        finish(&mut stream, Some(8), Some(0));
    });
    let calls = server.count.clone();
    let dir = root.clone();
    let metrics = MetricsServer::new(move |index, _| {
        assert!(dir.join("wave-000000/reservation.json").exists());
        assert!(!dir.join("wave-000000/wave.json").exists());
        if index == 0 {
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            response(format!("{DRAFT}{{model=\"a\",engine=\"0\"}} 10\n{ACCEPTED}{{engine=\"0\",model=\"a\"}} 4\n{RUNNING}{{engine=\"0\"}} 2\n").as_bytes())
        } else {
            assert_eq!(index, 1);
            assert_eq!(settled.load(Ordering::SeqCst), 2);
            response(format!("{DRAFT}{{engine=\"0\",model=\"a\"}} 18\n{ACCEPTED}{{model=\"a\",engine=\"0\"}} 10\n{RUNNING}{{engine=\"0\"}} 0\n").as_bytes())
        }
    });
    successful(
        &command(&temp, &server, &metrics.endpoint, "on", workload(2, 0, 1))
            .output()
            .unwrap(),
    );
    assert_eq!(metrics.count.load(Ordering::SeqCst), 2);
    let output = report(&root);
    let telemetry = &output["baseline_metrics"]["waves"][0];
    assert_eq!(telemetry["acceptance"][0]["ratio"], 0.75);
    assert_eq!(telemetry["counters"].as_array().unwrap().len(), 2);
    assert!(
        telemetry["counters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["name"] != RUNNING)
    );
    assert_eq!(telemetry["before_series"][0]["value"], 2.0);
    assert_eq!(telemetry["after_series"][0]["value"], 0.0);
    assert!(
        telemetry["receipt"]["measured_duration_us"]
            .as_u64()
            .unwrap()
            >= wave(&temp, "on", 0)["elapsed_us"].as_u64().unwrap()
    );
    assert_eq!(output["changes"][0]["eligible"], true);
    assert_eq!(
        output["baseline_metrics"]["config"]["endpoint"],
        metrics.endpoint
    );
    assert_eq!(metrics.count.load(Ordering::SeqCst), 2);
}

#[test]
fn metrics_missing_reset_zero_and_unmatched_scopes_never_create_ratios() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let metrics = MetricsServer::new(|index, _| {
        let raw = if index == 0 {
            format!(
                "{DRAFT}{{model=\"reset\"}} 10\n{ACCEPTED}{{model=\"reset\"}} 5\n{DRAFT}{{model=\"zero\"}} 0\n{ACCEPTED}{{model=\"zero\"}} 0\n{DRAFT}{{model=\"only-draft\"}} 1\n{ACCEPTED}{{model=\"only-accepted\"}} 1\n{DRAFT}{{model=\"gone\"}} 1\n"
            )
        } else {
            format!(
                "{DRAFT}{{model=\"reset\"}} 2\n{ACCEPTED}{{model=\"reset\"}} 1\n{DRAFT}{{model=\"zero\"}} 0\n{ACCEPTED}{{model=\"zero\"}} 0\n{DRAFT}{{model=\"only-draft\"}} 2\n{ACCEPTED}{{model=\"only-accepted\"}} 2\n"
            )
        };
        response(raw.as_bytes())
    });
    successful(
        &command(&temp, &server, &metrics.endpoint, "on", workload(1, 0, 1))
            .output()
            .unwrap(),
    );
    let output = report(&temp.path("on"));
    let telemetry = &output["baseline_metrics"]["waves"][0];
    assert!(
        telemetry["acceptance"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["ratio"].is_null())
    );
    let counters = telemetry["counters"].as_array().unwrap();
    assert!(
        counters
            .iter()
            .filter(|v| v["labels"]["model"] == "reset")
            .all(|v| v["delta"].is_null() && v["status"] == "reset_observed")
    );
    assert!(counters.iter().any(|v| v["labels"]["model"] == "gone"
        && v["delta"].is_null()
        && v["status"] == "missing"));
}

#[test]
fn metrics_parser_bounds_and_invalid_samples_remain_diagnostic_failures() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let labels = (0..17)
        .map(|i| format!("l{i}=\"v\""))
        .collect::<Vec<_>>()
        .join(",");
    let samples = vec![
        format!("{DRAFT}{{a=\"x\",a=\"y\"}} 1\n"),
        format!("{DRAFT}{{a=\"x\",b=\"y\"}} 1\n{DRAFT}{{b=\"y\",a=\"x\"}} 2\n"),
        format!("{DRAFT} NaN\n"),
        format!("{DRAFT} +Inf\n"),
        format!("{DRAFT} -1\n"),
        format!("{DRAFT}{{{labels}}} 1\n"),
        format!("{DRAFT}{{a=\"{}\"}} 1\n", "x".repeat(4096)),
        format!("#{}\n", "x".repeat(64 * 1024)),
        (0..257)
            .map(|i| format!("{RUNNING}{{lane=\"{i}\"}} 0\n"))
            .collect(),
        format!("{DRAFT}{{a=\"bad\\q\"}} 1\n"),
        format!("{DRAFT} 1e999\n"),
    ];
    let count = samples.len();
    let metrics = MetricsServer::new(move |index, _| response(samples[index / 2].as_bytes()));
    successful(
        &command(
            &temp,
            &server,
            &metrics.endpoint,
            "invalid",
            workload(1, 0, count as u32),
        )
        .output()
        .unwrap(),
    );
    let output = report(&temp.path("invalid"));
    assert_eq!(output["changes"][0]["eligible"], true);
    for wave in output["baseline_metrics"]["waves"].as_array().unwrap() {
        assert_eq!(wave["receipt"]["before"]["status"], "parse_error");
        assert_eq!(wave["receipt"]["after"]["status"], "parse_error");
        assert!(wave["before_series"].as_array().unwrap().is_empty());
        assert!(wave["receipt"]["before"]["error"].is_string());
    }
    assert_eq!(metrics.count.load(Ordering::SeqCst), count * 2);
}

#[test]
fn metrics_raw_budget_skips_without_calls_and_survives_pause_resume() {
    let temp = Temp::new();
    let (ready, releases) = std::sync::mpsc::sync_channel(1);
    let server = Server::new(move |stream, index, request| {
        if index == 7 {
            let (release, wait) = std::sync::mpsc::sync_channel(0);
            ready.send(release).unwrap();
            wait.recv_timeout(Duration::from_secs(30)).unwrap();
        }
        fast(stream, index, request);
    });
    let metrics = MetricsServer::new(|index, _| {
        assert!(index < 16, "retained raw budget was reset");
        response(&padded())
    });
    let mut work = workload(1, 0, 9);
    work["limits"]["total_ms"] = json!(15000);
    work["limits"]["idle_ms"] = json!(12000);
    let child = command(&temp, &server, &metrics.endpoint, "budget", work)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let release = releases.recv_timeout(Duration::from_secs(60)).unwrap();
    successful(
        &cli()
            .arg("pause")
            .arg(temp.path("budget"))
            .output()
            .unwrap(),
    );
    release.send(()).unwrap();
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(2));
    assert_eq!(metrics.count.load(Ordering::SeqCst), 16);
    let old = fs::read(temp.path("budget/wave-000007/metrics.json")).unwrap();
    let resumed = cli()
        .arg("resume")
        .arg(temp.path("budget"))
        .arg("--json")
        .env("GRILL_METRICS_TEST_KEY", "fixture-secret")
        .output()
        .unwrap();
    assert_eq!(
        resumed.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(metrics.count.load(Ordering::SeqCst), 16);
    assert_eq!(server.count.load(Ordering::SeqCst), 9);
    assert_eq!(
        fs::read(temp.path("budget/wave-000007/metrics.json")).unwrap(),
        old
    );
    let receipt = value(temp.path("budget/wave-000008/metrics.json"));
    for side in ["before", "after"] {
        assert_eq!(receipt[side]["status"], "skipped_budget");
        assert_eq!(receipt[side]["raw_bytes"], 0);
        assert_eq!(receipt[side]["charged_us"], 0);
    }
    let output = compare(&temp.path("budget"));
    assert_eq!(output.status.code(), Some(2));
    let output: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        output["baseline_metrics"]["budget"]["raw_bytes"],
        16 * 1024 * 1024
    );
}

#[test]
fn metrics_failures_do_not_retry_redirect_or_change_performance_eligibility() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let sink = MetricsServer::new(|_, _| panic!("metrics redirect followed"));
    let location = sink.endpoint.clone();
    let metrics = MetricsServer::new(move |index, _| {
        match index {
        0 => format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes(),
        1 => b"HTTP/1.1 503 Unavailable\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops".to_vec(),
        2 => response(&vec![b'x'; 1024 * 1024 + 1]),
        3 => Vec::new(),
        _ => panic!("unexpected retry"),
    }
    });
    successful(
        &command(
            &temp,
            &server,
            &metrics.endpoint,
            "errors",
            workload(1, 0, 2),
        )
        .output()
        .unwrap(),
    );
    let output = report(&temp.path("errors"));
    let waves = &output["baseline_metrics"]["waves"];
    assert_eq!(waves[0]["receipt"]["before"]["status"], "http_error");
    assert_eq!(waves[0]["receipt"]["after"]["status"], "http_error");
    assert_eq!(waves[1]["receipt"]["before"]["status"], "body_limit");
    assert_eq!(waves[1]["receipt"]["after"]["status"], "transport_error");
    assert_eq!(output["changes"][0]["eligible"], true);
    assert_eq!(metrics.count.load(Ordering::SeqCst), 4);
    assert_eq!(sink.count.load(Ordering::SeqCst), 0);
}

#[test]
fn metrics_tamper_missing_companion_and_rehashed_malformed_raw_fail_closed() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let metrics = MetricsServer::new(|_, _| response(format!("{DRAFT} 1\n").as_bytes()));
    successful(
        &command(&temp, &server, &metrics.endpoint, "on", workload(1, 0, 1))
            .output()
            .unwrap(),
    );
    let root = temp.path("on");
    let raw = root.join("wave-000000/metrics-before.bin");
    let original = fs::read(&raw).unwrap();
    fs::write(&raw, b"tampered").unwrap();
    assert_eq!(compare(&root).status.code(), Some(1));
    fs::write(&raw, &original).unwrap();
    let path = root.join("wave-000000/metrics.json");
    let original = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert_eq!(compare(&root).status.code(), Some(1));
    fs::write(&path, original).unwrap();
    let invalid = format!("{DRAFT} NaN\n").into_bytes();
    fs::write(&raw, &invalid).unwrap();
    rewrite_receipt(&root, |receipt| {
        receipt["before"]["raw_bytes"] = json!(invalid.len());
        receipt["before"]["raw_sha256"] = json!(digest(&invalid));
    });
    assert_eq!(compare(&root).status.code(), Some(1));
    assert_eq!(metrics.count.load(Ordering::SeqCst), 2);
}

#[test]
fn metrics_partial_publication_is_unsettled_and_never_resumed() {
    let temp = Temp::new();
    let (ready, seen) = std::sync::mpsc::sync_channel(1);
    let server = Server::new(move |stream, index, request| {
        let (release, wait) = std::sync::mpsc::sync_channel(0);
        ready.send(release).unwrap();
        wait.recv_timeout(Duration::from_secs(30)).unwrap();
        fast(stream, index, request);
    });
    let metrics = MetricsServer::new(|_, _| response(b""));
    let child = command(
        &temp,
        &server,
        &metrics.endpoint,
        "partial",
        workload(1, 0, 1),
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let release = seen.recv_timeout(Duration::from_secs(30)).unwrap();
    fs::create_dir(temp.path("partial/wave-000000/metrics.json")).unwrap();
    release.send(()).unwrap();
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(1));
    assert!(!temp.path("partial/wave-000000/wave.json").exists());
    let output = compare(&temp.path("partial"));
    assert_eq!(output.status.code(), Some(2));
    let output: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        output["baseline"][0]["trial_states"][0],
        "reserved_unsettled"
    );
    let resumed = cli()
        .arg("resume")
        .arg(temp.path("partial"))
        .output()
        .unwrap();
    assert_eq!(resumed.status.code(), Some(1));
    assert_eq!(metrics.count.load(Ordering::SeqCst), 2);
}

#[test]
fn metrics_endpoint_policy_is_admitted_before_any_request() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let metrics = MetricsServer::new(|_, _| panic!("invalid endpoint dispatched"));
    for (index, endpoint) in [
        "http://localhost/metrics",
        "http://192.0.2.1/metrics",
        "https://user:secret@example.com/metrics",
        "https://example.com/metrics?secret=x",
        "https://example.com/metrics#fragment",
    ]
    .into_iter()
    .enumerate()
    {
        let mut cmd = command(
            &temp,
            &server,
            endpoint,
            &format!("bad-{index}"),
            workload(1, 0, 1),
        );
        assert_eq!(cmd.output().unwrap().status.code(), Some(1));
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
    assert_eq!(metrics.count.load(Ordering::SeqCst), 0);
}

#[test]
fn metrics_whole_run_deadline_exhausts_and_later_snapshots_skip() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let (ready, releases) = std::sync::mpsc::sync_channel(32);
    let metrics = MetricsServer::new(move |index, _| {
        assert!(index < 16, "whole-run deadline budget renewed");
        let (release, wait) = std::sync::mpsc::sync_channel(0);
        ready.send(release).unwrap();
        wait.recv_timeout(Duration::from_secs(60)).unwrap();
        response(b"")
    });
    let output = command(
        &temp,
        &server,
        &metrics.endpoint,
        "deadline",
        workload(1, 0, 16),
    )
    .output()
    .unwrap();
    for release in releases.try_iter() {
        release.send(()).unwrap();
    }
    successful(&output);
    let output = report(&temp.path("deadline"));
    let waves = output["baseline_metrics"]["waves"].as_array().unwrap();
    assert_eq!(waves[0]["receipt"]["before"]["status"], "deadline");
    assert_eq!(
        waves.last().unwrap()["receipt"]["after"]["status"],
        "skipped_budget"
    );
    assert!(metrics.count.load(Ordering::SeqCst) <= 15);
    assert_eq!(server.count.load(Ordering::SeqCst), 16);
    assert_eq!(output["changes"][0]["eligible"], true);
}

#[test]
fn metrics_declarations_cannot_silently_pair_different_measurement_conditions() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let first = MetricsServer::new(|_, _| response(b""));
    let second = MetricsServer::new(|_, _| response(b""));
    successful(&run(&temp, &server, "off", &workload(1, 0, 1)));
    successful(
        &command(&temp, &server, &first.endpoint, "first", workload(1, 0, 1))
            .output()
            .unwrap(),
    );
    successful(
        &command(
            &temp,
            &server,
            &second.endpoint,
            "second",
            workload(1, 0, 1),
        )
        .output()
        .unwrap(),
    );
    for other in ["off", "second"] {
        let output = cli()
            .arg("compare")
            .arg(temp.path("first"))
            .arg(temp.path(other))
            .arg("--json")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }
    assert_eq!(first.count.load(Ordering::SeqCst), 2);
    assert_eq!(second.count.load(Ordering::SeqCst), 2);
}

#[test]
fn metrics_interrupt_retains_settled_wave_and_refuses_continuation() {
    let temp = Temp::new();
    let (ready, releases) = std::sync::mpsc::sync_channel(1);
    let server = Server::new(move |mut stream, _, _| {
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"index":0,"delta":{"content":"partial"}}]}),
        );
        let (release, wait) = std::sync::mpsc::sync_channel(0);
        ready.send(release).unwrap();
        wait.recv_timeout(Duration::from_secs(30)).unwrap();
    });
    let metrics = MetricsServer::new(|_, _| response(b""));
    let child = command(
        &temp,
        &server,
        &metrics.endpoint,
        "interrupted",
        workload(1, 0, 2),
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let release = releases.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let output = child.wait_with_output().unwrap();
    release.send(()).unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        wave(&temp, "interrupted", 0)["attempts"][0]["status"],
        "interrupted"
    );
    assert!(temp.path("interrupted/wave-000000/metrics.json").exists());
    assert!(!temp.path("interrupted/wave-000001").exists());
    assert_eq!(compare(&temp.path("interrupted")).status.code(), Some(2));
    assert_eq!(
        cli()
            .arg("resume")
            .arg(temp.path("interrupted"))
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
    assert_eq!(metrics.count.load(Ordering::SeqCst), 2);
}

#[test]
fn metrics_parser_accepts_exact_bounds_and_decodes_full_label_identity() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let mut labels = (1..16).map(|i| format!("l{i}=\"v\"")).collect::<Vec<_>>();
    let escaped = "line\\nquote\\\"slash\\\\";
    let empty = format!("{{l0=\"{escaped}\",{}}}", labels.join(","));
    let padding = "x".repeat(4096 - empty.len());
    labels.insert(0, format!("l0=\"{escaped}{padding}\""));
    let label_set = format!("{{{}}}", labels.join(","));
    assert_eq!(label_set.len(), 4096);
    let metrics = MetricsServer::new(move |index, _| {
        let mut raw = format!(
            "#{}\n{DRAFT}{label_set} {}\n",
            "x".repeat(64 * 1024 - 1),
            index + 1
        );
        for series in 0..255 {
            raw.push_str(&format!("{RUNNING}{{series=\"{series}\"}} 0\n"));
        }
        response(raw.as_bytes())
    });
    successful(
        &command(
            &temp,
            &server,
            &metrics.endpoint,
            "bounds",
            workload(1, 0, 1),
        )
        .output()
        .unwrap(),
    );
    let output = report(&temp.path("bounds"));
    let counter = &output["baseline_metrics"]["waves"][0]["counters"][0];
    assert_eq!(counter["delta"], 1.0);
    assert_eq!(counter["labels"].as_object().unwrap().len(), 16);
    assert!(
        counter["labels"]["l0"]
            .as_str()
            .unwrap()
            .starts_with("line\nquote\"slash\\")
    );
    assert_eq!(
        output["baseline_metrics"]["waves"][0]["before_series"]
            .as_array()
            .unwrap()
            .len(),
        256
    );
}

#[test]
fn metrics_unselected_series_do_not_discard_selected_observations() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let metrics = MetricsServer::new(|index, _| {
        let mut body = String::new();
        for series in 0..1000 {
            body.push_str(&format!("unselected{{series=\"{series}\"}} NaN\n"));
        }
        body.push_str(&format!(
            "{DRAFT} {}\n{ACCEPTED} {}\n",
            index * 4,
            index * 2
        ));
        response(body.as_bytes())
    });
    successful(
        &command(
            &temp,
            &server,
            &metrics.endpoint,
            "selected",
            workload(1, 0, 1),
        )
        .output()
        .unwrap(),
    );
    let output = report(&temp.path("selected"));
    let wave = &output["baseline_metrics"]["waves"][0];
    assert_eq!(wave["receipt"]["before"]["status"], "complete");
    assert_eq!(wave["before_series"].as_array().unwrap().len(), 2);
    assert_eq!(wave["acceptance"][0]["ratio"], 0.5);
}

#[test]
fn metrics_rehashed_impossible_failure_metadata_is_rejected() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let metrics = MetricsServer::new(|_, _| response(format!("{DRAFT} 1\n").as_bytes()));
    successful(
        &command(
            &temp,
            &server,
            &metrics.endpoint,
            "metadata",
            workload(1, 0, 1),
        )
        .output()
        .unwrap(),
    );
    let root = temp.path("metadata");
    let original = value(root.join("wave-000000/metrics.json"));
    for (status, http, scrape) in [
        (
            "unsupported",
            Value::Null,
            original["before"]["scrape_us"].clone(),
        ),
        (
            "transport_error",
            Value::Null,
            original["before"]["scrape_us"].clone(),
        ),
        ("transport_error", json!(200), json!(0)),
    ] {
        rewrite_receipt(&root, |receipt| {
            *receipt = original.clone();
            receipt["before"]["status"] = json!(status);
            receipt["before"]["error"] = json!("fixture failure");
            receipt["before"]["http_status"] = http;
            receipt["before"]["scrape_us"] = scrape;
        });
        assert_eq!(compare(&root).status.code(), Some(1));
    }
    rewrite_receipt(&root, |receipt| *receipt = original);
    successful(&compare(&root));
}

#[test]
fn metrics_nonstandard_http_failure_does_not_abort_model_collection() {
    let temp = Temp::new();
    let server = Server::new(fast);
    let metrics = MetricsServer::new(|_, _| {
        b"HTTP/1.1 600 Failure\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
    });
    successful(
        &command(
            &temp,
            &server,
            &metrics.endpoint,
            "http600",
            workload(1, 0, 1),
        )
        .output()
        .unwrap(),
    );
    let output = report(&temp.path("http600"));
    assert_eq!(output["changes"][0]["eligible"], true);
    for side in ["before", "after"] {
        assert_eq!(
            output["baseline_metrics"]["waves"][0]["receipt"][side]["status"],
            "http_error"
        );
        assert_eq!(
            output["baseline_metrics"]["waves"][0]["receipt"][side]["http_status"],
            600
        );
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    assert_eq!(metrics.count.load(Ordering::SeqCst), 2);
}
