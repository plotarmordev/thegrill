#![cfg(target_os = "linux")]
// Regressions against an independent counted loopback HTTP fixture. Every
// scenario proves dispatch counts and persisted evidence through the real CLI;
// nothing here contacts a model service.
use serde::Deserialize;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "grill-runner-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("scratch directory: {e}"),
            }
        }
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The documented identity encoding H(domain, bytes), independently restated.
fn digest(domain: &str, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"the-grill\0");
    hash.update(domain.as_bytes());
    hash.update(b"\0v1\0");
    hash.update(bytes);
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// ----- Counted loopback fixture -----

enum Step {
    Send(Vec<u8>),
    Sleep(Duration),
    Wait(std::sync::mpsc::Receiver<()>),
}

struct Server {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    connections: Arc<AtomicUsize>,
}

impl Server {
    fn endpoint(&self) -> String {
        format!("http://{}/v1/chat/completions", self.addr)
    }
    fn requests(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn request(&self, i: usize) -> Vec<u8> {
        self.requests.lock().unwrap()[i].clone()
    }
    fn wait_requests(&self, n: usize) {
        let start = Instant::now();
        while self.requests() < n {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "fixture never saw request {n}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => buffer.push(byte[0]),
        }
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buffer).to_string();
    let length = head
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    buffer.extend_from_slice(&body);
    Some(buffer)
}

fn serve(scripts: Vec<Vec<Step>>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(AtomicUsize::new(0));
    let scripts = Arc::new(Mutex::new(
        scripts.into_iter().map(Some).collect::<Vec<_>>(),
    ));
    let (r, c) = (requests.clone(), connections.clone());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            c.fetch_add(1, Ordering::SeqCst);
            let (requests, scripts) = (r.clone(), scripts.clone());
            thread::spawn(move || {
                while let Some(request) = read_request(&mut stream) {
                    let index = {
                        let mut all = requests.lock().unwrap();
                        all.push(request);
                        all.len() - 1
                    };
                    let script = scripts
                        .lock()
                        .unwrap()
                        .get_mut(index)
                        .and_then(Option::take);
                    let Some(script) = script else { return };
                    for step in script {
                        match step {
                            Step::Send(bytes) => {
                                if stream
                                    .write_all(&bytes)
                                    .and_then(|_| stream.flush())
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Step::Sleep(d) => thread::sleep(d),
                            Step::Wait(release) => {
                                release.recv_timeout(Duration::from_secs(20)).unwrap();
                            }
                        }
                    }
                }
            });
        }
    });
    Server {
        addr,
        requests,
        connections,
    }
}

fn entity(status: &str, content_type: &str, body: &[u8]) -> Vec<Step> {
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    vec![Step::Send(bytes)]
}

fn ok_json(body: &str) -> Vec<Step> {
    entity("200 OK", "application/json", body.as_bytes())
}

fn answer(text: &str, finish: &str) -> String {
    format!(
        "{{\"id\":\"x\",\"object\":\"chat.completion\",\"choices\":[{{\"index\":0,\"message\":{{\"role\":\"assistant\",\"content\":{},\"tool_calls\":[]}},\"finish_reason\":\"{finish}\"}}],\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}}}",
        serde_json::to_string(text).unwrap()
    )
}

fn chunk(bytes: &[u8]) -> Step {
    let mut out = format!("{:x}\r\n", bytes.len()).into_bytes();
    out.extend_from_slice(bytes);
    out.extend_from_slice(b"\r\n");
    Step::Send(out)
}

fn sse_head() -> Step {
    Step::Send(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(),
    )
}

fn sse_end() -> Step {
    Step::Send(b"0\r\n\r\n".to_vec())
}

fn delta(content: &str, finish: Option<&str>) -> String {
    format!(
        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}},\"finish_reason\":{}}}]}}\n\n",
        serde_json::to_string(content).unwrap(),
        finish.map_or("null".to_string(), |f| format!("\"{f}\""))
    )
}

// ----- CLI helpers -----

const PACK: &[u8] = include_bytes!("../examples/synthetic-pack.json");
const B: &[u8] = include_bytes!("../examples/submission-b.json");
const ONE: &str = r#"{"version":1,"label":"one","worlds":["w"],"groups":["g"],"cases":[{"id":"tile","world":"w","group":"g","messages":[{"role":"user","content":"Name the teal tile."}],"accepted":["T"],"qualification":{"valid":["{\"answer\":\"T\"}"],"wrong":["{\"answer\":\"t\"}"]}}]}"#;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_grill"));
    c.env_remove("GRILL_TEST_TOKEN");
    c
}

fn run_args(pack: &Path, endpoint: &str, out: &Path) -> Vec<String> {
    vec![
        "run".into(),
        pack.to_string_lossy().into_owned(),
        "--endpoint".into(),
        endpoint.into(),
        "--model".into(),
        "fixture-model".into(),
        "--out".into(),
        out.to_string_lossy().into_owned(),
        "--local-http".into(),
        "--token-cap".into(),
        "128".into(),
    ]
}

fn run(pack: &Path, endpoint: &str, out: &Path, extra: &[&str]) -> Output {
    bin()
        .args(run_args(pack, endpoint, out))
        .args(extra)
        .output()
        .unwrap()
}

