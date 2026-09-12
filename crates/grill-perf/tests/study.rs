use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

#[path = "support/conversation.rs"]
mod conversation;
#[path = "support/selection.rs"]
mod selection;

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "grill-perf-study-{}-{}",
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
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(TcpStream, usize, Value) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, done) = (count.clone(), stop.clone());
        let join = thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut headers = Vec::new();
                        let mut byte = [0];
                        while !headers.ends_with(b"\r\n\r\n") {
                            stream.read_exact(&mut byte).unwrap();
                            headers.push(byte[0]);
                            assert!(headers.len() < 64 * 1024);
                        }
                        let headers = String::from_utf8(headers).unwrap();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        assert!(length < 2 * 1024 * 1024);
                        let mut body = vec![0; length];
                        stream.read_exact(&mut body).unwrap();
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        let index = seen.fetch_add(1, Ordering::SeqCst);
                        handler(stream, index, body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
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

fn response(mut stream: TcpStream, tokens: Option<u64>, error: bool) {
    let mut body =
        String::from("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"1 2\"}}]}\n\n");
    if error {
        body.push_str("data: {\"error\":{\"message\":\"synthetic failure\"}}\n\n");
    } else {
        body.push_str(
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        );
        if let Some(tokens) = tokens {
            body.push_str(&format!(
                "data: {}\n\n",
                json!({"choices":[],"usage":{
                "prompt_tokens":4,"completion_tokens":tokens,"total_tokens":tokens+4}})
            ));
        }
        body.push_str("data: [DONE]\n\n");
    }
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
}

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_grill-perf"))
}
fn decoded(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid CLI JSON: {error}; status={}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn deployment(temp: &Temp, name: &str, settings: &str) -> PathBuf {
    let path = temp.path(name);
    write_json(
        &path,
        &json!({"model_revision":"synthetic-revision", "runtime":"fixture",
        "hardware":"offline-cpu", "settings":settings}),
    );
    path
}
fn baseline(temp: &Temp, server: &Server, declaration: &Path, name: &str) -> Output {
    command()
        .args([
            "baseline",
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
        ])
        .arg("--deployment")
        .arg(declaration)
        .arg("--out")
        .arg(temp.path(name))
        .args(["--local-http", "--json"])
        .output()
        .unwrap()
}
fn check(temp: &Temp, baseline: &str, declaration: &Path, name: &str) -> Output {
    command()
        .arg("check")
        .arg(temp.path(baseline))
        .arg("--deployment")
        .arg(declaration)
        .args(["--change", "settings", "--out"])
        .arg(temp.path(name))
        .arg("--json")
        .output()
        .unwrap()
}
fn compare(before: &Path, after: &Path) -> Output {
    command()
        .arg("compare")
        .arg(before)
        .arg(after)
        .arg("--json")
        .output()
        .unwrap()
}
fn display_label(code: &str) -> &str {
    match code {
        "IMPROVED" => "MEASURED FASTER",
        "REGRESSED" => "MEASURED SLOWER",
        "INCONCLUSIVE" => "INCONCLUSIVE",
        "INVALID" => "INVALID",
        _ => panic!("unexpected result code: {code}"),
    }
}

fn assert_presentation(before: &Path, after: &Path, code: &str) {
    let stored_json = fs::read(after.join("report.json")).unwrap();
    let stored_text = fs::read(after.join("report.txt")).unwrap();
    let machine = compare(before, after);
    assert_eq!(decoded(&machine)["result"], code);
    let output = command()
        .arg("compare")
        .arg(before)
        .arg(after)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), machine.status.code());
    let text = String::from_utf8(output.stdout).unwrap();
    let mut lines = text.lines();
    assert_eq!(
        lines.next().unwrap().split(':').next(),
        Some(display_label(code))
    );
    assert!(lines.next().unwrap().starts_with("Scope: structured C1"));
    assert_eq!(compare(before, after).stdout, machine.stdout);
    assert_eq!(fs::read(after.join("report.json")).unwrap(), stored_json);
    assert_eq!(fs::read(after.join("report.txt")).unwrap(), stored_text);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn file_hash(path: &Path) -> String {
    hash(&fs::read(path).unwrap())
}

