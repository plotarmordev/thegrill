use super::*;
use sha2::{Digest, Sha256};

const METRICS: [&str; 4] = [
    "wave_latency_us",
    "achieved_completion_tokens_per_second",
    "decode_tokens_per_second",
    "prefill_tokens_per_second",
];
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn save(path: impl AsRef<Path>, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn declaration(work: &Value, tolerance: u32, spread: u32) -> Value {
    json!({"version":1,"method":"observed-envelope-v1","id":"synthetic-policy",
        "collector_sha256":hash(&fs::read(env!("CARGO_BIN_EXE_grill-perf")).unwrap()),
        "workload_source_sha256":hash(&serde_json::to_vec(work).unwrap()),"min_trials":3,
        "cells":work["cells"].as_array().unwrap().iter().map(|cell| json!({"cell":cell["id"],
            "metrics":METRICS.iter().map(|metric| json!({"metric":metric,"max_regression_bps":tolerance,"max_reference_spread_bps":spread})).collect::<Vec<_>>() })).collect::<Vec<_>>()})
}
fn collect(temp: &Temp, server: &Server, name: &str, policy: bool) -> Output {
    let mut command = cli();
    command
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
        .arg("--out")
        .arg(temp.path(name));
    if policy {
        command.arg("--policy").arg(temp.path("policy.json"));
    }
    command.output().unwrap()
}
struct Fixture {
    temp: Temp,
    _server: Server,
}
impl Fixture {
    fn new(work: Value, tolerance: u32, spread: u32) -> Self {
        let temp = Temp::new();
        save(temp.path("work.json"), &work);
        save(temp.path("deployment.json"), &deployment());
        save(
            temp.path("policy.json"),
            &declaration(&work, tolerance, spread),
        );
        let root = temp.0.clone();
        let requests = work["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                c["concurrency"].as_u64().unwrap()
                    * (c["trials"].as_u64().unwrap() + c["warmup_trials"].as_u64().unwrap())
            })
            .sum::<u64>() as usize;
        let server = Server::new(move |mut stream, index, _| {
            let role = ["a", "b", "a2"][index / requests];
            let policy = fs::read(root.join("policy.json")).unwrap();
            assert_eq!(
                fs::read(root.join(role).join("policy.json")).unwrap(),
                policy
            );
            assert_eq!(
                value(root.join(role).join("plan.json"))["policy_sha256"],
                hash(&policy)
            );
            header(&mut stream, "text/event-stream");
            frame(
                &mut stream,
                json!({"id":"fixture","choices":[{"delta":{"content":format!("synthetic-{index}")}}]}),
            );
            finish(&mut stream, Some(8), Some(0));
        });
        for (index, role) in ["a", "b", "a2"].into_iter().enumerate() {
            successful(&collect(&temp, &server, role, true));
            // Explicit synthetic declared starts avoid clock-resolution premises.
            rewrite_plan(&temp.path(role), |p| {
                p["started_unix_ms"] = json!((index + 1) * 1000)
            });
            synthetic_times(&temp.path(role), &[(10000, 20000)]);
        }
        Self {
            temp,
            _server: server,
        }
    }
    fn decide(&self, reference: Option<&str>) -> Output {
        let mut command = cli();
        command
            .arg("decide")
            .arg(self.temp.path("a"))
            .arg(self.temp.path("b"))
            .arg("--json");
        if let Some(reference) = reference {
            command.arg("--reference").arg(self.temp.path(reference));
        }
        command.output().unwrap()
    }
}
// Synthetic evidence transformations retain the real CLI's request/body verifier.
// They are numeric fixtures, not an assertion that the edited times were observed.
fn rewrite_plan(root: &Path, change: impl FnOnce(&mut Value)) {
    let mut plan = value(root.join("plan.json"));
    change(&mut plan);
    save(root.join("plan.json"), &plan);
    let plan_hash = hash(&fs::read(root.join("plan.json")).unwrap());
    for spec in plan["waves"].as_array().unwrap() {
        let dir = root.join(format!("wave-{:06}", spec["index"].as_u64().unwrap()));
        if !dir.exists() {
            continue;
        }
        let mut reservation = value(dir.join("reservation.json"));
        reservation["plan_sha256"] = json!(plan_hash);
        save(dir.join("reservation.json"), &reservation);
        if dir.join("wave.json").exists() {
            let mut wave = value(dir.join("wave.json"));
            wave["plan_sha256"] = json!(plan_hash);
            wave["reservation_sha256"] =
                json!(hash(&fs::read(dir.join("reservation.json")).unwrap()));
            save(dir.join("wave.json"), &wave);
        }
    }
    let session_path = root.join("session-000000/session.json");
    let mut session = value(&session_path);
    session["plan_sha256"] = json!(plan_hash);
    session["collector_sha256"] = plan["collector_sha256"].clone();
    save(session_path, &session);
}
fn synthetic_times(root: &Path, times: &[(u64, u64)]) {
    let plan = value(root.join("plan.json"));
    for (index, spec) in plan["waves"].as_array().unwrap().iter().enumerate() {
        let path = root.join(format!(
            "wave-{:06}/wave.json",
            spec["index"].as_u64().unwrap()
        ));
        let mut wave = value(&path);
        let (first, settle) = times[index % times.len()];
        for attempt in wave["attempts"].as_array_mut().unwrap() {
            attempt["timing"] = json!({"dispatch_offset_us":0,"headers_us":0,"first_body_us":0,
                "first_generated_text_us":first,"last_generated_text_us":first,
                "terminal_us":settle,"first_generated_channel":"answer",
                "first_answer_text_us":first,"settle_us":settle,"capture_parse_us":0});
        }
        wave["elapsed_us"] = json!(settle);
        wave["dispatch_spread_us"] = json!(0);
        wave["achieved_completion_tokens_per_second"] =
            json!(wave["completion_tokens"].as_u64().unwrap() as f64 * 1_000_000.0 / settle as f64);
        save(path, &wave);
    }
}
fn report(output: &Output, outcome: &str, exit: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["decision"], outcome);
    value
}
fn gate<'a>(report: &'a Value, metric: &str) -> &'a Value {
    report["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["metric"] == metric)
        .unwrap()
}
fn reason(value: &Value, expected: &str) {
    assert!(
        value["reason_codes"]
            .as_array()
            .unwrap()
            .contains(&json!(expected)),
        "{value}"
    );
}