fn cli(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[derive(Deserialize)]
struct Terminal {
    reason: String,
    commitment: String,
    stop: Option<String>,
    dispatched: bool,
    body: TerminalBody,
    #[serde(rename = "final")]
    final_artifact: Option<serde_json::Value>,
    http: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct TerminalBody {
    bytes: u64,
    complete: bool,
    truncated: bool,
    terminal_offset: Option<u64>,
    surplus_retained: u64,
    surplus_observed: u64,
}
#[derive(Deserialize)]
struct Case {
    evidence: String,
    outcome: String,
    reason: Option<String>,
}
#[derive(Deserialize)]
struct View {
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Counts {
    collected: u32,
    unresolved: u32,
    not_started: u32,
    invalid: u32,
}
#[derive(Deserialize)]
struct Inspection {
    counts: Counts,
    view: String,
}
#[derive(Deserialize)]
struct Bounds {
    lower: i64,
    upper: i64,
}
#[derive(Deserialize)]
struct Compared {
    n: u32,
    left_evidence: String,
    right_evidence: String,
    delta_b_minus_a: Bounds,
}

fn terminal(run: &Path, attempt: u32) -> Terminal {
    let path = run
        .join("attempts")
        .join(format!("{attempt:06}"))
        .join("terminal.json");
    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
}

fn view(path: &Path) -> View {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn inspect(path: &Path) -> Inspection {
    let output = cli(&["inspect", &path.to_string_lossy(), "--json"]);
    success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn failed_preflight_makes_no_request() {
    let s = Scratch::new();
    let server = serve(vec![]);
    let pack = s.write("one.json", ONE.as_bytes());
    let base = server.endpoint();
    for (endpoint, extra) in [
        (format!("{base}?x=1"), vec![]),
        (format!("{base}#f"), vec![]),
        (base.replace("127.0.0.1", "localhost"), vec![]),
        (base.clone(), vec!["--auth-env", "GRILL_TEST_TOKEN"]),
        (base.clone(), vec!["--idle-ms", "60000"]),
        (base.clone(), vec!["--top-p-milli", "0"]),
    ] {
        let out = s.path("never");
        let output = run(&pack, &endpoint, &out, &extra);
        assert_eq!(output.status.code(), Some(1), "{endpoint} {extra:?}");
        assert!(!out.exists());
    }
    let output = bin()
        .args([
            "run",
            &pack.to_string_lossy(),
            "--endpoint",
            &base,
            "--model",
            "m",
            "--out",
            "x",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);
    assert_eq!(server.requests(), 0);
}

#[test]
fn plan_is_read_only_and_matches_v2_dispatched_bytes() {
    let s = Scratch::new();
    let mut wire: serde_json::Value = serde_json::from_str(ONE).unwrap();
    wire["version"] = 2.into();
    wire["cases"][0].as_object_mut().unwrap().remove("accepted");
    wire["cases"][0]["messages"][0]["content"] = "Grid input: é\n[[0]]".into();
    wire["cases"][0]["acceptance"] = serde_json::json!({"kind": "grid-set", "outputs": [[[9]]]});
    wire["cases"][0]["qualification"] = serde_json::json!({
        "valid": [r#"{"answer":[[[9]]]}"#], "wrong": [r#"{"answer":[[[0]]]}"#]
    });
    let pack = s.write("v2.json", &serde_json::to_vec(&wire).unwrap());
    let server = serve(vec![ok_json(&answer(r#"{"answer":[[[9]]]}"#, "stop"))]);
    let out = s.path("run");
    let mut args = run_args(&pack, &server.endpoint(), &out);
    args[0] = "plan".into();
    let planned = bin().args(&args).arg("--json").output().unwrap();
    success(&planned);
    assert!(!out.exists());
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);
    assert_eq!(server.requests(), 0);
    let p: serde_json::Value = serde_json::from_slice(&planned.stdout).unwrap();
    assert_eq!(p["provider_cap_enforcement"], "unknown");
    assert_eq!(p["requested_output_tokens"], 128);
    assert_eq!(p["prompt_content_bytes"], "Grid input: é\n[[0]]".len());
    assert_eq!(p["pack_bytes"], fs::metadata(&pack).unwrap().len());
    assert!(p["plan"]["bound_bytes"].as_u64().unwrap() > p["request_bytes"].as_u64().unwrap());
    success(&run(&pack, &server.endpoint(), &out, &[]));
    assert_eq!(server.requests(), 1);
    let request = String::from_utf8(server.request(0)).unwrap();
    let body = request.split_once("\r\n\r\n").unwrap().1;
    assert_eq!(p["request_bytes"], body.len());
    assert_eq!(p["cases"][0]["request_bytes"], body.len());
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        p["messages_json_bytes"],
        serde_json::to_vec(&body["messages"]).unwrap().len()
    );
    assert!(body.get("acceptance").is_none() && body.get("qualification").is_none());
    assert_eq!(body["messages"], wire["cases"][0]["messages"]);
    assert_eq!(
        view(&out.join("grades/initial/view.json")).cases[0].outcome,
        "success"
    );
    let inspection = inspect(&out);
    assert_eq!(inspection.view, "verified");
    let regraded = s.path("regraded");
    fs::remove_file(pack).unwrap();
    success(&cli(&[
        "regrade",
        &out.to_string_lossy(),
        "--out",
        &regraded.to_string_lossy(),
    ]));
    assert_eq!(
        view(&regraded.join("view.json")).cases[0].outcome,
        "success"
    );
    assert_eq!(server.requests(), 1);
}

#[test]
fn inspection_usage_coverage_keeps_absence_unknown_and_zero_present() {
    let s = Scratch::new();
    let responses = [
        Some(serde_json::json!({"prompt_tokens": 0})),
        Some(serde_json::json!({"completion_tokens": 7})),
        None,
        None,
    ]
    .into_iter()
    .map(|usage| {
        let mut response: serde_json::Value =
            serde_json::from_str(&answer(r#"{"answer":"T"}"#, "stop")).unwrap();
        response["usage"] = usage.unwrap_or(serde_json::Value::Null);
        ok_json(&serde_json::to_string(&response).unwrap())
    })
    .collect();
    let server = serve(responses);
    let pack = s.write("pack.json", PACK);
    let out = s.path("run");
    success(&run(&pack, &server.endpoint(), &out, &[]));
    let result = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&result);
    let inspection: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(inspection["usage_status"], "unvalidated-provider-snapshot");
    assert_eq!(
        inspection["usage_coverage"],
        serde_json::json!({
            "snapshots": 2,
            "prompt_tokens": {"present": 1, "unknown": 3},
            "completion_tokens": {"present": 1, "unknown": 3},
            "total_tokens": {"present": 0, "unknown": 4}
        })
    );
    assert_eq!(inspection["attempts"][0]["usage"]["prompt_tokens"], 0);
    assert!(inspection["attempts"][0]["usage"]["completion_tokens"].is_null());
    assert!(inspection["attempts"][2]["usage"].is_null());
    assert_eq!(inspection["attempts"][2]["usage_status"], "unknown");
    assert_eq!(server.requests(), 4);
}

#[test]
fn missing_final_content_retains_completion_facts() {
    for streaming in [false, true] {
        let s = Scratch::new();
        let body = if streaming {
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":128,\"total_tokens\":128}}\n\ndata: [DONE]\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":99999}}\n\n"
        } else {
            r#"{"choices":[{"message":{"role":"assistant","reasoning_content":"thinking"},"finish_reason":"length"}],"usage":{"prompt_tokens":0,"completion_tokens":128,"total_tokens":128}}"#
        };
        let server = serve(vec![entity(
            "200 OK",
            if streaming {
                "text/event-stream"
            } else {
                "application/json"
            },
            body.as_bytes(),
        )]);
        let pack = s.write("pack.json", ONE.as_bytes());
        let out = s.path("run");
        let flags: &[&str] = if streaming { &["--stream"] } else { &[] };
        success(&run(&pack, &server.endpoint(), &out, flags));
        let inspected = cli(&["inspect", &out.to_string_lossy(), "--json"]);
        success(&inspected);
        let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
        let attempt = &report["attempts"][0];
        assert_eq!(
            attempt["outcome"], "unknown",
            "metadata must not manufacture an answer"
        );
        assert_eq!(attempt["stop"], "length");
        assert_eq!(attempt["result"], "output_limit");
        assert_eq!(attempt["usage"]["prompt_tokens"], 0);
        assert_eq!(attempt["usage"]["completion_tokens"], 128);
        assert_eq!(report["result_counts"]["output_limit"], 1);
        let regrade = s.path("regrade");
        success(&cli(&[
            "regrade",
            &out.to_string_lossy(),
            "--out",
            &regrade.to_string_lossy(),
        ]));
        let inspected = cli(&["inspect", &regrade.to_string_lossy(), "--json"]);
        success(&inspected);
        let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
        assert_eq!(report["attempts"][0]["stop"], "length");
        assert_eq!(report["attempts"][0]["usage"]["completion_tokens"], 128);
        assert_eq!(report["attempts"][0]["metadata_source"], "terminal");
        assert_eq!(
            fs::read(out.join("grades/initial/view.json")).unwrap(),
            fs::read(regrade.join("view.json")).unwrap()
        );
        assert_eq!(server.requests(), 1);
    }
}

// Preserve the view's hash lineage so negative tests reach the metadata guards,
// rather than failing merely because a changed terminal digest was left stale.
fn replace_first_terminal(run: &Path, terminal: &serde_json::Value) {
    let bytes = serde_json::to_vec_pretty(terminal).unwrap();
    fs::write(run.join("attempts/000000/terminal.json"), &bytes).unwrap();
    let path = run.join("grades/initial/view.json");
    let mut view: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    view["cases"][0]["terminal"] = digest("terminal-source", &bytes).into();
    fs::write(path, serde_json::to_vec_pretty(&view).unwrap()).unwrap();
}

#[test]
fn terminal_versions_reject_impossible_completion_metadata() {
    let s = Scratch::new();
    let events =
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n";
    let server = serve(vec![entity(
        "200 OK",
        "text/event-stream",
        events.as_bytes(),
    )]);
    let pack = s.write("pack.json", ONE.as_bytes());
    let out = s.path("run");
    success(&run(&pack, &server.endpoint(), &out, &["--stream"]));
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("attempts/000000/terminal.json")).unwrap())
            .unwrap();
    for invalid in [
        "v1_stop",
        "content_filter",
        "incomplete",
        "missing_boundary",
        "future_version",
    ] {
        let mut changed = original.clone();
        match invalid {
            "v1_stop" => changed["version"] = 1.into(),
            "content_filter" => changed["stop"] = "content_filter".into(),
            "incomplete" => changed["body"]["complete"] = false.into(),
            "missing_boundary" => {
                changed["body"]["terminal_offset"] = serde_json::Value::Null;
                changed["body"]["surplus_retained"] = 0.into();
                changed["body"]["surplus_observed"] = 0.into();
            }
            "future_version" => changed["version"] = 3.into(),
            _ => unreachable!(),
        }
        replace_first_terminal(&out, &changed);
        let inspected = cli(&["inspect", &out.to_string_lossy(), "--json"]);
        success(&inspected);
        let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
        assert_eq!(
            report["attempts"][0]["result"], "invalid_evidence",
            "{invalid}"
        );
        assert_eq!(report["view"], "ungradeable", "{invalid}");
        assert!(report["attempts"][0]["stop"].is_null());
        assert!(report["attempts"][0]["usage"].is_null());
    }
    assert_eq!(server.requests(), 1);
}

#[test]
fn legacy_recovery_does_not_promote_other_shapes_or_conflicting_facts() {
    use sha2::{Digest, Sha256};
    let s = Scratch::new();
    let absent = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":7}}\n\ndata: [DONE]\n\n: surplus\n\n";
    let server = serve(vec![entity(
        "200 OK",
        "text/event-stream",
        absent.as_bytes(),
    )]);
    let pack = s.write("pack.json", ONE.as_bytes());
    let out = s.path("run");
    success(&run(&pack, &server.endpoint(), &out, &["--stream"]));
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("attempts/000000/terminal.json")).unwrap())
            .unwrap();
    for guard in [
        "other_delivery",
        "parse_fault",
        "wrong_boundary",
        "conflicting_usage",
    ] {
        let body = match guard {
            "other_delivery" => {
                "data: {\"choices\":[{\"delta\":{\"content\":\"text\"},\"finish_reason\":\"length\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":7}}\n\ndata: [DONE]\n\n: surplus\n\n"
            }
            "parse_fault" => {
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call\"}]},\"finish_reason\":\"length\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":7}}\n\ndata: [DONE]\n\n: surplus\n\n"
            }
            _ => absent,
        };
        // Construct a complete hash-valid archival fixture with explicit surplus,
        // independently of how the TCP stack split the original response chunks.
        let done = body.find("data: [DONE]\n\n").unwrap() + "data: [DONE]\n\n".len();
        let offset = done + usize::from(guard == "wrong_boundary");
        let mut legacy = original.clone();
        legacy["version"] = 1.into();
        legacy["stop"] = serde_json::Value::Null;
        legacy["usage"] = if guard == "conflicting_usage" {
            serde_json::json!({"prompt_tokens":null,"completion_tokens":9,"total_tokens":null})
        } else {
            serde_json::Value::Null
        };
        legacy["body"]["bytes"] = body.len().into();
        legacy["body"]["sha256"] = Sha256::digest(body.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .into();
        legacy["body"]["terminal_offset"] = offset.into();
        legacy["body"]["surplus_retained"] = (body.len() - offset).into();
        legacy["body"]["surplus_observed"] = (body.len() - offset).into();
        legacy["http"]["content_length"] = body.len().into();
        fs::write(out.join("attempts/000000/body.bin"), body.as_bytes()).unwrap();
        replace_first_terminal(&out, &legacy);
        let inspected = cli(&["inspect", &out.to_string_lossy(), "--json"]);
        success(&inspected);
        let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
        assert_eq!(report["view"], "verified", "{guard}");
        assert_eq!(report["attempts"][0]["result"], "other_ungraded", "{guard}");
        assert!(report["attempts"][0]["stop"].is_null(), "{guard}");
        if guard == "conflicting_usage" {
            assert_eq!(report["attempts"][0]["metadata_source"], "terminal");
            assert_eq!(report["attempts"][0]["usage"]["completion_tokens"], 9);
        } else {
            assert_eq!(report["attempts"][0]["metadata_source"], "unavailable");
            assert!(report["attempts"][0]["usage"].is_null());
        }
    }
    assert_eq!(server.requests(), 1);
}

#[test]
fn legacy_metadata_recovery_is_read_only_and_evidence_bound() {
    let s = Scratch::new();
    let events = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"work\"},\"finish_reason\":\"length\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":0}}\n\ndata: [DONE]\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":999}}\n\n";
    let server = serve(vec![entity(
        "200 OK",
        "text/event-stream",
        events.as_bytes(),
    )]);
    let pack = s.write("pack.json", ONE.as_bytes());
    let out = s.path("run");
    success(&run(&pack, &server.endpoint(), &out, &["--stream"]));
    let terminal_path = out.join("attempts/000000/terminal.json");
    let view_path = out.join("grades/initial/view.json");
    let mut terminal: serde_json::Value =
        serde_json::from_slice(&fs::read(&terminal_path).unwrap()).unwrap();
    terminal["version"] = 1.into();
    terminal["stop"] = serde_json::Value::Null;
    terminal["usage"] = serde_json::Value::Null;
    let legacy_terminal = serde_json::to_vec_pretty(&terminal).unwrap();
    fs::write(&terminal_path, &legacy_terminal).unwrap();
    let mut view: serde_json::Value =
        serde_json::from_slice(&fs::read(&view_path).unwrap()).unwrap();
    view["cases"][0]["terminal"] = digest("terminal-source", &legacy_terminal).into();
    let legacy_view = serde_json::to_vec_pretty(&view).unwrap();
    fs::write(&view_path, &legacy_view).unwrap();

    let inspected = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&inspected);
    let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(report["view"], "verified");
    assert_eq!(report["attempts"][0]["outcome"], "unknown");
    assert_eq!(report["attempts"][0]["result"], "output_limit");
    assert_eq!(
        report["attempts"][0]["metadata_source"],
        "verified_raw_body"
    );
    assert_eq!(report["attempts"][0]["usage"]["completion_tokens"], 0);
    assert!(report["attempts"][0]["usage"]["prompt_tokens"].is_null());
    assert_eq!(fs::read(&terminal_path).unwrap(), legacy_terminal);
    assert_eq!(fs::read(&view_path).unwrap(), legacy_view);
    let regrade = s.path("legacy-regrade");
    success(&cli(&[
        "regrade",
        &out.to_string_lossy(),
        "--out",
        &regrade.to_string_lossy(),
    ]));
    assert_eq!(
        fs::read(regrade.join("attempts/000000/terminal.json")).unwrap(),
        legacy_terminal
    );
    let inspected = cli(&["inspect", &regrade.to_string_lossy(), "--json"]);
    success(&inspected);
    let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(report["attempts"][0]["result"], "other_ungraded");
    assert_eq!(report["attempts"][0]["metadata_source"], "unavailable");
    assert!(
        report["attempts"][0]["usage"].is_null(),
        "raw-free v1 cannot invent missing usage"
    );
    let compared = cli(&[
        "compare",
        &out.to_string_lossy(),
        &regrade.to_string_lossy(),
        "--json",
    ]);
    success(&compared);
    let comparison: serde_json::Value = serde_json::from_slice(&compared.stdout).unwrap();
    assert_eq!(
        comparison["delta_b_minus_a"],
        serde_json::json!({"lower":-1,"upper":1,"denominator":1})
    );
    assert_eq!(comparison["left_results"]["output_limit"], 1);
    assert_eq!(comparison["right_results"]["other_ungraded"], 1);

    fs::write(out.join("attempts/000000/body.bin"), b"forged raw usage").unwrap();
    let inspected = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&inspected);
    let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(report["attempts"][0]["result"], "invalid_evidence");
    assert_eq!(report["attempts"][0]["metadata_source"], "unavailable");
    assert!(report["attempts"][0]["usage"].is_null());
    assert!(report["attempts"][0]["stop"].is_null());
    assert_eq!(server.requests(), 1);
}