// These are synthetic fixture receipts, not edits to collected/historical evidence.
// Independent fingerprint construction keeps the offline test sensitive to loader drift.
fn native_hash(root: &Path) -> String {
    let mut fingerprint = Sha256::new();
    fingerprint.update(b"grill-perf-evidence-v1\0");
    fingerprint.update(file_hash(&root.join("plan.json")).as_bytes());
    fingerprint.update(
        read_json(&root.join("plan.json"))["source_sha256"]
            .as_str()
            .unwrap()
            .as_bytes(),
    );
    for index in 0..4 {
        let wave = root.join(format!("wave-{index:06}"));
        fingerprint.update(file_hash(&wave.join("reservation.json")).as_bytes());
        fingerprint.update(b"published\0");
        fingerprint.update(file_hash(&wave.join("wave.json")).as_bytes());
    }
    let mut history = Sha256::new();
    history.update(file_hash(&root.join("session-000000/session.json")).as_bytes());
    history.update(b"settled\0");
    history.update(file_hash(&root.join("session-000000/run.json")).as_bytes());
    history.update(b"root-present\0");
    fingerprint.update(hex(&history.finalize()).as_bytes());
    hex(&fingerprint.finalize())
}

fn seal_native(root: &Path) -> String {
    let plan = read_json(&root.join("plan.json"));
    let plan_hash = file_hash(&root.join("plan.json"));
    for index in 0..4 {
        let wave = root.join(format!("wave-{index:06}"));
        let mut reservation = read_json(&wave.join("reservation.json"));
        reservation["plan_sha256"] = json!(plan_hash);
        write_json(&wave.join("reservation.json"), &reservation);
        let mut receipt = read_json(&wave.join("wave.json"));
        receipt["plan_sha256"] = json!(plan_hash);
        receipt["reservation_sha256"] = json!(file_hash(&wave.join("reservation.json")));
        write_json(&wave.join("wave.json"), &receipt);
    }
    let path = root.join("session-000000/session.json");
    let mut session = read_json(&path);
    session["plan_sha256"] = json!(plan_hash);
    session["collector_sha256"] = plan["collector_sha256"].clone();
    session["started_unix_ms"] = plan["started_unix_ms"].clone();
    write_json(&path, &session);
    plan_hash
}

fn repin_collector(root: &Path, collector: &str) {
    let mut capture = read_json(&root.join("capture.json"));
    capture["collector_sha256"] = json!(collector);
    for index in 0..8 {
        let path = root.join(format!("acquisition-{index:02}"));
        let mut plan = read_json(&path.join("plan.json"));
        plan["collector_sha256"] = json!(collector);
        write_json(&path.join("plan.json"), &plan);
        capture["acquisitions"][index]["plan_sha256"] = json!(seal_native(&path));
        capture["acquisitions"][index]["evidence_sha256"] = json!(native_hash(&path));
    }
    write_json(&root.join("capture.json"), &capture);
}

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn retime(root: &Path, elapsed: [u64; 8], baseline: Option<(&str, u64)>) {
    let mut capture = read_json(&root.join("capture.json"));
    let mut started = capture["acquisitions"][0]["started_unix_ms"]
        .as_u64()
        .unwrap();
    if let Some((hash, finished)) = baseline {
        capture["baseline_sha256"] = json!(hash);
        started = finished + 1;
    }
    for (index, duration) in elapsed.into_iter().enumerate() {
        let acquisition = root.join(format!("acquisition-{index:02}"));
        let mut plan = read_json(&acquisition.join("plan.json"));
        plan["started_unix_ms"] = json!(started);
        write_json(&acquisition.join("plan.json"), &plan);
        for wave in 0..4 {
            let path = acquisition.join(format!("wave-{wave:06}/wave.json"));
            let mut receipt = read_json(&path);
            receipt["elapsed_us"] = json!(duration);
            receipt["dispatch_spread_us"] = json!(0);
            receipt["achieved_completion_tokens_per_second"] =
                json!(400.0 * 1_000_000.0 / duration as f64);
            receipt["attempts"][0]["timing"] = json!({
                "dispatch_offset_us":0,"headers_us":1,"first_body_us":2,
                "first_generated_text_us":2,"first_generated_channel":"answer",
                "first_answer_text_us":2,"last_generated_text_us":2,
                "terminal_us":duration-1,"settle_us":duration,"capture_parse_us":0});
            write_json(&path, &receipt);
        }
        capture["acquisitions"][index]["plan_sha256"] = json!(seal_native(&acquisition));
        capture["acquisitions"][index]["started_unix_ms"] = json!(started);
        let finished = started + (duration * 4).div_ceil(1000) + 1;
        capture["acquisitions"][index]["finished_unix_ms"] = json!(finished);
        started = finished + 1;
        capture["acquisitions"][index]["evidence_sha256"] = json!(native_hash(&acquisition));
        capture["acquisitions"][index]["median_achieved_completion_tokens_per_second"] =
            json!(400.0 * 1_000_000.0 / duration as f64);
    }
    write_json(&root.join("capture.json"), &capture);
}