#[test]
fn policy_capture_all_roles_and_exact_zero_tolerance_determinism() {
    let fixture = Fixture::new(workload(2, 1, 3), 0, 0);
    let first = fixture.decide(Some("a2"));
    let decision = report(&first, "PASS", 0);
    assert_eq!(first.stdout, fixture.decide(Some("a2")).stdout);
    for metric in METRICS {
        assert_eq!(gate(&decision, metric)["decision"], "PASS");
    }
    assert_eq!(
        gate(&decision, METRICS[2])["coverage"]["candidate"]["expected_observations"],
        6
    );
    synthetic_times(&fixture.temp.path("b"), &[(10001, 20002)]);
    let changed = report(&fixture.decide(Some("a2")), "REGRESSION", 3);
    for metric in METRICS {
        assert_eq!(gate(&changed, metric)["decision"], "REGRESSION");
    }
    assert_ne!(
        decision["roles"]["candidate"]["evidence_sha256"],
        changed["roles"]["candidate"]["evidence_sha256"]
    );
}

#[test]
fn policy_exact_9999_latency_and_rate_boundaries() {
    let fixture = Fixture::new(workload(1, 1, 3), 9999, 0);
    for role in ["a", "a2"] {
        synthetic_times(&fixture.temp.path(role), &[(50, 100)]);
    }
    synthetic_times(&fixture.temp.path("b"), &[(50, 199)]);
    let before = report(&fixture.decide(Some("a2")), "PASS", 0);
    assert_eq!(gate(&before, METRICS[0])["decision"], "PASS");
    // Integer latency equality: 19999 / 10000 = 1 + 9999 / 10000.
    for role in ["a", "a2"] {
        synthetic_times(&fixture.temp.path(role), &[(5000, 10000)]);
    }
    synthetic_times(&fixture.temp.path("b"), &[(5000, 19999)]);
    report(&fixture.decide(Some("a2")), "PASS", 0);
    synthetic_times(&fixture.temp.path("b"), &[(5000, 20000)]);
    let over = report(&fixture.decide(Some("a2")), "REGRESSION", 3);
    assert_eq!(gate(&over, METRICS[0])["decision"], "REGRESSION");
    for role in ["a", "a2"] {
        synthetic_times(&fixture.temp.path(role), &[(50, 100)]);
    }
    synthetic_times(&fixture.temp.path("b"), &[(500000, 1000000)]);
    let exact = report(&fixture.decide(Some("a2")), "REGRESSION", 3);
    for metric in &METRICS[1..] {
        assert_eq!(gate(&exact, metric)["decision"], "PASS");
    }
    synthetic_times(&fixture.temp.path("b"), &[(500001, 1000002)]);
    let over = report(&fixture.decide(Some("a2")), "REGRESSION", 3);
    for metric in &METRICS[1..] {
        assert_eq!(gate(&over, metric)["decision"], "REGRESSION");
    }
}

