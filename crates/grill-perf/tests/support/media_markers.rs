use super::*;

fn line(event: &str, ns: u64) -> String {
    format!("{{\"grill_media\":1,\"event\":\"{event}\",\"monotonic_ns\":{ns}}}\n")
}
fn valid() -> String {
    [
        ("runner_started", 100),
        ("model_loaded", 1100),
        ("generation_started", 2101),
        ("generation_finished", 5102),
        ("output_written", 6102),
    ]
    .into_iter()
    .map(|(event, ns)| line(event, ns))
    .collect()
}
fn fixture(script: &str) -> Fixture {
    let mut f = capture_fixture(&format!("cp \"$PREPARED\" \"$1\"\n{script}"));
    f.runner["markers"] = json!("file");
    f.runner["args"] = json!(["{output}", "{marker_file}"]);
    f.runner["env"]["MARKERS"] = json!(valid());
    f
}

#[test]
fn marker_admission_and_projection() {
    let mut f = fixture("printf '%s' \"$MARKERS\" > \"$2\"");
    assert_eq!(
        f.report()["samples"][0]["args"][1],
        "run-000000/markers.ndjson"
    );
    f.runner["args"] = json!(["{output}"]);
    f.reject("exactly once");
    f.runner["args"] = json!(["{output}", "{marker_file}{marker_file}"]);
    f.reject("exactly once");
    f.runner["args"] = json!(["{output}", "{marker_file}"]);
    f.runner["setup"] = json!({"args":["{marker_file}"]});
    f.reject("forbidden");
    f.runner.as_object_mut().unwrap().remove("setup");
    let mut gate = f.workload["gates"][0].clone();
    gate["metric"] = json!("generation_us");
    f.workload["gates"]
        .as_array_mut()
        .unwrap()
        .push(gate.clone());
    f.report();
    f.workload["gates"].as_array_mut().unwrap().push(gate);
    f.reject("duplicate");
    f.workload["gates"].as_array_mut().unwrap().pop();
    f.runner.as_object_mut().unwrap().remove("markers");
    f.reject("requires runner.markers");
}

#[test]
fn marker_capture_inspect_and_tampering() {
    let f = fixture("printf '%s' \"$MARKERS\" > \"$2\"");
    let report = f.captured();
    assert_eq!(report["cells"][0]["markers_by_status"], json!({"valid":3}));
    assert_eq!(report["cells"][0]["generation_us"]["mean"], 3.0);
    assert_eq!(report["cells"][0]["load_to_generation_us"]["mean"], 2.0);
    let run = &report["runs"][0];
    assert_eq!(run["markers"]["events"][4]["offset_us"], 6);
    let inspected = f.inspect_capture();
    successful(&inspected);
    assert_eq!(
        report,
        serde_json::from_slice::<Value>(&inspected.stdout).unwrap()
    );
    let path = f.root.path("capture/run-000000/markers.ndjson");
    fs::write(&path, line("runner_started", 0)).unwrap();
    inspection_rejected(&f, "retained evidence hash mismatch");
    fs::remove_file(path).unwrap();
    inspection_rejected(&f, "retained evidence hash mismatch");
}

