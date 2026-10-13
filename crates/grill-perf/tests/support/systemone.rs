use super::*;

type Handler = dyn Fn(&Value) -> (u16, String) + Send + Sync;

/// A loopback System One fixture. Every connection is answered from a caller
/// supplied handler; the requests it received are retained for assertions.
struct DecisionServer {
    base: String,
    endpoint: String,
    seen: Arc<std::sync::Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl DecisionServer {
    fn new(handler: impl Fn(&Value) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let endpoint = format!("{base}/v1/systemone");
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let (flag, requests) = (stop.clone(), seen.clone());
        let join = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                };
                let handler = handler.clone();
                let requests = requests.clone();
                thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    // A foreign local client can reach this ephemeral port; ignore it.
                    let Some(request) = read_request(&mut stream) else {
                        return;
                    };
                    requests.lock().unwrap().push(request.clone());
                    let (status, body) = handler(&request);
                    let reason = if status == 200 { "OK" } else { "ERROR" };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body.as_bytes());
                    let _ = stream.flush();
                });
            }
        });
        Self {
            base,
            endpoint,
            seen,
            stop,
            join: Some(join),
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for DecisionServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn answers_for(request: &Value) -> Value {
    let mut answers = serde_json::Map::new();
    for (id, question) in request["questions"].as_object().unwrap() {
        answers.insert(id.clone(), answer_for(question));
    }
    json!({"answers": answers, "usage": {"input_tokens": 17, "output_tokens": 0}})
}

fn answer_for(question: &Value) -> Value {
    match question["type"].as_str().unwrap() {
        "noul" => json!({"type": "noul", "noul": 0.5}),
        "choice" => {
            let names: Vec<String> = question["criteria"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            let picked = names[0].clone();
            let mut probabilities = serde_json::Map::new();
            for name in &names {
                let value = if name == &picked {
                    0.6
                } else {
                    0.4 / (names.len() - 1) as f64
                };
                probabilities.insert(name.clone(), json!(value));
            }
            json!({
                "type": "choice",
                "choice": picked,
                "confidence": 0.6,
                "probabilities": probabilities,
            })
        }
        "score" => {
            let levels = question["criteria"].as_array().unwrap().len();
            let mut probabilities = serde_json::Map::new();
            for index in 0..levels {
                probabilities.insert(index.to_string(), json!(1.0 / levels as f64));
            }
            json!({
                "type": "score",
                "score": (levels as f64 - 1.0) / 2.0,
                "confidence": 1.0 / levels as f64,
                "probabilities": probabilities,
            })
        }
        other => panic!("fixture received an unknown question type {other}"),
    }
}

fn immediate(request: &Value) -> (u16, String) {
    (200, answers_for(request).to_string())
}

fn plan_json() -> Value {
    json!({
        "version": 1,
        "kind": "systemone-plan-v1",
        "name": "fixture",
        "protocol": "systemone-v1",
        "requests": [
            {
                "id": "one",
                "state": "Neutral fixture state for the single-question cell.",
                "questions": {"asked": {"type": "noul", "instructions": "Does the fixture state hold?"}},
            },
            {
                "id": "three",
                "state": "Neutral fixture state for the multi-question cell.",
                "questions": {
                    "asked": {"type": "noul", "instructions": "Does the fixture state hold?"},
                    "label": {
                        "type": "choice",
                        "instructions": "Which fixture label applies?",
                        "criteria": {"alpha": "First label", "beta": "Second label"},
                    },
                    "level": {
                        "type": "score",
                        "instructions": "Which fixture level applies?",
                        "criteria": ["low", "mid", "high"],
                    },
                },
            },
            {
                "id": "image",
                "state": null,
                "images": ["data:image/png;base64,AA=="],
                "questions": {
                    "count": {
                        "type": "choice",
                        "instructions": "How many fixture objects appear?",
                        "criteria": {"one": "One", "two": "Two"},
                    },
                },
            },
        ],
        "cells": [
            {"id": "single", "requests": ["one"], "concurrency": 1, "warmup_bursts": 1, "measured_bursts": 3},
            {"id": "mixed", "requests": ["three", "image"], "concurrency": 2, "warmup_bursts": 1, "measured_bursts": 3},
        ],
        "limits": {"request_timeout_ms": 5000, "burst_timeout_ms": 10000, "response_bytes": 65536},
        "slots": [
            {"id": "a", "arm": "a", "warmup": false, "index": 0},
            {"id": "b", "arm": "b", "warmup": false, "index": 1},
            {"id": "a2", "arm": "a2", "warmup": false, "index": 2},
        ],
        "gates": [
            {"cell": "single", "metric": "latency_us", "max_adverse_bps": 900, "max_reference_spread_bps": 5000},
            {"cell": "mixed", "metric": "decisions_per_second", "max_adverse_bps": 900, "max_reference_spread_bps": 5000},
        ],
    })
}

fn tolerant(mut plan: Value) -> Value {
    for gate in plan["gates"].as_array_mut().unwrap() {
        gate["max_adverse_bps"] = json!(9999);
        gate["max_reference_spread_bps"] = json!(1000000);
    }
    plan
}

fn fixtures(temp: &Temp, name: &str, plan: &Value) -> PathBuf {
    let plan_path = temp.path(&format!("{name}-plan.json"));
    fs::write(&plan_path, serde_json::to_vec(plan).unwrap()).unwrap();
    fs::write(
        temp.path("deployment.json"),
        serde_json::to_vec(&json!({
            "model_revision": "fixture-revision",
            "runtime": "fixture-runtime",
            "hardware": "loopback",
            "settings": "fixture-settings",
        }))
        .unwrap(),
    )
    .unwrap();
    plan_path
}

fn capture_server(
    temp: &Temp,
    server: &DecisionServer,
    plan: &Path,
    slot: usize,
    name: &str,
) -> Output {
    cli()
        .arg("systemone")
        .arg("capture")
        .arg("--plan")
        .arg(plan)
        .arg("--slot")
        .arg(slot.to_string())
        .arg("--endpoint")
        .arg(&server.endpoint)
        .arg("--deployment")
        .arg(temp.path("deployment.json"))
        .arg("--local-http")
        .arg("--out")
        .arg(temp.path(name))
        .output()
        .unwrap()
}

fn capture(temp: &Temp, endpoint: &str, plan: &Path, slot: usize, name: &str) -> Output {
    cli()
        .arg("systemone")
        .arg("capture")
        .arg("--plan")
        .arg(plan)
        .arg("--slot")
        .arg(slot.to_string())
        .arg("--endpoint")
        .arg(endpoint)
        .arg("--deployment")
        .arg(temp.path("deployment.json"))
        .arg("--local-http")
        .arg("--out")
        .arg(temp.path(name))
        .output()
        .unwrap()
}

fn inspect(temp: &Temp, name: &str) -> Output {
    cli()
        .arg("systemone")
        .arg("inspect")
        .arg(temp.path(name))
        .output()
        .unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn capture_records_latency_and_declared_decision_rates() {
    let temp = Temp::new();
    let server = DecisionServer::new(immediate);
    let plan = fixtures(&temp, "capture", &plan_json());
    let output = capture_server(&temp, &server, &plan, 0, "a");
    successful(&output);
    let report = stdout_json(&output);
    assert_eq!(
        report["support"],
        json!("local-unreleased-extension-not-released-v0.6-support")
    );
    assert_eq!(
        report["collector"]["version"],
        json!(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(report["protocol"], json!("systemone-v1"));
    assert_eq!(report["clock"]["units"], json!("microseconds"));
    assert_eq!(report["waves"]["planned"], json!(8));
    assert_eq!(report["waves"]["retained"], json!(8));
    assert_eq!(report["waves"]["measured"], json!(6));
    assert_eq!(report["waves"]["complete_measured"], json!(6));
    assert_eq!(report["failure_codes"], json!([]));
    let cells = report["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0]["cell"], json!("single"));
    assert_eq!(cells[0]["latency_us"]["samples"], json!(3));
    assert_eq!(cells[0]["requests_per_second"]["samples"], json!(3));
    assert_eq!(cells[0]["decisions"], json!(3));
    assert_eq!(cells[1]["cell"], json!("mixed"));
    assert_eq!(cells[1]["latency_us"]["samples"], json!(6));
    assert_eq!(cells[1]["decisions"], json!(12));
    assert_eq!(cells[1]["decisions_per_second"]["samples"], json!(3));
    for cell in cells {
        for metric in ["latency_us", "requests_per_second", "decisions_per_second"] {
            assert!(
                cell[metric]["samples"].as_u64().unwrap() >= 1,
                "{metric} missing for {}",
                cell["cell"]
            );
        }
    }
    let printed = String::from_utf8(output.stdout).unwrap();
    for absent in ["completion_tokens", "tokens_per_second", "throughput"] {
        assert!(!printed.contains(absent), "receipt fabricated {absent}");
    }

    let seen = server.requests();
    assert_eq!(seen.len(), 12);
    let body = &seen[1];
    assert_eq!(
        body,
        &json!({
            "state": "Neutral fixture state for the single-question cell.",
            "questions": {"asked": {"type": "noul", "instructions": "Does the fixture state hold?"}},
        })
    );
    let retained = temp.path("a/request-000001/body.bin");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(retained).unwrap()).unwrap(),
        *body
    );
    let image = seen.iter().find(|body| body["images"].is_array()).unwrap();
    assert_eq!(image["state"], Value::Null);
    assert_eq!(image["images"], json!(["data:image/png;base64,AA=="]));
}

#[test]
fn capture_fails_closed_on_generated_tokens() {
    let temp = Temp::new();
    let server = DecisionServer::new(|request| {
        let mut response = answers_for(request);
        response["usage"]["output_tokens"] = json!(8);
        (200, response.to_string())
    });
    let plan = fixtures(&temp, "generated", &plan_json());
    let output = capture_server(&temp, &server, &plan, 0, "a");
    assert_eq!(output.status.code(), Some(2));
    let report = stdout_json(&output);
    assert_eq!(report["outcome"], json!("INCONCLUSIVE"));
    assert_eq!(report["waves"]["complete_measured"], json!(0));
    assert_eq!(report["waves"]["retained"], json!(8));
    assert!(
        report["failure_codes"]
            .as_array()
            .unwrap()
            .contains(&json!("output_tokens_nonzero"))
    );
    let replay = inspect(&temp, "a");
    assert_eq!(replay.status.code(), Some(2));
    assert_eq!(stdout_json(&replay), report);
}

#[test]
fn capture_rejects_a_non_system_one_route_before_dispatch() {
    let temp = Temp::new();
    let server = DecisionServer::new(immediate);
    let plan = fixtures(&temp, "route", &plan_json());
    let output = capture(
        &temp,
        &format!("{}/v1/chat/completions", server.base),
        &plan,
        0,
        "a",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("must end with /v1/systemone"));
    assert!(server.requests().is_empty());
    assert!(!temp.path("a/receipt.json").exists());
}

#[test]
fn inspect_detects_changed_retained_response_bytes() {
    let temp = Temp::new();
    let server = DecisionServer::new(immediate);
    let plan = fixtures(&temp, "tamper", &plan_json());
    successful(&capture_server(&temp, &server, &plan, 0, "a"));
    let response = temp.path("a/request-000004/response.bin");
    let mut bytes = fs::read(&response).unwrap();
    bytes.push(b' ');
    fs::write(&response, bytes).unwrap();
    let output = inspect(&temp, "a");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("retained response bytes changed"));
}

#[test]
fn compare_reports_a_declared_change_under_the_observed_envelope() {
    let temp = Temp::new();
    let slow = DecisionServer::new(|request| {
        thread::sleep(Duration::from_millis(40));
        immediate(request)
    });
    let fast = DecisionServer::new(immediate);
    // Hosted macOS runners oversleep the 40 ms fixture delay by up to ~150 ms, so a
    // tight reference-spread bound tests the host timer, not the comparison.
    let mut wide = plan_json();
    for gate in wide["gates"].as_array_mut().unwrap() {
        gate["max_reference_spread_bps"] = json!(1000000);
    }
    let plan = fixtures(&temp, "compare", &wide);
    successful(&capture_server(&temp, &slow, &plan, 0, "a"));
    successful(&capture(&temp, &fast.endpoint, &plan, 1, "b"));
    successful(&capture_server(&temp, &slow, &plan, 2, "a2"));
    let output = cli()
        .args(["systemone", "compare", "--plan"])
        .arg(&plan)
        .arg("--a")
        .arg(temp.path("a"))
        .arg("--b")
        .arg(temp.path("b"))
        .arg("--a2")
        .arg(temp.path("a2"))
        .output()
        .unwrap();
    successful(&output);
    let report = stdout_json(&output);
    assert_eq!(report["kind"], json!("systemone-comparison-v1"));
    assert_eq!(report["outcome"], json!("PASS"));
    assert_eq!(report["reasons"], json!([]));
    assert_eq!(report["acquisitions"].as_array().unwrap().len(), 3);
    let gates = report["gates"].as_array().unwrap();
    assert_eq!(gates.len(), 2);
    for gate in gates {
        assert_eq!(gate["counts"], json!([3, 3, 3]));
        assert_eq!(gate["outcome"], json!("PASS"));
        assert!(gate["reference"].is_array());
        assert!(gate["candidate"].is_array());
    }
    let latency = &gates[0]["candidate"][1];
    let reference = &gates[0]["reference"][0];
    let ratio = |value: &Value| {
        value["numerator"].as_u64().unwrap() as f64 / value["denominator"].as_u64().unwrap() as f64
    };
    assert!(
        ratio(latency) < ratio(reference),
        "candidate latency must be lower for the fixture change"
    );
}

#[test]
fn compare_rejects_reused_or_cross_plan_acquisitions() {
    let temp = Temp::new();
    let server = DecisionServer::new(immediate);
    let plan = fixtures(&temp, "reuse", &plan_json());
    successful(&capture_server(&temp, &server, &plan, 0, "a"));
    successful(&capture_server(&temp, &server, &plan, 1, "b"));
    let reused = cli()
        .args(["systemone", "compare", "--plan"])
        .arg(&plan)
        .arg("--a")
        .arg(temp.path("a"))
        .arg("--b")
        .arg(temp.path("b"))
        .arg("--a2")
        .arg(temp.path("a"))
        .output()
        .unwrap();
    assert_eq!(reused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&reused.stderr).contains("is not an a2 arm capture"));

    let mut other = plan_json();
    other["name"] = json!("other_fixture");
    let other_plan = fixtures(&temp, "other", &other);
    let crossed = cli()
        .args(["systemone", "compare", "--plan"])
        .arg(&other_plan)
        .arg("--a")
        .arg(temp.path("a"))
        .arg("--b")
        .arg(temp.path("b"))
        .arg("--a2")
        .arg(temp.path("b"))
        .output()
        .unwrap();
    assert_eq!(crossed.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&crossed.stderr).contains("different plan than the one supplied")
    );
}

#[test]
fn compare_requires_every_declared_slot() {
    let temp = Temp::new();
    let server = DecisionServer::new(immediate);
    let mut plan = plan_json();
    plan["slots"] = json!([
        {"id": "warm", "arm": "a", "warmup": true, "index": 0},
        {"id": "a", "arm": "a", "warmup": false, "index": 1},
        {"id": "b", "arm": "b", "warmup": false, "index": 2},
        {"id": "a2", "arm": "a2", "warmup": false, "index": 3},
    ]);
    let plan = fixtures(&temp, "slots", &tolerant(plan));
    successful(&capture_server(&temp, &server, &plan, 1, "a"));
    successful(&capture_server(&temp, &server, &plan, 2, "b"));
    successful(&capture_server(&temp, &server, &plan, 3, "a2"));
    let output = cli()
        .args(["systemone", "compare", "--plan"])
        .arg(&plan)
        .arg("--a")
        .arg(temp.path("a"))
        .arg("--b")
        .arg(temp.path("b"))
        .arg("--a2")
        .arg(temp.path("a2"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report = stdout_json(&output);
    assert_eq!(report["outcome"], json!("INCONCLUSIVE"));
    assert!(report["reasons"].as_array().unwrap().contains(&json!(
        "every declared slot, including warmups and failed captures, is required"
    )));
}

#[test]
fn compare_flags_reference_drift() {
    let temp = Temp::new();
    let first = DecisionServer::new(immediate);
    let second = DecisionServer::new(immediate);
    let plan = fixtures(&temp, "drift", &tolerant(plan_json()));
    successful(&capture_server(&temp, &first, &plan, 0, "a"));
    successful(&capture(&temp, &second.endpoint, &plan, 1, "b"));
    successful(&capture(&temp, &second.endpoint, &plan, 2, "a2"));
    let output = cli()
        .args(["systemone", "compare", "--plan"])
        .arg(&plan)
        .arg("--a")
        .arg(temp.path("a"))
        .arg("--b")
        .arg(temp.path("b"))
        .arg("--a2")
        .arg(temp.path("a2"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report = stdout_json(&output);
    assert_eq!(report["outcome"], json!("INCONCLUSIVE"));
    assert!(report["reasons"].as_array().unwrap().contains(&json!(
        "the unchanged reference arm recorded a different clock, endpoint or deployment"
    )));
}