#[test]
fn policy_reference_spread_and_straddling_are_not_pass() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 10000);
    synthetic_times(&fixture.temp.path("a2"), &[(20000, 40000)]);
    synthetic_times(&fixture.temp.path("b"), &[(15000, 30000)]);
    let decision = report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2);
    reason(gate(&decision, METRICS[1]), "envelope_straddles_tolerance");
    synthetic_times(&fixture.temp.path("a2"), &[(20001, 40002)]);
    let decision = report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2);
    for metric in METRICS {
        reason(gate(&decision, metric), "reference_spread_exceeded");
    }
}

#[test]
fn policy_missing_reused_out_of_order_and_unqualified_reference() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 0);
    reason(
        &report(&fixture.decide(None), "INCONCLUSIVE", 2),
        "missing_reference",
    );
    reason(
        &report(&fixture.decide(Some("a")), "INCONCLUSIVE", 2),
        "role_reuse",
    );
    rewrite_plan(&fixture.temp.path("a2"), |p| {
        p["started_unix_ms"] = json!(1500)
    });
    reason(
        &report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2),
        "declared_starts_out_of_order",
    );
    rewrite_plan(&fixture.temp.path("a2"), |p| {
        p["started_unix_ms"] = json!(3000);
        p["deployment"]["settings"] = json!("private-different-setting");
    });
    let output = fixture.decide(Some("a2"));
    reason(&report(&output, "INCONCLUSIVE", 2), "reference_unqualified");
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("private-different-setting")
    );
}

#[test]
fn policy_partial_lane_and_warmup_coverage_preserves_other_regression() {
    let fixture = Fixture::new(workload(2, 1, 3), 0, 0);
    synthetic_times(&fixture.temp.path("b"), &[(10001, 20002)]);
    let path = fixture.temp.path("b/wave-000001/wave.json");
    let mut wave = value(&path);
    // Prefill is undefined at a zero first-text interval, but latency is measured.
    wave["attempts"][0]["timing"]["first_generated_text_us"] = json!(0);
    wave["attempts"][0]["timing"]["first_answer_text_us"] = json!(0);
    wave["attempts"][0]["timing"]["last_generated_text_us"] = json!(0);
    save(&path, &wave);
    let decision = report(&fixture.decide(Some("a2")), "REGRESSION", 3);
    reason(gate(&decision, METRICS[3]), "metric_unavailable");
    assert_eq!(
        gate(&decision, METRICS[3])["coverage"]["candidate"]["observed_observations"],
        5
    );
    assert_eq!(gate(&decision, METRICS[0])["decision"], "REGRESSION");
    let no_warmup = Fixture::new(workload(1, 0, 3), 0, 0);
    let decision = report(&no_warmup.decide(Some("a2")), "INCONCLUSIVE", 2);
    for metric in METRICS {
        reason(gate(&decision, metric), "warmup_incomplete");
    }
}

