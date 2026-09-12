use super::*;
use std::cell::RefCell;

const STEPS: usize = 11;

fn fixture(fault: Option<(usize, &'static str)>) -> Server {
    fixture_version(false, fault)
}

fn fixture_version(v2: bool, fault: Option<(usize, &'static str)>) -> Server {
    let prefixes = RefCell::new(Vec::<(String, Vec<u8>)>::new());
    Server::new(move |mut stream, index, request| {
        let step = index % if v2 { 13 } else { STEPS };
        let streaming = v2 && step != 7;
        assert_eq!(request["stream"], streaming);
        if streaming {
            assert_eq!(request["stream_options"], json!({"include_usage":true}));
        } else {
            assert!(request.get("stream_options").is_none());
        }
        assert_eq!(request["max_tokens"], 128);
        let messages = request["messages"].as_array().unwrap();
        let salt = request["cache_salt"].as_str().unwrap().to_owned();
        let encoded = serde_json::to_vec(messages).unwrap();
        let mut prefixes = prefixes.borrow_mut();
        let cached = prefixes
            .iter()
            .filter(|(old, _)| old == &salt)
            .map(|(_, bytes)| {
                // The fixture maps four bytes to a token and aligns to sixteen-token blocks.
                bytes
                    .iter()
                    .zip(&encoded)
                    .take_while(|(a, b)| a == b)
                    .count()
                    / 64
                    * 16
            })
            .max()
            .unwrap_or(0);
        if matches!(step, 0 | 2 | 9) || (v2 && step == 11) {
            assert_eq!(cached, 0);
        } else {
            assert!(cached > 0);
        }
        assert_eq!(cached % 16, 0);
        if step == 0 || step == 2 {
            let system = messages[0]["content"].as_str().unwrap();
            assert_eq!(
                system.matches("Neutral retained context block. ").count(),
                256
            );
        }
        let mut fact = None;
        for message in messages {
            if message["role"] == "user" {
                let text = message["content"].as_str().unwrap();
                if let Some(value) = text.split("SET fact=").nth(1) {
                    fact = Some(value.split('.').next().unwrap().to_owned());
                }
            }
            if message["role"] == "tool" {
                fact = Some(message["content"].as_str().unwrap().to_owned());
                let calls = messages.iter().find_map(|m| m.get("tool_calls")).unwrap();
                assert_eq!(message["tool_call_id"], calls[0]["id"]);
                assert_eq!(calls[0]["function"]["name"], "lookup_fact");
                assert_eq!(
                    serde_json::from_str::<Value>(
                        calls[0]["function"]["arguments"].as_str().unwrap()
                    )
                    .unwrap(),
                    json!({"key":"harbor"})
                );
            }
        }
        if step == 4 {
            assert_eq!(fact.as_deref(), Some("indigo"));
        }
        if step == 5 {
            assert_eq!(fact.as_deref(), Some("copper"));
        }
        if step == 6 {
            assert_eq!(fact.as_deref(), Some("amber"));
        }
        if step == 8 {
            assert_eq!(fact.as_deref(), Some("sapphire"));
        }
        if v2 && step >= 11 {
            assert_eq!(fact.as_deref(), Some("amber"));
        }
        let injected = fault
            .filter(|(target, _)| *target == step)
            .map(|(_, kind)| kind);
        let mut content = json!({"fact":fact.unwrap()}).to_string();
        if step == 1 {
            content = format!("\n{content}\n");
        }
        if injected == Some("leak") {
            content = json!({"fact":"copper"}).to_string();
        }
        if injected == Some("stale") {
            content = json!({"fact":"copper"}).to_string();
        }
        if injected == Some("strict") {
            content = format!("\n{content}\n");
        }
        let (message, finish) = if step == 7 {
            assert_eq!(request["tools"][0]["function"]["name"], "lookup_fact");
            let arguments = if injected == Some("arguments") {
                "{\"key\":\"other\",\"key\":\"harbor\"}"
            } else {
                "{\"key\":\"harbor\"}"
            };
            (
                json!({"role":"assistant","content":null,"tool_calls":[{"id":if injected == Some("id") { String::new() } else { format!("fixture_call_{index}") },"type":"function","function":{"name":if injected == Some("name") { "unrequested_tool" } else { "lookup_fact" },"arguments":arguments}}]}),
                "tool_calls",
            )
        } else {
            (json!({"role":"assistant","content":content}), "stop")
        };
        let cached = if injected == Some("evict") { 0 } else { cached };
        let usage = json!({"prompt_tokens":encoded.len()/4+1,"completion_tokens":12,"prompt_tokens_details":{"cached_tokens":cached}});
        if v2 {
            // Retain only the latest request for each of two LRU history slots.
            if let Some(position) = prefixes.iter().position(|(old, _)| old == &salt) {
                prefixes.remove(position);
            } else if prefixes.len() == 2 {
                prefixes.remove(0);
            }
        }
        prefixes.push((salt, encoded));
        drop(prefixes);
        if streaming {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let content = message["content"].as_str().unwrap();
            let split = content.len() / 2;
            let first = if injected == Some("stream-tool") {
                json!({"tool_calls":[{"index":0,"id":"unexpected","type":"function","function":{"name":"lookup_fact","arguments":"{}"}}]})
            } else {
                json!({"role":"assistant","content": &content[..split]})
            };
            write!(
                stream,
                "data: {}\n\n",
                json!({"choices":[{"index":0,"delta":first,"finish_reason":null}]})
            )
            .unwrap();
            stream.flush().unwrap();
            if injected == Some("stream-tool") {
                return;
            }
            write!(stream, "data: {}\n\ndata: {}\n\n",
                json!({"choices":[{"index":0,"delta":{"content":&content[split..]},"finish_reason":"stop"}]}),
                json!({"choices":[],"usage":usage})).unwrap();
            stream.flush().unwrap();
            if injected == Some("missing-done") {
                return;
            }
            if step == 1 {
                thread::sleep(Duration::from_millis(50));
            }
            write!(stream, "data: [DONE]\n\n").unwrap();
        } else {
            let body = json!({"choices":[{"index":0,"message":message,"finish_reason":finish}],"usage":usage}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
        }
    })
}

fn workload() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/conversation-v1.json")
}
fn run_sequence(temp: &Temp, server: &Server) -> Output {
    run_sequence_path(temp, server, workload())
}

fn run_sequence_path(temp: &Temp, server: &Server, workload: PathBuf) -> Output {
    command()
        .arg("run")
        .arg(workload)
        .args([
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
            "--local-http",
            "--json",
            "--out",
        ])
        .arg(temp.path("run"))
        .output()
        .unwrap()
}

fn selected_control(v2: bool) {
    let temp = Temp::new();
    let server = fixture_version(v2, None);
    let steps = if v2 { 13 } else { STEPS };
    let declaration = deployment(&temp, "deployment.json", "fixture-settings");
    let baseline = command()
        .current_dir(&temp.0)
        .args([
            "baseline",
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
            "--local-http",
            "--json",
        ])
        .arg("--selection")
        .arg(workload().with_file_name(if v2 {
            "conversation-selection-v2.json"
        } else {
            "conversation-selection-v1.json"
        }))
        .arg("--deployment")
        .arg(&declaration)
        .arg("--out")
        .arg(temp.path("baseline"))
        .output()
        .unwrap();
    let report = decoded(&baseline);
    assert_eq!(report["baseline_ready"], true, "{report}");
    assert_eq!(server.count.load(Ordering::SeqCst), 8 * steps);
    let candidate = command()
        .current_dir(&temp.0)
        .arg("check")
        .arg(temp.path("baseline"))
        .args(["--change", "none", "--json"])
        .arg("--deployment")
        .arg(&declaration)
        .arg("--out")
        .arg(temp.path("candidate"))
        .output()
        .unwrap();
    let report = decoded(&candidate);
    assert_eq!(report["result"], "DESCRIPTIVE", "{report}");
    assert_eq!(server.count.load(Ordering::SeqCst), 16 * steps);
    let offline = decoded(&compare(&temp.path("baseline"), &temp.path("candidate")));
    assert_eq!(offline, report);
    let wave = read_json(&temp.path("baseline/acquisition-00/wave-000001/wave.json"));
    assert_eq!(wave["attempts"][0]["sequence"]["correct"], true);
    assert_eq!(wave["attempts"][0]["sequence"]["canonical_match"], false);
    if v2 {
        let timing = &wave["attempts"][0]["timing"];
        let first = timing["first_generated_text_us"].as_u64().unwrap();
        assert_eq!(timing["first_answer_text_us"], first);
        assert!(timing["terminal_us"].as_u64().unwrap() > first);
        assert!(
            timing["terminal_us"].as_u64().unwrap()
                > timing["last_generated_text_us"].as_u64().unwrap()
        );
        let tool = read_json(&temp.path("baseline/acquisition-00/wave-000007/wave.json"));
        for field in [
            "first_generated_text_us",
            "first_answer_text_us",
            "last_generated_text_us",
        ] {
            assert!(tool["attempts"][0]["timing"][field].is_null());
        }
        let returned = read_json(&temp.path("baseline/acquisition-00/wave-000011/wave.json"));
        assert_eq!(returned["attempts"][0]["usage"]["cached_prompt_tokens"], 0);
        assert_eq!(returned["attempts"][0]["sequence"]["correct"], true);
        assert_eq!(returned["eligible"], true);
        let recovered = read_json(&temp.path("baseline/acquisition-00/wave-000012/wave.json"));
        assert!(
            recovered["attempts"][0]["usage"]["cached_prompt_tokens"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert_eq!(
            recovered["attempts"][0]["sequence"]["parent"],
            "return-beta"
        );
        assert_eq!(recovered["eligible"], true);
        let request = read_json(&temp.path("baseline/acquisition-00/wave-000003/reservation.json"));
        let request: Value =
            serde_json::from_str(request["requests"][0].as_str().unwrap()).unwrap();
        assert!(
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "assistant"
                    && message["content"] == "\n{\"fact\":\"copper\"}\n")
        );
    } else {
        assert!(wave["attempts"][0]["timing"]["first_generated_text_us"].is_null());
    }
    let path = temp.path("baseline/acquisition-00/wave-000005/wave.json");
    let mut tampered = read_json(&path);
    tampered["attempts"][0]["sequence"]["parent"] = json!("edit-alpha");
    write_json(&path, &tampered);
    assert_eq!(
        decoded(&compare(&temp.path("baseline"), &temp.path("candidate")))["result"],
        "INVALID"
    );
}

#[test]
fn conversation_selected_control_replays_real_branches_and_tool_history() {
    selected_control(false);
}

#[test]
fn conversation_v2_replays_streamed_facts_eviction_and_recovery() {
    selected_control(true);
}

#[test]
fn conversation_failures_stop_admission_and_keep_semantics_distinct_from_cache() {
    for (step, fault) in [
        (2, "leak"),
        (4, "stale"),
        (6, "evict"),
        (7, "arguments"),
        (7, "name"),
        (7, "id"),
        (9, "strict"),
    ] {
        let temp = Temp::new();
        let server = fixture(Some((step, fault)));
        let output = run_sequence(&temp, &server);
        let report = decoded(&output);
        assert_eq!(
            report["status"],
            if fault == "evict" {
                "stopped-after-ineligible-response"
            } else {
                "stopped-after-sequence-check"
            },
            "{report}"
        );
        assert_eq!(server.count.load(Ordering::SeqCst), step + 1);
        assert!(!temp.path(&format!("run/wave-{:06}", step + 1)).exists());
        let wave = read_json(&temp.path(&format!("run/wave-{step:06}/wave.json")));
        assert_eq!(wave["eligible"], fault != "evict");
        let check = &wave["attempts"][0]["sequence"];
        assert_eq!(check["correct"], fault == "evict" || fault == "strict");
        if fault == "strict" {
            assert_eq!(check["strict_match"], false);
        }
        let replay = command()
            .arg("compare")
            .arg(temp.path("run"))
            .arg(temp.path("run"))
            .arg("--json")
            .output()
            .unwrap();
        let replay = decoded(&replay);
        assert_eq!(&replay["baseline"][step]["sequence_check"], check);
        assert_eq!(replay["changes"][step]["eligible"], false);
    }
}

#[test]
fn conversation_v2_invalid_or_incomplete_evidence_stops_and_replays() {
    for (step, fault, status, correct) in [
        (0, "stream-tool", "unsupported", false),
        (1, "missing-done", "incomplete", false),
        (2, "leak", "complete", false),
        (4, "stale", "complete", false),
        (7, "arguments", "complete", false),
        (9, "strict", "complete", true),
        (12, "evict", "complete", true),
    ] {
        let temp = Temp::new();
        let server = fixture_version(true, Some((step, fault)));
        let output = run_sequence_path(
            &temp,
            &server,
            workload().with_file_name("conversation-v2.json"),
        );
        let report = decoded(&output);
        assert_ne!(report["status"], "completed");
        assert_eq!(server.count.load(Ordering::SeqCst), step + 1);
        assert!(!temp.path(&format!("run/wave-{:06}", step + 1)).exists());
        let wave = read_json(&temp.path(&format!("run/wave-{step:06}/wave.json")));
        assert_eq!(wave["attempts"][0]["status"], status);
        assert_eq!(wave["attempts"][0]["sequence"]["correct"], correct);
        assert_eq!(wave["eligible"], status == "complete" && fault != "evict");
        if fault == "missing-done" {
            let timing = &wave["attempts"][0]["timing"];
            assert!(timing["first_generated_text_us"].is_u64());
            assert!(timing["first_answer_text_us"].is_u64());
            assert!(timing["terminal_us"].is_null());
        }
        let replay = decoded(
            &command()
                .arg("compare")
                .arg(temp.path("run"))
                .arg(temp.path("run"))
                .arg("--json")
                .output()
                .unwrap(),
        );
        assert_eq!(
            replay["baseline"][step]["sequence_check"],
            wave["attempts"][0]["sequence"]
        );
        assert_eq!(replay["changes"][step]["eligible"], false);
    }
}

#[test]
fn old_profiles_cannot_silently_adopt_sequence_or_tool_controls() {
    let temp = Temp::new();
    let server = fixture(None);
    let mut value = read_json(&workload());
    value["version"] = json!(1);
    value["request"]["profile"] = json!("vllm-fixed-v1");
    write_json(&temp.path("legacy.json"), &value);
    let output = command()
        .arg("run")
        .arg(temp.path("legacy.json"))
        .args([
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
            "--local-http",
            "--out",
        ])
        .arg(temp.path("run"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
    assert!(!temp.path("run").exists());
}

#[test]
fn oversized_declared_root_is_rejected_before_any_sequence_traffic() {
    let temp = Temp::new();
    let server = fixture(None);
    let mut value = read_json(&workload());
    value["cases"][2]["fill"]["repeat"] = json!(5000);
    write_json(&temp.path("oversized.json"), &value);
    let output = command()
        .arg("run")
        .arg(temp.path("oversized.json"))
        .args([
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
            "--local-http",
            "--out",
        ])
        .arg(temp.path("run"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
    assert!(!temp.path("run").exists());
}

#[test]
fn interrupted_sequence_retains_partial_response_without_followups_or_resume() {
    let temp = Temp::new();
    let started = Arc::new(AtomicBool::new(false));
    let observed = started.clone();
    let server = Server::new(move |mut stream, _, _| {
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 4096\r\n\r\n{{\"choices\":[").unwrap();
        stream.flush().unwrap();
        observed.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_secs(2));
    });
    let child = command()
        .arg("run")
        .arg(workload())
        .args([
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
            "--local-http",
            "--json",
            "--out",
        ])
        .arg(temp.path("run"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::SeqCst));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let output = child.wait_with_output().unwrap();
    assert_eq!(decoded(&output)["status"], "interrupted");
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    let wave = read_json(&temp.path("run/wave-000000/wave.json"));
    assert_eq!(wave["attempts"][0]["status"], "interrupted");
    assert_eq!(wave["eligible"], false);
    assert!(!temp.path("run/wave-000001").exists());
    let replay = command()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    let replay = decoded(&replay);
    assert_eq!(replay["baseline"][0]["sequence_check"]["correct"], false);
    let resume = command()
        .arg("resume")
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert!(!resume.status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}
