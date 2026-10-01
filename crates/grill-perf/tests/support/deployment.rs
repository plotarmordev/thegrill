use super::*;

const SOURCE: &str = "0123456789abcdef0123456789abcdef01234567";
const GAP_MS: u64 = 60_000;
const STEADY: [u64; 8] = [
    400_000, 410_000, 390_000, 405_000, 395_000, 402_000, 398_000, 400_000,
];
const SHIFT: &str = "baseline deployment shifted between capture periods";
/// A named edit that turns an admissible capture into one that must be rejected.
type Forgery = (&'static str, fn(&Path));

fn binary_source() -> String {
    let output = command().arg("--version").output().unwrap();
    let line = String::from_utf8(output.stdout).unwrap();
    let (_, source) = line.trim_end().rsplit_once("(source ").unwrap();
    source.trim_end_matches(')').to_owned()
}

fn edit_capture(root: &Path, edit: impl FnOnce(&mut Value)) {
    rewrite(root, edit, |_| ());
}

fn as_v1(root: &Path) {
    edit_capture(root, |capture| {
        capture["version"] = json!(1);
        capture["kind"] = json!("performance-capture-v1");
        for field in ["collector", "client_placement", "workload"] {
            capture.as_object_mut().unwrap().remove(field);
        }
    });
}

fn check_none(temp: &Temp, baseline: &str, declaration: &Path, name: &str) -> Output {
    command()
        .arg("check")
        .arg(temp.path(baseline))
        .arg("--deployment")
        .arg(declaration)
        .args(["--change", "none", "--out"])
        .arg(temp.path(name))
        .arg("--json")
        .output()
        .unwrap()
}

fn compare_captures(
    temp: &Temp,
    baseline: &str,
    candidate: &str,
    reference: Option<&str>,
) -> Output {
    let mut cli = command();
    cli.arg("compare")
        .arg(temp.path(baseline))
        .arg(temp.path(candidate));
    if let Some(reference) = reference {
        cli.arg("--reference").arg(temp.path(reference));
    }
    cli.arg("--json").output().unwrap()
}

fn rejected(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(decoded(output)["result"], "INVALID");
}

#[test]
fn capture_v3_records_collector_placement_and_builtin_while_v1_still_checks() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let declaration = deployment(&temp, "serving.json", "same");
    let output = command()
        .args([
            "baseline",
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
        ])
        .args(["--workload", "portable-v1", "--client-placement", "network"])
        .arg("--deployment")
        .arg(&declaration)
        .arg("--out")
        .arg(temp.path("portable"))
        .args(["--local-http", "--json"])
        .output()
        .unwrap();
    let report = decoded(&output);
    assert_eq!(report["baseline_ready"], true, "{report}");
    // cap-reached fixes every request's output count, so the completion floor is known.
    assert!(report["baseline_accounting"]["minimum_full_capture_tokens_per_second"].is_number());
    // The scope describes the captured built-in, not the exact400 baseline workloads.
    assert!(
        !report["scope"].as_str().unwrap().contains("exact400"),
        "{report}"
    );
    let capture = read_json(&temp.path("portable/capture.json"));
    assert_eq!(capture["version"], 3);
    assert_eq!(capture["kind"], "performance-capture-v3");
    assert_eq!(capture["workload"], "portable-v1");
    assert_eq!(capture["client_placement"], "network");
    assert_eq!(capture["collector"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(capture["collector"]["source_commit"], binary_source());
    assert!(!capture["collector"]["target"].as_str().unwrap().is_empty());
    assert_eq!(
        fs::read(temp.path("portable/workload.json")).unwrap(),
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/portable-v1.json")).unwrap()
    );

    // Only v3 names a built-in; a v1 manifest must hold baseline-v1 bytes.
    copy_tree(&temp.path("portable"), &temp.path("legacy-portable"));
    as_v1(&temp.path("legacy-portable"));
    let count = server.count.load(Ordering::SeqCst);
    rejected(&check_none(
        &temp,
        "legacy-portable",
        &declaration,
        "legacy-portable-control",
    ));
    assert_eq!(server.count.load(Ordering::SeqCst), count);

    // A v1 capture holds baseline-v1 bytes, so forge it from an explicit baseline-v1 capture.
    let current = command()
        .args([
            "baseline",
            "--endpoint",
            &server.endpoint,
            "--model",
            "fixture",
        ])
        .args([
            "--workload",
            "baseline-v1",
            "--client-placement",
            "same-host",
        ])
        .arg("--deployment")
        .arg(&declaration)
        .arg("--out")
        .arg(temp.path("current"))
        .args(["--local-http", "--json"])
        .output()
        .unwrap();
    assert!(current.status.success());
    copy_tree(&temp.path("current"), &temp.path("legacy"));
    as_v1(&temp.path("legacy"));
    let control = decoded(&check_none(&temp, "legacy", &declaration, "legacy-control"));
    assert_ne!(control["result"], "INVALID", "{control}");
    let candidate = read_json(&temp.path("legacy-control/capture.json"));
    assert_eq!(candidate["version"], 1);
    assert!(candidate.get("collector").is_none());
    assert_eq!(
        decoded(&compare_captures(&temp, "legacy", "legacy-control", None)),
        control
    );

    // Without --workload, a baseline captures baseline-v2, which turns thinking off for every template family.
    assert!(
        baseline(&temp, &server, &declaration, "default")
            .status
            .success()
    );
    assert_eq!(
        read_json(&temp.path("default/capture.json"))["workload"],
        "baseline-v2"
    );
    assert_eq!(
        fs::read(temp.path("default/workload.json")).unwrap(),
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/baseline-v2.json")).unwrap()
    );
}

#[test]
fn deployment_check_admits_a_changed_deployment_only_from_the_same_recorded_source() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let other = Server::new(|stream, _, body| {
        assert_eq!(body["model"], "other-model");
        response(stream, Some(400), false);
    });
    let before = deployment(&temp, "before.json", "same");
    let after = deployment(&temp, "after.json", "changed");
    assert!(baseline(&temp, &server, &before, "a").status.success());
    let dispatched = || server.count.load(Ordering::SeqCst) + other.count.load(Ordering::SeqCst);
    let count = dispatched();
    let check = |baseline: &str, declaration: &Path, name: &str, placement: &str| {
        command()
            .arg("check")
            .arg(temp.path(baseline))
            .arg("--deployment")
            .arg(declaration)
            .args(["--change", "deployment", "--client-placement", placement])
            .args([
                "--endpoint",
                &other.endpoint,
                "--local-http",
                "--model",
                "other-model",
            ])
            .arg("--out")
            .arg(temp.path(name))
            .arg("--json")
            .output()
            .unwrap()
    };
    rejected(&check("a", &before, "identical", "same-host"));
    rejected(&check("a", &after, "network", "network"));
    let forgeries: [Forgery; 3] = [
        ("v1", as_v1),
        ("unrecorded", |root| {
            edit_capture(root, |capture| {
                capture["collector"]["source_commit"] = json!("unrecorded");
            });
        }),
        ("other-version", |root| {
            rewrite(
                root,
                |capture| capture["collector"]["version"] = json!("0.0.0-other"),
                |plan| plan["tool_version"] = json!("0.0.0-other"),
            );
        }),
    ];
    for (name, forge) in forgeries {
        copy_tree(&temp.path("a"), &temp.path(name));
        forge(&temp.path(name));
        rejected(&check(
            name,
            &after,
            &format!("{name}-candidate"),
            "same-host",
        ));
    }
    let restricted = command()
        .arg("check")
        .arg(temp.path("a"))
        .arg("--deployment")
        .arg(&after)
        .args(["--change", "settings", "--model", "other-model", "--out"])
        .arg(temp.path("restricted"))
        .output()
        .unwrap();
    assert_eq!(restricted.status.code(), Some(1));
    assert!(!temp.path("restricted").exists());
    assert_eq!(dispatched(), count);

    // Another platform's binary: the gate stays for other changes, not for deployments.
    let foreign = hash(b"other-platform-collector");
    repin_collector(&temp.path("a"), &foreign);
    rejected(&check_none(&temp, "a", &before, "foreign-control"));
    assert_eq!(dispatched(), count);
    let output = check("a", &after, "b", "same-host");
    if binary_source() == "unrecorded" {
        rejected(&output);
        assert_eq!(dispatched(), count);
        return;
    }
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(decoded(&output)["result"], "PENDING");
    assert_eq!(other.count.load(Ordering::SeqCst), 32);
    let candidate = read_json(&temp.path("b/capture.json"));
    assert_eq!(candidate["change"], "deployment");
    assert_eq!(candidate["model"], "other-model");
    assert_eq!(candidate["endpoint"], other.endpoint);
    assert_ne!(candidate["collector_sha256"], foreign);
    let offline = compare_captures(&temp, "a", "b", None);
    assert_eq!(offline.status.code(), Some(2));
    assert_eq!(decoded(&offline)["result"], "INCONCLUSIVE");
}

/// Rewrites a check of A as the declared `change` to `declaration`.
fn redeclare(root: &Path, change: &str, declaration: &Value) {
    write_json(&root.join("deployment.json"), declaration);
    let digest = file_hash(&root.join("deployment.json"));
    rewrite(
        root,
        |capture| {
            capture["change"] = json!(change);
            capture["deployment_sha256"] = json!(digest);
        },
        |plan| plan["deployment"] = declaration.clone(),
    );
}

/// Baseline A, a deployment candidate B and the unchanged control A2, all offline-retimeable.
fn triple(temp: &Temp) {
    let prompt_tokens = Arc::new(AtomicUsize::new(4));
    let served = prompt_tokens.clone();
    let server = Server::new(move |stream, _, _| {
        respond(
            stream,
            "1 2",
            Some(400),
            false,
            served.load(Ordering::SeqCst) as u64,
        );
    });
    let declaration = deployment(temp, "a.json", "same");
    assert!(baseline(temp, &server, &declaration, "a").status.success());
    for (name, tokens) in [("b", 9), ("a2", 4)] {
        prompt_tokens.store(tokens, Ordering::SeqCst);
        let output = check_none(temp, "a", &declaration, name);
        assert_ne!(decoded(&output)["result"], "INVALID");
    }
    // B is an unchanged capture rewritten as another platform's deployment (its request bodies,
    // and so its model, stay); dispatching a real one needs a recorded-source build and is
    // covered by the admission test.
    redeclare(
        &temp.path("b"),
        "deployment",
        &json!({"model_revision":"synthetic-revision-q4", "runtime":"other-fixture",
            "hardware":"other-cpu", "settings":"same"}),
    );
    let endpoint = "http://127.0.0.2:9/v1/chat/completions";
    let collector = hash(b"other-platform-collector");
    rewrite(
        &temp.path("b"),
        |capture| {
            capture["endpoint"] = json!(endpoint);
            capture["collector_sha256"] = json!(collector);
        },
        |plan| {
            plan["endpoint"] = json!(endpoint);
            plan["collector_sha256"] = json!(collector);
        },
    );
    for name in ["a", "b", "a2"] {
        edit_capture(&temp.path(name), |capture| {
            capture["collector"]["source_commit"] = json!(SOURCE);
        });
    }
}

/// Retimes A as a steady baseline, then B and A2 after the given gaps.
fn schedule(temp: &Temp, candidate: [u64; 8], control: [u64; 8], gaps: [u64; 2]) {
    let finish = |name: &str| {
        read_json(&temp.path(&format!("{name}/capture.json")))["acquisitions"][7]["finished_unix_ms"]
            .as_u64()
            .unwrap()
    };
    retime(&temp.path("a"), [400_000; 8], None);
    let a = file_hash(&temp.path("a/capture.json"));
    retime(
        &temp.path("b"),
        candidate,
        Some((&a, finish("a") + gaps[0])),
    );
    retime(&temp.path("a2"), control, Some((&a, finish("b") + gaps[1])));
}

#[test]
fn deployment_verdict_needs_the_same_direction_against_both_baseline_periods() {
    let temp = Temp::new();
    triple(&temp);
    for (candidate, control, expected, exit, shifted) in [
        (STEADY.map(|us| us / 2), STEADY, "IMPROVED", 0, false),
        (STEADY.map(|us| us * 2), STEADY, "REGRESSED", 2, false),
        (
            STEADY.map(|us| us / 2),
            STEADY.map(|us| us / 4),
            "INCONCLUSIVE",
            2,
            true,
        ),
        (
            STEADY.map(|us| us * 2),
            STEADY.map(|us| us * 4),
            "INCONCLUSIVE",
            2,
            true,
        ),
    ] {
        schedule(&temp, candidate, control, [GAP_MS, GAP_MS]);
        let output = compare_captures(&temp, "a", "b", Some("a2"));
        let report = decoded(&output);
        assert_eq!(report["result"], expected, "{report}");
        assert_eq!(output.status.code(), Some(exit));
        assert_eq!(report["kind"], "performance-deployment-comparison-v1");
        assert_eq!(
            report["deployment"]["baseline"]["endpoint"],
            read_json(&temp.path("a/capture.json"))["endpoint"]
        );
        assert_eq!(
            report["deployment"]["candidate"]["endpoint"],
            "http://127.0.0.2:9/v1/chat/completions"
        );
        assert!(
            report["deployment"]["candidate_against_reference"]["model_based_interval_percent"]
                .is_array()
        );
        for (side, tokens) in [("baseline", 4.0), ("candidate", 9.0)] {
            assert_eq!(report["deployment"][side]["median_prompt_tokens"], tokens);
        }
        let reasons = report["reasons"].as_array().unwrap();
        assert_eq!(
            reasons.iter().any(|reason| reason == SHIFT),
            shifted,
            "{report}"
        );
        assert!(
            reasons
                .iter()
                .any(|reason| reason.as_str().unwrap().contains("prompt tokens")),
            "{report}"
        );
        let text = command()
            .arg("compare")
            .arg(temp.path("a"))
            .arg(temp.path("b"))
            .arg("--reference")
            .arg(temp.path("a2"))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(text.stdout)
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .split(':')
                .next(),
            Some(display_label(expected))
        );
    }
    let missing = compare_captures(&temp, "a", "b", None);
    let report = decoded(&missing);
    assert_eq!(report["result"], "INCONCLUSIVE", "{report}");
    assert_eq!(missing.status.code(), Some(2));
    assert!(report["model_based_interval_percent"].is_null());
}

