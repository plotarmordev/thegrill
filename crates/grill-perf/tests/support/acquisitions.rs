use super::*;
use sha2::{Digest, Sha256};

fn work() -> Value {
    let mut w = workload(1, 0, 1);
    w["version"] = json!(6);
    w["request"]["profile"] = json!("vllm-conversation-v3");
    w["request"]["seed"] = json!(0);
    w["acquisition"] = json!({"kind":"conversation","repetitions":3,"warmup_repetitions":1,
        "measured_steps":["follow"],"input_bytes":2097152,"retained_history_bytes":1048576});
    w["cases"] = json!([
        {"id":"prime","messages":[{"role":"user","content":"Remember sapphire."}],"step":{"history":"h","parent":null,"cache":"observe","expect":{"kind":"json","value":{"fact":"sapphire"},"strict":null}}},
        {"id":"follow","messages":[{"role":"user","content":"Repeat the fact."}],"step":{"history":"h","parent":"prime","cache":"observe","expect":{"kind":"json","value":{"fact":"sapphire"},"strict":null}}}
    ]);
    w["cells"] = json!([
        {"id":"prime-cell","case":"prime","concurrency":1,"warmup_trials":0,"trials":1},
        {"id":"follow-cell","case":"follow","concurrency":1,"warmup_trials":0,"trials":1}
    ]);
    w
}
fn response(mut stream: TcpStream, content: &str) {
    header(&mut stream, "text/event-stream");
    frame(
        &mut stream,
        json!({"choices":[{"delta":{"content":content}}]}),
    );
    finish(&mut stream, Some(8), None);
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
fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn acquisition6_resets_whole_histories_keeps_actual_parents_and_replays_wall_clock() {
    let temp = Temp::new();
    let server = Server::new(|stream, index, request| {
        let messages = request["messages"].as_array().unwrap();
        if index % 2 == 0 {
            assert_eq!(messages.len(), 1);
        } else {
            assert_eq!(messages[1]["content"], " {\"fact\":\"sapphire\"} ");
        }
        response(stream, " {\"fact\":\"sapphire\"} ");
    });
    successful(&run(&temp, &server, "run", &work()));
    assert_eq!(server.count.load(Ordering::SeqCst), 8);
    let plan = read(&temp.path("run/plan.json"));
    let original = plan["cache_namespace"].as_str().unwrap();
    for repetition in 0..4 {
        let phase = if repetition == 0 {
            "warmup"
        } else {
            "measured"
        };
        let index = if repetition == 0 { 0 } else { repetition - 1 };
        let expected = format!(
            "{}-h",
            Sha256::digest(format!("grill-acquisition-v1:{original}:{phase}:{index}"))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        for step in 0..2 {
            let wave = repetition * 2 + step;
            let reservation = read(&temp.path(&format!("run/wave-{wave:06}/reservation.json")));
            let body: Value =
                serde_json::from_str(reservation["requests"][0].as_str().unwrap()).unwrap();
            assert_eq!(body["cache_salt"], expected);
            assert_eq!(reservation["wave"]["phase"], phase);
            assert_eq!(
                reservation["wave"]["acquisition"],
                json!({"phase":phase,"index":index})
            );
        }
    }
    let out = replay(&temp, "run");
    successful(&out);
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let records = report["acquisition"]["baseline"]["records"]
        .as_array()
        .unwrap();
    assert_eq!(records.len(), 4);
    for record in records {
        assert_eq!(record["complete_eligible"], true);
        assert_eq!(record["measured_steps"], json!(["follow"]));
        assert_eq!(
            record["whole_conversation_wall_us"].as_u64().unwrap(),
            record["settled_offset_us"].as_u64().unwrap()
                - record["started_offset_us"].as_u64().unwrap()
        );
    }
    assert!(
        !cli()
            .arg("resume")
            .arg(temp.path("run"))
            .output()
            .unwrap()
            .status
            .success()
    );
    let path = temp.path("run/wave-000003/wave.json");
    let mut forged = read(&path);
    forged["acquisition_clock"]["settled_offset_us"] = json!(0);
    fs::write(path, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(!replay(&temp, "run").status.success());
}

#[test]
fn acquisition6_failed_warmup_or_required_control_stops_without_replacement() {
    for failing in [0, 2] {
        let temp = Temp::new();
        let server = Server::new(move |stream, index, _| {
            response(
                stream,
                if index == failing {
                    "{\"fact\":\"wrong\"}"
                } else {
                    "{\"fact\":\"sapphire\"}"
                },
            )
        });
        let out = run(&temp, &server, "run", &work());
        assert!(!out.status.success());
        assert_eq!(server.count.load(Ordering::SeqCst), failing + 1);
        let out = replay(&temp, "run");
        assert_eq!(out.status.code(), Some(2));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        let record = &report["acquisition"]["baseline"]["records"][failing / 2];
        assert_eq!(record["complete_eligible"], false);
        assert_eq!(record["ineligible_steps"], json!(["prime"]));
        assert_eq!(record["missing_steps"], json!(["follow"]));
        assert!(record["whole_conversation_wall_us"].is_null());
    }
}

#[test]
fn acquisition6_encoded_parent_growth_retains_and_replays_admission_failure() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| {
        response(
            stream,
            &format!("{}{{\"fact\":\"sapphire\"}}", " ".repeat(6000)),
        )
    });
    let mut w = work();
    w["acquisition"]["input_bytes"] = json!(4096);
    let out = run(&temp, &server, "run", &w);
    assert!(!out.status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    let failure = read(&temp.path("run/acquisition-failure.json"));
    assert_eq!(failure["wave"]["case"], "follow");
    assert!(!temp.path("run/wave-000001").exists());
    assert_eq!(replay(&temp, "run").status.code(), Some(2));
}

#[test]
fn acquisition6_history_budget_failure_keeps_observed_response() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| {
        response(
            stream,
            &format!("{}{{\"fact\":\"sapphire\"}}", " ".repeat(6000)),
        )
    });
    let mut w = work();
    w["acquisition"]["retained_history_bytes"] = json!(4096);
    assert!(!run(&temp, &server, "run", &w).status.success());
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    let wave = read(&temp.path("run/wave-000000/wave.json"));
    assert!(wave["attempts"][0]["response_bytes"].as_u64().unwrap() > 6000);
    assert_eq!(wave["attempts"][0]["sequence"]["correct"], true);
    assert!(wave["attempts"][0]["sequence"]["error"].is_string());
    assert_eq!(replay(&temp, "run").status.code(), Some(2));
}

