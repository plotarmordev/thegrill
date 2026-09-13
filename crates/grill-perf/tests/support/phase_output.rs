// Per-phase output budgets (workload version 3): the optional `warmup_output`
// override binds warmup wire controls, usage admission, offline evidence
// identity and raw ceilings, while measured controls stay declared by `output`.
use super::*;

fn version3(
    warmup: Option<(u32, &str)>,
    measured: (u32, &str),
    warmup_trials: u32,
    trials: u32,
) -> Value {
    let mut work = workload(1, warmup_trials, trials);
    work["version"] = json!(3);
    work["name"] = json!("phase-output-v3");
    let request = &mut work["request"];
    request["profile"] = json!("vllm-fixed-v1");
    request["output"] = json!({"tokens": measured.0, "mode": measured.1});
    if let Some((tokens, mode)) = warmup {
        request["warmup_output"] = json!({"tokens": tokens, "mode": mode});
    }
    work
}
fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON: {error}; status={}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
fn encoding(requests: &Value, lane: usize) -> Value {
    serde_json::from_str(requests["requests"][lane].as_str().unwrap()).unwrap()
}
fn inspect(temp: &Temp, name: &str, workload: &Value) -> Output {
    let path = temp.path(&format!("{name}.json"));
    fs::write(&path, serde_json::to_vec_pretty(workload).unwrap()).unwrap();
    cli()
        .args(["bundle", "inspect"])
        .arg(path)
        .output()
        .unwrap()
}
fn rejected(error: Output) -> String {
    assert_eq!(
        error.status.code(),
        Some(1),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&error.stdout),
        String::from_utf8_lossy(&error.stderr)
    );
    String::from_utf8(error.stderr).unwrap()
}

#[test]
fn phase_output_controls_render_per_phase_and_weight_raw_ceilings() {
    let temp = Temp::new();
    let server = Server::new(|mut stream, index, body| {
        let warmup = index == 0;
        assert_eq!(body["max_tokens"], json!(if warmup { 32 } else { 400 }));
        if warmup {
            assert!(body.get("min_tokens").is_none(), "{body}");
            assert!(body.get("ignore_eos").is_none(), "{body}");
        } else {
            assert_eq!(body["min_tokens"], json!(400));
            assert_eq!(body["ignore_eos"], json!(true));
        }
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"index":0,"delta":{"content":"1 2"}}]}),
        );
        finish(&mut stream, Some(if warmup { 32 } else { 400 }), Some(0));
    });
    let work = version3(Some((32, "cap")), (400, "exact"), 1, 3);
    let output = run(&temp, &server, "phase", &work);
    successful(&output);
    let summary = stdout_json(&output);
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["planned_waves"], 4);
    assert_eq!(summary["measured_waves"], 3);
    assert_eq!(summary["eligible_measured_waves"], 3);
    assert_eq!(summary["ineligible_warmup_waves"], 0);

    // Retained receipts pin the phase and the rendered controls that produced it.
    let warmup = value(temp.path("phase/wave-000000/reservation.json"));
    let warmup_body = encoding(&warmup, 0);
    assert_eq!(warmup_body["max_tokens"], 32);
    assert!(warmup_body.get("min_tokens").is_none());
    assert!(warmup_body.get("ignore_eos").is_none());
    let measured = value(temp.path("phase/wave-000001/reservation.json"));
    let measured_body = encoding(&measured, 0);
    assert_eq!(measured_body["max_tokens"], 400);
    assert_eq!(measured_body["min_tokens"], 400);
    assert_eq!(measured_body["ignore_eos"], true);
    assert_eq!(wave(&temp, "phase", 0)["spec"]["phase"], "warmup");
    assert_eq!(wave(&temp, "phase", 1)["spec"]["phase"], "measured");
    assert_eq!(wave(&temp, "phase", 0)["eligible"], true);
    assert_eq!(wave(&temp, "phase", 2)["eligible"], true);
    let plan = value(temp.path("phase/plan.json"));
    assert_eq!(plan["workload"]["request"]["warmup_output"]["tokens"], 32);
    assert_eq!(plan["workload"]["request"]["output"]["tokens"], 400);
    assert_eq!(plan["workload"]["request"]["output"]["mode"], "exact");

    // Offline reload and comparison consume phase-resolved evidence unchanged.
    let dispatched = server.count.load(Ordering::SeqCst);
    let replay = cli()
        .arg("compare")
        .arg(temp.path("phase"))
        .arg(temp.path("phase"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&replay);

    // Raw declared ceilings weight each phase by its own budget.
    let pins = stdout_json(&inspect(&temp, "phase", &work));
    assert_eq!(pins["warmup_requests"], 1);
    assert_eq!(pins["measured_requests"], 3);
    assert_eq!(pins["total_output_token_ceiling"], 32 + 3 * 400);
    let preflight = cli()
        .arg("preflight")
        .arg(temp.path("phase.json"))
        .args([
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture-model",
            "--local-http",
        ])
        .output()
        .unwrap();
    successful(&preflight);
    let report = stdout_json(&preflight);
    assert_eq!(report["warmup_requests"], 1);
    assert_eq!(report["measured_requests"], 3);
    assert_eq!(
        report["total_output_token_ceiling"],
        pins["total_output_token_ceiling"]
    );
    assert_eq!(report["request"]["warmup_output"]["tokens"], 32);
    assert_eq!(server.count.load(Ordering::SeqCst), dispatched);
}

