use super::{Output, Temp, Value, cli, fs, json, successful};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fixture {
    root: Temp,
    workload: Value,
    runner: Value,
}
impl Fixture {
    fn new() -> Self {
        let root = Temp::new();
        let program = root.path("runner");
        fs::write(&program, b"#!/bin/sh\ntouch LAUNCHED\nexit 99\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&program, root.path("link")).unwrap();
        let workload = json!({
            "kind":"media-workload-v1", "version":1,"name":"synthetic", "modality":"image",
            "request":{"prompt":"red bicycle {unknown}","seeds":[42,43,44],"width":1,"height":1,"steps":1,"images_per_run":1},
            "outputs":[{"placeholder":"output","container":"png","expect":{"width":1,"height":1}}],
            "cells":[{"id":"c1","warmup_runs":1,"measured_runs":3,"pause_between_runs_ms":10}],
            "slots":[{"id":"a","arm":"a"},{"id":"b","arm":"b"},{"id":"b2","arm":"b2"},{"id":"a2","arm":"a2"}],
            "limits":{"run_deadline_ms":100,"setup_deadline_ms":200,"output_bytes":1024,"stdout_bytes":1024,"stderr_bytes":1024},
            "gates":[{"cell":"c1","metric":"end_to_end_us","max_adverse_bps":500,"max_reference_spread_bps":2000}]
        });
        let runner = json!({"kind":"media-runner-v1","version":1,"id":"synthetic","execution":"in-process","program":root.path("link"),"args":["{prompt}","{seed}","{width}x{height}","{steps}","{negative_prompt}","{output}","; touch NEVER | $(exit 1)"],"env":{},"setup":{"args":["{output}"],"deadline_ms":150},"cache":{"declared_state":"unknown","paths":[]},"pins":[]});
        Self {
            root,
            workload,
            runner,
        }
    }
    fn save(&self) {
        fs::write(
            self.root.path("workload.json"),
            serde_json::to_vec(&self.workload).unwrap(),
        )
        .unwrap();
        fs::write(
            self.root.path("runner.json"),
            serde_json::to_vec(&self.runner).unwrap(),
        )
        .unwrap();
    }
    fn invoke(&self) -> Output {
        cli()
            .current_dir(&self.root.0)
            .args(["media", "preflight", "--workload"])
            .arg(self.root.path("workload.json"))
            .arg("--runner")
            .arg(self.root.path("runner.json"))
            .output()
            .unwrap()
    }
    fn report(&self) -> Value {
        self.save();
        let output = self.invoke();
        successful(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn reject(&self, expected: &str) {
        self.save();
        let out = self.invoke();
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn preflight_projects_without_running_or_creating_directories() {
    let f = Fixture::new();
    let report = f.report();
    assert_eq!(
        report["program"]["path"],
        f.root.path("runner").to_str().unwrap()
    );
    assert_eq!(
        report["program"]["sha256"],
        Sha256::digest(fs::read(f.root.path("runner")).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    assert_eq!(report["setup_program"], report["program"]);
    assert_eq!(report["warmup_runs"], 1);
    assert_eq!(report["measured_runs"], 3);
    assert_eq!(report["total_runs"], 4);
    assert_eq!(report["wall_clock_allowance_ms"], 590);
    assert_eq!(report["samples"][0]["phase"], "warmup");
    assert_eq!(report["samples"][1]["seed"], 43);
    assert_eq!(report["samples"][3]["seed"], 42);
    assert_eq!(
        report["samples"][0]["args"],
        json!([
            "red bicycle {unknown}",
            "42",
            "1x1",
            "1",
            "",
            "run-000000/output-00.png",
            "; touch NEVER | $(exit 1)"
        ])
    );
    for path in [
        "LAUNCHED",
        "NEVER",
        "setup",
        "run-000000",
        "run-000001",
        "run-000002",
        "run-000003",
    ] {
        assert!(!f.root.path(path).exists(), "unexpected {path}");
    }
}
#[test]
fn every_record_rejects_unknown_fields_and_positional_arrays() {
    let mut f = Fixture::new();
    f.workload["cells"][0]["request"] = f.workload["request"].clone();
    for pointer in [
        "",
        "/request",
        "/outputs/0",
        "/outputs/0/expect",
        "/cells/0",
        "/cells/0/request",
        "/slots/0",
        "/limits",
        "/gates/0",
    ] {
        let original = f.workload.pointer(pointer).unwrap().clone();
        f.workload.pointer_mut(pointer).unwrap()["unexpected"] = json!(true);
        f.reject("unknown field");
        *f.workload.pointer_mut(pointer).unwrap() = json!([]);
        f.reject("JSON objects");
        *f.workload.pointer_mut(pointer).unwrap() = original;
    }
    f.runner["pins"] = json!([{"path":"weights", "sha256":"0".repeat(64)}]);
    for pointer in ["", "/setup", "/cache", "/pins/0"] {
        let original = f.runner.pointer(pointer).unwrap().clone();
        f.runner.pointer_mut(pointer).unwrap()["unexpected"] = json!(true);
        f.reject("unknown field");
        *f.runner.pointer_mut(pointer).unwrap() = json!([]);
        f.reject("JSON objects");
        *f.runner.pointer_mut(pointer).unwrap() = original;
    }
    f.runner["env"] = json!([]);
    f.reject("environment must be a JSON object");
}
#[test]
fn image_only_and_optional_field_admission() {
    let mut f = Fixture::new();
    for modality in ["audio", "video"] {
        f.workload["modality"] = json!(modality);
        f.reject("not supported yet");
    }
    f.workload["modality"] = json!("image");
    for field in ["frames", "fps", "duration_us", "sample_rate"] {
        f.workload["request"][field] = Value::Null;
        f.reject("unknown field");
        f.workload["request"].as_object_mut().unwrap().remove(field);
    }
    for field in ["markers", "probe", "modality"] {
        f.runner[field] = json!("audio");
        f.reject("unknown field");
        f.runner.as_object_mut().unwrap().remove(field);
    }
    for field in ["negative_prompt", "guidance_milli"] {
        f.workload["request"][field] = Value::Null;
        f.reject("invalid type");
        f.workload["request"].as_object_mut().unwrap().remove(field);
    }
    for field in ["cwd", "setup"] {
        let original = f.runner.as_object_mut().unwrap().remove(field);
        f.runner[field] = Value::Null;
        f.reject("invalid type");
        f.runner.as_object_mut().unwrap().remove(field);
        if let Some(v) = original {
            f.runner[field] = v;
        }
    }
}
#[test]
fn rejects_duplicate_keys_and_unsupported_templates() {
    let mut f = Fixture::new();
    for template in [
        "{oops}",
        "{frames}",
        "{fps}",
        "{duration_s}",
        "{sample_rate}",
        "{marker_file}",
    ] {
        f.runner["args"] = json!(["{output}", template]);
        f.reject("unknown placeholder");
    }
    f.runner["args"] = json!(["{output}", "{prompt"]);
    f.reject("unclosed placeholder");
    f.save();
    for (source, key) in [
        (r#"{"kind":null,"kind":"media-runner-v1"}"#, "kind"),
        (r#"{"env":{"K":"a","K":"b"}}"#, "K"),
    ] {
        fs::write(f.root.path("runner.json"), source).unwrap();
        let out = f.invoke();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains(&format!("duplicate field `{key}`")));
    }
}
#[test]
fn output_contract_and_own_container_extension() {
    let mut f = Fixture::new();
    f.workload["outputs"][0]["container"] = json!("jpeg");
    assert_eq!(
        f.report()["samples"][0]["args"][5],
        "run-000000/output-00.jpeg"
    );
    let output = f.workload["outputs"][0].clone();
    f.workload["outputs"] = json!([{"placeholder":"output_dir","container":"png","expect":{"width":1,"height":1}}, output]);
    f.runner["args"] = json!(["{output_dir}", "{output}"]);
    f.runner["setup"]["args"] = json!(["{output_dir}", "{output}"]);
    assert_eq!(
        f.report()["samples"][0]["args"][1],
        "run-000000/output-00.jpeg"
    );
    f.runner["args"] = json!(["{output_dir}"]);
    f.reject("output declaration");
    f.workload["outputs"].as_array_mut().unwrap().remove(1);
    f.runner["setup"]["args"] = json!(["{output_dir}"]);
    f.workload["request"]["images_per_run"] = json!(2);
    assert_eq!(f.report()["samples"][0]["args"][0], "run-000000/out/");
    f.runner["args"] = json!(["{output}"]);
    f.reject("output declaration");
    f.runner["args"] = json!([]);
    f.reject("output declaration");
    f.workload["outputs"][0]["placeholder"] = json!("output");
    f.runner["args"] = json!(["{output}"]);
    f.runner["setup"]["args"] = json!(["{output}"]);
    f.reject("multiple images require only output_dir");
    f.workload["request"]["images_per_run"] = json!(1);
    f.runner["args"] = json!(["{output}", "{output}"]);
    f.reject("only once");
}
#[test]
fn cell_override_is_a_whole_request() {
    let mut f = Fixture::new();
    f.workload["cells"][0]["request"] = json!({"prompt":"override", "negative_prompt":"avoid", "seeds":[9], "width":32,"height":64,"steps":7,"images_per_run":1});
    let report = f.report();
    assert_eq!(
        report["samples"][0]["args"],
        json!([
            "override",
            "9",
            "32x64",
            "7",
            "avoid",
            "run-000000/output-00.png",
            "; touch NEVER | $(exit 1)"
        ])
    );
    f.workload["cells"][0]["request"]
        .as_object_mut()
        .unwrap()
        .remove("steps");
    f.reject("missing field `steps`");
}
#[test]
fn rejects_invalid_identifiers_slots_gates_and_bounds() {
    let mut f = Fixture::new();
    for (pointer, value, error) in [
        ("/cells/0/id", json!("invalid id"), "cell invalid id"),
        ("/name", json!("x".repeat(65)), "workload.name"),
        ("/gates/0/cell", json!("absent"), "gate cell absent"),
        ("/gates/0/metric", json!("generation_us"), "unknown variant"),
        ("/gates/0/max_adverse_bps", json!(10000), "max_adverse_bps"),
        (
            "/gates/0/max_reference_spread_bps",
            json!(1000001),
            "max_reference_spread_bps",
        ),
        ("/request/width", json!(0), "request.width"),
        ("/request/images_per_run", json!(65), "images_per_run"),
        ("/cells/0/warmup_runs", json!(21), "c1.warmup_runs"),
        ("/cells/0/measured_runs", json!(101), "c1.measured_runs"),
        (
            "/limits/run_deadline_ms",
            json!(0),
            "limits.run_deadline_ms",
        ),
    ] {
        let original = f.workload.pointer(pointer).unwrap().clone();
        *f.workload.pointer_mut(pointer).unwrap() = value;
        f.reject(error);
        *f.workload.pointer_mut(pointer).unwrap() = original;
    }
    f.workload["slots"].as_array_mut().unwrap().pop();
    f.reject("slots require a, b and a2");
}
#[test]
fn program_and_working_directory_admission() {
    let mut f = Fixture::new();
    f.runner["program"] = json!("link");
    assert_eq!(
        f.report()["program"]["path"],
        f.root.path("runner").to_str().unwrap()
    );
    f.runner["cwd"] = json!(f.root.path("absent"));
    f.reject("runner.cwd");
    f.runner.as_object_mut().unwrap().remove("cwd");
    fs::set_permissions(f.root.path("runner"), fs::Permissions::from_mode(0o600)).unwrap();
    f.reject("executable regular file");
    fs::set_permissions(f.root.path("runner"), fs::Permissions::from_mode(0o700)).unwrap();
    f.runner["setup"]["program"] = json!(f.root.path("workload.json"));
    f.reject("executable regular file");
    f.runner["setup"]["program"] = json!(f.root.path("absent"));
    f.reject("resolve program");
}

#[test]
fn large_executable_program_is_admitted() {
    let f = Fixture::new();
    fs::OpenOptions::new()
        .write(true)
        .open(f.root.path("runner"))
        .unwrap()
        .set_len(5 * 1024 * 1024)
        .unwrap();
    let report = f.report();
    assert_eq!(
        report["program"]["path"],
        f.root.path("runner").to_str().unwrap()
    );
    assert_eq!(report["program"], report["setup_program"]);
    assert!(!f.root.path("LAUNCHED").exists());
}

#[test]
fn setup_output_arguments_are_optional_but_must_be_declared() {
    let mut f = Fixture::new();
    for args in [json!([]), json!(["--prefetch"])] {
        f.runner["setup"]["args"] = args.clone();
        assert_eq!(f.report()["setup_args"], args);
    }
    for args in [json!(["{output_dir}"]), json!(["{output}", "{output}"])] {
        f.runner["setup"]["args"] = args;
        f.reject("runner.setup.args:");
    }
    f.runner["setup"]["args"] = json!(["{output}", "{unknown}"]);
    f.reject("runner.setup.args: unknown placeholder");
    f.runner["setup"]["args"] = json!(["{output}"]);
    assert_eq!(f.report()["setup_args"], json!(["setup/output-00.png"]));
    f.workload["cells"][0]["request"] = f.workload["request"].clone();
    f.workload["cells"][0]["request"]["images_per_run"] = json!(2);
    f.reject("cell c1.images_per_run");
    f.workload["request"]["images_per_run"] = json!(2);
    f.workload["cells"][0]["request"]["images_per_run"] = json!(1);
    assert_eq!(
        f.report()["samples"][0]["args"][5],
        "run-000000/output-00.png"
    );
}

fn png(width: u32, height: u32) -> Vec<u8> {
    fn chunk(bytes: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        bytes.extend((data.len() as u32).to_be_bytes());
        let start = bytes.len();
        bytes.extend(tag);
        bytes.extend(data);
        let mut crc = u32::MAX;
        for byte in &bytes[start..] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        bytes.extend((!crc).to_be_bytes());
    }
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = width.to_be_bytes().to_vec();
    header.extend(height.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    chunk(&mut bytes, b"IHDR", &header);
    chunk(
        &mut bytes,
        b"IDAT",
        &[0x78, 0x01, 0x01, 4, 0, 0xfb, 0xff, 0, 0, 0, 0, 0, 4, 0, 1],
    );
    chunk(&mut bytes, b"IEND", &[]);
    bytes
}

fn capture_fixture(script: &str) -> Fixture {
    let mut f = Fixture::new();
    fs::write(
        f.root.path("runner"),
        format!("#!/bin/sh\nset -eu\n{script}\n"),
    )
    .unwrap();
    fs::write(f.root.path("prepared"), png(1, 1)).unwrap();
    fs::write(
        f.root.path("deployment.json"),
        br#"{"model_revision":"fixture","runtime":"sh","hardware":"synthetic","settings":"local"}"#,
    )
    .unwrap();
    f.runner.as_object_mut().unwrap().remove("setup");
    f.runner["args"] = json!(["{output}"]);
    f.runner["cwd"] = json!(f.root.0);
    f.runner["env"] = json!({"PREPARED":f.root.path("prepared"),"ROOT":f.root.0});
    f.workload["limits"]["run_deadline_ms"] = json!(5000);
    f.workload["request"]["seeds"] = json!([42]);
    f
}

impl Fixture {
    fn capture_command(&self) -> std::process::Command {
        self.save();
        let mut command = cli();
        command
            .current_dir(&self.root.0)
            .args(["media", "capture", "--workload"])
            .arg(self.root.path("workload.json"))
            .arg("--runner")
            .arg(self.root.path("runner.json"))
            .arg("--deployment")
            .arg(self.root.path("deployment.json"))
            .args(["--slot", "a", "--out"])
            .arg(self.root.path("capture"));
        command
    }
    fn captured(&self) -> Value {
        let output = self.capture_command().output().unwrap();
        successful(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn inspect_capture(&self) -> Output {
        cli()
            .args(["media", "inspect"])
            .arg(self.root.path("capture"))
            .output()
            .unwrap()
    }
}

#[test]
fn capture_retains_setup_warmups_and_serial_samples_before_offline_inspect() {
    let mut f = capture_fixture(
        "test -f \"$ROOT/capture/receipt.json\"\ntest \"$PWD\" = \"$ROOT\"\nprintf '%s\\n' \"$$\" >> \"$ROOT/pids\"\ncp \"$PREPARED\" \"$1\"",
    );
    f.runner["setup"] = json!({"program":"/bin/sh","args":["-c","test -f \"$ROOT/capture/receipt.json\"; printf setup >> \"$ROOT/setup-count\"; printf setup-out"],"deadline_ms":200});
    let report = f.captured();
    assert_eq!(report["cells"][0]["end_to_end_us"]["n"], 3);
    assert_eq!(report["cells"][0]["eligible"], 3);
    assert_eq!(report["runs"].as_array().unwrap().len(), 4);
    assert_eq!(report["runs"][0]["phase"], "warmup");
    assert_eq!(report["setup"]["process"]["status"], "complete");
    assert_eq!(fs::read(f.root.path("setup-count")).unwrap(), b"setup");
    let pids = fs::read_to_string(f.root.path("pids")).unwrap();
    assert_eq!(
        pids.lines()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(
        report["runs"][1]["outputs"][0]["validation"]["label"],
        "container_checked"
    );
    assert_eq!(
        report["cells"][0]["determinism_by_seed"][0]["deterministic"],
        true
    );
    fs::remove_file(f.root.path("runner")).unwrap();
    fs::remove_file(f.root.path("prepared")).unwrap();
    let inspected = f.inspect_capture();
    successful(&inspected);
    assert_eq!(
        report,
        serde_json::from_slice::<Value>(&inspected.stdout).unwrap()
    );
}

#[test]
fn capture_checks_jpeg_content_without_trusting_extension() {
    let mut f = capture_fixture("cp \"$PREPARED\" \"$1/custom.bin\"");
    let bytes = [
        0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0, 1, 0, 1, 1, 1, 0x11, 0, 0xff, 0xda, 0, 8, 1, 1, 0, 0,
        63, 0, 0, 0xff, 0xd9,
    ];
    fs::write(f.root.path("prepared"), bytes).unwrap();
    f.runner["args"] = json!(["{output_dir}"]);
    f.workload["outputs"] =
        json!([{"placeholder":"output_dir","container":"jpeg","expect":{"width":1,"height":1}}]);
    let report = f.captured();
    assert_eq!(report["cells"][0]["eligible"], 3);
    assert_eq!(
        report["runs"][0]["outputs"][0]["original_name"],
        "custom.bin"
    );
    assert_eq!(
        report["runs"][0]["outputs"][0]["validation"]["container"],
        "jpeg"
    );
}

#[test]
fn capture_preserves_malformed_wrong_dimensions_missing_oversized_and_nonzero_samples() {
    for (script, bytes, status, reason) in [
        (
            "cp \"$PREPARED\" \"$1\"",
            b"not an image".to_vec(),
            "output_invalid",
            "output_invalid",
        ),
        (
            "cp \"$PREPARED\" \"$1\"",
            png(2, 1),
            "output_invalid",
            "output_invalid",
        ),
        ("true", png(1, 1), "output_missing", "output_missing"),
        (
            "cp \"$PREPARED\" \"$1\"",
            vec![0; 2048],
            "output_invalid",
            "output_oversized",
        ),
        (
            "cp \"$PREPARED\" \"$1\"; exit 7",
            png(1, 1),
            "nonzero_exit",
            "",
        ),
    ] {
        let f = capture_fixture(script);
        fs::write(f.root.path("prepared"), &bytes).unwrap();
        let report = f.captured();
        assert_eq!(report["cells"][0]["eligible"], 0);
        assert!(report["cells"][0]["end_to_end_us"].is_null());
        assert_eq!(report["cells"][0]["measured_runs"], 3);
        assert_eq!(report["runs"].as_array().unwrap().len(), 4);
        for run in report["runs"].as_array().unwrap() {
            assert_eq!(run["process"]["status"], status);
            assert_eq!(run["eligible"], false);
            if !reason.is_empty() {
                assert!(run["reasons"].as_array().unwrap().contains(&json!(reason)));
            } else {
                assert_eq!(run["process"]["exit_status"], 7);
            }
        }
        if bytes.len() <= 1024 && script != "true" {
            let retained = &report["runs"][0]["outputs"][0];
            assert_eq!(retained["sha256"], digest(&bytes));
            assert_eq!(
                fs::read(f.root.path(&format!(
                    "capture/run-000000/{}",
                    retained["path"].as_str().unwrap()
                )))
                .unwrap(),
                bytes
            );
            assert!(!f.root.path("capture/run-000000/output-00.png").exists());
        }
        successful(&f.inspect_capture());
    }
}

#[test]
fn capture_stdout_stderr_truncation_is_nonfatal_and_prefixes_are_bounded() {
    let mut f = capture_fixture(
        "printf 123456789abcdef; printf fedcba987654321 >&2; cp \"$PREPARED\" \"$1\"",
    );
    f.workload["limits"]["stdout_bytes"] = json!(5);
    f.workload["limits"]["stderr_bytes"] = json!(7);
    let report = f.captured();
    assert_eq!(report["cells"][0]["eligible"], 3);
    for run in report["runs"].as_array().unwrap() {
        assert_eq!(run["process"]["stdout"]["truncated"], true);
        assert_eq!(run["process"]["stderr"]["truncated"], true);
        assert_eq!(run["process"]["status"], "complete");
    }
    assert_eq!(
        fs::read(f.root.path("capture/run-000000/stdout.bin")).unwrap(),
        b"12345"
    );
    assert_eq!(
        fs::read(f.root.path("capture/run-000000/stderr.bin")).unwrap(),
        b"fedcba9"
    );
}

#[test]
fn capture_output_directory_requires_all_entries_and_retains_lexical_order() {
    for (script, reason) in [
        (
            "cp \"$PREPARED\" \"$1/a\"; cp \"$PREPARED\" \"$1/b\"; touch \"$1/.extra\"",
            "output_count",
        ),
        ("cp \"$PREPARED\" \"$1/a\"", "output_count"),
        (
            "cp \"$PREPARED\" \"$1/a\"; ln -s \"$PREPARED\" \"$1/b\"",
            "output_entry_type",
        ),
        (
            "cp \"$PREPARED\" \"$1/a\"; mkdir \"$1/b\"",
            "output_entry_type",
        ),
        ("cp \"$PREPARED\" \"$1/z\"; cp \"$PREPARED\" \"$1/a\"", ""),
    ] {
        let mut f = capture_fixture(script);
        f.runner["args"] = json!(["{output_dir}"]);
        f.workload["request"]["images_per_run"] = json!(2);
        f.workload["outputs"][0]["placeholder"] = json!("output_dir");
        let report = f.captured();
        if reason.is_empty() {
            assert_eq!(report["cells"][0]["eligible"], 3);
            assert_eq!(report["runs"][0]["outputs"][0]["original_name"], "a");
            assert_eq!(report["runs"][0]["outputs"][1]["original_name"], "z");
        } else {
            assert_eq!(report["cells"][0]["eligible"], 0);
            assert!(
                report["runs"][0]["reasons"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(reason))
            );
        }
    }
}

#[test]
fn capture_records_measurement_cache_growth_separately_from_setup() {
    let mut f = capture_fixture("printf x >> \"$ROOT/cache/data\"; cp \"$PREPARED\" \"$1\"");
    f.runner["cache"]["paths"] = json!([f.root.path("cache")]);
    f.runner["setup"] = json!({"program":"/bin/sh","args":["-c","mkdir \"$ROOT/cache\"; printf setup > \"$ROOT/cache/data\""]});
    let report = f.captured();
    assert_eq!(report["cache_mutated_during_measurement"], true);
    assert_eq!(report["runs"][0]["cache_mutated_during_measurement"], false);
    assert_eq!(report["runs"][1]["cache_mutated_during_measurement"], true);
    assert_eq!(report["runs"][1]["cache_after"][0]["bytes"], 7);
    assert_eq!(report["runs"][1]["cache_after"][0]["complete"], true);
    let before: Value =
        serde_json::from_slice(&fs::read(f.root.path("capture/cache-before.json")).unwrap())
            .unwrap();
    assert_eq!(before[0]["exists"], false);
}

#[test]
fn inspect_rejects_tampered_retained_output_and_stream() {
    for target in ["retained/output-00.png", "stdout.bin"] {
        let f = capture_fixture("cp \"$PREPARED\" \"$1\"");
        f.captured();
        fs::write(
            f.root.path(&format!("capture/run-000001/{target}")),
            b"tampered",
        )
        .unwrap();
        let output = f.inspect_capture();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("hash mismatch"));
    }
}

fn assert_process_stopped(pid: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || status.trim().is_empty() || status.trim().starts_with('Z') {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "descendant {pid} remains running: {status}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn capture_deadline_cleans_descendants_and_retains_every_failure() {
    let mut f = capture_fixture("sleep 60 &\nprintf '%s\\n' \"$!\" >> \"$ROOT/descendants\"\nwait");
    f.workload["limits"]["run_deadline_ms"] = json!(200);
    let report = f.captured();
    assert_eq!(report["runs"].as_array().unwrap().len(), 4);
    assert_eq!(report["cells"][0]["eligible"], 0);
    for run in report["runs"].as_array().unwrap() {
        assert_eq!(run["process"]["status"], "deadline_exceeded");
    }
    let pids = fs::read_to_string(f.root.path("descendants")).unwrap();
    assert_eq!(pids.lines().count(), 4);
    for pid in pids.lines() {
        assert_process_stopped(pid);
    }
}

#[test]
fn capture_cancellation_cleans_descendants_and_does_not_launch_replacements() {
    let f = capture_fixture("sleep 60 &\nprintf '%s\\n' \"$!\" >> \"$ROOT/descendants\"\nwait");
    let child = f
        .capture_command()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !f.root.path("descendants").exists() {
        assert!(std::time::Instant::now() < deadline, "runner never started");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let signal = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cells"][0]["eligible"], 0);
    for run in report["runs"].as_array().unwrap() {
        assert_eq!(run["process"]["status"], "cancelled");
    }
    assert_eq!(report["status"], "cancelled");
    for run in &report["runs"].as_array().unwrap()[1..] {
        assert_eq!(run["process"]["spawned"], false);
        assert_eq!(run["process"]["failure"], "cancelled_before_launch");
        assert!(run["process"]["exit_status"].is_null());
    }
    let pids = fs::read_to_string(f.root.path("descendants")).unwrap();
    assert_eq!(pids.lines().count(), 1);
    assert_process_stopped(pids.trim());
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn rewrite_evidence(fixture: &Fixture, name: &str, change: impl FnOnce(&mut Value)) {
    let path = fixture.root.path(&format!("capture/{name}"));
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(path, &bytes).unwrap();
    let manifest_path = fixture.root.path("capture/capture.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["files"][name] = json!(digest(&bytes));
    manifest["evidence_sha256"] = json!(digest(&serde_json::to_vec(&manifest["files"]).unwrap()));
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

fn inspection_rejected(fixture: &Fixture, expected: &str) {
    let output = fixture.inspect_capture();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains(expected), "{text}");
}

#[test]
fn inspect_rechecks_semantics_when_manifest_hashes_are_rewritten() {
    let invalid = capture_fixture("printf malformed > \"$1\"");
    invalid.captured();
    rewrite_evidence(&invalid, "run-000001/run.json", |run| {
        run["process"]["status"] = json!("complete");
        run["eligible"] = json!(true);
        run["reasons"] = json!([]);
    });
    inspection_rejected(&invalid, "eligibility mismatch");

    let reservation = capture_fixture("cp \"$PREPARED\" \"$1\"");
    reservation.captured();
    rewrite_evidence(&reservation, "run-000001/reservation.json", |record| {
        record["seed"] = json!(999)
    });
    inspection_rejected(&reservation, "reservation projection mismatch");

    let deadline = capture_fixture("cp \"$PREPARED\" \"$1\"");
    deadline.captured();
    rewrite_evidence(&deadline, "run-000001/run.json", |run| {
        run["process"]["end_to_end_us"] = json!(5_000_000);
    });
    inspection_rejected(&deadline, "eligibility mismatch");
}

#[test]
fn inspect_requires_manifest_self_digest_and_all_top_level_entries() {
    for mode in [
        "self_digest",
        "receipt",
        "cache-after-setup.json",
        "setup/setup.json",
    ] {
        let mut fixture = capture_fixture("cp \"$PREPARED\" \"$1\"");
        fixture.runner["setup"] = json!({"program":"/bin/sh","args":["-c","true"]});
        fixture.captured();
        let path = fixture.root.path("capture/capture.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if mode == "self_digest" {
            manifest["evidence_sha256"] = json!("0".repeat(64));
        } else {
            let removed = if mode == "receipt" {
                "receipt.json"
            } else {
                mode
            };
            manifest["files"].as_object_mut().unwrap().remove(removed);
            manifest["evidence_sha256"] =
                json!(digest(&serde_json::to_vec(&manifest["files"]).unwrap()));
            if mode == "receipt" {
                let receipt_path = fixture.root.path("capture/receipt.json");
                let mut receipt: Value =
                    serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
                receipt["collector"]["version"] = json!("tampered");
                fs::write(receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            }
        }
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        inspection_rejected(
            &fixture,
            if mode == "self_digest" {
                "manifest digest mismatch"
            } else {
                "manifest"
            },
        );
    }
}

#[test]
fn typed_evidence_rejects_unknown_fields_even_with_matching_hashes() {
    for name in [
        "receipt.json",
        "run-000000/reservation.json",
        "run-000000/run.json",
        "setup/setup.json",
    ] {
        let mut fixture = capture_fixture("cp \"$PREPARED\" \"$1\"");
        fixture.runner["setup"] = json!({"program":"/bin/sh","args":["-c","true"]});
        fixture.captured();
        rewrite_evidence(&fixture, name, |record| {
            record["unrecognized"] = json!(true)
        });
        inspection_rejected(&fixture, "unknown field");
    }
}

#[test]
fn failed_setup_retains_evidence_stops_samples_and_exits_nonzero() {
    let mut fixture = capture_fixture("touch \"$ROOT/LAUNCHED\"; cp \"$PREPARED\" \"$1\"");
    fixture.runner["setup"] =
        json!({"program":"/bin/sh","args":["-c","printf setup-failure >&2; exit 3"]});
    let output = fixture.capture_command().output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "setup_failed");
    assert_eq!(report["setup"]["process"]["exit_status"], 3);
    assert!(report["runs"].as_array().unwrap().is_empty());
    assert_eq!(report["cells"][0]["eligible"], 0);
    assert!(report["cells"][0]["end_to_end_us"].is_null());
    assert!(!fixture.root.path("LAUNCHED").exists());
    assert!(!fixture.root.path("capture/run-000000").exists());
    assert!(!fixture.root.path("capture/setup/out").exists());
    assert_eq!(
        fs::read(fixture.root.path("capture/setup/stderr.bin")).unwrap(),
        b"setup-failure"
    );
    let inspection = fixture.inspect_capture();
    assert_eq!(inspection.status.code(), Some(1));
    assert_eq!(
        report,
        serde_json::from_slice::<Value>(&inspection.stdout).unwrap()
    );
}

#[test]
fn setup_output_is_renamed_hashed_and_rechecked_offline() {
    let mut fixture = capture_fixture("cp \"$PREPARED\" \"$1\"");
    fixture.runner["setup"] = json!({"args":["{output}"]});
    let report = fixture.captured();
    let output = &report["setup"]["outputs"][0];
    assert_eq!(output["sha256"], digest(&png(1, 1)));
    assert_eq!(output["validation"]["label"], "container_checked");
    assert!(!fixture.root.path("capture/setup/output-00.png").exists());
    assert!(!fixture.root.path("capture/setup/out").exists());
    assert!(!fixture.root.path("capture/run-000000/out").exists());
    fs::write(
        fixture.root.path("capture/setup/retained/output-00.png"),
        b"tampered",
    )
    .unwrap();
    inspection_rejected(&fixture, "hash mismatch");
}

#[test]
fn cache_symlinks_are_observed_without_following_or_stopping_walk() {
    let mut fixture = capture_fixture("cp \"$PREPARED\" \"$1\"");
    fs::create_dir_all(fixture.root.path("cache/blobs")).unwrap();
    fs::create_dir_all(fixture.root.path("cache/snapshots/revision")).unwrap();
    fs::write(fixture.root.path("cache/blobs/a"), [0; 50]).unwrap();
    fs::write(fixture.root.path("cache/blobs/b"), [0; 100]).unwrap();
    fs::write(fixture.root.path("outside"), [0; 200]).unwrap();
    symlink(
        "../../blobs/a",
        fixture.root.path("cache/snapshots/revision/a"),
    )
    .unwrap();
    symlink("missing", fixture.root.path("cache/dangling")).unwrap();
    symlink(
        fixture.root.path("outside"),
        fixture.root.path("cache/external"),
    )
    .unwrap();
    fixture.runner["cache"]["paths"] = json!([fixture.root.path("cache")]);
    let report = fixture.captured();
    assert_eq!(report["cache_mutated_during_measurement"], false);
    for run in report["runs"].as_array().unwrap() {
        let cache = &run["cache_after"][0];
        assert_eq!(cache["complete"], true);
        assert_eq!(cache["files"], 2);
        assert_eq!(cache["bytes"], 150);
        assert_eq!(cache["symlinks"], 3);
        assert_eq!(cache["links"].as_array().unwrap().len(), 3);
        assert!(
            cache["links"]
                .as_array()
                .unwrap()
                .iter()
                .any(|link| link["target"] == "../../blobs/a")
        );
    }
}

#[test]
fn cancellation_interrupts_pause_without_launching_the_next_sample() {
    let mut fixture = capture_fixture("printf x >> \"$ROOT/launches\"; cp \"$PREPARED\" \"$1\"");
    fixture.workload["cells"][0]["pause_between_runs_ms"] = json!(10_000);
    let child = fixture
        .capture_command()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !fixture.root.path("capture/run-000000/run.json").exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let stopped = std::time::Instant::now();
    let output = child.wait_with_output().unwrap();
    assert!(
        stopped.elapsed() < std::time::Duration::from_secs(5),
        "pause ignored cancellation"
    );
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "cancelled");
    assert_eq!(fs::read(fixture.root.path("launches")).unwrap(), b"x");
    for run in &report["runs"].as_array().unwrap()[1..] {
        assert_eq!(run["process"]["spawned"], false);
        assert_eq!(run["process"]["failure"], "cancelled_before_launch");
    }
}

#[test]
fn spawn_failure_and_output_errors_have_codes_without_paths() {
    let fixture = capture_fixture("true");
    fs::write(fixture.root.path("runner"), b"#!/missing-interpreter\n").unwrap();
    let report = fixture.captured();
    for run in report["runs"].as_array().unwrap() {
        assert_eq!(run["process"]["status"], "spawn_failed");
        assert_eq!(run["process"]["spawned"], false);
        assert_eq!(run["process"]["failure"], "spawn_failed");
        assert!(
            run["process"]["detail"]
                .as_str()
                .unwrap()
                .contains("<program>")
        );
        assert!(
            !run["process"]["detail"]
                .as_str()
                .unwrap()
                .contains(fixture.root.0.to_str().unwrap())
        );
        assert!(
            run["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .all(|reason| !reason.as_str().unwrap().contains('/'))
        );
    }
}

#[test]
fn output_directory_rejects_non_utf8_filenames() {
    let mut fixture = capture_fixture("name=$(printf '\\377'); cp \"$PREPARED\" \"$1/$name\"");
    fixture.runner["args"] = json!(["{output_dir}"]);
    fixture.workload["outputs"][0]["placeholder"] = json!("output_dir");
    let report = fixture.captured();
    assert_eq!(report["cells"][0]["eligible"], 0);
    assert!(
        report["runs"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("output_non_utf8"))
    );
}