#[test]
fn policy_all_cells_and_metrics_remain_in_scope() {
    let mut work = workload(1, 1, 3);
    let mut second = work["cells"][0].clone();
    second["id"] = json!("second");
    work["cells"].as_array_mut().unwrap().push(second);
    let fixture = Fixture::new(work, 0, 0);
    let decision = report(&fixture.decide(Some("a2")), "PASS", 0);
    let scope = decision["gates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["cell"].as_str().unwrap(), g["metric"].as_str().unwrap()))
        .collect::<std::collections::HashSet<_>>();
    for cell in ["cell", "second"] {
        for metric in METRICS {
            assert!(scope.contains(&(cell, metric)));
        }
    }
}

#[test]
fn policy_sidecar_raw_body_and_outcome_fingerprints() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 0);
    let before = report(&fixture.decide(Some("a2")), "PASS", 0);
    let path = fixture.temp.path("b/wave-000001/response-0000.bin");
    let bytes = fs::read(&path).unwrap();
    let changed = String::from_utf8(bytes)
        .unwrap()
        .replace("synthetic-", "synthetic-edited-");
    fs::write(&path, &changed).unwrap();
    report(&fixture.decide(Some("a2")), "ERROR", 1);
    let receipt = fixture.temp.path("b/wave-000001/wave.json");
    let mut wave = value(&receipt);
    let delta = changed.len() - wave["attempts"][0]["response_bytes"].as_u64().unwrap() as usize;
    wave["attempts"][0]["response_bytes"] = json!(changed.len());
    wave["attempts"][0]["response_sha256"] = json!(hash(changed.as_bytes()));
    wave["attempts"][0]["terminal_offset"] =
        json!(wave["attempts"][0]["terminal_offset"].as_u64().unwrap() + delta as u64);
    save(receipt, &wave);
    let after = report(&fixture.decide(Some("a2")), "PASS", 0);
    assert_ne!(
        before["roles"]["candidate"]["evidence_sha256"],
        after["roles"]["candidate"]["evidence_sha256"]
    );
    let mut outcome = value(fixture.temp.path("b/run.json"));
    outcome["wave_preparation_us"] = json!(outcome["wave_preparation_us"].as_u64().unwrap() + 1);
    save(fixture.temp.path("b/run.json"), &outcome);
    save(fixture.temp.path("b/session-000000/run.json"), &outcome);
    let end = report(&fixture.decide(Some("a2")), "PASS", 0);
    assert_ne!(
        after["roles"]["candidate"]["evidence_sha256"],
        end["roles"]["candidate"]["evidence_sha256"]
    );
    let policy_path = fixture.temp.path("a2/policy.json");
    let mut bytes = fs::read(&policy_path).unwrap();
    bytes.push(b' ');
    fs::write(&policy_path, &bytes).unwrap();
    reason(
        &report(&fixture.decide(Some("a2")), "ERROR", 1),
        "policy_hash_mismatch",
    );
    rewrite_plan(&fixture.temp.path("a2"), |p| {
        p["policy_sha256"] = json!(hash(&bytes))
    });
    reason(
        &report(&fixture.decide(Some("a2")), "ERROR", 1),
        "conflicting_policy",
    );
    fs::remove_file(&policy_path).unwrap();
    reason(
        &report(&fixture.decide(Some("a2")), "ERROR", 1),
        "invalid_policy",
    );
}

#[test]
fn policy_absent_candidate_cannot_be_bound_retrospectively_and_errors_are_private() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 0);
    rewrite_plan(&fixture.temp.path("b"), |p| {
        p.as_object_mut().unwrap().remove("policy_sha256");
    });
    reason(
        &report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2),
        "missing_policy",
    );
    fs::write(
        fixture.temp.path("b/plan.json"),
        b"private-loader-diagnostic-prompt",
    )
    .unwrap();
    let output = fixture.decide(Some("a2"));
    reason(&report(&output, "ERROR", 1), "invalid_evidence");
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("private-loader-diagnostic-prompt")
    );
}