#[test]
fn deployment_reference_must_be_a_later_unchanged_control_of_the_baseline() {
    let temp = Temp::new();
    triple(&temp);
    let faster = STEADY.map(|us| us / 2);
    for gaps in [[GAP_MS, GAP_MS / 2], [GAP_MS / 2, GAP_MS]] {
        schedule(&temp, faster, STEADY, gaps);
        rejected(&compare_captures(&temp, "a", "b", Some("a2")));
    }
    schedule(&temp, faster, STEADY, [GAP_MS, GAP_MS]);
    assert_eq!(
        decoded(&compare_captures(&temp, "a", "b", Some("a2")))["result"],
        "IMPROVED"
    );
    rejected(&compare_captures(&temp, "a", "b", Some("b")));
    rejected(&compare_captures(&temp, "a", "a2", Some("b")));
    copy_tree(&temp.path("a2"), &temp.path("settings-control"));
    redeclare(
        &temp.path("settings-control"),
        "settings",
        &json!({"model_revision":"synthetic-revision", "runtime":"fixture",
            "hardware":"offline-cpu", "settings":"changed"}),
    );
    rejected(&compare_captures(&temp, "a", "b", Some("settings-control")));
    // A2 captured between A and B is not a later control.
    let a = file_hash(&temp.path("a/capture.json"));
    let finished = read_json(&temp.path("a/capture.json"))["acquisitions"][7]["finished_unix_ms"]
        .as_u64()
        .unwrap();
    retime(&temp.path("a2"), STEADY, Some((&a, finished + GAP_MS)));
    rejected(&compare_captures(&temp, "a", "b", Some("a2")));
}