#[test]
fn complete_workflow_replays_without_mutation_and_assesses_acquisition_not_wave_samples() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, body| {
        assert_eq!(body["model"], "fixture");
        assert_eq!(body["max_tokens"], 400);
        assert_eq!(body["stream"], true);
        assert_eq!(body["temperature"].as_f64(), Some(0.0));
        assert_eq!(body["top_p"].as_f64(), Some(1.0));
        response(stream, Some(400), false);
    });
    let before_deployment = deployment(&temp, "before.json", "before");
    let after_deployment = deployment(&temp, "after.json", "after");
    let before = baseline(&temp, &server, &before_deployment, "before");
    assert!(
        before.status.success(),
        "{}",
        String::from_utf8_lossy(&before.stderr)
    );
    assert_eq!(decoded(&before)["baseline_ready"], true);
    let before_manifest = fs::read(temp.path("before/capture.json")).unwrap();
    let after = check(&temp, "before", &after_deployment, "after");
    let initial = decoded(&after);
    assert_ne!(initial["result"], "INVALID", "{initial}");
    let written_text = fs::read_to_string(temp.path("after/report.txt")).unwrap();
    assert_eq!(
        written_text.lines().next().unwrap().split(':').next(),
        Some(display_label(initial["result"].as_str().unwrap()))
    );
    assert!(
        written_text
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("Scope: structured C1")
    );
    assert_eq!(initial["model"], "fixture");
    assert_eq!(initial["declared_change"], "settings");
    for side in ["baseline_accounting", "candidate_accounting"] {
        assert_eq!(initial[side]["dispatched_requests"], 32);
        assert_eq!(initial[side]["complete_requests"], 32);
        assert_eq!(initial[side]["reported_completion_tokens"], 12800);
        assert_eq!(initial[side]["missing_completion_usage_requests"], 0);
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 64);
    assert_eq!(
        before_manifest,
        fs::read(temp.path("before/capture.json")).unwrap()
    );
    let recorded_report = fs::read(temp.path("after/report.json")).unwrap();
    for _ in 0..2 {
        assert_eq!(
            decoded(&compare(&temp.path("before"), &temp.path("after"))),
            initial
        );
        assert_eq!(
            recorded_report,
            fs::read(temp.path("after/report.json")).unwrap()
        );
    }
    drop(server);

    retime(&temp.path("before"), [400_000; 8], None);
    let baseline_hash = file_hash(&temp.path("before/capture.json"));
    let baseline_finish =
        read_json(&temp.path("before/capture.json"))["acquisitions"][7]["finished_unix_ms"]
            .as_u64()
            .unwrap();
    for (durations, expected) in [
        (
            [
                200_000, 210_000, 190_000, 205_000, 195_000, 202_000, 198_000, 200_000,
            ],
            "IMPROVED",
        ),
        (
            [
                800_000, 810_000, 790_000, 805_000, 795_000, 802_000, 798_000, 800_000,
            ],
            "REGRESSED",
        ),
        (
            [
                100_000, 1_600_000, 100_000, 1_600_000, 100_000, 1_600_000, 100_000, 1_600_000,
            ],
            "INCONCLUSIVE",
        ),
    ] {
        retime(
            &temp.path("after"),
            durations,
            Some((&baseline_hash, baseline_finish)),
        );
        let report = decoded(&compare(&temp.path("before"), &temp.path("after")));
        assert_eq!(report["result"], expected, "{report}");
        assert_presentation(&temp.path("before"), &temp.path("after"), expected);
        let logs = durations.map(|us| (400_000.0 / us as f64).ln());
        let mean = logs.iter().sum::<f64>() / 8.0;
        let variance = logs.iter().map(|value| (value - mean).powi(2)).sum::<f64>() / 7.0;
        let se = (variance / 8.0).sqrt();
        let interval = report["model_based_interval_percent"].as_array().unwrap();
        let observed = report["observed_change_percent"].as_f64().unwrap();
        assert!((observed - mean.exp_m1() * 100.0).abs() < 1e-9);
        for (actual, sign) in interval.iter().zip([-1.0, 1.0]) {
            let expected = (mean + sign * 2.364624251 * se).exp_m1() * 100.0;
            assert!((actual.as_f64().unwrap() - expected).abs() < 1e-9);
        }
    }
    retime(
        &temp.path("after"),
        [200_000; 8],
        Some((&baseline_hash, baseline_finish)),
    );
    let degenerate = decoded(&compare(&temp.path("before"), &temp.path("after")));
    assert_eq!(degenerate["result"], "INCONCLUSIVE");
    assert!((degenerate["observed_change_percent"].as_f64().unwrap() - 100.0).abs() < 1e-9);
    assert!(degenerate["model_based_interval_percent"].is_null());
    assert!(degenerate["model_based_confidence_level"].is_null());
    let mixed = decoded(&compare(
        &temp.path("before"),
        &temp.path("after/acquisition-00"),
    ));
    assert_eq!(mixed["result"], "INVALID");
    assert!(mixed["model_based_interval_percent"].is_null());
    let raw = compare(
        &temp.path("before/acquisition-00"),
        &temp.path("after/acquisition-00"),
    );
    assert!(
        raw.status.success(),
        "{}",
        String::from_utf8_lossy(&raw.stderr)
    );
    assert!(decoded(&raw).get("changes").is_some());
    assert!(decoded(&raw).get("result").is_none());
    for path in [
        "before/acquisition-00/report.json",
        "after/acquisition-00/report.json",
    ] {
        write_json(&temp.path(path), &json!({"exported_raw_comparison": true}));
    }
    let exported = compare(
        &temp.path("before/acquisition-00"),
        &temp.path("after/acquisition-00"),
    );
    assert!(exported.status.success());
    assert_eq!(exported.stdout, raw.stdout);
    let good_capture = fs::read(temp.path("after/capture.json")).unwrap();
    let mut overlap = read_json(&temp.path("after/capture.json"));
    overlap["acquisitions"][0]["finished_unix_ms"] =
        overlap["acquisitions"][1]["finished_unix_ms"].clone();
    write_json(&temp.path("after/capture.json"), &overlap);
    assert_eq!(
        decoded(&compare(&temp.path("before"), &temp.path("after")))["result"],
        "INVALID"
    );
    assert_presentation(&temp.path("before"), &temp.path("after"), "INVALID");
    fs::write(temp.path("after/capture.json"), &good_capture).unwrap();

    fs::rename(
        temp.path("after/acquisition-01"),
        temp.path("saved-acquisition"),
    )
    .unwrap();
    copy_tree(
        &temp.path("after/acquisition-00"),
        &temp.path("after/acquisition-01"),
    );
    let mut duplicated = read_json(&temp.path("after/capture.json"));
    duplicated["acquisitions"][1] = duplicated["acquisitions"][0].clone();
    duplicated["acquisitions"][1]["directory"] = json!("acquisition-01");
    write_json(&temp.path("after/capture.json"), &duplicated);
    assert_eq!(
        decoded(&compare(&temp.path("before"), &temp.path("after")))["result"],
        "INVALID"
    );
    fs::remove_dir_all(temp.path("after/acquisition-01")).unwrap();
    fs::rename(
        temp.path("saved-acquisition"),
        temp.path("after/acquisition-01"),
    )
    .unwrap();
    fs::write(temp.path("after/capture.json"), &good_capture).unwrap();

    let collector = read_json(&temp.path("after/capture.json"))["collector_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    repin_collector(&temp.path("after"), &hash(b"synthetic-other-collector"));
    assert_eq!(
        decoded(&compare(&temp.path("before"), &temp.path("after")))["result"],
        "INVALID"
    );
    repin_collector(&temp.path("after"), &collector);

    fs::write(
        temp.path("after/acquisition-00/wave-000000/response-0000.bin"),
        b"corrupt",
    )
    .unwrap();
    let invalid = decoded(&compare(&temp.path("before"), &temp.path("after")));
    assert_eq!(invalid["result"], "INVALID");
    assert!(invalid["observed_change_percent"].is_null());
    assert_eq!(
        recorded_report,
        fs::read(temp.path("after/report.json")).unwrap()
    );
    for root in ["before", "after"] {
        fs::remove_file(temp.path(&format!("{root}/capture.json"))).unwrap();
        fs::remove_file(temp.path(&format!("{root}/report.json"))).unwrap();
    }
    let missing_markers = decoded(&compare(&temp.path("before"), &temp.path("after")));
    assert_eq!(missing_markers["result"], "INVALID");
    assert!(!temp.path("after/capture.json").exists());
    assert!(!temp.path("after/report.json").exists());
}

