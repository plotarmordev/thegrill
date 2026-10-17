use super::*;

fn concurrent_baseline(temp: &Temp, server: &Server, declaration: &Path, seconds: &str) -> Output {
    command()
        .args([
            "baseline",
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
        ])
        .args(["--workload", "portable-c8-v1", "--seconds", seconds])
        .arg("--deployment")
        .arg(declaration)
        .arg("--out")
        .arg(temp.path("a"))
        .args(["--local-http", "--client-placement", "same-host", "--json"])
        .output()
        .unwrap()
}

fn concurrent_triple(temp: &Temp) {
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let before = deployment(temp, "before.json", "before");
    let after = deployment(temp, "after.json", "after");
    let output = concurrent_baseline(temp, &server, &before, "300");
    assert!(output.status.success(), "{}", decoded(&output));
    assert_ne!(decoded(&check(temp, "a", &after, "b"))["result"], "INVALID");
    assert_ne!(
        decoded(&check_none(temp, "a", &before, "a2"))["result"],
        "INVALID"
    );
}

#[test]
fn concurrent_builtin_captures_whole_waves_and_assesses_acquisitions_not_lanes() {
    let temp = Temp::new();
    concurrent_triple(&temp);
    let capture = read_json(&temp.path("a/capture.json"));
    assert_eq!(capture["workload"], "portable-c8-v1");
    assert_eq!(capture["request_ceiling"], 256);
    assert_eq!(capture["output_token_ceiling"], 102400);
    assert_eq!(
        fs::read(temp.path("a/workload.json")).unwrap(),
        include_bytes!("../../examples/portable-c8-v1.json")
    );
    for acquisition in 0..8 {
        for wave in 0..4 {
            let receipt = read_json(&temp.path(&format!(
                "a/acquisition-{acquisition:02}/wave-{wave:06}/wave.json"
            )));
            assert_eq!(receipt["attempts"].as_array().unwrap().len(), 8);
            assert_eq!(receipt["eligible"], true);
        }
    }
    for (durations, result) in [
        (STEADY.map(|us| us / 2), "IMPROVED"),
        (STEADY.map(|us| us * 2), "REGRESSED"),
        (STEADY, "INCONCLUSIVE"),
    ] {
        schedule(&temp, durations, STEADY, [GAP_MS; 2]);
        let report = decoded(&compare_captures(&temp, "a", "b", None));
        assert_eq!(report["result"], result, "{report}");
        let logs = durations.map(|us| (400_000.0 / us as f64).ln());
        let mean = logs.iter().sum::<f64>() / 8.0;
        let variance = logs.iter().map(|value| (value - mean).powi(2)).sum::<f64>() / 7.0;
        let se = (variance / 8.0).sqrt();
        assert!(
            (report["observed_change_percent"].as_f64().unwrap() - mean.exp_m1() * 100.0).abs()
                < 1e-9
        );
        for (actual, sign) in report["model_based_interval_percent"]
            .as_array()
            .unwrap()
            .iter()
            .zip([-1.0, 1.0])
        {
            let expected = (mean + sign * 2.364624251 * se).exp_m1() * 100.0;
            assert!((actual.as_f64().unwrap() - expected).abs() < 1e-9);
        }
        for side in ["baseline", "candidate"] {
            assert_eq!(
                report[format!("{side}_acquisition_medians")]
                    .as_array()
                    .unwrap()
                    .len(),
                8
            );
            let cells = report[format!("{side}_acquisition_cells")]
                .as_array()
                .unwrap();
            assert_eq!(cells.len(), 8);
            for cell in cells {
                assert_eq!(cell["first_answer_text_us"].as_array().unwrap().len(), 24);
                for wave in cell["lane_decode_tokens_per_second"].as_array().unwrap() {
                    assert_eq!(wave.as_array().unwrap().len(), 8);
                }
            }
        }
        assert_eq!(report["conservative_degrees_of_freedom"], 7);
    }
    let text = fs::read_to_string(temp.path("a/report.txt")).unwrap();
    assert!(text.contains("Per-stream observations (descriptive"));
    assert!(text.contains("aggregate across lanes"));
}

