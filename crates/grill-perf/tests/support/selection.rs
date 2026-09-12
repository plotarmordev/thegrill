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
    assert_eq!(decoded(&rejected)["result"], "INVALID");
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