#[test]
fn policy_admission_rejects_schema_scope_pins_and_trial_minima_before_dispatch() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let work = workload(1, 1, 3);
    save(temp.path("work.json"), &work);
    save(temp.path("deployment.json"), &deployment());
    let policy = declaration(&work, 0, 0);
    let mut invalid = Vec::new();
    for (field, value) in [
        ("version", json!(2)),
        ("method", json!("unknown")),
        ("unknown", json!(true)),
        ("collector_sha256", json!("0".repeat(64))),
        ("workload_source_sha256", json!("0".repeat(64))),
        ("min_trials", json!(4)),
        ("cells", json!([])),
    ] {
        let mut p = policy.clone();
        p[field] = value;
        invalid.push(p);
    }
    let mut duplicate = policy.clone();
    duplicate["cells"][0]["metrics"][1] = policy["cells"][0]["metrics"][0].clone();
    invalid.push(duplicate);
    let mut duplicate_cell = policy.clone();
    duplicate_cell["cells"]
        .as_array_mut()
        .unwrap()
        .push(policy["cells"][0].clone());
    invalid.push(duplicate_cell);
    for (field, value) in [
        ("cell", json!("unknown-cell")),
        ("metrics", json!([])),
        ("unknown", json!(true)),
    ] {
        let mut p = policy.clone();
        p["cells"][0][field] = value;
        invalid.push(p);
    }
    for (field, value) in [
        ("metric", json!("unknown")),
        ("max_regression_bps", json!(10000)),
        ("max_reference_spread_bps", json!(1000001)),
        ("unknown", json!(0)),
    ] {
        let mut p = policy.clone();
        p["cells"][0]["metrics"][0][field] = value;
        invalid.push(p);
    }
    for (index, policy) in invalid.iter().enumerate() {
        save(temp.path("policy.json"), policy);
        let name = format!("rejected-{index}");
        assert_eq!(collect(&temp, &server, &name, true).status.code(), Some(1));
        assert!(!temp.path(&name).exists());
    }
    fs::write(temp.path("policy.json"), vec![b' '; 65537]).unwrap();
    assert_eq!(
        collect(&temp, &server, "oversized", true).status.code(),
        Some(1)
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}

#[test]
fn policy_legacy_default_roundtrip_is_unbound_not_pass() {
    let temp = Temp::new();
    let server = Server::new(normal);
    successful(&run(&temp, &server, "old", &workload(1, 0, 1)));
    let paths = [
        "plan.json",
        "workload.json",
        "run.json",
        "wave-000000/reservation.json",
        "wave-000000/wave.json",
        "wave-000000/response-0000.bin",
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|p| fs::read(temp.path("old").join(p)).unwrap())
        .collect();
    assert!(
        value(temp.path("old/plan.json"))
            .get("policy_sha256")
            .is_none()
    );
    let compare = cli()
        .arg("compare")
        .arg(temp.path("old"))
        .arg(temp.path("old"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&compare);
    let decision = cli()
        .arg("decide")
        .arg(temp.path("old"))
        .arg(temp.path("old"))
        .arg("--json")
        .output()
        .unwrap();
    reason(&report(&decision, "INCONCLUSIVE", 2), "missing_policy");
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(temp.path("old").join(path)).unwrap(), bytes);
    }
}

fn synthetic_usage(root: &Path, tokens: impl Fn(usize) -> u64) {
    let plan = value(root.join("plan.json"));
    for spec in plan["waves"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["phase"] == "measured")
    {
        let dir = root.join(format!("wave-{:06}", spec["index"].as_u64().unwrap()));
        let mut wave = value(dir.join("wave.json"));
        let mut total = 0;
        for (lane, attempt) in wave["attempts"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            let tokens = tokens(lane);
            total += tokens;
            let usage = json!({"prompt_tokens":4,"completion_tokens":tokens,"total_tokens":tokens+4,
                "prompt_tokens_details":{"cached_tokens":0}});
            let path = dir.join(format!("response-{lane:04}.bin"));
            let original = fs::read_to_string(&path).unwrap();
            let raw = original
                .split_inclusive('\n')
                .map(|line| {
                    if let Some(data) = line.strip_prefix("data: ")
                        && let Ok(mut event) = serde_json::from_str::<Value>(data)
                        && event.get("usage").is_some()
                    {
                        event["usage"] = usage.clone();
                        return format!("data: {event}\n");
                    }
                    line.to_owned()
                })
                .collect::<String>();
            fs::write(&path, &raw).unwrap();
            let delta = raw.len() as i64 - original.len() as i64;
            attempt["response_bytes"] = json!(raw.len());
            attempt["response_sha256"] = json!(hash(raw.as_bytes()));
            attempt["terminal_offset"] =
                json!(attempt["terminal_offset"].as_i64().unwrap() + delta);
            attempt["usage"]["completion_tokens"] = json!(tokens);
            attempt["usage"]["total_tokens"] = json!(tokens + 4);
        }
        wave["completion_tokens"] = json!(total);
        wave["achieved_completion_tokens_per_second"] =
            json!(total as f64 * 1_000_000.0 / wave["elapsed_us"].as_u64().unwrap() as f64);
        save(dir.join("wave.json"), &wave);
    }
}