#[test]
fn concurrent_lane_below_cap_invalidates_the_wave_without_replacement() {
    let temp = Temp::new();
    let server = Server::new(|stream, index, _| {
        response(stream, Some(if index == 10 { 399 } else { 400 }), false)
    });
    let before = deployment(&temp, "before.json", "before");
    let output = concurrent_baseline(&temp, &server, &before, "300");
    let report = decoded(&output);
    assert_eq!(report["result"], "INVALID", "{report}");
    let receipt = read_json(&temp.path("a/acquisition-00/wave-000001/wave.json"));
    let failed = receipt["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["usage"]["completion_tokens"] == 399)
        .unwrap();
    assert_eq!(report["first_failure"]["lane"], failed["lane"]);
    assert!(!temp.path("a/acquisition-00/wave-000004").exists());
    let capture = read_json(&temp.path("a/capture.json"));
    for acquisition in &capture["acquisitions"].as_array().unwrap()[1..] {
        assert!(acquisition["plan_sha256"].is_null());
    }
    assert_eq!(
        decoded(&compare(&temp.path("a"), &temp.path("a")))["result"],
        "INVALID"
    );
}

#[test]
fn concurrent_builtin_single_field_reference_verdict() {
    let temp = Temp::new();
    concurrent_triple(&temp);
    schedule(&temp, STEADY.map(|us| us / 2), STEADY, [GAP_MS; 2]);
    let report = decoded(&compare_captures(&temp, "a", "b", Some("a2")));
    assert_eq!(report["result"], "IMPROVED", "{report}");
    assert!(report["scope"].as_str().unwrap().contains("C8"));
    assert_eq!(
        report["deployment"]["candidate_against_reference"]["result"],
        "IMPROVED"
    );
    schedule(
        &temp,
        STEADY.map(|us| us / 2),
        STEADY.map(|us| us / 4),
        [GAP_MS; 2],
    );
    let report = decoded(&compare_captures(&temp, "a", "b", Some("a2")));
    assert_eq!(report["result"], "INCONCLUSIVE", "{report}");
    assert!(
        report["reasons"]
            .to_string()
            .contains("not directional against both baseline capture periods")
    );
}

#[test]
fn concurrent_builtin_deployment_pair_pending_then_three_way() {
    let temp = Temp::new();
    if binary_source() == "unrecorded" {
        concurrent_triple(&temp);
        redeclare(
            &temp.path("b"),
            "deployment",
            &json!({"model_revision":"other-revision", "runtime":"other-runtime", "hardware":"other-cpu", "settings":"after"}),
        );
        for name in ["a", "b", "a2"] {
            edit_capture(&temp.path(name), |capture| {
                capture["collector"]["source_commit"] = json!(SOURCE)
            });
        }
    } else {
        let server = Server::new(|stream, _, _| response(stream, Some(400), false));
        let before = deployment(&temp, "before.json", "before");
        let after = deployment(&temp, "after.json", "after");
        assert!(
            concurrent_baseline(&temp, &server, &before, "300")
                .status
                .success()
        );
        let candidate = command()
            .arg("check")
            .arg(temp.path("a"))
            .arg("--deployment")
            .arg(after)
            .args([
                "--change",
                "deployment",
                "--client-placement",
                "same-host",
                "--json",
            ])
            .arg("--out")
            .arg(temp.path("b"))
            .output()
            .unwrap();
        assert_eq!(decoded(&candidate)["result"], "PENDING");
        assert_ne!(
            decoded(&check_none(&temp, "a", &before, "a2"))["result"],
            "INVALID"
        );
    }
    schedule(&temp, STEADY.map(|us| us / 2), STEADY, [GAP_MS; 2]);
    let pending = decoded(&compare_captures(&temp, "a", "b", None));
    assert_eq!(pending["result"], "INCONCLUSIVE", "{pending}");
    let report = decoded(&compare_captures(&temp, "a", "b", Some("a2")));
    assert_eq!(report["result"], "IMPROVED", "{report}");
    assert!(
        report["scope"]
            .as_str()
            .unwrap()
            .contains("at C8 including prefill")
    );
}

#[test]
fn outputs_pairs_192_lanes_for_a_concurrent_capture() {
    let temp = Temp::new();
    let server = Server::new(|stream, index, _| {
        respond(
            stream,
            if index == 266 { "1 3" } else { "1 2" },
            Some(400),
            false,
            4,
        );
    });
    let before = deployment(&temp, "before.json", "before");
    let after = deployment(&temp, "after.json", "after");
    assert!(
        concurrent_baseline(&temp, &server, &before, "300")
            .status
            .success()
    );
    assert_ne!(
        decoded(&check(&temp, "a", &after, "b"))["result"],
        "INVALID"
    );
    let output = command()
        .arg("outputs")
        .arg(temp.path("a"))
        .arg(temp.path("b"))
        .arg("--json")
        .output()
        .unwrap();
    let report = decoded(&output);
    assert_eq!(
        [
            &report["identical"],
            &report["differs"],
            &report["unavailable"]
        ],
        [191, 1, 0]
    );
    let lane = report["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|lane| lane["status"] == "differs")
        .unwrap();
    assert_eq!([&lane["acquisition"], &lane["trial"]], [0, 0]);
    let index = lane["lane"].as_u64().unwrap();
    let body = fs::read_to_string(temp.path(&format!(
        "b/acquisition-00/wave-000001/response-{index:04}.bin"
    )))
    .unwrap();
    assert!(body.contains("1 3"));
}

#[test]
fn concurrent_budget_stop_reports_aggregate_rate() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| {
        thread::sleep(Duration::from_millis(1500));
        let _ = stream.shutdown(std::net::Shutdown::Both);
    });
    let before = deployment(&temp, "before.json", "before");
    let report = decoded(&concurrent_baseline(&temp, &server, &before, "1"));
    assert_eq!(
        report["baseline_accounting"]["minimum_full_capture_tokens_per_second"],
        102400.0
    );
    assert_eq!(report["result"], "INCONCLUSIVE", "{report}");
    assert!(!report["baseline_ready"].as_bool().unwrap());
    let capture = read_json(&temp.path("a/capture.json"));
    assert!(capture["acquisitions"][1]["plan_sha256"].is_null());
}