#[test]
fn marker_failures_never_change_eligibility() {
    let start = line("runner_started", 100);
    let variants = [
        ("", "missing", String::new()),
        ("ln -s \"$PREPARED\" \"$2\"", "unreadable", String::new()),
        (
            "dd if=/dev/zero of=\"$2\" bs=20480 count=1 2>/dev/null",
            "oversized",
            String::new(),
        ),
        (
            "printf '%s' \"$MARKERS\" > \"$2\"",
            "invalid",
            line("unknown", 0),
        ),
        (
            "printf '%s' \"$MARKERS\" > \"$2\"",
            "invalid",
            start.clone() + &start,
        ),
        (
            "printf '%s' \"$MARKERS\" > \"$2\"",
            "invalid",
            line("generation_finished", 100) + &line("generation_started", 100),
        ),
        (
            "printf '%s' \"$MARKERS\" > \"$2\"",
            "invalid",
            start.clone() + &line("model_loaded", 99),
        ),
        (
            "printf '%s' \"$MARKERS\" > \"$2\"",
            "invalid",
            start.clone() + &line("output_written", 1_000_000_000_000_000_000),
        ),
        (
            "printf '%s' \"$MARKERS\" > \"$2\"",
            "invalid",
            start.trim_end().into(),
        ),
        (
            "printf '%s' \"$MARKERS\" > \"$2\"",
            "invalid",
            start.replace("\"grill_media\":1", "\"grill_media\":1,\"extra\":0"),
        ),
        (
            "printf '%s' \"$MARKERS\" > \"$2\"",
            "invalid",
            start.replace("\"grill_media\":1", "\"grill_media\":2"),
        ),
        ("printf '%s' \"$MARKERS\" > \"$2\"", "valid", start),
    ];
    for (script, status, bytes) in variants {
        let mut f = fixture(script);
        f.runner["env"]["MARKERS"] = json!(bytes);
        let report = f.captured();
        for run in report["runs"].as_array().unwrap() {
            assert_eq!(run["eligible"], true, "{status}");
            assert_eq!(run["markers"]["status"], status);
            assert!(run["markers"]["generation_us"].is_null());
            assert!(run["markers"]["load_to_generation_us"].is_null());
            if status == "invalid" {
                assert!(
                    run["markers"]["detail"]
                        .as_str()
                        .unwrap()
                        .starts_with("line ")
                );
            }
        }
        successful(&f.inspect_capture());
        if status == "missing" {
            fs::write(f.root.path("capture/run-000000/markers.ndjson"), valid()).unwrap();
            inspection_rejected(&f, "retained evidence hash mismatch");
        }
    }
}

#[test]
fn marker_generation_gates_and_missing_endpoints() {
    for (candidate, code, outcome) in [
        ("fast", 0, "PASS"),
        ("slow", 3, "REGRESSION"),
        ("missing", 2, "INCONCLUSIVE"),
        ("prefix", 2, "INCONCLUSIVE"),
        ("one-missing", 2, "INCONCLUSIVE"),
        ("one-prefix", 2, "INCONCLUSIVE"),
    ] {
        let mut f = comparison_fixture(3, false);
        f.runner["markers"] = json!("file");
        f.runner["args"] = json!(["{output}", "{marker_file}"]);
        f.runner["env"]["START"] = json!(line("generation_started", 0));
        f.runner["env"]["FAST"] = json!(line("generation_finished", 100));
        f.runner["env"]["SLOW"] = json!(line("generation_finished", 900));
        fs::write(f.root.path("runner"), b"#!/bin/sh\ncp \"$PREPARED\" \"$1\"\ncase \"$2\" in */run-000000/*) case \"$DELAY\" in one-missing) exit 0;; one-prefix) printf '%s' \"$START\" > \"$2\"; exit 0;; esac;; esac\n[ \"$DELAY\" = missing ] && exit 0\nprintf '%s' \"$START\" > \"$2\"\n[ \"$DELAY\" = prefix ] && exit 0\nif [ \"$DELAY\" = slow ]; then printf '%s' \"$SLOW\" >> \"$2\"; else printf '%s' \"$FAST\" >> \"$2\"; fi\n").unwrap();
        f.workload["gates"][0]["max_adverse_bps"] = json!(9999);
        let mut gate = f.workload["gates"][0].clone();
        gate["metric"] = json!("generation_us");
        gate["max_adverse_bps"] = json!(500);
        f.workload["gates"].as_array_mut().unwrap().push(gate);
        for (slot, variant) in [("a", "reference"), ("b", candidate), ("a2", "reference")] {
            f.capture_timed_slot(slot, variant);
            f.set_slot_times(slot, |_| 1000);
        }
        let report = f.comparison(false, code);
        assert_eq!(report["gates"][0]["outcome"], "PASS");
        assert_eq!(report["gates"][1]["outcome"], outcome);
        if code == 2 {
            assert_eq!(report["gates"][1]["reason"], "markers_unavailable");
        }
        if code == 0 {
            assert_eq!(
                report["gates"][1]["reference"][0],
                json!({"numerator":100,"denominator":1000})
            );
        }
    }
}