#[test]
fn policy_defined_zero_throughput_is_not_an_undefined_or_matched_rate() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 0);
    synthetic_usage(&fixture.temp.path("b"), |_| 0);
    let decision = report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2);
    let aggregate = gate(&decision, METRICS[1]);
    reason(aggregate, "output_amounts_mismatch");
    assert_eq!(
        aggregate["coverage"]["candidate"]["observed_observations"],
        3
    );
    assert_eq!(aggregate["ranges"]["candidate"][0]["numerator"], 0);
    assert_eq!(
        gate(&decision, METRICS[2])["coverage"]["candidate"]["observed_observations"],
        0
    );
    for role in ["a", "a2"] {
        synthetic_usage(&fixture.temp.path(role), |_| 0);
    }
    let decision = report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2);
    reason(gate(&decision, METRICS[1]), "nonpositive_reference");
    assert_eq!(gate(&decision, METRICS[0])["decision"], "PASS");
}

#[test]
fn policy_ordered_lane_amounts_and_decode_dropout_are_not_flattened() {
    let fixture = Fixture::new(workload(2, 1, 3), 0, 0);
    for role in ["a", "b", "a2"] {
        synthetic_usage(
            &fixture.temp.path(role),
            |lane| if lane == 0 { 1 } else { 8 },
        );
    }
    let decision = report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2);
    reason(gate(&decision, METRICS[2]), "metric_unavailable");
    assert_eq!(
        gate(&decision, METRICS[2])["coverage"]["candidate"]["observed_observations"],
        3
    );
    assert_eq!(gate(&decision, METRICS[1])["decision"], "PASS");
    synthetic_usage(
        &fixture.temp.path("a2"),
        |lane| if lane == 0 { 8 } else { 1 },
    );
    let decision = report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2);
    for metric in METRICS {
        reason(gate(&decision, metric), "output_amounts_mismatch");
    }
}

#[test]
fn policy_loader_uses_recorded_collector_not_evaluator_binary() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 0);
    let before = report(&fixture.decide(Some("a2")), "PASS", 0);
    let mut policy = value(fixture.temp.path("policy.json"));
    policy["collector_sha256"] = json!("1".repeat(64));
    let bytes = serde_json::to_vec(&policy).unwrap();
    for role in ["a", "b", "a2"] {
        fs::write(fixture.temp.path(role).join("policy.json"), &bytes).unwrap();
        rewrite_plan(&fixture.temp.path(role), |p| {
            p["collector_sha256"] = policy["collector_sha256"].clone();
            p["policy_sha256"] = json!(hash(&bytes));
        });
    }
    let decision = report(&fixture.decide(Some("a2")), "PASS", 0);
    assert_eq!(decision["evaluator_sha256"], before["evaluator_sha256"]);
    assert_ne!(
        decision["evaluator_sha256"],
        decision["roles"]["baseline"]["collector_sha256"]
    );
    rewrite_plan(&fixture.temp.path("b"), |p| {
        p["collector_sha256"] = json!("2".repeat(64))
    });
    reason(
        &report(&fixture.decide(Some("a2")), "ERROR", 1),
        "policy_collector_mismatch",
    );
    policy["workload_source_sha256"] = json!("3".repeat(64));
    let bytes = serde_json::to_vec(&policy).unwrap();
    fs::write(fixture.temp.path("b/policy.json"), &bytes).unwrap();
    rewrite_plan(&fixture.temp.path("b"), |p| {
        p["collector_sha256"] = policy["collector_sha256"].clone();
        p["policy_sha256"] = json!(hash(&bytes));
    });
    reason(
        &report(&fixture.decide(Some("a2")), "ERROR", 1),
        "policy_source_mismatch",
    );
}

#[test]
fn policy_copied_lineage_cannot_be_relabelled_as_an_acquisition() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 0);
    fn copy(source: &Path, destination: &Path) {
        fs::create_dir(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &destination.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
    }
    copy(&fixture.temp.path("a"), &fixture.temp.path("copy"));
    rewrite_plan(&fixture.temp.path("copy"), |p| {
        p["started_unix_ms"] = json!(4000)
    });
    reason(
        &report(&fixture.decide(Some("copy")), "INCONCLUSIVE", 2),
        "role_reuse",
    );
}