#[test]
#[ignore = "requires GRILL_PERF_BEFORE pointing to the prechange binary"]
fn c1_report_json_is_byte_identical_to_before() {
    let before_binary = std::env::var_os("GRILL_PERF_BEFORE").expect("prechange binary path");
    for workload in ["portable-v1", "baseline-v2"] {
        let temp = Temp::new();
        let server = Server::new(|stream, _, _| response(stream, Some(400), false));
        let before = deployment(&temp, "before.json", "before");
        let after = deployment(&temp, "after.json", "after");
        let baseline = command()
            .args([
                "baseline",
                "--endpoint",
                &server.endpoint,
                "--model",
                "fixture",
            ])
            .args([
                "--workload",
                workload,
                "--client-placement",
                "same-host",
                "--local-http",
                "--json",
            ])
            .arg("--deployment")
            .arg(before)
            .arg("--out")
            .arg(temp.path("a"))
            .output()
            .unwrap();
        assert!(baseline.status.success(), "{}", decoded(&baseline));
        let report = decoded(&baseline);
        assert!(report.get("baseline_acquisition_cells").is_none());
        assert!(report.get("candidate_acquisition_cells").is_none());
        let candidate = check(&temp, "a", &after, "b");
        assert_ne!(decoded(&candidate)["result"], "INVALID");
        let old = Command::new(&before_binary)
            .arg("compare")
            .arg(temp.path("a"))
            .arg(temp.path("b"))
            .arg("--json")
            .output()
            .unwrap();
        let current = compare(&temp.path("a"), &temp.path("b"));
        assert_ne!(decoded(&old)["result"], "INVALID");
        assert_eq!(old.status.code(), current.status.code());
        assert_eq!(old.stdout, current.stdout, "{workload}");
    }
}