#[test]
fn reference_adapter_capture_inspect_compare_and_forwarding() {
    let mut f = comparison_fixture(3, false);
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    successful(&python);
    let python = String::from_utf8(python.stdout).unwrap().trim().to_owned();
    let adapter =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/media/mflux_markers.py");
    fs::create_dir(f.root.path("fakegen-0.0.dist-info")).unwrap();
    fs::write(
        f.root.path("fakegen-0.0.dist-info/METADATA"),
        "Name: fakegen\nVersion: 0.0\n",
    )
    .unwrap();
    fs::write(
        f.root.path("fakegen-0.0.dist-info/entry_points.txt"),
        "[console_scripts]\nfake-generate = fakegen:main\n",
    )
    .unwrap();
    fs::write(f.root.path("fakegen.py"), "import os, sys, time, shutil\n\ndef denoise():\n    time.sleep(float(os.environ.get('DELAY', '0')))\n\ndef main():\n    denoise()\n    denoise()\n    shutil.copyfile(os.environ['PREPARED'], sys.argv[sys.argv.index('--output') + 1])\n    return int(os.environ.get('EXIT', '0'))\n").unwrap();
    f.runner["program"] = json!(python);
    f.runner["markers"] = json!("file");
    f.runner["env"]["PYTHONPATH"] = json!(f.root.0);
    f.runner["args"] = json!([
        "-S",
        adapter,
        "--grill-marker-file",
        "{marker_file}",
        "--grill-entry",
        "fake-generate",
        "--grill-output-flag=--output",
        "--grill-hook",
        "generation_started=before:fakegen:denoise",
        "--grill-hook",
        "generation_finished=after:fakegen:denoise",
        "--",
        "--output",
        "{output}"
    ]);
    f.workload["gates"][0]["metric"] = json!("generation_us");
    f.workload["gates"][0]["max_adverse_bps"] = json!(5000);
    for (slot, delay) in [("a", "0.08"), ("b", "0.005"), ("a2", "0.08")] {
        f.capture_timed_slot(slot, delay);
    }
    let report = f.comparison(false, 0);
    let markers = &report["acquisitions"][0]["runs"][0]["markers"];
    assert_eq!(markers["status"], "valid");
    assert_eq!(
        markers["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["event"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("runner_started"),
            json!("generation_started"),
            json!("generation_finished"),
            json!("output_written")
        ]
    );
    let output = std::process::Command::new(&python)
        .arg("-S")
        .arg(&adapter)
        .args([
            "--grill-entry",
            "fake-generate",
            "--grill-output-flag=--output",
            "--",
            "--output",
        ])
        .arg(f.root.path("forwarded.png"))
        .env("PYTHONPATH", &f.root.0)
        .env("PREPARED", f.root.path("prepared"))
        .env("EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    assert!(!f.root.path("markers.ndjson").exists());
    assert!(f.root.path("forwarded.png").exists());
}

#[test]
fn marker_metrics_apply_to_audio_and_video() {
    for mut f in [audio_fixture(), video_fixture(false)] {
        f.runner["markers"] = json!("file");
        f.runner["args"] = json!(["{output}", "{marker_file}"]);
        f.runner["env"]["MARKERS"] = json!(valid());
        fs::write(
            f.root.path("runner"),
            b"#!/bin/sh\ncp \"$PREPARED\" \"$1\"\nprintf '%s' \"$MARKERS\" > \"$2\"\n",
        )
        .unwrap();
        let report = f.captured();
        assert_eq!(report["cells"][0]["generation_us"]["mean"], 3.0);
        let inspected = f.inspect_capture();
        successful(&inspected);
        assert_eq!(
            report,
            serde_json::from_slice::<Value>(&inspected.stdout).unwrap()
        );
    }
}

#[test]
fn marker_record_must_match_declaration() {
    let f = fixture("printf '%s' \"$MARKERS\" > \"$2\"");
    f.captured();
    rewrite_evidence(&f, "run-000000/run.json", |run| {
        run["markers"] = Value::Null;
    });
    inspection_rejected(&f, "marker record declaration mismatch");
}
