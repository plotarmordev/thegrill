use super::*;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

fn scheduled() -> Value {
    let mut work = workload(1, 0, 1);
    work["version"] = json!(4);
    work["name"] = json!("schedule-fixture");
    work["cases"] = json!([
        {"id":"a","messages":[{"role":"user","content":"A"}]},
        {"id":"b","messages":[{"role":"user","content":"B {fill}"}],"fill":{"unit":"long ","repeat":10000}}
    ]);
    work["cells"] = json!([]);
    let request = json!({"profile":"portable-chat-v1","stream":true,"output":{"tokens":3,"mode":"cap"},"cache":"observe","seed":9});
    work["schedule"] = json!([
        {"id":"solo-a","kind":"solo","warmup_trials":0,"trials":1,"lanes":[{"id":"a","case":"a","arrival":{"kind":"fixed_offset","offset_us":0}}]},
        {"id":"solo-b","kind":"solo","warmup_trials":0,"trials":1,"lanes":[{"id":"b","case":"b","request":request,"arrival":{"kind":"fixed_offset","offset_us":0}}]},
        {"id":"mixed","kind":"overlap","warmup_trials":0,"trials":1,"lanes":[
            {"id":"decode","case":"a","arrival":{"kind":"fixed_offset","offset_us":0},"control":{"scenario":"solo-a","lane":"a"}},
            {"id":"prefill","case":"b","request":request,"arrival":{"kind":"after_first_generated","lane":"decode","offset_us":40000},"control":{"scenario":"solo-b","lane":"b"}}
        ]}
    ]);
    work
}

fn mixed_first(mut work: Value) -> Value {
    work["schedule"].as_array_mut().unwrap().rotate_right(1);
    work
}

fn replay(temp: &Temp, name: &str) -> Output {
    cli()
        .arg("compare")
        .arg(temp.path(name))
        .arg(temp.path(name))
        .arg("--json")
        .output()
        .unwrap()
}

/// Requests 0 and 1 are the solo controls. In later overlap waves the decode lane
/// keeps generating until the prefill lane has generated, however late it is scheduled.
fn paced(
    mut stream: TcpStream,
    index: usize,
    body: Value,
    (prefill_generated, prefill_seen): (&Sender<()>, &Mutex<Receiver<()>>),
) {
    let a = body["messages"][0]["content"] == "A";
    let tokens = if a { 8 } else { 3 };
    assert_eq!(body["max_tokens"], tokens);
    assert_eq!(body["seed"], if a { 42 } else { 9 });
    if !a {
        assert!(body["messages"][0]["content"].as_str().unwrap().len() > 50000);
        // A full replacement must not inherit default temperature/top_p.
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
    }
    header(&mut stream, "text/event-stream");
    if !a {
        thread::sleep(Duration::from_millis(80));
    }
    if a {
        frame(
            &mut stream,
            json!({"choices":[{"delta":{"role":"assistant"}}]}),
        );
        thread::sleep(Duration::from_millis(60));
    }
    frame(
        &mut stream,
        json!({"choices":[{"delta":{"content":"first"}}]}),
    );
    if index >= 2 {
        if a {
            // A prefill lane that never arrives fails the overlap assertions.
            let _ = prefill_seen
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        } else {
            prefill_generated.send(()).unwrap();
        }
    }
    // Text written on separate connections must also be read in that order.
    thread::sleep(Duration::from_millis(300));
    frame(
        &mut stream,
        json!({"choices":[{"delta":{"content":"last"}}]}),
    );
    thread::sleep(Duration::from_millis(120));
    finish(&mut stream, Some(tokens), None);
}