#[test]
fn output_limits_do_not_replace_gradeable_answers() {
    let s = Scratch::new();
    let server = serve(vec![
        ok_json(&answer(r#"{"answer":"T"}"#, "length")),
        ok_json(&answer(r#"{"answer":"incorrect"}"#, "length")),
        ok_json(&answer("partial answer", "length")),
        ok_json(r#"{"choices":[{"message":{"content":null},"finish_reason":"length"}]}"#),
    ]);
    let pack = s.write("pack.json", PACK);
    let out = s.path("run");
    success(&run(&pack, &server.endpoint(), &out, &[]));
    let inspected = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&inspected);
    let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    let results: Vec<_> = report["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["result"].as_str().unwrap())
        .collect();
    assert_eq!(
        results,
        ["correct", "wrong_answer", "output_limit", "output_limit"]
    );
    assert_eq!(report["coverage"]["success"], 1);
    assert_eq!(report["coverage"]["failure"], 2);
    assert_eq!(report["coverage"]["unknown"], 1);
    assert_eq!(report["attempts"][2]["outcome"], "malformed");
    assert_eq!(report["attempts"][3]["outcome"], "unknown");
}

#[test]
fn execution_errors_are_distinct_from_incorrect_answers() {
    let s = Scratch::new();
    let server = serve(vec![
        entity(
            "503 Service Unavailable",
            "application/json",
            br#"{"error":"busy"}"#,
        ),
        ok_json(r#"{"error":{"message":"busy"}}"#),
        vec![Step::Sleep(Duration::from_millis(300))],
        ok_json(&answer(r#"{"answer":"wrong"}"#, "stop")),
    ]);
    let pack = s.write("pack.json", PACK);
    let out = s.path("run");
    success(&run(
        &pack,
        &server.endpoint(),
        &out,
        &["--idle-ms", "200", "--total-ms", "1000"],
    ));
    let inspected = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&inspected);
    let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(report["result_counts"]["service_error"], 2);
    assert_eq!(report["result_counts"]["timeout"], 1);
    assert_eq!(report["result_counts"]["wrong_answer"], 1);
    assert_eq!(report["coverage"]["unknown"], 3);
    assert_eq!(report["coverage"]["failure"], 1);
    assert_eq!(server.requests(), 4);
}

#[test]
fn inspection_does_not_merge_stream_usage_snapshots() {
    let s = Scratch::new();
    let events = format!(
        "{}data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":12,\"total_tokens\":20}}}}\n\ndata: {{\"choices\":[],\"usage\":{{\"completion_tokens\":0}}}}\n\ndata: [DONE]\n\n",
        delta(r#"{"answer":"T"}"#, Some("stop"))
    );
    let server = serve(vec![vec![sse_head(), chunk(events.as_bytes()), sse_end()]]);
    let pack = s.write("pack.json", ONE.as_bytes());
    let out = s.path("run");
    success(&run(&pack, &server.endpoint(), &out, &["--stream"]));
    let result = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&result);
    let inspection: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        inspection["attempts"][0]["usage"],
        serde_json::json!({
            "prompt_tokens": null, "completion_tokens": 0, "total_tokens": null
        })
    );
    assert_eq!(inspection["usage_coverage"]["prompt_tokens"]["unknown"], 1);
    assert_eq!(
        inspection["usage_coverage"]["completion_tokens"]["present"],
        1
    );
    assert_eq!(inspection["usage_status"], "unvalidated-provider-snapshot");
    assert_eq!(server.requests(), 1);
}

#[test]
fn profile_v2_declares_controls_on_the_wire_and_retains_usage() {
    let s = Scratch::new();
    let events = format!(
        "{}data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":3,\"completion_tokens\":4,\"total_tokens\":7}}}}\n\ndata: [DONE]\n\n",
        delta(r#"{"answer":"T"}"#, Some("stop"))
    );
    let server = serve(vec![vec![sse_head(), chunk(events.as_bytes()), sse_end()]]);
    let pack = s.write("one.json", ONE.as_bytes());
    let out = s.path("run");
    success(&run(
        &pack,
        &server.endpoint(),
        &out,
        &[
            "--stream",
            "--profile",
            "declared-chat-completions-v2",
            "--reasoning-effort",
            "high",
            "--include-usage",
        ],
    ));
    let request = String::from_utf8(server.request(0)).unwrap();
    let body = request.split_once("\r\n\r\n").unwrap().1;
    assert!(
        body.ends_with(
            "\"stream\":true,\"max_completion_tokens\":128,\"reasoning_effort\":\"high\",\"stream_options\":{\"include_usage\":true}}"
        ),
        "{body}"
    );
    let t = terminal(&out, 0);
    assert_eq!(
        (t.reason.as_str(), t.stop.as_deref()),
        ("committed", Some("stop"))
    );
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("attempts/000000/terminal.json")).unwrap())
            .unwrap();
    assert_eq!(raw["usage"]["total_tokens"], 7);
    let plan: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("plan.json")).unwrap()).unwrap();
    assert_eq!(plan["protocol"]["profile"], "declared-chat-completions-v2");
    assert_eq!(plan["protocol"]["reasoning_effort"], "high");
    assert_eq!(plan["protocol"]["include_usage"], true);
    assert_eq!(
        view(&out.join("grades/initial/view.json")).cases[0].outcome,
        "success"
    );
    // Stray controls under the default v1 profile and nonstreaming usage are
    // refused at preflight: no request, no output directory.
    for extra in [
        ["--include-usage", "--stream"],
        ["--reasoning-effort", "low"],
    ] {
        let stray = s.path("stray");
        let output = run(&pack, &server.endpoint(), &stray, &extra);
        assert_eq!(output.status.code(), Some(1), "{extra:?}");
        assert!(!stray.exists());
    }
    let nonstream = run(
        &pack,
        &server.endpoint(),
        &s.path("nonstream"),
        &[
            "--profile",
            "declared-chat-completions-v2",
            "--include-usage",
        ],
    );
    assert_eq!(nonstream.status.code(), Some(1));
    assert!(!s.path("nonstream").exists());
    assert_eq!(server.requests(), 1);
}

#[test]
fn nonstream_attempts_dispatch_exactly_once_and_regrade_offline() {
    let s = Scratch::new();
    let server = serve(vec![
        ok_json(&answer("{\"answer\":\"T\"}", "length")),
        entity(
            "500 Internal Server Error",
            "application/json",
            b"{\"error\":\"x\"}",
        ),
        vec![Step::Send(
            b"HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\n\r\n".to_vec(),
        )],
        entity("200 OK", "text/plain", b"{\"answer\":\"x\"}"),
    ]);
    let pack = s.write("pack.json", PACK);
    let out = s.path("run");
    let output = run(
        &pack,
        &server.endpoint(),
        &out,
        &["--total-ms", "30000", "--idle-ms", "10000"],
    );
    success(&output);
    assert_eq!(server.requests(), 4);
    let first = String::from_utf8(server.request(0)).unwrap();
    assert!(first.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    let lower = first.to_ascii_lowercase();
    assert!(lower.contains("\r\naccept: application/json\r\n"));
    assert!(lower.contains("\r\naccept-encoding: identity\r\n"));
    assert!(!lower.contains("authorization"));
    let body = &first[first.find("\r\n\r\n").unwrap() + 4..];
    assert!(body.starts_with("{\"model\":\"fixture-model\",\"messages\":[{\"role\":\"user\""));
    assert!(body.ends_with("\"stream\":false,\"max_completion_tokens\":128}"));
    assert!(!body.contains("max_tokens") && !body.contains("temperature"));

    let t = terminal(&out, 0);
    assert_eq!(
        (t.reason.as_str(), t.commitment.as_str()),
        ("committed", "committed")
    );
    assert_eq!(t.stop.as_deref(), Some("length"));
    assert!(t.dispatched && t.body.complete && t.final_artifact.is_some());
    assert_eq!(terminal(&out, 1).reason, "http_status");
    let redirected = terminal(&out, 2);
    assert_eq!(redirected.reason, "http_status");
    assert_eq!(redirected.http.unwrap()["status"], 302);
    assert_eq!(terminal(&out, 3).reason, "http_entity");
    for i in 1..4 {
        assert!(
            !out.join("attempts")
                .join(format!("{i:06}"))
                .join("final.txt")
                .exists()
        );
    }
    let initial = out.join("grades/initial/view.json");
    let v = view(&initial);
    assert_eq!(v.cases[0].outcome, "success");
    assert_eq!(v.cases[0].evidence, "collected");
    assert!(
        v.cases[1..]
            .iter()
            .all(|c| c.outcome == "unknown" && c.evidence == "uncommitted")
    );

    // Offline regrade of a relocated run with the original pack gone and no
    // credential: no client exists, so the fixture cannot observe a request.
    fs::remove_file(&pack).unwrap();
    let moved = s.path("moved-run");
    fs::rename(&out, &moved).unwrap();
    let regrade = s.path("regrade");
    success(&cli(&[
        "regrade",
        &moved.to_string_lossy(),
        "--out",
        &regrade.to_string_lossy(),
    ]));
    assert_eq!(
        fs::read(regrade.join("view.json")).unwrap(),
        fs::read(moved.join("grades/initial/view.json")).unwrap()
    );
    assert!(regrade.join("attempts/000000/final.txt").exists());
    assert!(!regrade.join("attempts/000000/body.bin").exists());
    let relocated_view = s.path("relocated-view");
    fs::rename(&regrade, &relocated_view).unwrap();
    let inspection = inspect(&relocated_view);
    assert_eq!(
        (inspection.counts.collected, inspection.view.as_str()),
        (1, "verified")
    );
    let output = cli(&[
        "compare",
        &moved.to_string_lossy(),
        &relocated_view.to_string_lossy(),
        "--json",
    ]);
    success(&output);
    let c: Compared = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (c.n, c.delta_b_minus_a.lower, c.delta_b_minus_a.upper),
        (4, -3, 3)
    );
    // A submitted view under the same declared protocol pairs as a system contrast.
    let pack_again = s.write("pack-again.json", PACK);
    let b = s.write("b.json", B);
    let bv = s.path("bv");
    success(&cli(&[
        "grade",
        &pack_again.to_string_lossy(),
        &b.to_string_lossy(),
        "--out",
        &bv.to_string_lossy(),
    ]));
    let output = cli(&[
        "compare",
        &moved.to_string_lossy(),
        &bv.to_string_lossy(),
        "--json",
    ]);
    success(&output);
    let c: Compared = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        c.left_evidence.starts_with("client-collected")
            && c.right_evidence.starts_with("submitted")
    );
    assert_eq!((c.delta_b_minus_a.lower, c.delta_b_minus_a.upper), (-1, 2));
    let expected_comparison: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let plan: serde_json::Value =
        serde_json::from_slice(&fs::read(moved.join("plan.json")).unwrap()).unwrap();
    let mut study: serde_json::Value =
        serde_json::from_slice(include_bytes!("../examples/synthetic-study.json")).unwrap();
    study["left_system"] = plan["system"].clone();
    study["protocol"] = plan["protocol"].clone();
    let manifest = s.write("study.json", &serde_json::to_vec(&study).unwrap());
    // A raw run and its relocated raw-body-free regrade must produce the same
    // study analysis, without losing unknown cases or contacting the endpoint.
    let mut reports = Vec::new();
    for left in [&moved, &relocated_view] {
        let result = cli(&[
            "study",
            "compare",
            &manifest.to_string_lossy(),
            &pack_again.to_string_lossy(),
            &left.to_string_lossy(),
            &bv.to_string_lossy(),
            "--json",
        ]);
        success(&result);
        let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["comparison"], expected_comparison);
        assert_eq!(report["families"][1]["summary"]["left"]["unknown"], 3);
        reports.push(report);
    }
    assert_eq!(reports[0], reports[1]);
    assert_eq!(server.requests(), 4);
}

fn one_case(s: &Scratch, server: &Server, name: &str, extra: &[&str]) -> (PathBuf, Output) {
    let pack = s.path("one.json");
    if !pack.exists() {
        fs::write(&pack, ONE).unwrap();
    }
    let out = s.path(name);
    let output = run(&pack, &server.endpoint(), &out, extra);
    (out, output)
}

#[test]
fn streaming_commitment_boundaries() {
    let s = Scratch::new();
    let done_then_surplus = b"data: [DONE]\r\n\r\nid: late\ndata: {\"choices\":[]}\n\n";
    let server = serve(vec![
        // BOM, CRLF, comment keepalive, split frames, usage after finish, surplus after DONE.
        vec![
            sse_head(),
            chunk(b"\xEF\xBB\xBF: keepalive\r\n"),
            chunk(delta("", None).replace('\n', "\r\n").as_bytes()),
            chunk(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"{\\\"answer\\\":"),
            chunk(b"\"},\"finish_reason\":null}]}\n\n"),
            chunk(delta("\"T\"}", Some("stop")).as_bytes()),
            chunk(b"data: {\"choices\":[],\"usage\":{\"completion_tokens\":3}}\n\n"),
            chunk(done_then_surplus),
            sse_end(),
        ],
        // EOF before DONE.
        vec![
            sse_head(),
            chunk(delta("{\"answer\":\"T\"}", Some("stop")).as_bytes()),
            sse_end(),
        ],
        // Content after finish.
        vec![
            sse_head(),
            chunk(delta("{\"answer\":\"T\"}", Some("stop")).as_bytes()),
            chunk(delta("x", None).as_bytes()),
            chunk(b"data: [DONE]\n\n"),
            sse_end(),
        ],
        // Completed content filter.
        vec![
            sse_head(),
            chunk(delta("{\"an", Some("content_filter")).as_bytes()),
            chunk(b"data: [DONE]\n\n"),
            sse_end(),
        ],
        // Idle deadline: headers, one comment, then silence.
        vec![
            sse_head(),
            chunk(b": hello\n"),
            Step::Sleep(Duration::from_millis(1500)),
            sse_end(),
        ],
        // Total deadline: comments keep idle alive but never finish.
        vec![
            sse_head(),
            chunk(b": 1\n"),
            Step::Sleep(Duration::from_millis(200)),
            chunk(b": 2\n"),
            Step::Sleep(Duration::from_millis(200)),
            chunk(b": 3\n"),
            Step::Sleep(Duration::from_millis(200)),
            chunk(b": 4\n"),
            Step::Sleep(Duration::from_millis(200)),
            chunk(b": 5\n"),
            Step::Sleep(Duration::from_millis(200)),
            sse_end(),
        ],
        // Raw cap overflow.
        vec![sse_head(), chunk(&[b'x'; 300]), sse_end()],
        // Artifact cap overflow within a valid stream.
        vec![
            sse_head(),
            chunk(delta(&"y".repeat(40), None).as_bytes()),
            sse_end(),
        ],
        // Bearer credential dispatch.
        vec![
            sse_head(),
            chunk(delta("{\"answer\":\"T\"}", Some("stop")).as_bytes()),
            chunk(b"data: [DONE]\n\n"),
            sse_end(),
        ],
    ]);

    let (out, output) = one_case(&s, &server, "ok", &["--stream"]);
    success(&output);
    let t = terminal(&out, 0);
    assert_eq!(t.reason, "committed");
    assert_eq!(t.stop.as_deref(), Some("stop"));
    let body = fs::read(out.join("attempts/000000/body.bin")).unwrap();
    assert_eq!(body.len() as u64, t.body.bytes);
    assert!(body.starts_with(b"\xEF\xBB\xBF: keepalive"));
    let offset = t.body.terminal_offset.unwrap();
    assert!(body[..offset as usize].ends_with(b"data: [DONE]\r\n\r\n"));
    let surplus = (done_then_surplus.len() - b"data: [DONE]\r\n\r\n".len()) as u64;
    assert!(t.body.surplus_observed <= surplus);
    assert!(t.body.surplus_retained <= t.body.surplus_observed);
    assert_eq!(offset + t.body.surplus_retained, t.body.bytes);
    assert_eq!(
        fs::read(out.join("attempts/000000/final.txt")).unwrap(),
        b"{\"answer\":\"T\"}"
    );
    assert_eq!(
        view(&out.join("grades/initial/view.json")).cases[0].outcome,
        "success"
    );
    let stream_request = String::from_utf8(server.request(0))
        .unwrap()
        .to_ascii_lowercase();
    assert!(stream_request.contains("\r\naccept: text/event-stream\r\n"));
    assert!(stream_request.ends_with("\"stream\":true,\"max_completion_tokens\":128}"));

    let (out, output) = one_case(&s, &server, "eof", &["--stream"]);
    success(&output);
    let t = terminal(&out, 0);
    assert_eq!(
        (t.reason.as_str(), t.commitment.as_str()),
        ("stream_incomplete", "uncommitted")
    );
    assert!(t.body.bytes > 0 && t.final_artifact.is_none());
    assert!(!out.join("attempts/000000/final.txt").exists());
    let v = view(&out.join("grades/initial/view.json"));
    assert_eq!(
        (v.cases[0].outcome.as_str(), v.cases[0].evidence.as_str()),
        ("unknown", "uncommitted")
    );
    assert_eq!(v.cases[0].reason.as_deref(), Some("stream_incomplete"));

    let (out, output) = one_case(&s, &server, "after-finish", &["--stream"]);
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "malformed_envelope");

    let (out, output) = one_case(&s, &server, "filtered", &["--stream"]);
    success(&output);
    let t = terminal(&out, 0);
    assert_eq!(
        (t.reason.as_str(), t.commitment.as_str()),
        ("refused", "committed")
    );
    assert!(t.final_artifact.is_none());
    let v = view(&out.join("grades/initial/view.json"));
    assert_eq!(
        (v.cases[0].outcome.as_str(), v.cases[0].evidence.as_str()),
        ("refused", "refused")
    );

    let (out, output) = one_case(
        &s,
        &server,
        "idle",
        &["--stream", "--total-ms", "5000", "--idle-ms", "400"],
    );
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "idle_deadline");

    let (out, output) = one_case(
        &s,
        &server,
        "total",
        &["--stream", "--total-ms", "600", "--idle-ms", "500"],
    );
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "total_deadline");

    let (out, output) = one_case(&s, &server, "cap", &["--stream", "--response-bytes", "200"]);
    success(&output);
    let t = terminal(&out, 0);
    assert_eq!(t.reason, "response_cap");
    assert!(t.body.truncated && t.body.bytes == 200);
    assert_eq!(
        fs::read(out.join("attempts/000000/body.bin"))
            .unwrap()
            .len(),
        200
    );

    let (out, output) = one_case(
        &s,
        &server,
        "artifact",
        &["--stream", "--artifact-bytes", "32"],
    );
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "artifact_cap");

    let out = s.path("auth");
    let output = bin()
        .args(run_args(&s.path("one.json"), &server.endpoint(), &out))
        .args(["--stream", "--auth-env", "GRILL_TEST_TOKEN"])
        .env("GRILL_TEST_TOKEN", "secret-value-1")
        .output()
        .unwrap();
    success(&output);
    let request = String::from_utf8(server.request(8)).unwrap();
    assert!(
        request
            .to_ascii_lowercase()
            .contains("\r\nauthorization: bearer secret-value-1\r\n")
    );
    for name in [
        "plan.json",
        "attempts/000000/reservation.json",
        "attempts/000000/terminal.json",
    ] {
        let text = fs::read_to_string(out.join(name)).unwrap();
        assert!(!text.contains("secret-value-1"), "{name}");
    }
    assert!(
        fs::read_to_string(out.join("plan.json"))
            .unwrap()
            .contains("\"auth_env\": \"GRILL_TEST_TOKEN\"")
    );
    assert_eq!(server.requests(), 9);
}