#[test]
fn unchanged_control_dispatches_same_deployment_and_replays_period_shifts_offline() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let declaration = deployment(&temp, "same.json", "same");
    let before = baseline(&temp, &server, &declaration, "before");
    assert!(before.status.success());
    assert!(decoded(&before)["declared_change"].is_null());
    let baseline_manifest = fs::read(temp.path("before/capture.json")).unwrap();
    assert!(read_json(&temp.path("before/capture.json"))["change"].is_null());
    let output = command()
        .arg("check")
        .arg(temp.path("before"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--out"])
        .arg(temp.path("control"))
        .arg("--json")
        .output()
        .unwrap();
    let initial = decoded(&output);
    assert_ne!(initial["result"], "INVALID", "{initial}");
    assert_eq!(initial["declared_change"], "none");
    assert_eq!(initial["baseline_complete_acquisitions"], 8);
    assert_eq!(initial["candidate_complete_acquisitions"], 8);
    assert_eq!(initial["candidate_accounting"]["dispatched_requests"], 32);
    assert_eq!(server.count.load(Ordering::SeqCst), 64);
    assert_eq!(
        fs::read(temp.path("control/deployment.json")).unwrap(),
        fs::read(temp.path("before/deployment.json")).unwrap()
    );
    assert_eq!(
        baseline_manifest,
        fs::read(temp.path("before/capture.json")).unwrap()
    );
    assert_eq!(
        read_json(&temp.path("control/capture.json"))["change"],
        "none"
    );
    drop(server);

    let recorded_report = fs::read(temp.path("control/report.json")).unwrap();
    assert_eq!(
        decoded(&compare(&temp.path("before"), &temp.path("control"))),
        initial
    );
    assert_eq!(
        recorded_report,
        fs::read(temp.path("control/report.json")).unwrap()
    );
    retime(&temp.path("before"), [400_000; 8], None);
    let baseline_hash = file_hash(&temp.path("before/capture.json"));
    let baseline_finish =
        read_json(&temp.path("before/capture.json"))["acquisitions"][7]["finished_unix_ms"]
            .as_u64()
            .unwrap();
    for (durations, expected) in [
        (
            [
                200_000, 210_000, 190_000, 205_000, 195_000, 202_000, 198_000, 200_000,
            ],
            "IMPROVED",
        ),
        (
            [
                800_000, 810_000, 790_000, 805_000, 795_000, 802_000, 798_000, 800_000,
            ],
            "REGRESSED",
        ),
    ] {
        retime(
            &temp.path("control"),
            durations,
            Some((&baseline_hash, baseline_finish)),
        );
        let report = decoded(&compare(&temp.path("before"), &temp.path("control")));
        assert_eq!(report["result"], expected, "{report}");
        assert_eq!(report["declared_change"], "none");
        assert_presentation(&temp.path("before"), &temp.path("control"), expected);
    }
    let mut capture = read_json(&temp.path("control/capture.json"));
    capture["change"] = Value::Null;
    write_json(&temp.path("control/capture.json"), &capture);
    assert_eq!(
        decoded(&compare(&temp.path("before"), &temp.path("control")))["result"],
        "INVALID"
    );
}

#[test]
fn unchanged_control_rejects_each_changed_field_and_preserves_selected_change_rules() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let declaration = deployment(&temp, "same.json", "same");
    assert!(
        baseline(&temp, &server, &declaration, "before")
            .status
            .success()
    );
    let original = read_json(&declaration);
    let count = server.count.load(Ordering::SeqCst);
    let rejected = |change: &str, name: &str| {
        let output = command()
            .arg("check")
            .arg(temp.path("before"))
            .arg("--deployment")
            .arg(&declaration)
            .args(["--change", change, "--out"])
            .arg(temp.path(name))
            .arg("--json")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let report = decoded(&output);
        assert_eq!(report["result"], "INVALID", "{report}");
        assert_eq!(report["declared_change"], change);
        assert!(report["candidate_accounting"].is_null());
        assert!(!temp.path(&format!("{name}/capture.json")).exists());
        assert!(!temp.path(&format!("{name}/acquisition-00")).exists());
        assert_eq!(server.count.load(Ordering::SeqCst), count);
    };
    for (field, other) in [
        ("model_revision", "runtime"),
        ("runtime", "hardware"),
        ("hardware", "settings"),
        ("settings", "model_revision"),
    ] {
        let mut changed = original.clone();
        changed[field] = json!("changed");
        write_json(&declaration, &changed);
        rejected("none", &format!("control-{field}"));

        changed[other] = json!("undeclared-change");
        write_json(&declaration, &changed);
        rejected(field, &format!("undeclared-{field}"));

        write_json(&declaration, &original);
        rejected(field, &format!("missing-change-{field}"));
    }
}