#[test]
fn schedule4_mixed_and_fixed_arrivals_replay_named_controls_and_real_overlap() {
    let temp = Temp::new();
    let (prefill_generated, prefill_seen) = channel();
    let prefill_seen = Mutex::new(prefill_seen);
    let server = Server::new(move |stream, index, body| {
        paced(stream, index, body, (&prefill_generated, &prefill_seen));
    });
    let mut work = scheduled();
    work["limits"]["total_ms"] = json!(15000);
    work["limits"]["idle_ms"] = json!(12000);
    let mut fixed = work["schedule"][2].clone();
    fixed["id"] = json!("fixed");
    fixed["lanes"][0]["arrival"]["offset_us"] = json!(30000);
    fixed["lanes"][1]["arrival"] = json!({"kind":"fixed_offset","offset_us":100000});
    work["schedule"].as_array_mut().unwrap().push(fixed);
    successful(&run(&temp, &server, "run", &work));
    assert_eq!(server.count.load(Ordering::SeqCst), 6);
    assert!(server.peak.load(Ordering::SeqCst) <= 2);
    let receipt = wave(&temp, "run", 2);
    assert_eq!(receipt["version"], 2);
    assert!(receipt["spec"].get("case").is_none());
    assert_eq!(receipt["spec"]["lanes"][1]["id"], "prefill");
    let a = &receipt["attempts"][0]["timing"];
    let b = &receipt["attempts"][1]["timing"];
    let at = |v: &Value, key: &str| v[key].as_u64().unwrap();
    let first_a = at(a, "dispatch_offset_us") + at(a, "first_generated_text_us");
    let last_a = at(a, "dispatch_offset_us") + at(a, "last_generated_text_us");
    let first_b = at(b, "dispatch_offset_us") + at(b, "first_generated_text_us");
    assert_eq!(
        receipt["schedule"]["lanes"][1]["trigger_offset_us"],
        first_a
    );
    assert!(at(b, "dispatch_offset_us") >= first_a + 40000);
    assert!(first_a.max(at(b, "dispatch_offset_us")) < last_a.min(first_b));
    assert_eq!(
        receipt["schedule"]["overlap"][0]["left_decode_right_prefill"],
        true
    );
    assert_eq!(receipt["schedule"]["overlap"][0]["generated_text"], true);
    assert!(at(a, "terminal_us") > at(a, "last_generated_text_us") + 50000);
    let fixed = wave(&temp, "run", 3);
    assert!(
        fixed["attempts"][0]["timing"]["dispatch_offset_us"]
            .as_u64()
            .unwrap()
            >= 30000
    );
    assert!(
        fixed["attempts"][1]["timing"]["dispatch_offset_us"]
            .as_u64()
            .unwrap()
            >= 100000
    );
    assert_eq!(fixed["elapsed_us"], fixed["schedule"]["settled_offset_us"]);
    let output = replay(&temp, "run");
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["schedule"]["baseline"][0]["lanes"][0]["repetitions"][0]["matched_solo_complete"],
        true
    );
    assert_eq!(
        report["schedule"]["baseline"][2]["lanes"][1]["repetitions"][0]["matched_solo_complete"],
        true
    );
    assert_eq!(
        report["schedule"]["baseline"][2]["lanes"][0]["repetitions"][0]["completion_tokens"],
        8
    );
    assert_eq!(
        report["schedule"]["baseline"][2]["lanes"][1]["repetitions"][0]["completion_tokens"],
        3
    );
    // A receipt cannot claim mixed overlap using terminal tail instead of text.
    let path = temp.path("run/wave-000002/wave.json");
    let mut forged = receipt;
    forged["schedule"]["overlap"][0]["generated_text"] = json!(false);
    fs::write(path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(!replay(&temp, "run").status.success());
}

#[test]
fn schedule4_missing_trigger_and_early_settlement_keep_undispatched_lanes() {
    for (text, reason) in [(false, "missing_trigger"), (true, "source_settled")] {
        let temp = Temp::new();
        let server = Server::new(move |mut stream, _, _| {
            header(&mut stream, "text/event-stream");
            if text {
                frame(
                    &mut stream,
                    json!({"choices":[{"delta":{"content":"one"}}]}),
                );
            }
            finish(&mut stream, Some(8), None);
        });
        let mut work = mixed_first(scheduled());
        work["schedule"][0]["lanes"][1]["arrival"]["offset_us"] = json!(1000000);
        let output = run(&temp, &server, "run", &work);
        assert!(!output.status.success());
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
        let receipt = wave(&temp, "run", 0);
        assert_eq!(receipt["schedule"]["fatal"], reason);
        assert_eq!(receipt["attempts"][1]["dispatched"], false);
        assert_eq!(receipt["attempts"][1]["timing"]["dispatch_offset_us"], 0);
        assert_eq!(receipt["attempts"][1]["response_bytes"], 0);
        assert!(!temp.path("run/wave-000001").exists());
        assert_eq!(replay(&temp, "run").status.code(), Some(2));
    }
}