fn sse_entity(body: &[u8]) -> Vec<Step> {
    let mut bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    vec![Step::Send(bytes)]
}

#[test]
fn in_cap_done_commits_regardless_of_surplus_and_semantics_stay_strict() {
    let s = Scratch::new();
    let committed = format!(
        "{}data: [DONE]\n\n",
        delta("{\"answer\":\"T\"}", Some("stop"))
    );
    let mut whole = committed.clone().into_bytes();
    whole.extend(vec![b'\xff'; 4000]);
    whole.extend_from_slice(
        b"data: {\"choices\":[{\"delta\":{\"content\":\"late\"},\"finish_reason\":null}]}\n\n",
    );
    whole.extend(vec![b'\xfe'; 4000]);
    let server = serve(vec![
        // Content-Length far beyond the cap; DONE within it; invalid UTF-8 after it.
        sse_entity(&whole),
        // The same prefix and surplus co-read in one chunked frame.
        vec![sse_head(), chunk(&whole), sse_end()],
        // Response identity changes between contributing chunks.
        vec![
            sse_head(),
            chunk(b"data: {\"id\":\"one\",\"choices\":[{\"delta\":{\"content\":\"{\\\"answer\\\":\"},\"finish_reason\":null}]}\n\n"),
            chunk(b"data: {\"id\":\"two\",\"choices\":[{\"delta\":{\"content\":\"\\\"T\\\"}\"},\"finish_reason\":\"stop\"}]}\n\n"),
            chunk(b"data: [DONE]\n\n"),
            sse_end(),
        ],
        // Non-null top-level error beside a valid-looking choice.
        ok_json(r#"{"error":{"message":"quota"},"choices":[{"index":0,"message":{"role":"assistant","content":"{\"answer\":\"T\"}"},"finish_reason":"stop"}]}"#),
        // User role presented as the final answer.
        ok_json(r#"{"choices":[{"index":0,"message":{"role":"user","content":"{\"answer\":\"T\"}"},"finish_reason":"stop"}]}"#),
        // Invalid UTF-8 inside ignored metadata.
        {
            let mut body = br#"{"choices":[{"index":0,"message":{"role":"assistant","content":"{\"answer\":\"T\"}"},"finish_reason":"stop"}],"meta":"#.to_vec();
            body.extend_from_slice(b"\"\xff\"}");
            entity("200 OK", "application/json", &body)
        },
        // Duplicate content-type and non-identity content-encoding member.
        {
            let body = answer("{\"answer\":\"T\"}", "stop");
            let mut bytes = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            bytes.extend_from_slice(body.as_bytes());
            vec![Step::Send(bytes)]
        },
        {
            let body = answer("{\"answer\":\"T\"}", "stop");
            let mut bytes = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: identity\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            bytes.extend_from_slice(body.as_bytes());
            vec![Step::Send(bytes)]
        },
        // Headers after 300 ms, first body after another 300 ms, idle 400 ms.
        vec![
            Step::Sleep(Duration::from_millis(300)),
            sse_head(),
            Step::Sleep(Duration::from_millis(300)),
            chunk(delta("{\"answer\":\"T\"}", Some("stop")).as_bytes()),
            chunk(b"data: [DONE]\n\n"),
            sse_end(),
        ],
        // Valid finish and DONE but only a reasoning channel: unsupported, yet the
        // DONE offset is honest evidence and the run stays inspectable/regradable.
        vec![
            sse_head(),
            chunk(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"thinking\"},\"finish_reason\":\"stop\"}]}\n\n"),
            chunk(b"data: [DONE]\n\n"),
            sse_end(),
        ],
    ]);

    for name in ["length-surplus", "chunked-surplus"] {
        let (out, output) = one_case(&s, &server, name, &["--stream", "--response-bytes", "400"]);
        success(&output);
        let t = terminal(&out, 0);
        assert_eq!(
            (t.reason.as_str(), t.commitment.as_str()),
            ("committed", "committed"),
            "{name}"
        );
        assert_eq!(t.body.terminal_offset, Some(committed.len() as u64));
        assert!(t.body.bytes <= 400 && t.body.truncated == (t.body.bytes == 400));
        assert_eq!(
            t.body.surplus_retained,
            t.body.bytes - committed.len() as u64
        );
        assert!(t.body.surplus_observed >= t.body.surplus_retained);
        assert_eq!(
            view(&out.join("grades/initial/view.json")).cases[0].outcome,
            "success"
        );
        assert_eq!(
            fs::read(out.join("attempts/000000/body.bin"))
                .unwrap()
                .len() as u64,
            t.body.bytes
        );
    }
    let (out, output) = one_case(&s, &server, "id-change", &["--stream"]);
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "malformed_envelope");
    let (out, output) = one_case(&s, &server, "provider-error", &[]);
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "provider_error");
    let (out, output) = one_case(&s, &server, "user-role", &[]);
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "unsupported_response");
    let (out, output) = one_case(&s, &server, "bad-utf8", &[]);
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "malformed_envelope");
    for name in ["dup-type", "dup-encoding"] {
        let (out, output) = one_case(&s, &server, name, &[]);
        success(&output);
        assert_eq!(terminal(&out, 0).reason, "http_entity", "{name}");
    }
    let (out, output) = one_case(
        &s,
        &server,
        "late-body",
        &["--stream", "--idle-ms", "400", "--total-ms", "5000"],
    );
    success(&output);
    assert_eq!(terminal(&out, 0).reason, "idle_deadline");
    for name in [
        "id-change",
        "provider-error",
        "user-role",
        "bad-utf8",
        "dup-type",
        "dup-encoding",
        "late-body",
    ] {
        assert!(
            !s.path(name).join("attempts/000000/final.txt").exists(),
            "{name}"
        );
    }
    let (out, output) = one_case(&s, &server, "reasoning-only", &["--stream"]);
    success(&output);
    let t = terminal(&out, 0);
    assert_eq!(
        (t.reason.as_str(), t.commitment.as_str()),
        ("unsupported_response", "uncommitted")
    );
    assert!(t.body.complete && t.body.terminal_offset.is_some() && t.final_artifact.is_none());
    let v = view(&out.join("grades/initial/view.json"));
    assert_eq!(
        (v.cases[0].evidence.as_str(), v.cases[0].outcome.as_str()),
        ("uncommitted", "unknown")
    );
    let regrade = s.path("reasoning-only-regrade");
    success(&cli(&[
        "regrade",
        &out.to_string_lossy(),
        "--out",
        &regrade.to_string_lossy(),
    ]));
    assert_eq!(server.requests(), 10);
}