#[test]
fn check_rejects_missing_baseline_and_mismatched_declarations_before_dispatch() {
    let temp = Temp::new();
    let response_kind = Arc::new(AtomicUsize::new(0));
    let selected = response_kind.clone();
    let server = Server::new(move |stream, _, _| match selected.load(Ordering::SeqCst) {
        1 => response(stream, Some(399), false),
        2 => response(stream, None, false),
        3 => response(stream, Some(400), true),
        _ => response(stream, Some(400), false),
    });
    let before = deployment(&temp, "before.json", "before");
    let after = deployment(&temp, "after.json", "after");
    let missing = check(&temp, "missing", &after, "missing-report");
    assert_eq!(decoded(&missing)["result"], "INVALID");
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
    let both_missing = compare(&temp.path("absent-before"), &temp.path("absent-after"));
    assert_eq!(both_missing.status.code(), Some(1));
    assert_eq!(decoded(&both_missing)["result"], "INVALID");
    assert!(!temp.path("absent-before").exists());
    assert!(!temp.path("absent-after").exists());
    assert_eq!(
        read_json(&temp.path("missing-report/report.json")),
        decoded(&missing)
    );
    let collected = baseline(&temp, &server, &before, "before");
    assert!(collected.status.success());
    for (kind, tokens, error) in [
        (1, Some(399), false),
        (2, None, false),
        (3, Some(400), true),
    ] {
        response_kind.store(kind, Ordering::SeqCst);
        let root = format!("invalid-candidate-{kind}");
        let before_count = server.count.load(Ordering::SeqCst);
        let report = decoded(&check(&temp, "before", &after, &root));
        assert_eq!(report["result"], "INVALID");
        assert_response_failure(&report, &temp.path(&root), tokens, error);
        assert_eq!(server.count.load(Ordering::SeqCst), before_count + 1);
        assert_eq!(
            decoded(&compare(&temp.path("before"), &temp.path(&root))),
            report
        );
    }
    response_kind.store(0, Ordering::SeqCst);
    let count = server.count.load(Ordering::SeqCst);
    let unchanged = check(&temp, "before", &before, "unchanged");
    assert_eq!(decoded(&unchanged)["result"], "INVALID");
    let mut changed = read_json(&after);
    changed["hardware"] = json!("also-changed");
    write_json(&after, &changed);
    let mismatch = decoded(&check(&temp, "before", &after, "mismatch"));
    assert_eq!(mismatch["result"], "INVALID");
    assert_eq!(mismatch["baseline_complete_acquisitions"], 8);
    assert_eq!(mismatch["candidate_complete_acquisitions"], 0);
    assert_eq!(
        mismatch["baseline_acquisition_medians"],
        read_json(&temp.path("before/report.json"))["baseline_acquisition_medians"]
    );
    assert_eq!(server.count.load(Ordering::SeqCst), count);
    let original = read_json(&temp.path("before/capture.json"))["collector_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    repin_collector(&temp.path("before"), &hash(b"synthetic-other-collector"));
    let foreign = check(&temp, "before", &before, "foreign-collector");
    let report = decoded(&foreign);
    assert_eq!(report["result"], "INVALID");
    assert!(
        report["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("collector binary differs"))
    );
    assert_eq!(server.count.load(Ordering::SeqCst), count);
    repin_collector(&temp.path("before"), &original);
    fs::remove_file(temp.path("before/acquisition-00/wave-000000/response-0000.bin")).unwrap();
    let corrupt = check(&temp, "before", &after, "corrupt");
    assert_eq!(decoded(&corrupt)["result"], "INVALID");
    assert_eq!(server.count.load(Ordering::SeqCst), count);
}

fn assert_response_failure(report: &Value, root: &Path, tokens: Option<u64>, error: bool) {
    let failure = &report["first_failure"];
    assert_eq!(failure["acquisition"], 0);
    assert_eq!(failure["wave"]["index"], 0);
    assert_eq!(failure["wave"]["phase"], "warmup");
    assert_eq!(failure["lane"], 0);
    assert_eq!(failure["expected_completion_tokens"], 400);
    assert_eq!(
        failure["usage"]["completion_tokens"],
        json!(if error { None } else { tokens })
    );
    assert_eq!(
        failure["status"],
        if error { "unsupported" } else { "complete" }
    );
    let reason = if error {
        "response_not_complete"
    } else if tokens.is_none() {
        "completion_usage_unavailable"
    } else {
        "reported_output_not_exact"
    };
    assert!(
        failure["eligibility_errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == reason)
    );
    assert_eq!(
        failure["receipt_path"],
        json!(root.join("acquisition-00/wave-000000/wave.json"))
    );
    assert_eq!(
        failure["response_path"],
        json!(root.join("acquisition-00/wave-000000/response-0000.bin"))
    );
    let capture = read_json(&root.join("capture.json"));
    assert_eq!(report["stop_reason"], capture["stop_reason"]);
    assert_eq!(
        failure["acquisition_status"],
        capture["acquisitions"][0]["status"]
    );
}

#[test]
fn invalid_warmup_stops_without_replacement_and_retains_raw_response() {
    for (tokens, error) in [(Some(399), false), (None, false), (Some(400), true)] {
        let temp = Temp::new();
        let server = Server::new(move |stream, _, _| response(stream, tokens, error));
        let declaration = deployment(&temp, "deployment.json", "before");
        let output = baseline(&temp, &server, &declaration, "before");
        let report = decoded(&output);
        assert_eq!(report["result"], "INVALID", "{report}");
        assert_response_failure(&report, &temp.path("before"), tokens, error);
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
        assert_eq!(report["baseline_complete_acquisitions"], 0);
        assert!(report["model_based_interval_percent"].is_null());
        assert_eq!(report["baseline_accounting"]["dispatched_requests"], 1);
        if tokens.is_none() || error {
            assert!(report["baseline_accounting"]["reported_completion_tokens"].is_null());
            assert_eq!(
                report["baseline_accounting"]["missing_completion_usage_requests"],
                1
            );
        } else {
            assert_eq!(
                report["baseline_accounting"]["reported_completion_tokens"],
                399
            );
        }
        assert!(
            temp.path("before/acquisition-00/wave-000000/response-0000.bin")
                .is_file()
        );
        assert!(!temp.path("before/acquisition-00/wave-000001").exists());
        let manifest = read_json(&temp.path("before/capture.json"));
        assert_eq!(manifest["status"], "invalid");
        assert!(manifest["acquisitions"][1]["plan_sha256"].is_null());
    }
}

#[test]
fn whole_budget_stop_withholds_direction_and_does_not_dispatch_replacement() {
    let temp = Temp::new();
    let server = Server::new(|mut stream, _, _| {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        thread::sleep(Duration::from_secs(2));
    });
    let declaration = deployment(&temp, "deployment.json", "before");
    let start = Instant::now();
    let output = command()
        .args([
            "baseline",
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
        ])
        .arg("--deployment")
        .arg(&declaration)
        .arg("--out")
        .arg(temp.path("before"))
        .args(["--local-http", "--seconds", "1", "--json"])
        .output()
        .unwrap();
    let report = decoded(&output);
    assert_eq!(report["result"], "INCONCLUSIVE", "{report}");
    assert_eq!(report["baseline_ready"], false);
    assert!(report["observed_change_percent"].is_null());
    assert!(report["model_based_interval_percent"].is_null());
    assert!(server.count.load(Ordering::SeqCst) <= 1);
    assert!(start.elapsed() < Duration::from_secs(30));
    assert_eq!(
        read_json(&temp.path("before/capture.json"))["status"],
        "incomplete"
    );
    let after = deployment(&temp, "after.json", "after");
    let count = server.count.load(Ordering::SeqCst);
    let output = check(&temp, "before", &after, "after");
    assert_eq!(decoded(&output)["result"], "INCONCLUSIVE");
    assert_eq!(server.count.load(Ordering::SeqCst), count);
}

#[test]
fn interruption_still_cancels_an_active_request_after_the_first_acquisition() {
    let temp = Temp::new();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let server = Server::new(move |mut stream, index, _| {
        if index != 4 {
            response(stream, Some(400), false);
            return;
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
            .unwrap();
        stream.flush().unwrap();
        entered.send(()).unwrap();
        let _ = gate.recv_timeout(Duration::from_secs(15));
    });
    let declaration = deployment(&temp, "before.json", "before");
    let mut child = command()
        .args([
            "baseline",
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture-model",
            "--local-http",
            "--json",
        ])
        .arg("--deployment")
        .arg(declaration)
        .arg("--out")
        .arg(temp.path("before"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let outcome = (|| -> Result<(), String> {
        ready
            .recv_timeout(Duration::from_secs(30))
            .map_err(|e| e.to_string())?;
        if unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) } != 0 {
            return Err("cannot interrupt fixture collector".into());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("second-acquisition network wait ignored interruption".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    })();
    let _ = release.send(());
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(output.status.code(), Some(2));
    let report = decoded(&output);
    assert_eq!(report["result"], "INCONCLUSIVE");
    assert_eq!(report["baseline_complete_acquisitions"], 1);
    assert_eq!(server.count.load(Ordering::SeqCst), 5);
}
