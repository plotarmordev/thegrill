use super::*;

#[test]
fn shared_traces_collect_arrivals_and_stop_on_ineligible_responses() {
    for trace in measurement_fixtures::traces() {
        let temp = Temp::new();
        let name = trace.name;
        let server = Server::new(move |mut stream, _, _| {
            header(&mut stream, "text/event-stream");
            let origin = Instant::now();
            for (at, bytes) in &trace.chunks {
                thread::sleep(Duration::from_micros(*at).saturating_sub(origin.elapsed()));
                if stream.write_all(bytes).is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
            }
        });
        let mut work = workload(1, 1, 2);
        work["request"]["profile"] = json!("vllm-fixed-v1");
        work["request"]["output"]["mode"] = json!("exact");
        let output = run(&temp, &server, "run", &work);
        let invalid = matches!(name, "missing-usage" | "early-output" | "error");
        assert_eq!(
            output.status.code(),
            Some(if invalid { 2 } else { 0 }),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            server.count.load(Ordering::SeqCst),
            if invalid { 1 } else { 3 },
            "{name}"
        );
        let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
        if invalid {
            assert_eq!(
                summary["status"],
                if name == "error" {
                    "stopped-after-response-failure"
                } else {
                    "stopped-after-ineligible-response"
                }
            );
            assert!(!temp.path("run/wave-000001").exists());
        }
        let receipt = wave(&temp, "run", 0);
        let timing = &receipt["attempts"][0]["timing"];
        assert!(timing["last_generated_text_us"].is_number());
        assert_eq!(timing["terminal_us"].is_number(), name != "error");
        let comparison = cli()
            .arg("compare")
            .arg(temp.path("run"))
            .arg(temp.path("run"))
            .arg("--json")
            .output()
            .unwrap();
        assert_eq!(
            comparison.status.code(),
            Some(if invalid { 2 } else { 0 }),
            "{name}: {}",
            String::from_utf8_lossy(&comparison.stderr)
        );
        if !invalid {
            let report: Value = serde_json::from_slice(&comparison.stdout).unwrap();
            let measured = wave(&temp, "run", 1);
            let attempt = &measured["attempts"][0];
            let first = attempt["timing"]["first_generated_text_us"]
                .as_u64()
                .unwrap();
            let last = attempt["timing"]["last_generated_text_us"]
                .as_u64()
                .unwrap();
            let rate = &report["baseline"][0]["lane_text_decode_tokens_per_second"][0][0];
            if last == first {
                assert!(rate.is_null());
            } else {
                let expected = 7_000_000.0 / (last - first) as f64;
                assert!((rate.as_f64().unwrap() - expected).abs() <= expected * 1e-9);
            }
        }
    }
}