fn spawn_run(pack: &Path, endpoint: &str, out: &Path) -> Child {
    bin()
        .args(run_args(pack, endpoint, out))
        .args(["--stream", "--total-ms", "60000", "--idle-ms", "60000"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn interrupt_and_crash_windows_keep_partial_evidence() {
    let s = Scratch::new();
    let held = || {
        vec![
            sse_head(),
            chunk(delta("{\"ans", None).as_bytes()),
            Step::Sleep(Duration::from_secs(30)),
            sse_end(),
        ]
    };
    let server = serve(vec![held(), held()]);
    let pack = s.write("pack.json", PACK);

    // First Ctrl-C: interrupted terminal, retained partial body, no further admission.
    let out = s.path("interrupted");
    let mut child = spawn_run(&pack, &server.endpoint(), &out);
    server.wait_requests(1);
    thread::sleep(Duration::from_millis(300));
    success(
        &Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .output()
            .unwrap(),
    );
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(1));
    let t = terminal(&out, 0);
    assert_eq!(
        (t.reason.as_str(), t.commitment.as_str()),
        ("interrupted", "uncommitted")
    );
    assert!(t.body.bytes > 0 && !t.body.complete);
    let body = fs::read(out.join("attempts/000000/body.bin")).unwrap();
    assert!(body.starts_with(b"data: {\"choices\""));
    assert!(!out.join("attempts/000001").exists());
    let v = view(&out.join("grades/initial/view.json"));
    assert_eq!(v.cases.len(), 4);
    assert_eq!(v.cases[0].evidence, "uncommitted");
    assert!(
        v.cases[1..]
            .iter()
            .all(|c| c.evidence == "not_started" && c.outcome == "unknown")
    );

    // Forced termination: reservation and orphan partial body, no terminal.
    let out = s.path("crashed");
    let mut child = spawn_run(&pack, &server.endpoint(), &out);
    server.wait_requests(2);
    thread::sleep(Duration::from_millis(300));
    child.kill().unwrap();
    child.wait().unwrap();
    let attempt = out.join("attempts/000000");
    assert!(attempt.join("reservation.json").exists());
    assert!(attempt.join("body.partial").exists());
    assert!(!attempt.join("terminal.json").exists());
    assert!(!out.join("grades").exists());
    let connections = server.connections.load(Ordering::SeqCst);
    let inspection = inspect(&out);
    assert_eq!(
        (
            inspection.counts.unresolved,
            inspection.counts.not_started,
            inspection.view.as_str()
        ),
        (1, 3, "absent")
    );
    let regrade = s.path("crash-regrade");
    success(&cli(&[
        "regrade",
        &out.to_string_lossy(),
        "--out",
        &regrade.to_string_lossy(),
    ]));
    let v = view(&regrade.join("view.json"));
    assert_eq!(
        (v.cases[0].evidence.as_str(), v.cases[0].outcome.as_str()),
        ("unresolved", "unknown")
    );
    assert!(!regrade.join("attempts/000000/terminal.json").exists());
    assert!(regrade.join("attempts/000000/reservation.json").exists());
    assert_eq!(server.connections.load(Ordering::SeqCst), connections);
    assert_eq!(server.requests(), 2);
}

#[test]
fn quality_last_attempt_sigint_preserves_interrupted_lifecycle_and_zero_send_failure() {
    let s = Scratch::new();
    let (_release, hold) = std::sync::mpsc::channel();
    let server = serve(vec![vec![sse_head(), Step::Wait(hold)]]);
    let pack = s.write("one.json", ONE.as_bytes());
    let out = s.path("last-interrupted");
    let mut child = spawn_run(&pack, &server.endpoint(), &out);
    server.wait_requests(1);
    success(
        &Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .output()
            .unwrap(),
    );
    assert_eq!(child.wait().unwrap().code(), Some(1));
    let terminal = terminal(&out, 0);
    assert_eq!(terminal.reason, "interrupted");
    assert_eq!(terminal.commitment, "uncommitted");
    let status = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&status);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["lifecycle"]["state"], "interrupted");
    assert_eq!(status["lifecycle"]["next_attempt"], 1);
    assert_eq!(
        view(&out.join("grades/initial/view.json")).cases[0].outcome,
        "unknown"
    );
    let before = fs::read(out.join("lifecycle/000000/end.json")).unwrap();
    assert!(!cli(&["resume", &out.to_string_lossy()]).status.success());
    assert_eq!(server.requests(), 1);
    assert!(!out.join("lifecycle/000001").exists());
    assert_eq!(
        fs::read(out.join("lifecycle/000000/end.json")).unwrap(),
        before
    );
}

