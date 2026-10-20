use super::*;

fn workload() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/conversation-tools-v3.json")
}

fn run(temp: &Temp, server: &Server, source: &Path) -> Output {
    command()
        .arg("run")
        .arg(source)
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

fn replay(temp: &Temp) -> Output {
    command()
        .arg("compare")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap()
}

fn delta(stream: &mut TcpStream, value: Value, finish: Option<&str>) -> bool {
    write!(
        stream,
        "data: {}\n\n",
        json!({"choices":[{"index":0,"delta":value,"finish_reason":finish}]})
    )
    .is_ok()
        && stream.flush().is_ok()
}

fn fixture(fault: Option<&'static str>) -> Server {
    Server::new(move |mut stream, index, request| {
        assert_eq!(request["stream"], true);
        assert_eq!(request["stream_options"], json!({"include_usage":true}));
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        if request.get("tools").is_none() {
            let messages = request["messages"].as_array().unwrap();
            let calls = messages
                .iter()
                .find_map(|message| message.get("tool_calls"))
                .unwrap();
            let result = messages
                .iter()
                .find(|message| message["role"] == "tool")
                .unwrap();
            assert_eq!(calls[0]["id"], result["tool_call_id"]);
            assert_eq!(calls[0]["function"]["name"], "lookup_fact");
            assert_eq!(
                calls[0]["function"]["arguments"],
                "{ \"key\": \"harbor\" }\n"
            );
            assert_eq!(result["content"], "sapphire");
            assert!(delta(
                &mut stream,
                json!({"content":"{\"fact\":\"sapphire\"}"}),
                Some("stop")
            ));
        } else {
            let id = if fault == Some("reuse-id") {
                "call_shared".to_owned()
            } else {
                format!("call_{index}")
            };
            if !delta(
                &mut stream,
                json!({"tool_calls":[{"index":0,"id":id,"type":"function"}]}),
                None,
            ) {
                return;
            }
            thread::sleep(Duration::from_millis(30));
            if !delta(
                &mut stream,
                json!({"tool_calls":[{"index":0,"function":{"name":"look"}}]}),
                None,
            ) {
                return;
            }
            thread::sleep(Duration::from_millis(30));
            let name = if fault == Some("name") {
                "up_other"
            } else {
                "up_fact"
            };
            if !delta(
                &mut stream,
                json!({"tool_calls":[{"index":0,"function":{"name":name,"arguments":"{ \"key\": "}}]}),
                None,
            ) {
                return;
            }
            if fault == Some("name") {
                return;
            }
            thread::sleep(Duration::from_millis(30));
            let arguments = match fault {
                Some("arguments") => "\"other\",\"key\":\"harbor\" }\n",
                Some("incomplete") => "\"harbor\"",
                _ => "\"harbor\" }\n",
            };
            if !delta(
                &mut stream,
                json!({"tool_calls":[{"index":0,"function":{"arguments":arguments}}]}),
                if fault == Some("named-stop") {
                    Some("stop")
                } else {
                    None
                },
            ) {
                return;
            }
            if fault == Some("duplicate-id")
                && !delta(
                    &mut stream,
                    json!({"tool_calls":[{"index":0,"id":id}]}),
                    None,
                )
            {
                return;
            }
            if fault == Some("trailing")
                && !delta(
                    &mut stream,
                    json!({"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}),
                    None,
                )
            {
                return;
            }
            let finish = if fault == Some("length") {
                "length"
            } else {
                "tool_calls"
            };
            if fault != Some("named-stop") && !delta(&mut stream, json!({}), Some(finish)) {
                return;
            }
        }
        if write!(
            stream,
            "data: {}\n\n",
            json!({"choices":[],"usage":{"prompt_tokens":32,"completion_tokens":12}})
        )
        .is_err()
        {
            return;
        }
        if stream.flush().is_err() {
            return;
        }
        if fault == Some("missing-done") {
            return;
        }
        thread::sleep(Duration::from_millis(70));
        let _ = write!(stream, "data: [DONE]\n\n");
    })
}

#[test]
fn tool_conversation_streams_fragments_links_actual_history_and_replays() {
    let temp = Temp::new();
    let server = fixture(Some("named-stop"));
    let result = decoded(&run(&temp, &server, &workload()));
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    let plan = read_json(&temp.path("run/plan.json"));
    assert_eq!(plan["version"], 4);
    let wave = read_json(&temp.path("run/wave-000000/wave.json"));
    assert_eq!(wave["version"], 2);
    assert_eq!(wave["attempts"][0]["sequence"]["correct"], true);
    let timing = &wave["attempts"][0]["timing"];
    let first = timing["first_tool_delta_us"].as_u64().unwrap();
    let valid = timing["first_validated_tool_call_us"].as_u64().unwrap();
    let terminal = timing["terminal_us"].as_u64().unwrap();
    assert!(first < valid && valid < terminal, "{timing}");
    assert!(timing["first_generated_text_us"].is_null());
    let followup = read_json(&temp.path("run/wave-000001/wave.json"));
    assert_eq!(followup["attempts"][0]["sequence"]["parent"], "lookup");
    assert_eq!(followup["attempts"][0]["sequence"]["correct"], true);
    let offline = decoded(&replay(&temp));
    assert_eq!(
        offline["baseline"][0]["sequence_check"],
        wave["attempts"][0]["sequence"]
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    let mut forged = wave;
    forged["attempts"][0]["timing"]["first_validated_tool_call_us"] = json!(first);
    write_json(&temp.path("run/wave-000000/wave.json"), &forged);
    assert!(!replay(&temp).status.success());
}

#[test]
fn tool_conversation_invalid_and_partial_streams_stop_without_followup() {
    for fault in [
        "name",
        "arguments",
        "incomplete",
        "duplicate-id",
        "trailing",
        "missing-done",
        "length",
    ] {
        let temp = Temp::new();
        let server = fixture(Some(fault));
        let report = decoded(&run(&temp, &server, &workload()));
        assert_ne!(report["status"], "completed", "{fault}: {report}");
        assert_eq!(server.count.load(Ordering::SeqCst), 1, "{fault}");
        assert!(!temp.path("run/wave-000001").exists());
        let wave = read_json(&temp.path("run/wave-000000/wave.json"));
        assert_eq!(wave["attempts"][0]["sequence"]["correct"], false, "{fault}");
        assert!(
            wave["attempts"][0]["timing"]["first_tool_delta_us"].is_u64(),
            "{fault}"
        );
        assert!(
            wave["attempts"][0]["timing"]["first_validated_tool_call_us"].is_null(),
            "{fault}"
        );
        let offline = decoded(&replay(&temp));
        assert_eq!(
            offline["baseline"][0]["sequence_check"], wave["attempts"][0]["sequence"],
            "{fault}"
        );
    }
}

#[test]
fn tool_conversation_reused_parent_call_id_is_rejected_and_replayed() {
    let temp = Temp::new();
    let server = fixture(Some("reuse-id"));
    let mut source = read_json(&workload());
    let mut lookup = source["cases"][0].clone();
    lookup["id"] = json!("lookup-again");
    lookup["messages"] =
        json!([{"role":"user","content":"Call lookup_fact with key harbor again."}]);
    lookup["step"]["parent"] = json!("followup");
    let mut followup = source["cases"][1].clone();
    followup["id"] = json!("followup-again");
    followup["step"]["parent"] = json!("lookup-again");
    source["cases"]
        .as_array_mut()
        .unwrap()
        .extend([lookup, followup]);
    for id in ["lookup-again", "followup-again"] {
        source["cells"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":id,"case":id,"concurrency":1,"warmup_trials":0,"trials":1}));
    }
    write_json(&temp.path("workload.json"), &source);
    let report = decoded(&run(&temp, &server, &temp.path("workload.json")));
    assert_ne!(report["status"], "completed");
    assert_eq!(server.count.load(Ordering::SeqCst), 3);
    let wave = read_json(&temp.path("run/wave-000002/wave.json"));
    assert_eq!(wave["attempts"][0]["status"], "malformed");
    assert!(wave["attempts"][0]["timing"]["first_validated_tool_call_us"].is_null());
    assert!(!temp.path("run/wave-000003").exists());
    let offline = decoded(&replay(&temp));
    assert_eq!(offline["baseline"][2]["sequence_check"]["correct"], false);
}

#[test]
fn tool_conversation_cancel_retains_partial_without_resume_or_replacement() {
    let temp = Temp::new();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let server = Server::new(move |mut stream, _, _| {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        assert!(delta(
            &mut stream,
            json!({"tool_calls":[{"index":0,"id":"partial","type":"function","function":{"name":"lookup_fact","arguments":"{"}}]}),
            None
        ));
        entered.send(()).unwrap();
        let _ = gate.recv_timeout(Duration::from_secs(10));
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
    // Startup hashes the unoptimized collector binary, which takes seconds on a loaded host.
    ready.recv_timeout(Duration::from_secs(60)).unwrap();
    // SAFETY: kill takes plain integers; the unreaped child keeps its PID reserved.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let output = child.wait_with_output().unwrap();
    let _ = release.send(());
    assert_eq!(decoded(&output)["status"], "interrupted");
    let wave = read_json(&temp.path("run/wave-000000/wave.json"));
    assert_eq!(wave["attempts"][0]["status"], "interrupted");
    assert!(wave["attempts"][0]["timing"]["first_validated_tool_call_us"].is_null());
    assert!(!temp.path("run/wave-000001").exists());
    assert_eq!(
        decoded(&replay(&temp))["baseline"][0]["sequence_check"]["correct"],
        false
    );
    let resume = command()
        .arg("resume")
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert!(!resume.status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}

#[test]
fn tool_acquisition6_repeats_actual_calls_and_per_acquisition_id_namespace() {
    let temp = Temp::new();
    let server = fixture(Some("reuse-id"));
    let mut source = read_json(&workload());
    source["version"] = json!(6);
    source["limits"]["wave_buffer_bytes"] = json!(16 * 1024 * 1024);
    source["acquisition"] = json!({"kind":"conversation","repetitions":3,"warmup_repetitions":1,
        "measured_steps":["lookup","followup"],"input_bytes":1048576,"retained_history_bytes":1048576});
    write_json(&temp.path("workload.json"), &source);
    assert_eq!(
        decoded(&run(&temp, &server, &temp.path("workload.json")))["status"],
        "completed"
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 8);
    let report = decoded(&replay(&temp));
    assert_eq!(
        report["acquisition"]["baseline"]["records"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    for index in [0, 2, 4, 6] {
        let wave = read_json(&temp.path(&format!("run/wave-{index:06}/wave.json")));
        assert!(wave["attempts"][0]["timing"]["first_tool_delta_us"].is_u64());
        assert!(wave["attempts"][0]["timing"]["first_validated_tool_call_us"].is_u64());
    }
}

/// Three-step history: factual prime, fixed tool call, factual follow-up.
/// `shared` toggles whether the tool declaration is sent on every step.
fn shared_workload(shared: bool) -> Value {
    let mut tool_expect = json!({"kind":"tool","key":"harbor","result":"sapphire"});
    if shared {
        tool_expect["shared"] = json!(true);
    }
    json!({
        "version": 6,
        "name": "shared-tools-fixture",
        "request": {
            "profile": "vllm-conversation-v3",
            "stream": true,
            "output": {"tokens": 128, "mode": "cap"},
            "cache": "observe",
            "temperature_milli": 0,
            "top_p_milli": 1000,
            "seed": 0,
            "thinking_control": {"kind": "vllm-enable-thinking-v1", "enabled": false}
        },
        "limits": {"total_ms": 60000, "idle_ms": 10000, "response_bytes": 65536, "wave_buffer_bytes": 16777216},
        "cases": [
            {"id": "prime", "messages": [{"role": "user", "content": "Remember sapphire."}],
             "step": {"history": "h", "parent": null, "cache": "reported-prefix-zero",
                      "expect": {"kind": "json", "value": {"fact": "sapphire"}, "strict": null}}},
            {"id": "lookup", "messages": [{"role": "user", "content": "Call lookup_fact with key harbor."}],
             "step": {"history": "h", "parent": "prime", "cache": "reported-prefix-hit",
                      "expect": tool_expect}},
            {"id": "followup", "messages": [{"role": "user", "content": "Return the local tool fixture value as the fact."}],
             "step": {"history": "h", "parent": "lookup", "cache": "reported-prefix-hit",
                      "expect": {"kind": "json", "value": {"fact": "sapphire"}, "strict": null}}}
        ],
        "cells": [
            {"id": "prime", "case": "prime", "concurrency": 1, "warmup_trials": 0, "trials": 1},
            {"id": "lookup", "case": "lookup", "concurrency": 1, "warmup_trials": 0, "trials": 1},
            {"id": "followup", "case": "followup", "concurrency": 1, "warmup_trials": 0, "trials": 1}
        ],
        "acquisition": {"kind": "conversation", "repetitions": 1, "warmup_repetitions": 0,
            "measured_steps": ["lookup", "followup"], "input_bytes": 1048576, "retained_history_bytes": 1048576}
    })
}

/// Fixture modeling a tool-head chat template: the rendered prefix starts with
/// the serialized tool declarations, so a request that adds `tools` shares no
/// leading blocks with tool-free requests under the same salt. `tool_choice`
/// is a decoding control and does not enter the prompt prefix.
fn shared_fixture() -> Server {
    let prefixes = std::cell::RefCell::new(Vec::<(String, Vec<u8>)>::new());
    Server::new(move |mut stream, index, request| {
        let salt = request["cache_salt"].as_str().unwrap().to_owned();
        let tool_call = request["tool_choice"].is_object();
        if tool_call {
            assert_eq!(request["tool_choice"]["function"]["name"], "lookup_fact");
        }
        // Tool-head templates render declarations before messages; the head
        // bytes keep that order so a step-scoped declaration diverges at the
        // first block exactly like the serving stack.
        let mut head = b"{\"tools\":".to_vec();
        head.extend(
            serde_json::to_vec(&request.get("tools").cloned().unwrap_or(Value::Null)).unwrap(),
        );
        head.extend(b",\"messages\":");
        head.extend(serde_json::to_vec(&request["messages"]).unwrap());
        head.push(b'}');
        let mut prefixes = prefixes.borrow_mut();
        let cached = prefixes
            .iter()
            .filter(|(old, _)| old == &salt)
            .map(|(_, bytes)| bytes.iter().zip(&head).take_while(|(a, b)| a == b).count() / 64 * 16)
            .max()
            .unwrap_or(0);
        if let Some(position) = prefixes.iter().position(|(old, _)| old == &salt) {
            prefixes.remove(position);
        }
        prefixes.push((salt, head));
        drop(prefixes);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        if tool_call {
            assert!(delta(
                &mut stream,
                json!({"tool_calls":[{"index":0,"id":format!("call_{index}"),"type":"function",
                    "function":{"name":"lookup_fact","arguments":"{\"key\": \"harbor\"}"}}]}),
                Some("tool_calls")
            ));
        } else {
            assert!(delta(
                &mut stream,
                json!({"content":"{\"fact\":\"sapphire\"}"}),
                Some("stop")
            ));
        }
        if write!(
            stream,
            "data: {}\n\n",
            json!({"choices":[],"usage":{"prompt_tokens":4096,"completion_tokens":8,
                "prompt_tokens_details":{"cached_tokens":cached}}})
        )
        .is_err()
        {
            return;
        }
        let _ = stream.flush();
        let _ = write!(stream, "data: [DONE]\n\n");
    })
}

#[test]
fn step_scoped_tool_declaration_diverges_leading_prefix_and_stops() {
    let temp = Temp::new();
    let server = shared_fixture();
    let source = temp.path("workload.json");
    write_json(&source, &shared_workload(false));
    let report = decoded(&run(&temp, &server, &source));
    assert_ne!(report["status"], "completed", "{report}");
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    assert!(!temp.path("run/wave-000002").exists());
    let wave = read_json(&temp.path("run/wave-000001/wave.json"));
    let attempt = &wave["attempts"][0];
    assert_eq!(attempt["usage"]["cached_prompt_tokens"], 0);
    assert_eq!(
        attempt["eligibility_errors"],
        json!(["provider_reported_prefix_cache_miss"])
    );
    // The tool call itself was correct; only the impossible cache gate failed.
    assert_eq!(attempt["sequence"]["correct"], true);
    assert_eq!(wave["eligible"], false);
    let request: Value = serde_json::from_str(
        read_json(&temp.path("run/wave-000001/reservation.json"))["requests"][0]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(request["tools"].is_array());
    assert!(request["tool_choice"].is_object());
    let prime: Value = serde_json::from_str(
        read_json(&temp.path("run/wave-000000/reservation.json"))["requests"][0]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(prime.get("tools").is_none());
    assert!(prime.get("tool_choice").is_none());
}

#[test]
fn shared_tool_declaration_keeps_leading_prefix_and_tool_step_hits() {
    let temp = Temp::new();
    let server = shared_fixture();
    let source = temp.path("workload.json");
    write_json(&source, &shared_workload(true));
    let report = decoded(&run(&temp, &server, &source));
    if report["status"] != "completed" {
        for index in 0..3 {
            let path = temp.path(&format!("run/wave-{index:06}/wave.json"));
            if path.exists() {
                eprintln!("wave {index}: {}", read_json(&path)["attempts"][0]);
            }
        }
    }
    assert_eq!(report["status"], "completed", "{report}");
    assert_eq!(server.count.load(Ordering::SeqCst), 3);
    for (index, tool_call) in [(0, false), (1, true), (2, false)] {
        let request: Value = serde_json::from_str(
            read_json(&temp.path(&format!("run/wave-{index:06}/reservation.json")))["requests"][0]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(request["tools"][0]["function"]["name"], "lookup_fact");
        if tool_call {
            assert_eq!(request["tool_choice"]["function"]["name"], "lookup_fact");
        } else {
            assert_eq!(request["tool_choice"], "none");
        }
    }
    for index in [1, 2] {
        let wave = read_json(&temp.path(&format!("run/wave-{index:06}/wave.json")));
        let attempt = &wave["attempts"][0];
        assert!(
            attempt["usage"]["cached_prompt_tokens"].as_u64().unwrap() > 0,
            "wave {index}: {attempt}"
        );
        assert_eq!(attempt["sequence"]["correct"], true);
        assert_eq!(wave["eligible"], true);
    }
    let followup = read_json(&temp.path("run/wave-000002/reservation.json"));
    let request: Value = serde_json::from_str(followup["requests"][0].as_str().unwrap()).unwrap();
    let messages = request["messages"].as_array().unwrap();
    let calls = messages.iter().find_map(|m| m.get("tool_calls")).unwrap();
    assert_eq!(calls[0]["function"]["name"], "lookup_fact");
    assert_eq!(messages.iter().filter(|m| m["role"] == "tool").count(), 1);
    let offline = decoded(&replay(&temp));
    let record = &offline["acquisition"]["baseline"]["records"][0];
    assert_eq!(record["complete_eligible"], true, "{offline}");
    assert_eq!(record["ineligible_steps"], json!([]));
}

#[test]
fn shared_tool_declarations_require_uniform_history_and_bounded_versions() {
    let assert_admitted = |source: &Path| {
        let output = command()
            .arg("preflight")
            .arg(source)
            .args([
                "--endpoint",
                "http://127.0.0.1:1/v1/chat/completions",
                "--model",
                "fixture",
                "--local-http",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    // Mixed shared flags inside one history are rejected before dispatch.
    let temp = Temp::new();
    let server = shared_fixture();
    let mut mixed = shared_workload(true);
    mixed["cases"].as_array_mut().unwrap().insert(
        2,
        json!({"id": "lookup-again", "messages": [{"role": "user", "content": "Call lookup_fact with key harbor."}],
            "step": {"history": "h", "parent": "lookup", "cache": "reported-prefix-hit",
                "expect": {"kind": "tool", "key": "harbor", "result": "sapphire"}}}),
    );
    mixed["cases"].as_array_mut().unwrap().push(
        json!({"id": "followup-again", "messages": [{"role": "user", "content": "Return the local tool fixture value as the fact."}],
            "step": {"history": "h", "parent": "lookup-again", "cache": "reported-prefix-hit",
                "expect": {"kind": "json", "value": {"fact": "sapphire"}, "strict": null}}}),
    );
    mixed["cells"].as_array_mut().unwrap().insert(
        2,
        json!({"id": "lookup-again", "case": "lookup-again", "concurrency": 1, "warmup_trials": 0, "trials": 1}),
    );
    mixed["cells"].as_array_mut().unwrap().push(
        json!({"id": "followup-again", "case": "followup-again", "concurrency": 1, "warmup_trials": 0, "trials": 1}),
    );
    let source = temp.path("mixed.json");
    write_json(&source, &mixed);
    assert!(!run(&temp, &server, &source).status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
    // The identical ordering/limits are valid once flags agree; an unrelated
    // validation failure must not make the negative regression pass.
    mixed["cases"][2]["step"]["expect"]["shared"] = json!(true);
    write_json(&source, &mixed);
    assert_admitted(&source);

    // Workload v2 has no bounded tool trace allowance for shared declarations.
    let temp = Temp::new();
    let server = shared_fixture();
    let mut legacy = shared_workload(true);
    legacy["version"] = json!(2);
    legacy["request"]["profile"] = json!("vllm-conversation-v2");
    legacy.as_object_mut().unwrap().remove("acquisition");
    let source = temp.path("legacy.json");
    write_json(&source, &legacy);
    assert!(!run(&temp, &server, &source).status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
    legacy["cases"][1]["step"]["expect"]["shared"] = json!(false);
    write_json(&source, &legacy);
    assert_admitted(&source);
}