#[test]
fn new_arrival_fields_and_metric_contract_are_revalidated() {
    use sha2::{Digest, Sha256};
    let digest = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let temp = Temp::new();
    let server = Server::new(normal);
    successful(&run(&temp, &server, "run", &workload(1, 0, 1)));
    let path = temp.path("run/wave-000000/wave.json");
    let original = fs::read(&path).unwrap();
    let receipt: Value = serde_json::from_slice(&original).unwrap();
    let settle = receipt["attempts"][0]["timing"]["settle_us"]
        .as_u64()
        .unwrap();
    for (field, replacement) in [
        ("last_generated_text_us", Value::Null),
        ("last_generated_text_us", json!(settle + 1)),
        ("terminal_us", Value::Null),
        ("terminal_us", json!(0)),
    ] {
        let mut changed = receipt.clone();
        changed["attempts"][0]["timing"][field] = replacement;
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(
            cli()
                .arg("compare")
                .arg(temp.path("run"))
                .arg(temp.path("run"))
                .output()
                .unwrap()
                .status
                .code(),
            Some(1),
            "{field}"
        );
    }
    fs::write(&path, original).unwrap();
    let plan_path = temp.path("run/plan.json");
    let plan = value(&plan_path);
    assert_eq!(plan["version"], 3);
    assert_eq!(plan["metric_contract"], "generated-text-arrival-v2");
    for (version, contract) in [
        (3, Value::Null),
        (3, json!("settlement-v1")),
        (2, json!("generated-text-arrival-v2")),
    ] {
        let mut changed = plan.clone();
        changed["version"] = json!(version);
        changed["metric_contract"] = contract;
        let bytes = serde_json::to_vec(&changed).unwrap();
        fs::write(&plan_path, &bytes).unwrap();
        let plan_hash = digest(&bytes);
        let reservation_path = temp.path("run/wave-000000/reservation.json");
        let mut reservation = value(&reservation_path);
        reservation["plan_sha256"] = json!(plan_hash);
        let bytes = serde_json::to_vec(&reservation).unwrap();
        fs::write(&reservation_path, &bytes).unwrap();
        let mut changed_receipt = receipt.clone();
        changed_receipt["plan_sha256"] = json!(plan_hash);
        changed_receipt["reservation_sha256"] = json!(digest(&bytes));
        fs::write(&path, serde_json::to_vec(&changed_receipt).unwrap()).unwrap();
        let session_path = temp.path("run/session-000000/session.json");
        let mut session = value(&session_path);
        session["plan_sha256"] = json!(plan_hash);
        fs::write(&session_path, serde_json::to_vec(&session).unwrap()).unwrap();
        assert_eq!(
            cli()
                .arg("compare")
                .arg(temp.path("run"))
                .arg(temp.path("run"))
                .output()
                .unwrap()
                .status
                .code(),
            Some(1)
        );
    }
}

#[test]
fn legacy_session_evidence_keeps_settlement_semantics_and_rejects_cross_contract_comparison() {
    use sha2::{Digest, Sha256};
    fn rewrite(path: &Path, value: &Value) -> String {
        let bytes = serde_json::to_vec(value).unwrap();
        fs::write(path, &bytes).unwrap();
        Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    let temp = Temp::new();
    let server = Server::new(normal);
    for name in ["legacy", "current"] {
        successful(&run(&temp, &server, name, &workload(1, 0, 1)));
    }
    let root = temp.path("legacy");
    let mut plan = value(root.join("plan.json"));
    plan["version"] = json!(2);
    plan.as_object_mut().unwrap().remove("metric_contract");
    let plan_hash = rewrite(&root.join("plan.json"), &plan);
    let mut session = value(root.join("session-000000/session.json"));
    session["plan_sha256"] = json!(plan_hash);
    rewrite(&root.join("session-000000/session.json"), &session);
    let mut reservation = value(root.join("wave-000000/reservation.json"));
    reservation["plan_sha256"] = json!(plan_hash);
    let reservation_hash = rewrite(&root.join("wave-000000/reservation.json"), &reservation);
    let path = root.join("wave-000000/wave.json");
    let mut receipt = value(&path);
    receipt["plan_sha256"] = json!(plan_hash);
    receipt["reservation_sha256"] = json!(reservation_hash);
    let timing = receipt["attempts"][0]["timing"].as_object_mut().unwrap();
    timing.remove("last_generated_text_us");
    timing.remove("terminal_us");
    let first = timing["first_generated_text_us"].as_u64().unwrap();
    let settle = timing["settle_us"].as_u64().unwrap();
    rewrite(&path, &receipt);
    let before = fs::read(&path).unwrap();
    let output = cli()
        .arg("compare")
        .arg(&root)
        .arg(&root)
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["baseline"][0]["lane_text_decode_tokens_per_second"],
        json!([[null]])
    );
    let rate = report["baseline"][0]["lane_decode_tokens_per_second"][0][0]
        .as_f64()
        .unwrap();
    let expected = 7_000_000.0 / (settle - first) as f64;
    assert!((rate - expected).abs() <= expected * 1e-9);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        cli()
            .arg("compare")
            .arg(&root)
            .arg(temp.path("current"))
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
}