#[test]
fn tampered_or_unsafe_evidence_never_becomes_an_answer() {
    use std::os::unix::fs::symlink;
    let s = Scratch::new();
    let server = serve(vec![ok_json(&answer("{\"answer\":\"T\"}", "stop"))]);
    let (out, output) = one_case(&s, &server, "run", &[]);
    success(&output);
    let attempt = out.join("attempts/000000");
    let regrade = |name: &str| {
        cli(&[
            "regrade",
            &out.to_string_lossy(),
            "--out",
            &s.path(name).to_string_lossy(),
        ])
    };
    success(&regrade("baseline"));

    let final_path = attempt.join("final.txt");
    let original = fs::read(&final_path).unwrap();
    fs::write(&final_path, b"{\"answer\":\"teal\"}").unwrap();
    assert!(!regrade("edited-final").status.success());
    assert_eq!(inspect(&out).counts.invalid, 1);
    fs::write(&final_path, &original).unwrap();
    success(&regrade("restored"));

    let terminal_path = attempt.join("terminal.json");
    let receipt = fs::read_to_string(&terminal_path).unwrap();
    fs::write(
        &terminal_path,
        receipt.replacen("\"committed\"", "\"refused\"", 1),
    )
    .unwrap();
    assert!(!regrade("edited-terminal").status.success());
    fs::write(&terminal_path, &receipt).unwrap();
    let forged = out.join("grades/initial/view.json");
    let saved = fs::read_to_string(&forged).unwrap();
    fs::write(&forged, saved.replacen("\"success\"", "\"wrong\"", 1)).unwrap();
    assert_eq!(inspect(&out).view, "mismatch");
    assert!(
        !cli(&[
            "compare",
            &out.to_string_lossy(),
            &s.path("baseline").to_string_lossy()
        ])
        .status
        .success()
    );
    fs::write(&forged, &saved).unwrap();

    fs::remove_file(&final_path).unwrap();
    symlink(s.path("baseline/attempts/000000/final.txt"), &final_path).unwrap();
    assert!(!regrade("symlinked-final").status.success());
    fs::remove_file(&final_path).unwrap();
    fs::write(&final_path, &original).unwrap();

    let body = attempt.join("body.bin");
    let raw = fs::read(&body).unwrap();
    fs::remove_file(&body).unwrap();
    assert!(!regrade("missing-body").status.success());
    fs::write(&body, &raw).unwrap();
    success(&regrade("intact"));

    // Contradictory receipts with intact hash lineage: a rewritten request
    // projection and a physically impossible committed terminal.
    let reservation_path = attempt.join("reservation.json");
    let reservation_text = fs::read_to_string(&reservation_path).unwrap();
    let rewritten = {
        let mut value: serde_json::Value = serde_json::from_str(&reservation_text).unwrap();
        let body = value["body"]
            .as_str()
            .unwrap()
            .replacen("fixture-model", "other-model", 1);
        value["request"] = serde_json::Value::String(digest("request", body.as_bytes()));
        value["body"] = serde_json::Value::String(body);
        serde_json::to_vec_pretty(&value).unwrap()
    };
    fs::write(&reservation_path, &rewritten).unwrap();
    let relinked = {
        let mut value: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        let reservation_digest = digest("reservation-source", &rewritten);
        let request_digest =
            serde_json::from_slice::<serde_json::Value>(&rewritten).unwrap()["request"].clone();
        value["reservation"] = serde_json::Value::String(reservation_digest);
        value["request"] = request_digest;
        serde_json::to_vec_pretty(&value).unwrap()
    };
    fs::write(&terminal_path, &relinked).unwrap();
    assert!(!regrade("rewritten-request").status.success());
    fs::write(&reservation_path, &reservation_text).unwrap();
    fs::write(
        &terminal_path,
        receipt.replacen("\"dispatched\": true", "\"dispatched\": false", 1),
    )
    .unwrap();
    assert!(!regrade("undispatched-success").status.success());
    // Stored HTTP facts must fit the HTTP/1 profile and the retained body.
    for (name, field, value) in [
        (
            "wrong-media-type",
            "content_type",
            serde_json::json!("text/plain"),
        ),
        ("http2-version", "version", serde_json::json!("HTTP/2.0")),
        (
            "impossible-length",
            "content_length",
            serde_json::json!(999_999_999),
        ),
    ] {
        let mut mutated: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        mutated["http"][field] = value;
        fs::write(&terminal_path, serde_json::to_vec_pretty(&mutated).unwrap()).unwrap();
        assert!(!regrade(name).status.success(), "{name}");
    }
    fs::write(&terminal_path, &receipt).unwrap();
    success(&regrade("consistent-again"));

    // Fixed directory components must be real directories; surviving evidence
    // without its reservation is corruption, not a fresh start.
    let aside = s.path("aside-attempt");
    fs::rename(&attempt, &aside).unwrap();
    symlink(&aside, &attempt).unwrap();
    assert!(!regrade("symlinked-attempt").status.success());
    assert_eq!(inspect(&out).counts.invalid, 1);
    fs::remove_file(&attempt).unwrap();
    fs::rename(&aside, &attempt).unwrap();
    fs::remove_file(&reservation_path).unwrap();
    assert!(!regrade("orphan-terminal").status.success());
    assert_eq!(inspect(&out).counts.invalid, 1);
    fs::write(&reservation_path, &reservation_text).unwrap();
    let container = out.join("attempts");
    fs::rename(&container, s.path("aside-attempts")).unwrap();
    assert!(!regrade("no-container").status.success());
    assert!(!cli(&["inspect", &out.to_string_lossy()]).status.success());
    fs::rename(s.path("aside-attempts"), &container).unwrap();
    success(&regrade("structure-restored"));

    // Existing destinations are refused without touching owner files.
    let sentinel = s.path("intact/owner-file");
    fs::write(&sentinel, b"preserve").unwrap();
    assert!(!regrade("intact").status.success());
    assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");
    assert!(
        !run(&s.path("one.json"), &server.endpoint(), &out, &[])
            .status
            .success()
    );
    assert_eq!(server.requests(), 1);
}