#[test]
fn warmup_usage_is_admitted_against_the_phase_cap_not_measured() {
    let temp = Temp::new();
    let server = Server::new(|mut stream, _, body| {
        let warmup = body["max_tokens"] == 32;
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"index":0,"delta":{"content":"1 2"}}]}),
        );
        finish(&mut stream, Some(if warmup { 40 } else { 400 }), Some(0));
    });
    let output = run(
        &temp,
        &server,
        "over",
        &version3(Some((32, "cap")), (400, "exact"), 1, 3),
    );
    assert_eq!(output.status.code(), Some(2));
    let summary = stdout_json(&output);
    assert_eq!(summary["status"], "stopped-after-ineligible-response");
    let warmup = wave(&temp, "over", 0);
    assert_eq!(warmup["spec"]["phase"], "warmup");
    assert!(
        warmup["attempts"][0]["eligibility_errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error == "reported_output_exceeds_cap"),
        "{warmup}"
    );
    // 40 tokens is well inside the 400-token measured cap: only the active
    // warmup budget can have rejected it.
    assert!(!temp.path("over/wave-000001").exists());
}

#[test]
fn phase_output_admission_bounds_distinct_active_warmup() {
    let temp = Temp::new();
    let input = temp.path("boundary.json");
    let admit = |work: &Value| {
        fs::write(&input, serde_json::to_vec(work).unwrap()).unwrap();
        cli()
            .arg("preflight")
            .arg(&input)
            .args([
                "--endpoint",
                "http://127.0.0.1:1/v1/chat/completions",
                "--model",
                "fixture-model",
                "--local-http",
            ])
            .output()
            .unwrap()
    };
    for (concurrency, boundary) in [
        (1, "encoded request exceeds 2 MiB"),
        (17, "reservation receipt bound"),
    ] {
        let mut work = version3(Some((32, "cap")), (400, "cap"), 1, 1);
        work["cells"][0]["concurrency"] = json!(concurrency);
        work["limits"]["wave_buffer_bytes"] = json!(128 * 1024 * 1024);
        work["cases"][0]["messages"][0]["content"] = json!("{fill}");
        work["cases"][0]["fill"] = json!({"unit":"xxx","repeat":1});
        // Find the admitted edge through the CLI, without pinning serializer
        // overhead. C1 binds request size; C17 binds second-escaped receipts.
        let (mut low, mut high) = (1, 700_000);
        while low < high {
            let middle = (low + high + 1) / 2;
            work["cases"][0]["fill"]["repeat"] = json!(middle);
            if admit(&work).status.success() {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        work["cases"][0]["fill"]["repeat"] = json!(low);
        successful(&admit(&work));
        work["request"]["warmup_output"]["mode"] = json!("exact");
        let error = rejected(admit(&work));
        assert!(error.contains(boundary), "{error}");
        // The extra exact-output fields matter only for a dispatched phase.
        work["cells"][0]["warmup_trials"] = json!(0);
        successful(&admit(&work));
    }
}

#[test]
fn phase_output_override_schema_bounds_and_profile_are_enforced() {
    let temp = Temp::new();
    let mut legacy1 = workload(1, 1, 3);
    legacy1["request"]["warmup_output"] = json!({"tokens":32,"mode":"cap"});
    let error = rejected(inspect(&temp, "legacy1", &legacy1));
    assert!(error.contains("requires workload version 3"), "{error}");
    let mut legacy2 = legacy1.clone();
    legacy2["version"] = json!(2);
    let error = rejected(inspect(&temp, "legacy2", &legacy2));
    assert!(error.contains("requires workload version 3"), "{error}");
    for (name, version) in [("null-legacy", 1), ("null-current", 3)] {
        let mut work = if version == 3 {
            version3(None, (400, "exact"), 1, 1)
        } else {
            workload(1, 1, 1)
        };
        work["request"]["warmup_output"] = Value::Null;
        let error = rejected(inspect(&temp, name, &work));
        assert!(
            error.contains("null") && error.contains("OutputBudget"),
            "{error}"
        );
    }
    // The exact-mode profile restriction applies even to an unused warmup budget.
    let mut portable = version3(Some((32, "exact")), (8, "cap"), 0, 1);
    portable["request"]["profile"] = json!("portable-chat-v1");
    let error = rejected(inspect(&temp, "portable", &portable));
    assert!(error.contains("explicit vllm-fixed-v1"), "{error}");
    // Declared budgets are bounded even when no warmup wave is planned.
    for tokens in [0, 32_769] {
        let error = rejected(inspect(
            &temp,
            &format!("bound-{tokens}"),
            &version3(Some((tokens, "cap")), (400, "exact"), 0, 1),
        ));
        assert!(error.contains("invalid output budget"), "{error}");
    }
    // The bounds themselves stay inclusive.
    for tokens in [1, 32_768] {
        let output = inspect(
            &temp,
            &format!("accept-{tokens}"),
            &version3(Some((tokens, "cap")), (400, "exact"), 1, 1),
        );
        successful(&output);
        assert_eq!(
            stdout_json(&output)["request"]["warmup_output"]["tokens"],
            tokens
        );
    }
}

#[test]
fn warmup_only_change_is_incompatible_raw_evidence() {
    let temp = Temp::new();
    let server = Server::new(|mut stream, index, body| {
        let tokens = if index == 0 { 32 } else { 400 };
        assert_eq!(body["max_tokens"], json!(tokens));
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"index":0,"delta":{"content":"1 2"}}]}),
        );
        finish(&mut stream, Some(tokens), Some(0));
    });
    let phased = version3(Some((32, "cap")), (400, "exact"), 1, 3);
    successful(&run(&temp, &server, "a", &phased));
    let server = Server::new(|mut stream, _, body| {
        assert_eq!(body["max_tokens"], 400);
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"index":0,"delta":{"content":"1 2"}}]}),
        );
        finish(&mut stream, Some(400), Some(0));
    });
    successful(&run(
        &temp,
        &server,
        "b",
        &version3(None, (400, "exact"), 1, 3),
    ));
    let a = value(temp.path("a/plan.json"));
    let b = value(temp.path("b/plan.json"));
    assert!(a["workload"]["request"]["warmup_output"].is_object());
    assert!(b["workload"]["request"].get("warmup_output").is_none());
    assert_ne!(a["workload_sha256"], b["workload_sha256"]);
    // Measured rendering is identical; only the warmup request moved.
    assert_eq!(
        value(temp.path("a/wave-000001/reservation.json"))["requests"],
        value(temp.path("b/wave-000001/reservation.json"))["requests"]
    );
    assert_ne!(
        value(temp.path("a/wave-000000/reservation.json"))["requests"],
        value(temp.path("b/wave-000000/reservation.json"))["requests"]
    );
    let rejected = cli()
        .arg("compare")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("incompatible"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
}

#[test]
fn workload_version_three_requires_native_plan_version_three() {
    let temp = Temp::new();
    let server = Server::new(|mut stream, index, _| {
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"choices":[{"index":0,"delta":{"content":"1 2"}}]}),
        );
        finish(
            &mut stream,
            Some(if index == 0 { 32 } else { 400 }),
            Some(0),
        );
    });
    successful(&run(
        &temp,
        &server,
        "legacy",
        &version3(Some((32, "cap")), (400, "exact"), 1, 1),
    ));
    let path = temp.path("legacy/plan.json");
    let mut plan = value(&path);
    plan["version"] = json!(2);
    plan.as_object_mut().unwrap().remove("metric_contract");
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let output = cli()
        .arg("compare")
        .arg(temp.path("legacy"))
        .arg(temp.path("legacy"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("workload version 3 requires native performance plan version 3"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
