//! Speech-server runner fixtures: a loopback server answering by input text and voice.
use super::*;
use crate::Server;
use std::io::Write;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

const RATE: u32 = 8000;

fn wav(frames: usize) -> Vec<u8> {
    let data = frames * 2;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((36 + data as u32).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(RATE.to_le_bytes());
    bytes.extend((RATE * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((data as u32).to_le_bytes());
    bytes.extend(vec![0; data]);
    bytes
}

// Writes are best-effort: the collector closes timed-out and oversized replies.
fn reply(mut stream: TcpStream, status: u16, finish: &str, delay_ms: u64, chunks: &[&[u8]]) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nConnection: close\r\nX-Finish-Reason: {finish}\r\nTransfer-Encoding: chunked\r\n\r\n"
    );
    thread::sleep(Duration::from_millis(delay_ms));
    for chunk in chunks {
        let _ = write!(stream, "{:x}\r\n", chunk.len());
        let _ = stream.write_all(chunk);
        let _ = stream.write_all(b"\r\n");
        let _ = stream.flush();
    }
    let _ = stream.write_all(b"0\r\n\r\n");
}

/// Input text selects the reply shape; voice selects first-audio delay and length.
fn speech_server() -> Server {
    Server::new(|mut stream, _, request| {
        let (delay_ms, frames) = match request["voice"].as_str() {
            Some("slow") => (300, 800),
            Some("short") => (0, 400),
            _ => (150, 800),
        };
        let pcm = vec![0; frames * 2];
        let (first, rest) = pcm.split_at(frames);
        match request["input"].as_str().unwrap() {
            "wav" => {
                let body = wav(frames);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
            "status" => reply(stream, 422, "stop", 0, &[br#"{"error":"refused"}"#]),
            "length" => reply(stream, 200, "length", 0, &[first, rest]),
            "partial" => reply(stream, 200, "stop", 0, &[&[0; 3]]),
            "slow" => reply(stream, 200, "stop", 1500, &[first]),
            _ => reply(stream, 200, "stop", delay_ms, &[first, rest]),
        }
    })
}

fn speech_fixture(server: &Server, input: &str) -> Fixture {
    let mut f = Fixture::new();
    f.workload = json!({
        "kind":"media-workload-v1","version":1,"name":"speech","modality":"audio",
        "request":{"prompt":input,"seeds":[7],"sample_rate":RATE},
        "outputs":[],
        "cells":[{"id":"c1","warmup_runs":1,"measured_runs":3}],
        "slots":[{"id":"a","arm":"a"},{"id":"b","arm":"b"},{"id":"a2","arm":"a2"}],
        "limits":{"run_deadline_ms":500,"setup_deadline_ms":1,"output_bytes":65536,"stdout_bytes":1,"stderr_bytes":1},
        "gates":[{"cell":"c1","metric":"end_to_end_us","max_adverse_bps":500,"max_reference_spread_bps":1000000}]
    });
    f.runner = json!({
        "kind":"media-speech-server-v1","version":1,"id":"speech",
        "endpoint":server.endpoint.replace("chat/completions", "audio/speech"),
        "model":"fixture/speech-1","response_format":"pcm","stream":true,
        "pcm":{"channels":1,"sample_bytes":2}
    });
    fs::write(
        f.root.path("deployment.json"),
        br#"{"model_revision":"fixture","runtime":"loopback","hardware":"synthetic","settings":"local"}"#,
    )
    .unwrap();
    f
}

#[test]
fn speech_preflight_renders_request_bodies_and_closes_the_contract() {
    let server = speech_server();
    let mut f = speech_fixture(&server, "Hello there.");
    f.runner["voice"] = json!("alice");
    let report = f.report();
    assert_eq!(report["endpoint"], f.runner["endpoint"]);
    assert!(report.get("program").is_none());
    assert_eq!(report["total_runs"], 4);
    assert_eq!(report["wall_clock_allowance_ms"], 2000);
    assert_eq!(
        report["samples"][0]["body"],
        r#"{"model":"fixture/speech-1","input":"Hello there.","voice":"alice","response_format":"pcm","stream":true}"#
    );
    assert_eq!(server.count.load(std::sync::atomic::Ordering::SeqCst), 0);
    let runner = f.runner.clone();
    let workload = f.workload.clone();
    for (change, expected) in [
        (
            json!({"endpoint":"http://192.0.2.1/v1/audio/speech"}),
            "runner.endpoint",
        ),
        (
            json!({"response_format":"wav"}),
            "runner.stream requires response_format pcm",
        ),
        (json!({"pcm":{"channels":0,"sample_bytes":2}}), "runner.pcm"),
        (json!({"model":""}), "runner.model"),
        (json!({"probe":true}), "unknown field"),
        (json!({"kind":"media-speech-v0"}), "unknown variant"),
    ] {
        f.runner = runner.clone();
        for (key, value) in change.as_object().unwrap() {
            f.runner[key] = value.clone();
        }
        f.reject(expected);
    }
    f.runner = runner.clone();
    f.runner.as_object_mut().unwrap().remove("pcm");
    f.reject("runner.pcm");
    f.runner = runner.clone();
    f.runner["stream"] = json!(false);
    f.workload["gates"][0]["metric"] = json!("first_audio_us");
    f.reject("first_audio_us requires a streaming speech server runner");
    for (pointer, value, expected) in [
        (
            "/request/duration_us",
            json!(1000),
            "request.duration_us is forbidden",
        ),
        (
            "/request/negative_prompt",
            json!("noise"),
            "accept only prompt, seeds and sample_rate",
        ),
        (
            "/outputs",
            json!([{"placeholder":"output","container":"wav","expect":{"duration_us":1,"sample_rate":RATE,"channels":1}}]),
            "outputs must be empty",
        ),
        ("/modality", json!("image"), "require audio modality"),
    ] {
        f.runner = runner.clone();
        f.workload = workload.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        f.workload.pointer_mut(parent).unwrap()[key] = value;
        f.reject(expected);
    }
    f.workload = workload.clone();
    f.workload["request"]
        .as_object_mut()
        .unwrap()
        .remove("sample_rate");
    f.reject("request: missing field `sample_rate`");
}

#[test]
fn speech_capture_measures_whole_wav_and_streamed_first_audio() {
    let server = speech_server();
    let mut f = speech_fixture(&server, "wav");
    f.runner["response_format"] = json!("wav");
    f.runner["stream"] = json!(false);
    f.runner.as_object_mut().unwrap().remove("pcm");
    let report = f.captured();
    assert_eq!(
        report["claim"],
        "collector-observed-http-reply-time-and-framing-checked-audio-not-attested-execution-or-quality"
    );
    assert_eq!(report["execution"], "delegates-to-resident-service");
    assert_eq!(report["cells"][0]["eligible"], 3);
    let run = &report["runs"][1];
    let reply = &run["reply"];
    assert!(run.get("process").is_none());
    assert_eq!(reply["http_status"], 200);
    assert!(reply["first_audio_us"].is_null());
    assert_eq!(
        reply["audio"],
        json!({"channels":1,"sample_rate":RATE,"duration_us":100_000})
    );
    assert_eq!(reply["body"]["bytes"], 1644);
    let body = fs::read(f.root.path("capture/run-000001/body.bin")).unwrap();
    assert_eq!(reply["body"]["sha256"], super::digest(&body));
    let end_to_end = reply["end_to_end_us"].as_u64().unwrap();
    assert_eq!(run["realtime_factor"], end_to_end as f64 / 100_000.0);
    let inspected = f.inspect_capture();
    successful(&inspected);
    assert_eq!(
        report,
        serde_json::from_slice::<Value>(&inspected.stdout).unwrap()
    );

    let f = speech_fixture(&server, "pcm");
    let report = f.captured();
    assert_eq!(report["cells"][0]["eligible"], 3);
    let reply = &report["runs"][1]["reply"];
    let first = reply["first_audio_us"].as_u64().unwrap();
    // The fixture sends headers at once and the first PCM byte 150 ms later.
    assert!(first >= 150_000, "{first}");
    assert!(first <= reply["end_to_end_us"].as_u64().unwrap());
    assert_eq!(reply["audio"]["duration_us"], 100_000);
    assert!(
        reply["headers"]
            .as_array()
            .unwrap()
            .contains(&json!(["x-finish-reason", "stop"]))
    );
    assert_eq!(report["cells"][0]["first_audio_us"]["n"], 3);
    successful(&f.inspect_capture());
}

#[test]
fn speech_failures_are_retained_ineligible_and_rechecked_offline() {
    let server = speech_server();
    let mut f = speech_fixture(&server, "status");
    let cells = ["status", "length", "partial", "slow"];
    f.workload["cells"] = cells
        .iter()
        .map(|input| json!({"id":input,"warmup_runs":0,"measured_runs":1,"request":{"prompt":input,"seeds":[7],"sample_rate":RATE}}))
        .collect();
    f.workload["gates"][0]["cell"] = json!("status");
    let report = f.captured();
    assert_eq!(report["status"], "complete");
    for (index, reason) in [
        "http_status",
        "finish_not_stop",
        "partial_frame",
        "deadline_exceeded",
    ]
    .into_iter()
    .enumerate()
    {
        let run = &report["runs"][index];
        assert_eq!(run["eligible"], false, "{run}");
        assert_eq!(run["reasons"], json!([reason]), "{run}");
        assert_eq!(report["cells"][index]["ineligible_by_reason"][reason], 1);
    }
    assert_eq!(report["runs"][0]["reply"]["http_status"], 422);
    assert_eq!(
        fs::read(f.root.path("capture/run-000000/body.bin")).unwrap(),
        br#"{"error":"refused"}"#
    );
    assert_eq!(report["runs"][1]["reply"]["audio"]["duration_us"], 100_000);
    assert_eq!(report["runs"][2]["reply"]["body"]["bytes"], 3);
    let slow = &report["runs"][3]["reply"];
    assert_eq!(slow["failure"], "deadline_exceeded");
    assert!(slow["end_to_end_us"].as_u64().unwrap() >= 500_000);
    assert!(slow["http_status"] == 200 && slow["body"]["bytes"] == 0);
    let inspected = f.inspect_capture();
    successful(&inspected);
    assert_eq!(
        report,
        serde_json::from_slice::<Value>(&inspected.stdout).unwrap()
    );

    let body = f.root.path("capture/run-000000/body.bin");
    let original = fs::read(&body).unwrap();
    fs::write(&body, b"{}").unwrap();
    inspection_rejected(&f, "retained reply body hash mismatch");
    fs::write(&body, original).unwrap();
    rewrite_evidence(&f, "run-000001/run.json", |run| {
        run["reply"]["headers"] = json!([["x-finish-reason", "stop"]]);
    });
    inspection_rejected(&f, "reply eligibility mismatch");
}

#[test]
fn speech_compare_gates_first_audio_and_two_sided_duration_ratio() {
    let server = speech_server();
    let mut f = speech_fixture(&server, "pcm");
    f.workload["cells"][0]["warmup_runs"] = json!(0);
    f.workload["gates"] = json!([
        {"cell":"c1","metric":"first_audio_us","max_adverse_bps":500,"max_reference_spread_bps":1000000},
        {"cell":"c1","metric":"duration_ratio","max_adverse_bps":500,"max_reference_spread_bps":0}
    ]);
    // A 300 ms first-audio delay against none keeps the verdict scheduler-proof.
    for (out, slot, voice) in [
        ("a", "a", "slow"),
        ("b", "b", "fast"),
        ("a2", "a2", "slow"),
        ("b-short", "b", "short"),
        ("a2-again", "a2", "slow"),
    ] {
        f.runner["voice"] = json!(voice);
        f.save();
        successful(
            &cli()
                .args(["media", "capture", "--workload"])
                .arg(f.root.path("workload.json"))
                .arg("--runner")
                .arg(f.root.path("runner.json"))
                .arg("--deployment")
                .arg(f.root.path("deployment.json"))
                .args(["--slot", slot, "--out"])
                .arg(f.root.path(out))
                .output()
                .unwrap(),
        );
    }
    let report = f.comparison(false, 0);
    assert_eq!(report["outcome"], "PASS");
    let [first_audio, ratio] = [&report["gates"][0], &report["gates"][1]];
    assert_eq!(first_audio["outcome"], "PASS");
    assert_eq!(first_audio["reference_summary"]["first_audio_us"]["n"], 6);
    assert_eq!(ratio["outcome"], "PASS");
    assert_eq!(
        ratio["candidate_summary"]["duration_ratio_bps"]["min"],
        10_000
    );
    assert_eq!(ratio["cross_arm_hash_equality"][0]["equal"], true);

    let output = cli()
        .args(["media", "compare", "--a"])
        .arg(f.root.path("a"))
        .arg("--b")
        .arg(f.root.path("b-short"))
        .arg("--a2")
        .arg(f.root.path("a2-again"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["gates"][0]["outcome"], "PASS");
    let ratio = &report["gates"][1];
    assert_eq!(ratio["outcome"], "REGRESSION");
    assert_eq!(
        ratio["candidate_summary"]["duration_ratio_bps"]["max"],
        5_000
    );
    assert_eq!(ratio["cross_arm_hash_equality"][0]["equal"], false);
}