#[test]
fn quality_pause_drains_resume_is_exclusive_and_receipts_stay_historical() {
    let s = Scratch::new();
    let response = answer("{\"answer\":\"T\"}", "stop");
    let (release_first, first) = std::sync::mpsc::channel();
    let (release_second, second) = std::sync::mpsc::channel();
    let delayed = |release| {
        let mut steps = vec![Step::Wait(release)];
        steps.extend(ok_json(&response));
        steps
    };
    let server = serve(vec![
        delayed(first),
        delayed(second),
        ok_json(&response),
        ok_json(&response),
    ]);
    let pack = s.write("pack.json", PACK);
    let out = s.path("paused");
    let child = bin()
        .args(run_args(&pack, &server.endpoint(), &out))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    server.wait_requests(1);
    success(&cli(&["pause", &out.to_string_lossy()]));
    let status = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&status);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["lifecycle"]["state"], "pause_requested");
    assert!(!out.join("attempts/000000/terminal.json").exists());
    release_first.send(()).unwrap();
    success(&child.wait_with_output().unwrap());
    assert_eq!(server.requests(), 1);
    assert_eq!(terminal(&out, 0).reason, "committed");
    assert!(!out.join("attempts/000001").exists());
    let receipt = out.join("grades/initial/view.json");
    let original = fs::read(&receipt).unwrap();
    let terminal_bytes = fs::read(out.join("attempts/000000/terminal.json")).unwrap();
    let resumed = bin()
        .args(["resume", &out.to_string_lossy()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    server.wait_requests(2);
    assert!(!cli(&["resume", &out.to_string_lossy()]).status.success());
    release_second.send(()).unwrap();
    success(&resumed.wait_with_output().unwrap());
    assert_eq!(server.requests(), 4);
    let expected: serde_json::Value = serde_json::from_slice(PACK).unwrap();
    for i in 0..4 {
        let request = server.request(i);
        let boundary = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        let body: serde_json::Value = serde_json::from_slice(&request[boundary..]).unwrap();
        assert_eq!(body["messages"], expected["cases"][i]["messages"]);
    }
    assert_eq!(fs::read(&receipt).unwrap(), original);
    assert_eq!(
        fs::read(out.join("attempts/000000/terminal.json")).unwrap(),
        terminal_bytes
    );
    assert_eq!(inspect(&out).view, "historical");
    let status = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["lifecycle"]["state"], "completed");
    success(&cli(&["resume", &out.to_string_lossy()]));
    assert_eq!(server.requests(), 4);
    success(&cli(&[
        "regrade",
        &out.to_string_lossy(),
        "--out",
        &s.path("latest").to_string_lossy(),
    ]));
    assert_eq!(inspect(&s.path("latest")).view, "verified");

    // A historical receipt is evidence, not a replaceable cache.
    fs::write(&receipt, b"{}").unwrap();
    assert!(!cli(&["resume", &out.to_string_lossy()]).status.success());
    fs::write(&receipt, original).unwrap();
    // A stale initial grade and damaged lifecycle history retain separate diagnostics.
    let history = out.join("lifecycle/000001/end.json");
    let saved_history = fs::read(&history).unwrap();
    fs::write(&history, b"{}").unwrap();
    let status = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&status);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["view"], "mismatch");
    assert_eq!(status["lifecycle"]["state"], "blocked");
    assert!(status["lifecycle"]["detail"].is_string());
    assert!(!cli(&["resume", &out.to_string_lossy()]).status.success());
    fs::write(&history, saved_history).unwrap();
    let body = out.join("attempts/000000/body.bin");
    let saved = fs::read(&body).unwrap();
    fs::write(&body, b"damaged").unwrap();
    assert!(!cli(&["resume", &out.to_string_lossy()]).status.success());
    fs::write(&body, saved).unwrap();
    let plan = out.join("plan.json");
    let original = fs::read(&plan).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["system"]["model"] = "changed-model".into();
    fs::write(&plan, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(!cli(&["resume", &out.to_string_lossy()]).status.success());
    assert_eq!(server.requests(), 4);
}