#[test]
fn policy_incomplete_session_and_missing_wave_cannot_pass() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 0);
    fs::remove_file(fixture.temp.path("a2/run.json")).unwrap();
    reason(
        &report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2),
        "session_incomplete",
    );
    fs::remove_file(fixture.temp.path("a2/session-000000/run.json")).unwrap();
    fs::remove_dir_all(fixture.temp.path("a2/wave-000003")).unwrap();
    let decision = report(&fixture.decide(Some("a2")), "INCONCLUSIVE", 2);
    reason(gate(&decision, METRICS[0]), "metric_unavailable");
    assert_eq!(
        gate(&decision, METRICS[0])["coverage"]["reference"]["observed_waves"],
        2
    );
}

#[test]
fn policy_resume_reuses_capture_after_external_declaration_changes() {
    let temp = Temp::new();
    let work = workload(1, 1, 3);
    save(temp.path("work.json"), &work);
    save(temp.path("deployment.json"), &deployment());
    save(temp.path("policy.json"), &declaration(&work, 0, 0));
    let captured = fs::read(temp.path("policy.json")).unwrap();
    let (ready, releases) = std::sync::mpsc::sync_channel(1);
    let root = temp.path("run");
    let expected = captured.clone();
    let server = Server::new(move |mut stream, index, _| {
        assert_eq!(fs::read(root.join("policy.json")).unwrap(), expected);
        if index == 0 {
            let (release, wait) = std::sync::mpsc::sync_channel(0);
            ready.send(release).unwrap();
            wait.recv_timeout(Duration::from_secs(30)).unwrap();
        }
        header(&mut stream, "text/event-stream");
        frame(
            &mut stream,
            json!({"id":"fixture","choices":[{"delta":{"content":"synthetic-resume"}}]}),
        );
        finish(&mut stream, Some(8), Some(0));
    });
    let mut child = cli()
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
        .arg("--policy")
        .arg(temp.path("policy.json"))
        .arg("--out")
        .arg(temp.path("run"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    server.wait_for_request(&mut child);
    successful(&cli().arg("pause").arg(temp.path("run")).output().unwrap());
    releases
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .send(())
        .unwrap();
    let paused = child.wait_with_output().unwrap();
    assert_eq!(paused.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&paused.stdout).unwrap()["status"],
        "paused"
    );
    let plan = fs::read(temp.path("run/plan.json")).unwrap();
    fs::write(temp.path("policy.json"), b"invalid replacement declaration").unwrap();
    let resumed = cli()
        .arg("resume")
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(resumed.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&resumed.stdout).unwrap()["status"],
        "completed-with-ineligible-measurements"
    );
    assert_eq!(server.count.load(Ordering::SeqCst), 4);
    assert_eq!(fs::read(temp.path("run/policy.json")).unwrap(), captured);
    assert_eq!(fs::read(temp.path("run/plan.json")).unwrap(), plan);
    let output = cli()
        .arg("decide")
        .arg(temp.path("run"))
        .arg(temp.path("run"))
        .arg("--reference")
        .arg(temp.path("run"))
        .arg("--json")
        .output()
        .unwrap();
    reason(&report(&output, "INCONCLUSIVE", 2), "session_incomplete");
}

#[test]
fn policy_unbound_collector_identity_is_validated_before_emission() {
    let fixture = Fixture::new(workload(1, 1, 3), 0, 0);
    let marker = format!("{:x<64}", "private://synthetic-sensitive");
    for role in ["a", "b", "a2"] {
        rewrite_plan(&fixture.temp.path(role), |plan| {
            plan.as_object_mut().unwrap().remove("policy_sha256");
            plan["collector_sha256"] = json!(marker);
        });
        fs::remove_file(fixture.temp.path(role).join("policy.json")).unwrap();
    }
    let output = fixture.decide(Some("a2"));
    let decision = report(&output, "ERROR", 1);
    reason(&decision, "invalid_evidence");
    assert!(!String::from_utf8(output.stdout).unwrap().contains(&marker));
    for role in ["baseline", "candidate", "reference"] {
        assert_eq!(decision["roles"].get(role), Some(&Value::Null));
    }
}
