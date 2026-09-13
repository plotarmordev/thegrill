use super::*;

fn inputs(temp: &Temp, variant: bool) -> PathBuf {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let name = if variant {
        "concurrency-enable-thinking-selection-v1.json"
    } else {
        "concurrency-selection-v1.json"
    };
    let manifest = read_json(&examples.join(name));
    let workload = manifest["workload"].as_str().unwrap();
    fs::copy(examples.join(workload), temp.path(workload)).unwrap();
    fs::copy(examples.join(name), temp.path("selection.json")).unwrap();
    temp.path("selection.json")
}

fn selected_baseline(
    temp: &Temp,
    endpoint: &str,
    declaration: &Path,
    selection: &Path,
    name: &str,
) -> Command {
    let mut cli = command();
    cli.current_dir(&temp.0)
        .args([
            "baseline",
            "--endpoint",
            endpoint,
            "--model",
            "neutral-fixture",
            "--local-http",
            "--json",
        ])
        .arg("--deployment")
        .arg(declaration)
        .arg("--selection")
        .arg(selection)
        .arg("--out")
        .arg(temp.path(name));
    cli
}

struct AuthServer {
    endpoint: String,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl AuthServer {
    fn new() -> Self {
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
                        assert!(headers.lines().any(|line| line.eq_ignore_ascii_case(
                            "authorization: Bearer selection-fixture-secret"
                        )));
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
                        assert_eq!(body["model"], "neutral-fixture");
                        assert_eq!(body["chat_template_kwargs"], json!({"thinking":false}));
                        assert_eq!(body["max_tokens"], 64);
                        assert_eq!(body["min_tokens"], 64);
                        assert_eq!(body["ignore_eos"], true);
                        assert_eq!(
                            body["messages"][0]["content"],
                            "Count from 1 to 32. Output only the numbers, separated by spaces. No other text."
                        );
                        seen.fetch_add(1, Ordering::SeqCst);
                        response(stream, Some(64), false);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
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
impl Drop for AuthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Err(error) = self.join.take().unwrap().join()
            && !thread::panicking()
        {
            std::panic::resume_unwind(error);
        }
    }
}

fn assert_no_secret(root: &Path) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            assert_no_secret(&entry.path());
        } else {
            let bytes = fs::read(entry.path()).unwrap();
            assert!(
                !bytes
                    .windows(b"selection-fixture-secret".len())
                    .any(|s| s == b"selection-fixture-secret")
            );
        }
    }
}