#[test]
fn quality_crash_is_not_pause_and_legacy_evidence_remains_inspectable() {
    let s = Scratch::new();
    let server = serve(vec![vec![Step::Sleep(Duration::from_secs(30))]]);
    let pack = s.write("pack.json", PACK);
    let out = s.path("crash");
    let mut child = bin()
        .args(run_args(&pack, &server.endpoint(), &out))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    server.wait_requests(1);
    child.kill().unwrap();
    child.wait().unwrap();
    let reservation = fs::read(out.join("attempts/000000/reservation.json")).unwrap();
    assert!(!cli(&["resume", &out.to_string_lossy()]).status.success());
    assert_eq!(server.requests(), 1);
    assert_eq!(
        fs::read(out.join("attempts/000000/reservation.json")).unwrap(),
        reservation
    );
    let status = cli(&["inspect", &out.to_string_lossy(), "--json"]);
    success(&status);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["lifecycle"]["state"], "uncertain");
    assert_eq!(status["counts"]["unresolved"], 1);
    // Removing only the new sidecar simulates the archival layout, without changing attempts.
    fs::remove_dir_all(out.join("lifecycle")).unwrap();
    assert_eq!(inspect(&out).counts.unresolved, 1);
    assert!(!cli(&["resume", &out.to_string_lossy()]).status.success());
    assert!(!cli(&["pause", &out.to_string_lossy()]).status.success());
    assert!(!out.join("lifecycle").exists());
}