#[test]
fn offline_deployment_pair_needs_equal_placement_and_one_recorded_collector_source() {
    let temp = Temp::new();
    triple(&temp);
    schedule(&temp, STEADY.map(|us| us / 2), STEADY, [GAP_MS, GAP_MS]);
    let forgeries: [Forgery; 3] = [
        ("placement", |root| {
            edit_capture(root, |capture| {
                capture["client_placement"] = json!("network");
            });
        }),
        ("source", |root| {
            edit_capture(root, |capture| {
                capture["collector"]["source_commit"] = json!("f".repeat(40));
            });
        }),
        ("version", |root| {
            rewrite(
                root,
                |capture| capture["collector"]["version"] = json!("0.0.0-other"),
                |plan| plan["tool_version"] = json!("0.0.0-other"),
            );
        }),
    ];
    for (name, forge) in forgeries {
        copy_tree(&temp.path("b"), &temp.path(name));
        forge(&temp.path(name));
        rejected(&compare_captures(&temp, "a", name, Some("a2")));
    }
    // An unchanged control keeps the baseline's placement and a collector identity that matches
    // the native plans it claims to have produced.
    let forgeries: [Forgery; 3] = [
        ("moved", |root| {
            edit_capture(root, |capture| {
                capture["client_placement"] = json!("network");
            });
        }),
        ("claimed", |root| {
            edit_capture(root, |capture| {
                capture["collector"]["version"] = json!("0.0.0-other");
            });
        }),
        ("anonymous", |root| {
            edit_capture(root, |capture| {
                capture.as_object_mut().unwrap().remove("collector");
            });
        }),
    ];
    for (name, forge) in forgeries {
        copy_tree(&temp.path("a2"), &temp.path(name));
        forge(&temp.path(name));
        rejected(&compare_captures(&temp, "a", name, None));
    }
    for name in ["a", "b"] {
        edit_capture(&temp.path(name), |capture| {
            capture["collector"]["source_commit"] = json!("unrecorded");
        });
    }
    schedule(&temp, STEADY.map(|us| us / 2), STEADY, [GAP_MS, GAP_MS]);
    rejected(&compare_captures(&temp, "a", "b", Some("a2")));
}