#[test]
fn acquisition6_native_policy3_runs_declared_roles_and_reports_step_whole_and_tail() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| response(stream, "{\"fact\":\"sapphire\"}"));
    let w = work();
    let source = serde_json::to_vec(&w).unwrap();
    fs::write(temp.path("work.json"), &source).unwrap();
    fs::write(
        temp.path("deployment.json"),
        serde_json::to_vec(&deployment()).unwrap(),
    )
    .unwrap();
    let threshold = json!({"max_regression_bps":9999,"max_reference_spread_bps":1000000});
    let policy = json!({"version":3,"method":"observed-envelope-v3","id":"acquisition-fixture",
        "collector_sha256":Sha256::digest(fs::read(env!("CARGO_BIN_EXE_grill-perf")).unwrap()).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        "workload_source_sha256":Sha256::digest(&source).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),"min_trials":3,
        "cells":[{"cell":"follow-cell","metrics":[{"metric":"completion_latency_us","max_regression_bps":9999,"max_reference_spread_bps":1000000}]}],
        "whole_conversation":threshold,
        "tail":[{"target":{"kind":"whole_conversation"},"percentile":"p95","max_regression_bps":9999,"max_reference_spread_bps":1000000}]});
    fs::write(
        temp.path("policy.json"),
        serde_json::to_vec(&policy).unwrap(),
    )
    .unwrap();
    for name in ["a", "b", "a2"] {
        successful(
            &cli()
                .arg("run")
                .arg(temp.path("work.json"))
                .args([
                    "--endpoint",
                    &server.endpoint,
                    "--model",
                    "fixture-model",
                    "--local-http",
                    "--json",
                ])
                .arg("--deployment")
                .arg(temp.path("deployment.json"))
                .arg("--policy")
                .arg(temp.path("policy.json"))
                .arg("--out")
                .arg(temp.path(name))
                .output()
                .unwrap(),
        );
    }
    let out = cli()
        .arg("decide")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--reference")
        .arg(temp.path("a2"))
        .arg("--json")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["version"], 3);
    assert_ne!(report["decision"], "ERROR");
    assert_eq!(
        report["gates"][0]["coverage"]["baseline"]["expected_waves"],
        3
    );
    assert_eq!(
        report["whole_conversation"]["coverage"]["candidate"]["observed_observations"],
        3
    );
    assert_eq!(report["tail"][0]["decision"], "INCONCLUSIVE");
    assert!(report["tail"][0]["ranges"]["candidate"].is_null());
    assert_eq!(server.count.load(Ordering::SeqCst), 24);
}

/// Serves `long-context-recall-v1`, answering each step with the code its own
/// prompt records, or a wrong code at request `wrong`. Records each request's
/// filler length and whether the record precedes the filler.
fn recall(temp: &Temp, name: &str, wrong: Option<usize>) -> (Output, Vec<(usize, bool)>) {
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = requests.clone();
    let server = Server::new(move |stream, index, request| {
        assert_eq!(request["max_tokens"], 32);
        assert_eq!(
            request["chat_template_kwargs"],
            json!({"thinking":false,"enable_thinking":false})
        );
        let user = request["messages"][1]["content"].as_str().unwrap();
        let record = user.find("Record: the access code for vault ").unwrap();
        let (vault, code) = user[record + "Record: the access code for vault ".len()..]
            .split_once(".\n")
            .unwrap()
            .0
            .split_once(" is ")
            .unwrap();
        assert!(user.ends_with(&format!("What is the access code for vault {vault}?")));
        let filler = user
            .lines()
            .find(|line| line.starts_with(" the the"))
            .unwrap();
        assert_eq!(filler, " the".repeat(filler.len() / 4));
        let before = record < user.find(filler).unwrap();
        seen.lock().unwrap().push((filler.len() / 4, before));
        let answer = if wrong == Some(index) {
            "XX0-0000-XX"
        } else {
            code
        };
        response(stream, &json!({ "fact": answer }).to_string());
    });
    let workload: Value =
        serde_json::from_str(include_str!("../../examples/long-context-recall-v1.json")).unwrap();
    let output = run(temp, &server, name, &workload);
    assert_eq!(
        server.count.load(Ordering::SeqCst),
        requests.lock().unwrap().len()
    );
    let requests = requests.lock().unwrap().clone();
    (output, requests)
}

#[test]
fn long_context_recall_grades_each_code_at_its_depth_and_stops_on_a_miss() {
    let temp = Temp::new();
    let (output, requests) = recall(&temp, "recall", None);
    successful(&output);
    let steps = [
        (16_384, false),
        (16_384, true),
        (49_152, false),
        (49_152, true),
        (98_304, false),
        (98_304, true),
    ];
    assert_eq!(requests, steps.repeat(3));

    let (output, requests) = recall(&temp, "missed", Some(5));
    assert!(!output.status.success());
    assert_eq!(requests.len(), 6);
    let run: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(run["status"], "stopped-after-sequence-check", "{run}");
}