#[test]
fn schedule4_fatal_peer_prevents_later_due_lane_and_later_scenario() {
    let temp = Temp::new();
    let server = Server::new(|mut stream, _, _| {
        stream
            .write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
    });
    let mut work = mixed_first(scheduled());
    work["schedule"][0]["lanes"][1]["arrival"] = json!({"kind":"fixed_offset","offset_us":500000});
    assert!(!run(&temp, &server, "run", &work).status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    let receipt = wave(&temp, "run", 0);
    assert_eq!(receipt["schedule"]["fatal"], "lane_failed");
    assert_eq!(receipt["attempts"][0]["http_status"], 503);
    assert_eq!(receipt["attempts"][1]["dispatched"], false);
    assert!(!temp.path("run/wave-000001").exists());
    assert_eq!(replay(&temp, "run").status.code(), Some(2));
}

#[test]
fn schedule4_deadline_retains_wait_and_active_peer_without_replacement() {
    let temp = Temp::new();
    let server = Server::new(|mut stream, _, _| {
        header(&mut stream, "text/event-stream");
        // The source is text-capable but has not generated text at the deadline.
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    });
    let mut work = mixed_first(scheduled());
    work["limits"]["total_ms"] = json!(150);
    work["limits"]["idle_ms"] = json!(150);
    assert!(!run(&temp, &server, "run", &work).status.success());
    let receipt = wave(&temp, "run", 0);
    assert_eq!(receipt["schedule"]["fatal"], "deadline");
    assert!(receipt["elapsed_us"].as_u64().unwrap() >= 150000);
    assert_eq!(receipt["attempts"][1]["dispatched"], false);
    // The deadline can expire before the first connection under CPU contention.
    // It must never admit the waiting peer or replace the expired attempt.
    // The dispatched flag is collector-side, so it is deterministic under the accept race.
    assert_eq!(receipt["attempts"][0]["dispatched"], true);
    assert!(server.count.load(Ordering::SeqCst) <= 1);
    assert!(!temp.path("run/wave-000001").exists());
    assert_eq!(replay(&temp, "run").status.code(), Some(2));
}

#[test]
fn schedule4_cancel_settles_all_admitted_peers_before_publication() {
    let temp = Temp::new();
    let (sent, ready) = std::sync::mpsc::sync_channel(2);
    let server = Server::new(move |mut stream, _, _| {
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"delta":{"content":"active"}}]}),
        );
        sent.send(()).unwrap();
        let mut byte = [0];
        match stream.read(&mut byte) {
            Ok(0) => (),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => (),
            result => panic!("expected cancellation to close the peer: {result:?}"),
        }
    });
    let work = mixed_first(scheduled());
    let input = temp.path("work.json");
    fs::write(&input, serde_json::to_vec(&work).unwrap()).unwrap();
    let mut child = cli()
        .arg("run")
        .arg(input)
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
    server.wait_for_request(&mut child);
    server.wait_for_request(&mut child);
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!temp.path("run/wave-000000/wave.json").exists());
    // SAFETY: kill takes plain integers; the unreaped child keeps its PID reserved.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    assert!(!child.wait_with_output().unwrap().status.success());
    let receipt = wave(&temp, "run", 0);
    assert_eq!(receipt["schedule"]["fatal"], "cancelled");
    assert!(
        receipt["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["status"] == "interrupted")
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    assert!(!temp.path("run/wave-000001").exists());
    assert_eq!(replay(&temp, "run").status.code(), Some(2));
}

#[test]
fn schedule4_preflight_rejects_invalid_controls_triggers_and_legacy_extensions_without_traffic() {
    let temp = Temp::new();
    let server = Server::new(normal);
    for variant in [
        "control",
        "forward",
        "nonstream",
        "legacy",
        "null",
        "memory",
        "offset",
        "seed",
    ] {
        let mut work = scheduled();
        match variant {
            "control" => work["schedule"][2]["lanes"][1]["control"]["lane"] = json!("a"),
            "forward" => {
                work["schedule"][2]["lanes"][0]["arrival"] =
                    json!({"kind":"after_first_generated","lane":"prefill","offset_us":0})
            }
            "nonstream" => work["request"]["stream"] = json!(false),
            "legacy" => work["version"] = json!(3),
            "null" => work["schedule"] = Value::Null,
            "memory" => work["limits"]["wave_buffer_bytes"] = json!(1024),
            "offset" => work["schedule"][2]["lanes"][1]["arrival"]["offset_us"] = json!(u64::MAX),
            "seed" => work["schedule"][1]["lanes"][0]["request"]["seed"] = json!(i64::MAX),
            _ => unreachable!(),
        }
        assert!(
            !run(&temp, &server, variant, &work).status.success(),
            "{variant}"
        );
        assert!(!temp.path(variant).exists(), "{variant}");
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}

#[test]
fn schedule4_peer_failure_cancels_active_peer_and_retains_pending_position() {
    let temp = Temp::new();
    let active = Arc::new(AtomicBool::new(false));
    let started = active.clone();
    let server = Server::new(move |mut stream, _, body| {
        if body["messages"][0]["content"] == "A" {
            header(&mut stream, "text/event-stream");
            frame(
                &mut stream,
                json!({"choices":[{"delta":{"content":"active"}}]}),
            );
            started.store(true, Ordering::SeqCst);
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
        } else {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !started.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "peer was not dispatched");
                thread::sleep(Duration::from_millis(1));
            }
            stream
                .write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        }
    });
    let mut work = mixed_first(scheduled());
    work["schedule"][0]["lanes"][1]["arrival"] = json!({"kind":"fixed_offset","offset_us":0});
    let mut later = work["schedule"][0]["lanes"][0].clone();
    later["id"] = json!("later");
    later["arrival"]["offset_us"] = json!(1000000);
    work["schedule"][0]["lanes"]
        .as_array_mut()
        .unwrap()
        .push(later);
    assert!(!run(&temp, &server, "run", &work).status.success());
    let receipt = wave(&temp, "run", 0);
    assert_eq!(receipt["attempts"][0]["status"], "interrupted");
    assert_eq!(receipt["attempts"][1]["status"], "http_error");
    assert_eq!(receipt["attempts"][2]["dispatched"], false);
    assert_eq!(
        receipt["schedule"]["lanes"][2]["not_dispatched"],
        "lane_failed"
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    assert_eq!(replay(&temp, "run").status.code(), Some(2));
}

#[test]
fn schedule4_pause_publishes_whole_scenario_then_resumes_only_never_started_scenarios() {
    let temp = Temp::new();
    let hold = Arc::new(AtomicBool::new(true));
    let held = hold.clone();
    let (send, releases) = std::sync::mpsc::sync_channel(2);
    let server = Server::new(move |mut stream, _, body| {
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"delta":{"content":"first"}}]}),
        );
        if held.load(Ordering::SeqCst) {
            let (release, wait) = std::sync::mpsc::sync_channel(0);
            send.send(release).unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        frame(
            &mut stream,
            json!({"choices":[{"delta":{"content":"last"}}]}),
        );
        finish(&mut stream, body["max_tokens"].as_u64(), None);
    });
    let mut work = mixed_first(scheduled());
    work["limits"]["total_ms"] = json!(15000);
    work["limits"]["idle_ms"] = json!(12000);
    let input = temp.path("work.json");
    fs::write(&input, serde_json::to_vec(&work).unwrap()).unwrap();
    let mut child = cli()
        .arg("run")
        .arg(input)
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
    server.wait_for_request(&mut child);
    server.wait_for_request(&mut child);
    let releases: Vec<_> = (0..2)
        .map(|_| releases.recv_timeout(Duration::from_secs(3)).unwrap())
        .collect();
    successful(&cli().arg("pause").arg(temp.path("run")).output().unwrap());
    assert!(child.try_wait().unwrap().is_none());
    assert!(!temp.path("run/wave-000000/wave.json").exists());
    hold.store(false, Ordering::SeqCst);
    for release in releases {
        release.send(()).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["status"], "paused");
    assert!(!temp.path("run/wave-000001").exists());
    let retained = fs::read(temp.path("run/wave-000000/wave.json")).unwrap();
    let resumed = cli()
        .arg("resume")
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(resumed.status.code(), Some(2)); // continued acquisition, not uninterrupted evidence
    assert_eq!(server.count.load(Ordering::SeqCst), 4);
    assert_eq!(
        fs::read(temp.path("run/wave-000000/wave.json")).unwrap(),
        retained
    );
    assert_eq!(replay(&temp, "run").status.code(), Some(2));
}