#[test]
fn selected_cli_inherits_pinned_workload_and_auth_outside_checkout_and_replays_offline() {
    let temp = Temp::new();
    let selection = inputs(&temp, false);
    let declaration = deployment(&temp, "serving.json", "unchanged");
    let server = AuthServer::new();
    let before = selected_baseline(&temp, &server.endpoint, &declaration, &selection, "before")
        .args(["--auth-env", "GRILL_SELECTION_TEST_KEY"])
        .env("GRILL_SELECTION_TEST_KEY", "selection-fixture-secret")
        .output()
        .unwrap();
    let baseline = decoded(&before);
    assert!(before.status.success(), "{baseline}");
    assert_eq!(baseline["baseline_ready"], true);
    assert_eq!(baseline["baseline_complete_acquisitions"], 8);
    let scope = baseline["scope"].as_str().unwrap();
    assert!(scope.contains("Explicit selected workload"), "{scope}");
    assert!(!scope.contains("unavailable"), "{scope}");
    assert_eq!(baseline["baseline_accounting"]["dispatched_requests"], 224);
    assert_eq!(
        baseline["baseline_accounting"]["output_token_ceiling"],
        14336
    );
    assert_eq!(
        baseline["selected"]["manifest"]["operation_scope"],
        "unknown"
    );
    assert_eq!(read_json(&temp.path("before/capture.json"))["version"], 2);
    let pinned = fs::read(temp.path("before/selection.json")).unwrap();
    fs::remove_file(selection).unwrap();
    fs::remove_file(temp.path("concurrency-v1.json")).unwrap();
    let control = command()
        .current_dir(&temp.0)
        .arg("check")
        .arg(temp.path("before"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--out"])
        .arg(temp.path("control"))
        .env("GRILL_SELECTION_TEST_KEY", "selection-fixture-secret")
        .output()
        .unwrap();
    assert_eq!(control.status.code(), Some(0));
    let terminal = String::from_utf8_lossy(&control.stdout);
    assert!(
        terminal
            .lines()
            .next()
            .unwrap()
            .contains("DESCRIPTIVE ONLY")
    );
    assert!(!terminal.lines().next().unwrap().contains("INCONCLUSIVE"));
    let report = read_json(&temp.path("control/report.json"));
    assert_eq!(
        terminal.lines().nth(1).unwrap(),
        format!(
            "Scope: {}",
            report["selected"]["manifest"]["scope"].as_str().unwrap()
        )
    );
    assert_eq!(report["result"], "DESCRIPTIVE");
    assert_eq!(report["declared_change"], "none");
    assert_eq!(report["candidate_complete_acquisitions"], 8);
    assert_eq!(server.count.load(Ordering::SeqCst), 448);
    assert_eq!(
        fs::read(temp.path("control/selection.json")).unwrap(),
        pinned
    );
    assert!(report["observed_change_percent"].is_null());
    assert!(report["model_based_interval_percent"].is_null());
    assert_eq!(report["baseline_acquisition_medians"], json!([]));
    for side in ["baseline_acquisitions", "candidate_acquisitions"] {
        let acquisitions = report["selected"][side].as_array().unwrap();
        assert_eq!(acquisitions.len(), 8);
        for acquisition in acquisitions {
            let cells = acquisition["cells"].as_array().unwrap();
            assert_eq!(
                cells
                    .iter()
                    .map(|c| c["cell"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["structured-1", "structured-2", "structured-4"]
            );
            for cell in cells {
                assert_eq!(cell["eligible_trials"], 3);
                assert!(
                    cell["median_achieved_completion_tokens_per_second"]
                        .as_f64()
                        .unwrap()
                        > 0.0
                );
                assert!(cell["median_text_decode_tokens_per_second"].is_null());
            }
            for wave in acquisition["waves"].as_array().unwrap() {
                assert_eq!(
                    wave["attempts"].as_array().unwrap().len() as u64,
                    wave["spec"]["concurrency"].as_u64().unwrap()
                );
                assert!(wave["dispatch_spread_us"].as_u64().is_some());
                for lane in wave["attempts"].as_array().unwrap() {
                    assert_eq!(lane["status"], "complete");
                    assert!(lane["timing"]["first_generated_text_us"].as_u64().is_some());
                    assert!(lane["timing"]["settle_us"].as_u64().is_some());
                }
            }
        }
    }
    assert!(!String::from_utf8_lossy(&control.stdout).contains("selection-fixture-secret"));
    assert!(!String::from_utf8_lossy(&control.stderr).contains("selection-fixture-secret"));
    assert_no_secret(&temp.path("before"));
    assert_no_secret(&temp.path("control"));
    drop(server);
    let replay = command()
        .current_dir(&temp.0)
        .arg("compare")
        .arg(temp.path("before"))
        .arg(temp.path("control"))
        .arg("--json")
        .env_remove("GRILL_SELECTION_TEST_KEY")
        .output()
        .unwrap();
    assert_eq!(decoded(&replay), report);
    assert_eq!(read_json(&temp.path("control/report.json")), report);
    let terminal_replay = command()
        .current_dir(&temp.0)
        .arg("compare")
        .arg(temp.path("before"))
        .arg(temp.path("control"))
        .env_remove("GRILL_SELECTION_TEST_KEY")
        .output()
        .unwrap();
    assert!(terminal_replay.status.success());
    assert!(
        String::from_utf8_lossy(&terminal_replay.stdout)
            .lines()
            .next()
            .unwrap()
            .contains("DESCRIPTIVE ONLY")
    );
    let mut changed_selection = read_json(&temp.path("before/selection.json"));
    changed_selection["operation_scope"] = json!("normal");
    write_json(&temp.path("before/selection.json"), &changed_selection);
    let rejected = command()
        .arg("check")
        .arg(temp.path("before"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--json", "--out"])
        .arg(temp.path("drift"))
        .env_remove("GRILL_SELECTION_TEST_KEY")
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    let rejected = decoded(&rejected);
    assert_eq!(rejected["result"], "INVALID");
    assert!(rejected.get("selected").is_none(), "{rejected}");
    assert!(rejected["baseline_accounting"].is_null(), "{rejected}");
    assert_eq!(rejected["baseline_complete_acquisitions"], 0);
    assert!(
        rejected["scope"].as_str().unwrap().contains("unavailable"),
        "{rejected}"
    );
    assert!(!temp.path("drift/acquisition-00").exists());
}

#[test]
fn selected_preflight_rejects_source_normalized_control_and_path_drift_without_requests() {
    let temp = Temp::new();
    let selection = inputs(&temp, false);
    let original = read_json(&selection);
    let source_path = temp.path("concurrency-v1.json");
    let original_source = fs::read(&source_path).unwrap();
    let server = Server::new(|stream, _, _| response(stream, Some(64), false));
    let declaration = deployment(&temp, "serving.json", "before");
    for kind in [
        "source",
        "normalized",
        "controls",
        "path",
        "scope",
        "unknown-field",
    ] {
        let mut manifest = original.clone();
        fs::write(&source_path, &original_source).unwrap();
        match kind {
            "source" => {
                fs::write(&source_path, [original_source.as_slice(), b"\n"].concat()).unwrap();
            }
            "normalized" => manifest["workload_sha256"] = json!(hash(b"wrong normalized identity")),
            "controls" => {
                let mut source = read_json(&source_path);
                source["request"]["profile"] = json!("portable-chat-v1");
                write_json(&source_path, &source);
                manifest["source_sha256"] = json!(file_hash(&source_path));
            }
            "path" => manifest["workload"] = json!("../concurrency-v1.json"),
            "scope" => manifest["operation_scope"] = json!("safe-from-model-name"),
            "unknown-field" => manifest["provider"] = json!("lookup-forbidden"),
            _ => unreachable!(),
        }
        write_json(&selection, &manifest);
        let output = selected_baseline(&temp, &server.endpoint, &declaration, &selection, kind)
            .output()
            .unwrap();
        assert_eq!(decoded(&output)["result"], "INVALID", "{kind}");
        assert!(!temp.path(&format!("{kind}/acquisition-00")).exists());
        assert_eq!(server.count.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn selected_partial_concurrent_wave_retains_failed_lane_and_withholds_whole_cell() {
    let temp = Temp::new();
    let selection = inputs(&temp, true);
    let declaration = deployment(&temp, "serving.json", "before");
    let server = Server::new(|stream, index, body| {
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking":false})
        );
        response(stream, if index == 2 { None } else { Some(64) }, false);
    });
    let output = selected_baseline(&temp, &server.endpoint, &declaration, &selection, "partial")
        .output()
        .unwrap();
    let report = decoded(&output);
    assert_eq!(report["result"], "INVALID");
    assert_eq!(report["baseline_ready"], false);
    assert_eq!(server.count.load(Ordering::SeqCst), 3);
    assert_eq!(
        report["baseline_accounting"]["missing_completion_usage_requests"],
        1
    );
    let acquisition = &report["selected"]["baseline_acquisitions"][0];
    let wave = &acquisition["waves"][1];
    assert_eq!(wave["spec"]["concurrency"], 2);
    assert_eq!(wave["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(wave["eligible"], false);
    assert!(wave["achieved_completion_tokens_per_second"].is_null());
    for cell in acquisition["cells"].as_array().unwrap() {
        assert!(cell["median_achieved_completion_tokens_per_second"].is_null());
    }
    assert!(!temp.path("partial/acquisition-00/wave-000002").exists());
    assert!(
        temp.path("partial/acquisition-00/wave-000001/response-0001.bin")
            .is_file()
    );
}

#[test]
fn selected_budget_exhaustion_remains_incomplete_without_replacement() {
    let temp = Temp::new();
    let selection = inputs(&temp, false);
    let declaration = deployment(&temp, "serving.json", "before");
    let server = Server::new(|mut stream, _, _| {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        thread::sleep(Duration::from_secs(2));
    });
    let output = selected_baseline(&temp, &server.endpoint, &declaration, &selection, "bounded")
        .args(["--seconds", "1"])
        .output()
        .unwrap();
    let report = decoded(&output);
    assert_eq!(report["result"], "INCONCLUSIVE", "{report}");
    assert_eq!(report["baseline_ready"], false);
    assert!(report["model_based_interval_percent"].is_null());
    assert!(
        report["selected"]["baseline_timing"]["elapsed_through_capture_publication_us"]
            .as_u64()
            .unwrap()
            >= 1_000_000
    );
    assert!(server.count.load(Ordering::SeqCst) <= 1);
    let count = server.count.load(Ordering::SeqCst);
    let check = command()
        .arg("check")
        .arg(temp.path("bounded"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--json", "--out"])
        .arg(temp.path("candidate"))
        .output()
        .unwrap();
    assert_eq!(decoded(&check)["result"], "INCONCLUSIVE");
    assert_eq!(server.count.load(Ordering::SeqCst), count);
}

#[test]
fn explicit_c1_selection_never_uses_default_inference() {
    let temp = Temp::new();
    let source = include_bytes!("../../examples/baseline-v1.json");
    fs::write(temp.path("workload.json"), source).unwrap();
    write_json(
        &temp.path("selection.json"),
        &json!({
            "version":1,"id":"explicit-c1","workload":"workload.json",
            "source_sha256":hash(source),
            "workload_sha256":"d3677869d36f139a4f3907970553d3c09802041a5b995808c261bba04c13c052",
            "scope":"Explicit C1 descriptive control", "operation_scope":"normal"
        }),
    );
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let declaration = deployment(&temp, "serving.json", "same");
    let before = selected_baseline(
        &temp,
        &server.endpoint,
        &declaration,
        &temp.path("selection.json"),
        "before",
    )
    .output()
    .unwrap();
    assert!(before.status.success(), "{}", decoded(&before));
    let output = command()
        .arg("check")
        .arg(temp.path("before"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--json", "--out"])
        .arg(temp.path("control"))
        .output()
        .unwrap();
    let report = decoded(&output);
    assert_eq!(report["result"], "DESCRIPTIVE");
    assert!(output.status.success());
    assert_eq!(report["candidate_complete_acquisitions"], 8);
    assert!(report["observed_change_percent"].is_null());
    assert!(report["model_based_confidence_level"].is_null());
    assert_eq!(report["candidate_acquisition_medians"], json!([]));
    let original_selection = fs::read(temp.path("control/selection.json")).unwrap();
    let original_capture = fs::read(temp.path("control/capture.json")).unwrap();
    let original_timing = fs::read(temp.path("control/capture-timing.json")).unwrap();
    let mut selection = read_json(&temp.path("control/selection.json"));
    selection["operation_scope"] = json!("stress");
    write_json(&temp.path("control/selection.json"), &selection);
    let mut capture = read_json(&temp.path("control/capture.json"));
    capture["selection_sha256"] = json!(file_hash(&temp.path("control/selection.json")));
    write_json(&temp.path("control/capture.json"), &capture);
    let mut timing = read_json(&temp.path("control/capture-timing.json"));
    timing["capture_sha256"] = json!(file_hash(&temp.path("control/capture.json")));
    write_json(&temp.path("control/capture-timing.json"), &timing);
    let incompatible = compare(&temp.path("before"), &temp.path("control"));
    assert_eq!(decoded(&incompatible)["result"], "INVALID");
    assert!(decoded(&incompatible)["model_based_interval_percent"].is_null());
    fs::write(temp.path("control/selection.json"), original_selection).unwrap();
    fs::write(temp.path("control/capture.json"), original_capture).unwrap();
    fs::write(temp.path("control/capture-timing.json"), original_timing).unwrap();

    let original_baseline_timing = fs::read(temp.path("before/capture-timing.json")).unwrap();
    let mut changed_timing = read_json(&temp.path("before/capture-timing.json"));
    let elapsed = changed_timing["elapsed_through_capture_publication_us"]
        .as_u64()
        .unwrap();
    changed_timing["elapsed_through_capture_publication_us"] = json!(elapsed + 1);
    write_json(&temp.path("before/capture-timing.json"), &changed_timing);
    assert_eq!(
        decoded(&compare(&temp.path("before"), &temp.path("control")))["result"],
        "INVALID"
    );
    changed_timing["elapsed_through_capture_publication_us"] = json!(0);
    write_json(&temp.path("before/capture-timing.json"), &changed_timing);
    let count = server.count.load(Ordering::SeqCst);
    let impossible = command()
        .arg("check")
        .arg(temp.path("before"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--json", "--out"])
        .arg(temp.path("after-impossible-timing"))
        .output()
        .unwrap();
    assert_eq!(decoded(&impossible)["result"], "INVALID");
    assert_eq!(server.count.load(Ordering::SeqCst), count);
    fs::write(
        temp.path("before/capture-timing.json"),
        original_baseline_timing,
    )
    .unwrap();

    // A synthetic post-collection publication overrun cannot authorize candidate traffic.
    let mut timing = read_json(&temp.path("before/capture-timing.json"));
    timing["elapsed_through_capture_publication_us"] = json!(300_000_000u64);
    write_json(&temp.path("before/capture-timing.json"), &timing);
    let count = server.count.load(Ordering::SeqCst);
    let overrun = command()
        .arg("check")
        .arg(temp.path("before"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--json", "--out"])
        .arg(temp.path("after-overrun"))
        .output()
        .unwrap();
    assert_eq!(decoded(&overrun)["result"], "INCONCLUSIVE");
    assert_eq!(decoded(&overrun)["baseline_complete_acquisitions"], 8);
    assert_eq!(server.count.load(Ordering::SeqCst), count);
}

/// Sum of the retained native wave makespans for one acquisition directory.
fn acquisition_waves_us(root: &Path, index: usize) -> u64 {
    let mut total = 0u64;
    for entry in fs::read_dir(root.join(format!("acquisition-{index:02}"))).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("wave-"))
        {
            total += read_json(&path.join("wave.json"))["elapsed_us"]
                .as_u64()
                .unwrap();
        }
    }
    total
}

/// Shorten one declared acquisition below its retained native wave duration. Native receipts stay
/// untouched; only the capture declaration and the timing receipt that binds it change.
fn shorten_declared_finish(root: &Path, index: usize) -> (u64, u64) {
    let waves_us = acquisition_waves_us(root, index);
    assert!(
        waves_us > 1_000,
        "paced fixture must retain more than 1ms of native wave duration: {waves_us}us"
    );
    let mut capture = read_json(&root.join("capture.json"));
    let started = capture["acquisitions"][index]["started_unix_ms"]
        .as_u64()
        .unwrap();
    let finished = started + (waves_us - 1) / 1_000 - 1;
    let window_us = (waves_us - 1) / 1_000 * 1_000;
    capture["acquisitions"][index]["finished_unix_ms"] = json!(finished);
    write_json(&root.join("capture.json"), &capture);
    let mut timing = read_json(&root.join("capture-timing.json"));
    timing["capture_sha256"] = json!(file_hash(&root.join("capture.json")));
    write_json(&root.join("capture-timing.json"), &timing);
    (window_us, waves_us)
}

/// A verified load failure establishes no scope, accounting, selection, or directional verdict.
fn assert_unavailable_load_failure(report: &Value, window_us: u64, waves_us: u64) {
    assert_eq!(report["result"], "INVALID", "{report}");
    assert_eq!(report["baseline_ready"], false);
    assert!(report.get("selected").is_none(), "{report}");
    assert!(report["baseline_accounting"].is_null(), "{report}");
    assert!(report["candidate_accounting"].is_null(), "{report}");
    assert_eq!(report["baseline_complete_acquisitions"], 0);
    assert_eq!(report["candidate_complete_acquisitions"], 0);
    assert_eq!(report["baseline_acquisition_medians"], json!([]));
    assert_eq!(report["candidate_acquisition_medians"], json!([]));
    assert!(report["observed_change_percent"].is_null());
    assert!(report["model_based_interval_percent"].is_null());
    assert!(report["model_based_confidence_level"].is_null());
    assert!(report["first_failure"].is_null(), "{report}");
    let scope = report["scope"].as_str().unwrap();
    assert!(scope.contains("unavailable"), "{scope}");
    assert!(!scope.contains("One short synthetic structured"));
    assert!(!scope.contains("Explicit selected workload"));
    let reasons = report["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reason| reason.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        reasons.contains("native wave durations exceed the declared acquisition time window"),
        "{reasons}"
    );
    assert!(reasons.contains("acquisition-03"), "{reasons}");
    assert!(
        reasons.contains(&format!("waves_us={waves_us}")),
        "{reasons}"
    );
    assert!(
        reasons.contains(&format!("window_us={window_us}")),
        "{reasons}"
    );
}

#[test]
fn selected_window_overrun_replays_as_unavailable_without_partial_selection_or_verdict() {
    let temp = Temp::new();
    let selection = inputs(&temp, true);
    let declaration = deployment(&temp, "serving.json", "before");
    let server = Server::new(|stream, _, body| {
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
        // Bounded pacing keeps retained wave duration above the declaration allowance.
        thread::sleep(Duration::from_micros(500));
        response(stream, Some(64), false);
    });
    let output = selected_baseline(&temp, &server.endpoint, &declaration, &selection, "before")
        .output()
        .unwrap();
    let collected = decoded(&output);
    assert!(output.status.success(), "{collected}");
    assert_eq!(collected["baseline_ready"], true);
    assert_eq!(collected["baseline_complete_acquisitions"], 8);
    drop(server);

    let pristine_report = fs::read(temp.path("before/report.json")).unwrap();
    let pristine_text = fs::read(temp.path("before/report.txt")).unwrap();
    copy_tree(&temp.path("before"), &temp.path("clean"));
    let (window_us, waves_us) = shorten_declared_finish(&temp.path("before"), 3);
    assert!(
        window_us < waves_us,
        "injected window {window_us}us does not undercut waves {waves_us}us"
    );

    // Offline from here: no endpoint, clock, schema, or native receipt is involved.
    let rejected = command()
        .arg("check")
        .arg(temp.path("before"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--json", "--out"])
        .arg(temp.path("rejected"))
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    let report = decoded(&rejected);
    assert_unavailable_load_failure(&report, window_us, waves_us);
    assert!(!temp.path("rejected/capture.json").exists());
    assert!(!temp.path("rejected/acquisition-00").exists());
    assert_eq!(read_json(&temp.path("rejected/report.json")), report);

    let human = command()
        .arg("check")
        .arg(temp.path("before"))
        .arg("--deployment")
        .arg(&declaration)
        .args(["--change", "none", "--out"])
        .arg(temp.path("rejected-human"))
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(1));
    let text = String::from_utf8(human.stdout).unwrap();
    let mut lines = text.lines();
    assert!(lines.next().unwrap().starts_with("INVALID"), "{text}");
    let human_scope = lines.next().unwrap();
    assert!(human_scope.starts_with("Scope: "), "{text}");
    assert!(
        human_scope.to_ascii_lowercase().contains("unavailable"),
        "{text}"
    );
    assert!(!human_scope.contains("structured C1 (one concurrent"));
    assert!(!human_scope.contains("Explicit selected workload"));
    let counts = lines.next().unwrap();
    assert!(counts.starts_with("Complete acquisitions: "), "{text}");
    assert!(!counts.contains("0/8"), "{text}");
    assert!(counts.to_ascii_lowercase().contains("unavailable"));
    assert!(text.contains("acquisition-03"), "{text}");
    assert!(text.contains(&format!("waves_us={waves_us}")));
    assert!(text.contains(&format!("window_us={window_us}")));
    for verdict in ["MEASURED FASTER", "MEASURED SLOWER", "DESCRIPTIVE"] {
        assert!(!text.contains(verdict), "{verdict} in {text}");
    }

    let candidate_failure = compare(&temp.path("clean"), &temp.path("before"));
    assert_eq!(candidate_failure.status.code(), Some(1));
    assert_unavailable_load_failure(&decoded(&candidate_failure), window_us, waves_us);
    assert_eq!(
        compare(&temp.path("clean"), &temp.path("before")).stdout,
        candidate_failure.stdout
    );

    let baseline_failure = compare(&temp.path("before"), &temp.path("clean"));
    assert_eq!(baseline_failure.status.code(), Some(1));
    assert_unavailable_load_failure(&decoded(&baseline_failure), window_us, waves_us);

    // Replaying stored reports never rewrites them, even when their capture no longer verifies.
    assert_eq!(
        fs::read(temp.path("before/report.json")).unwrap(),
        pristine_report
    );
    assert_eq!(
        fs::read(temp.path("before/report.txt")).unwrap(),
        pristine_text
    );
    assert_eq!(
        fs::read(temp.path("clean/report.json")).unwrap(),
        pristine_report
    );
    assert_eq!(
        fs::read(temp.path("clean/report.txt")).unwrap(),
        pristine_text
    );
}

fn phase_workload(
    warmup: Option<(u32, &str)>,
    measured: (u32, &str),
    warmup_trials: u32,
    trials: u32,
) -> Value {
    let mut request = json!({
        "profile": "vllm-fixed-v1",
        "stream": true,
        "output": {"tokens": measured.0, "mode": measured.1},
        "cache": "observe",
        "temperature_milli": 0,
        "top_p_milli": 1000,
        "seed": 0
    });
    if let Some((tokens, mode)) = warmup {
        request["warmup_output"] = json!({"tokens": tokens, "mode": mode});
    }
    json!({
        "version": 3,
        "name": "phase-output-v3",
        "request": request,
        "limits": {"total_ms": 6000, "idle_ms": 3000, "response_bytes": 65536, "wave_buffer_bytes": 33554432},
        "cases": [{"id": "one", "messages": [{"role": "user", "content": "Say cafe."}]}],
        "cells": [{"id": "cell", "case": "one", "concurrency": 1, "warmup_trials": warmup_trials, "trials": trials}]
    })
}

/// Pin a version 3 workload through the production inspector so the selected
/// manifest binds the exact source and normalized digests the loader checks.
fn phase_selection(temp: &Temp, name: &str, workload: &Value) -> PathBuf {
    let leaf = format!("{name}.json");
    let source = temp.path(&leaf);
    fs::write(&source, serde_json::to_vec_pretty(workload).unwrap()).unwrap();
    let inspect = command()
        .args(["bundle", "inspect"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let pins: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    let manifest = temp.path(&format!("{name}-selection.json"));
    write_json(
        &manifest,
        &json!({
            "version": 1,
            "id": format!("{name}-v3"),
            "workload": leaf,
            "source_sha256": pins["source_sha256"],
            "workload_sha256": pins["workload_sha256"],
            "scope": "Prospective capped-warmup/exact-measured phase output fixture; descriptive per-cell observations only. All cells retained.",
            "operation_scope": "unknown"
        }),
    );
    manifest
}

#[test]
fn selected_phase_ceiling_weights_active_warmup_and_unused_warmup_keeps_the_floor() {
    let temp = Temp::new();
    let declaration = deployment(&temp, "serving.json", "unchanged");
    // Capped warmup32 + exact measured400: warmup output is a cap, so the
    // whole-capture completion floor is withheld even though measured is exact.
    let capped = phase_selection(
        &temp,
        "phase-capped",
        &phase_workload(Some((32, "cap")), (400, "exact"), 1, 3),
    );
    let server = Server::new(|stream, index, _| {
        response(stream, Some(if index % 4 == 0 { 32 } else { 400 }), false)
    });
    let output = selected_baseline(&temp, &server.endpoint, &declaration, &capped, "capped")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", decoded(&output));
    let report = decoded(&output);
    assert_eq!(report["baseline_ready"], true);
    assert_eq!(report["baseline_complete_acquisitions"], 8);
    assert_eq!(report["baseline_accounting"]["request_ceiling"], 32);
    assert_eq!(
        report["baseline_accounting"]["output_token_ceiling"],
        8 * 32 + 24 * 400
    );
    assert!(report["baseline_accounting"]["minimum_full_capture_tokens_per_second"].is_null());
    let capture = read_json(&temp.path("capped/capture.json"));
    assert_eq!(capture["request_ceiling"], 32);
    assert_eq!(capture["output_token_ceiling"], 9856);
    let warmup = read_json(&temp.path("capped/acquisition-00/wave-000000/wave.json"));
    assert_eq!(warmup["spec"]["phase"], "warmup");
    assert_eq!(warmup["eligible"], true);
    let measured = read_json(&temp.path("capped/acquisition-00/wave-000001/wave.json"));
    assert_eq!(measured["spec"]["phase"], "measured");
    assert_eq!(measured["eligible"], true);
    let human = fs::read_to_string(temp.path("capped/report.txt")).unwrap();
    assert!(
        !human.contains("Full completion needs"),
        "a capped phase must not imply a completion floor: {human}"
    );
    drop(server);

    // A declared but unused warmup budget cannot withhold the floor: only
    // planned phases count, and the inactive budget still had to validate.
    let inactive = phase_selection(
        &temp,
        "phase-inactive",
        &phase_workload(Some((32, "cap")), (400, "exact"), 0, 3),
    );
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let output = selected_baseline(&temp, &server.endpoint, &declaration, &inactive, "inactive")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", decoded(&output));
    let report = decoded(&output);
    assert_eq!(report["baseline_ready"], true);
    assert_eq!(report["baseline_accounting"]["request_ceiling"], 24);
    assert_eq!(
        report["baseline_accounting"]["output_token_ceiling"],
        24 * 400
    );
    assert_eq!(
        report["baseline_accounting"]["minimum_full_capture_tokens_per_second"].as_f64(),
        Some(9600.0 / 300.0)
    );
}

#[test]
fn selected_failure_expectation_and_wording_follow_the_failing_phase() {
    let temp = Temp::new();
    let declaration = deployment(&temp, "serving.json", "unchanged");
    let selection = phase_selection(
        &temp,
        "phase-capped",
        &phase_workload(Some((32, "cap")), (400, "exact"), 1, 3),
    );
    // Warmup overshoots its 32-token cap: the retained expectation is the
    // warmup budget and the requirement reads as a cap, not measured's exact 400.
    let server = Server::new(|stream, index, _| {
        response(stream, Some(if index % 4 == 0 { 40 } else { 400 }), false)
    });
    let output = selected_baseline(&temp, &server.endpoint, &declaration, &selection, "warmup")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report = decoded(&output);
    assert_eq!(report["result"], "INVALID");
    assert_eq!(report["first_failure"]["wave"]["phase"], "warmup");
    assert_eq!(report["first_failure"]["expected_completion_tokens"], 32);
    assert!(
        report["first_failure"]["eligibility_errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error == "reported_output_exceeds_cap"),
        "{report}"
    );
    let human = fs::read_to_string(temp.path("warmup/report.txt")).unwrap();
    assert!(human.contains("the declared cap was 32"), "{human}");
    assert!(!human.contains("exactly 400 were required"), "{human}");
    drop(server);

    // Measured is exact: the same workload now reports the measured budget and
    // the exact requirement, so the wording follows the failing phase.
    let server = Server::new(|stream, index, _| {
        response(stream, Some(if index % 4 == 0 { 32 } else { 399 }), false)
    });
    let output = selected_baseline(
        &temp,
        &server.endpoint,
        &declaration,
        &selection,
        "measured",
    )
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report = decoded(&output);
    assert_eq!(report["result"], "INVALID");
    assert_eq!(report["first_failure"]["wave"]["phase"], "measured");
    assert_eq!(report["first_failure"]["expected_completion_tokens"], 400);
    assert!(
        report["first_failure"]["eligibility_errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error == "reported_output_not_exact"),
        "{report}"
    );
    let human = fs::read_to_string(temp.path("measured/report.txt")).unwrap();
    assert!(human.contains("exactly 400 were required"), "{human}");
    assert!(!human.contains("the declared cap was 32"), "{human}");
}
