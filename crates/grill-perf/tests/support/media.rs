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
    png_with_text(width, height, &[])
}

fn png_with_text(width: u32, height: u32, text: &[u8]) -> Vec<u8> {
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
    if !text.is_empty() {
        chunk(&mut bytes, b"tEXt", text);
    }
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
    assert_eq!(report["cells"][0]["eligible"], 0);
    assert!(report["cells"][0]["end_to_end_us"].is_null());
    assert_eq!(
        report["cells"][0]["ineligible_by_reason"]["cache_mutated_during_measurement"],
        3
    );
    successful(&f.inspect_capture());
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
    rewrite_manifest(&f, |manifest| manifest["status"] = json!("complete"));
    inspection_rejected(&f, "capture status mismatch");
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn manifest_digest(manifest: &Value) -> String {
    digest(
        &serde_json::to_vec(&(
            &manifest["version"],
            &manifest["kind"],
            &manifest["status"],
            &manifest["finished_unix_ms"],
            &manifest["runs"],
            &manifest["files"],
        ))
        .unwrap(),
    )
}

fn rewrite_evidence(fixture: &Fixture, name: &str, change: impl FnOnce(&mut Value)) {
    rewrite_capture_file(&fixture.root.path("capture"), name, change);
}

fn rewrite_capture_file(root: &std::path::Path, name: &str, change: impl FnOnce(&mut Value)) {
    let path = root.join(name);
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(path, &bytes).unwrap();
    let manifest_path = root.join("capture.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["files"][name] = json!(digest(&bytes));
    manifest["evidence_sha256"] = json!(manifest_digest(&manifest));
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

fn rewrite_capture_receipt(root: &std::path::Path, change: impl FnOnce(&mut Value)) {
    // Keep the forged receipt binding coherent to isolate later admission checks.
    rewrite_capture_file(root, "receipt.json", change);
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("capture.json")).unwrap()).unwrap();
    let receipt_sha256 = &manifest["files"]["receipt.json"];
    for name in manifest["files"]
        .as_object()
        .unwrap()
        .keys()
        .filter(|name| name.ends_with("/run.json"))
    {
        rewrite_capture_file(root, name, |run| {
            run["receipt_sha256"] = receipt_sha256.clone()
        });
    }
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

    let cache = capture_fixture("cp \"$PREPARED\" \"$1\"");
    cache.captured();
    rewrite_evidence(&cache, "run-000001/run.json", |run| {
        run["cache_mutated_during_measurement"] = json!(true);
        run["reasons"] = json!([]);
        run["eligible"] = json!(true);
    });
    inspection_rejected(&cache, "cache eligibility reason mismatch");

    let status = capture_fixture("cp \"$PREPARED\" \"$1\"");
    status.captured();
    rewrite_manifest(&status, |manifest| {
        manifest["status"] = json!("setup_failed")
    });
    inspection_rejected(&status, "capture status mismatch");
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
            manifest["evidence_sha256"] = json!(manifest_digest(&manifest));
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
    rewrite_manifest(&fixture, |manifest| manifest["status"] = json!("complete"));
    inspection_rejected(&fixture, "capture status mismatch");
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
fn spawn_failure_has_machine_codes_and_separate_diagnostics() {
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
                .contains("spawn ")
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

// APFS refuses non-UTF-8 names, so only Linux can create this fixture.
#[cfg(target_os = "linux")]
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

fn rewrite_manifest(fixture: &Fixture, change: impl FnOnce(&mut Value)) {
    rewrite_capture_manifest(&fixture.root.path("capture"), change);
}

fn rewrite_capture_manifest(root: &std::path::Path, change: impl FnOnce(&mut Value)) {
    let path = root.join("capture.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut manifest);
    manifest["evidence_sha256"] = json!(manifest_digest(&manifest));
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

#[test]
fn inspect_checks_json_file_hashes_without_manifest_rewriting() {
    let fixture = capture_fixture("cp \"$PREPARED\" \"$1\"");
    fixture.captured();
    let path = fixture.root.path("capture/run-000001/run.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["process"]["end_to_end_us"] =
        json!(record["process"]["end_to_end_us"].as_u64().unwrap() + 1);
    fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    inspection_rejected(&fixture, "retained evidence hash mismatch");
}

#[test]
fn inspect_rechecks_forged_container_validation_with_matching_hashes() {
    let fixture = capture_fixture("printf malformed > \"$1\"");
    fixture.captured();
    rewrite_evidence(&fixture, "run-000001/run.json", |record| {
        record["process"]["status"] = json!("complete");
        record["eligible"] = json!(true);
        record["reasons"] = json!([]);
        record["outputs"][0]["validation"] = json!({
            "label":"container_checked", "container":"png", "width":1, "height":1, "reason":null
        });
    });
    inspection_rejected(&fixture, "retained output validation mismatch");
}

#[test]
fn inspect_rechecks_setup_process_success_with_matching_hashes() {
    let mut fixture = capture_fixture("cp \"$PREPARED\" \"$1\"");
    fixture.runner["setup"] = json!({"program":"/bin/sh","args":["-c","true"]});
    fixture.captured();
    rewrite_evidence(&fixture, "setup/setup.json", |record| {
        record["process"]["exit_status"] = json!(7);
    });
    inspection_rejected(&fixture, "process eligibility mismatch");
}

#[test]
fn inspect_rejects_extra_manifest_members_with_matching_hashes() {
    let fixture = capture_fixture("cp \"$PREPARED\" \"$1\"");
    fixture.captured();
    fs::write(fixture.root.path("capture/unexpected.json"), b"{}").unwrap();
    rewrite_manifest(&fixture, |manifest| {
        manifest["files"]["unexpected.json"] = json!(digest(b"{}"));
    });
    inspection_rejected(&fixture, "manifest completeness mismatch");
}

#[test]
fn incomplete_cache_observation_withholds_measured_eligibility() {
    let mut fixture = capture_fixture("cp \"$PREPARED\" \"$1\"");
    let mut deepest = fixture.root.path("cache");
    for _ in 0..66 {
        deepest.push("next");
    }
    fs::create_dir_all(deepest).unwrap();
    fixture.runner["cache"]["paths"] = json!([fixture.root.path("cache")]);
    let report = fixture.captured();
    assert!(report["cache_mutated_during_measurement"].is_null());
    assert_eq!(report["cells"][0]["eligible"], 0);
    assert!(report["cells"][0]["end_to_end_us"].is_null());
    assert_eq!(
        report["cells"][0]["ineligible_by_reason"]["cache_observation_incomplete"],
        3
    );
    for record in &report["runs"].as_array().unwrap()[1..] {
        assert_eq!(record["process"]["status"], "complete");
        assert_eq!(record["eligible"], false);
        assert!(record["cache_mutated_during_measurement"].is_null());
        assert_eq!(record["cache_after"][0]["complete"], false);
    }
    let inspected = fixture.inspect_capture();
    successful(&inspected);
    assert_eq!(
        report,
        serde_json::from_slice::<Value>(&inspected.stdout).unwrap()
    );
}

#[test]
fn retained_directory_exists_before_the_runner_starts() {
    let fixture =
        capture_fixture("test -d \"$(dirname \"$1\")/retained\"; cp \"$PREPARED\" \"$1\"");
    let report = fixture.captured();
    assert_eq!(report["cells"][0]["eligible"], 3);
}

fn comparison_fixture(measured: u32, b2: bool) -> Fixture {
    let mut f = capture_fixture("sleep \"$DELAY\"; cp \"$PREPARED\" \"$1\"");
    f.workload["cells"][0]["warmup_runs"] = json!(0);
    f.workload["cells"][0]["measured_runs"] = json!(measured);
    f.workload["cells"][0]["pause_between_runs_ms"] = json!(0);
    f.workload["gates"][0]["max_reference_spread_bps"] = json!(1000000);
    if !b2 {
        f.workload["slots"].as_array_mut().unwrap().remove(2);
    }
    f
}

impl Fixture {
    fn slot_command(&mut self, slot: &str, delay: &str) -> std::process::Command {
        self.runner["env"]["DELAY"] = json!(delay);
        self.save();
        let mut command = cli();
        command
            .args(["media", "capture", "--workload"])
            .arg(self.root.path("workload.json"))
            .arg("--runner")
            .arg(self.root.path("runner.json"))
            .arg("--deployment")
            .arg(self.root.path("deployment.json"))
            .args(["--slot", slot, "--out"])
            .arg(self.root.path(slot));
        command
    }

    fn capture_slot(&mut self, slot: &str, delay: &str) {
        successful(&self.slot_command(slot, delay).output().unwrap());
    }

    fn compare_slots(&self, b2: bool) -> Output {
        let mut command = cli();
        command
            .args(["media", "compare", "--a"])
            .arg(self.root.path("a"))
            .arg("--b")
            .arg(self.root.path("b"))
            .arg("--a2")
            .arg(self.root.path("a2"));
        if b2 {
            command.arg("--b2").arg(self.root.path("b2"));
        }
        command.output().unwrap()
    }

    fn comparison(&self, b2: bool, code: i32) -> Value {
        let output = self.compare_slots(b2);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn comparison_rejected(&self, b2: bool, reason: &str) {
        comparison_rejected(&self.compare_slots(b2), reason);
    }
}

fn comparison_rejected(output: &Output, reason: &str) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stdout.is_empty(),
        "invalid evidence produced a verdict"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(reason), "{stderr}");
}

#[test]
fn compare_pass_reports_pooled_statistics_and_hash_facts() {
    let mut f = comparison_fixture(3, false);
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 0);
    assert_eq!(report["kind"], "media-comparison-v1");
    assert_eq!(report["axis"], "same-checkpoint");
    assert_eq!(report["outcome"], "PASS");
    let gate = &report["gates"][0];
    assert_eq!(gate["reference_summary"]["end_to_end_us"]["n"], 6);
    assert_eq!(gate["candidate_summary"]["end_to_end_us"]["n"], 3);
    assert_eq!(
        gate["reference_summary"]["determinism_by_seed"][0]["deterministic"],
        true
    );
    assert_eq!(gate["cross_arm_hash_equality"][0]["equal"], true);
    let summary = &gate["reference_summary"];
    let mean = summary["end_to_end_us"]["mean"].as_f64().unwrap();
    assert_eq!(
        summary["seconds_per_image"].as_f64().unwrap(),
        mean / 1_000_000.0
    );
    assert_eq!(
        summary["images_per_second_end_to_end"].as_f64().unwrap(),
        1_000_000.0 / mean
    );
    assert_eq!(report, f.comparison(false, 0));
}

#[test]
fn compare_regression_reports_gate_direction() {
    let mut f = comparison_fixture(3, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.3");
    f.capture_slot("a2", "0");
    let report = f.comparison(false, 3);
    assert_eq!(report["gates"][0]["outcome"], "REGRESSION");
    assert_eq!(report["outcome"], "REGRESSION");
}

#[test]
fn compare_changed_model_caps_regression_without_hiding_gate() {
    let mut f = comparison_fixture(3, false);
    f.capture_slot("a", "0");
    let deployment = fs::read(f.root.path("deployment.json")).unwrap();
    let mut candidate: Value = serde_json::from_slice(&deployment).unwrap();
    candidate["model_revision"] = json!("other-checkpoint");
    fs::write(
        f.root.path("deployment.json"),
        serde_json::to_vec(&candidate).unwrap(),
    )
    .unwrap();
    f.capture_slot("b", "0.3");
    fs::write(f.root.path("deployment.json"), deployment).unwrap();
    f.capture_slot("a2", "0");
    let report = f.comparison(false, 2);
    assert_eq!(report["gates"][0]["outcome"], "REGRESSION");
    assert_eq!(report["axis"], "changed-model");
    assert_eq!(report["outcome"], "INCONCLUSIVE");
    assert_eq!(report["reasons"], json!(["changed_model_axis"]));
    assert_eq!(
        report["claim"],
        "observed-envelope-direction-under-declared-bounds-not-output-quality"
    );
}

#[test]
fn compare_unavailable_model_and_reference_identity_withhold_direction() {
    let mut f = comparison_fixture(3, false);
    fs::write(f.root.path("deployment.json"), b"{}").unwrap();
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 2);
    assert_eq!(report["axis"], "unavailable");
    assert_eq!(report["reference_identity"]["status"], "unavailable");
    assert_eq!(report["gates"][0]["outcome"], "PASS");
    assert_eq!(
        report["reasons"],
        json!([
            "model_identity_unavailable",
            "reference_identity_unavailable"
        ])
    );
}

#[test]
fn compare_insufficient_pooled_reference_samples_retains_statistics() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 2);
    assert_eq!(
        report["gates"][0]["reference_summary"]["end_to_end_us"]["n"],
        2
    );
    assert_eq!(report["gates"][0]["reason"], "insufficient_samples");
}

#[test]
fn compare_b2_pools_candidate_samples_and_requires_the_slot() {
    let mut f = comparison_fixture(2, true);
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("b2", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(true, 0);
    assert_eq!(
        report["gates"][0]["candidate_summary"]["end_to_end_us"]["n"],
        4
    );
    assert_eq!(report["acquisitions"][2]["slot"]["arm"], "b2");
    assert_eq!(f.compare_slots(false).status.code(), Some(1));
}

#[test]
fn compare_reference_spread_withholds_direction() {
    let mut f = comparison_fixture(3, false);
    fs::write(
        f.root.path("runner"),
        b"#!/bin/sh\nsleep \"$(cat \"$ROOT/delay\")\"\ncp \"$PREPARED\" \"$1\"\n",
    )
    .unwrap();
    f.workload["gates"][0]["max_reference_spread_bps"] = json!(1000);
    fs::write(f.root.path("delay"), b"0").unwrap();
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.3");
    fs::write(f.root.path("delay"), b"0.3").unwrap();
    f.capture_slot("a2", "0");
    let report = f.comparison(false, 2);
    assert_eq!(report["gates"][0]["reason"], "ReferenceSpreadExceeded");
}

#[test]
fn compare_rejects_reference_return_captured_before_candidate() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("a2", "0");
    f.capture_slot("b", "0.001");
    f.comparison_rejected(
        false,
        "chronology requires b to finish no later than a2 starts",
    );
}

#[test]
fn compare_rejects_overlapping_captures() {
    let mut f = comparison_fixture(1, false);
    f.workload["limits"]["run_deadline_ms"] = json!(60000);
    fs::write(f.root.path("runner"), b"#!/bin/sh\nif [ \"$DELAY\" = wait ]; then while [ ! -f \"$ROOT/release\" ]; do sleep 0.01; done; fi\ncp \"$PREPARED\" \"$1\"\n").unwrap();
    let child = f
        .slot_command("a", "wait")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !f.root.path("a/receipt.json").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "capture never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    f.capture_slot("b", "0");
    fs::write(f.root.path("release"), b"").unwrap();
    successful(&child.wait_with_output().unwrap());
    f.capture_slot("a2", "wait");
    f.comparison_rejected(
        false,
        "chronology requires a to finish no later than b starts",
    );
}

#[test]
fn compare_rejects_different_workload_bytes() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    let workload = f.workload.clone();
    f.workload["request"]["prompt"] = json!("different workload");
    f.capture_slot("b", "0.001");
    f.workload = workload;
    f.capture_slot("a2", "0");
    f.comparison_rejected(false, "captures must share byte-identical workloads");
}

#[test]
fn compare_rejects_reference_runner_declaration_drift() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    f.runner["id"] = json!("changed-reference");
    f.capture_slot("a2", "0");
    f.comparison_rejected(
        false,
        "A and A2 runner bytes, program or setup hash, or deployment differ",
    );
}

#[test]
fn compare_rejects_reference_program_drift() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    fs::write(
        f.root.path("runner"),
        b"#!/bin/sh\ncp \"$PREPARED\" \"$1\"\n",
    )
    .unwrap();
    f.capture_slot("a2", "0");
    f.comparison_rejected(
        false,
        "A and A2 runner bytes, program or setup hash, or deployment differ",
    );
}

#[test]
fn compare_rejects_reference_deployment_drift() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    fs::write(f.root.path("deployment.json"), b"{}").unwrap();
    f.capture_slot("a2", "0");
    f.comparison_rejected(
        false,
        "A and A2 runner bytes, program or setup hash, or deployment differ",
    );
}

#[test]
fn compare_rejects_tampered_retained_output() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    f.capture_slot("a2", "0");
    fs::write(
        f.root.path("b/run-000000/retained/output-00.png"),
        b"tampered",
    )
    .unwrap();
    f.comparison_rejected(false, "retained output hash mismatch");
}

#[test]
fn compare_rejects_wrong_arm_with_matching_chronology_and_runner_binding() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("b", "0");
    f.capture_slot("a", "0.001");
    f.capture_slot("a2", "0");
    let output = cli()
        .args(["media", "compare", "--a"])
        .arg(f.root.path("b"))
        .arg("--b")
        .arg(f.root.path("a"))
        .arg("--a2")
        .arg(f.root.path("a2"))
        .output()
        .unwrap();
    comparison_rejected(
        &output,
        "capture slot arm does not match its comparison position",
    );
}

#[test]
fn compare_rejects_unchanged_candidate() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0");
    f.comparison_rejected(
        false,
        "B must differ from A in runner, program, setup or deployment",
    );
}

#[test]
fn inspect_requires_completion_timestamp_and_rejects_reversed_clock() {
    for missing in [true, false] {
        let f = capture_fixture("cp \"$PREPARED\" \"$1\"");
        f.captured();
        rewrite_manifest(&f, |manifest| {
            if missing {
                manifest.as_object_mut().unwrap().remove("finished_unix_ms");
            } else {
                manifest["finished_unix_ms"] = json!(0);
            }
        });
        inspection_rejected(
            &f,
            if missing {
                "finished_unix_ms"
            } else {
                "completion precedes"
            },
        );
    }
}

#[test]
fn compare_rejects_candidate_return_runner_drift() {
    let mut f = comparison_fixture(3, true);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    f.capture_slot("b2", "0.002");
    f.capture_slot("a2", "0");
    f.comparison_rejected(
        true,
        "B2 must match B runner, program, setup and deployment",
    );
}

#[test]
fn compare_failed_samples_are_not_hidden_by_pooling() {
    let mut f = comparison_fixture(3, false);
    fs::write(
        f.root.path("runner"),
        b"#!/bin/sh\nif [ \"$DELAY\" = 0 ]; then cp \"$PREPARED\" \"$1\"; fi\n",
    )
    .unwrap();
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    f.capture_slot("a2", "0");
    let report = f.comparison(false, 2);
    assert_eq!(report["gates"][0]["candidate_summary"]["eligible"], 0);
    assert_eq!(
        report["gates"][0]["candidate_summary"]["ineligible_by_reason"]["output_missing"],
        3
    );
    assert!(report["gates"][0]["candidate_summary"]["end_to_end_us"].is_null());
}

#[test]
fn compare_requires_complete_captures() {
    let mut f = comparison_fixture(3, false);
    f.runner["setup"] = json!({"program":"/bin/sh","args":["-c","exit 7"]});
    for (slot, delay) in [("a", "0"), ("b", "0.3"), ("a2", "0")] {
        let output = f.slot_command(slot, delay).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["status"], "setup_failed");
    }
    f.comparison_rejected(false, "comparison requires complete captures");
}

#[test]
fn compare_two_candidate_samples_are_below_the_boundary() {
    let mut f = comparison_fixture(2, false);
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 2);
    let gate = &report["gates"][0];
    assert_eq!(gate["reference_summary"]["eligible"], 4);
    assert_eq!(gate["candidate_summary"]["eligible"], 2);
    assert_eq!(gate["reason"], "insufficient_samples");
}

#[test]
fn compare_one_ineligible_sample_withholds_direction_despite_sufficient_survivors() {
    let mut f = comparison_fixture(4, false);
    fs::write(f.root.path("runner"), b"#!/bin/sh\nif [ \"$DELAY\" = 0 ] && [ ! -f \"$ROOT/skipped\" ]; then touch \"$ROOT/skipped\"; exit 0; fi\nsleep \"$DELAY\"\ncp \"$PREPARED\" \"$1\"\n").unwrap();
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 2);
    let gate = &report["gates"][0];
    assert_eq!(gate["candidate_summary"]["eligible"], 3);
    assert_eq!(gate["candidate_summary"]["measured_runs"], 4);
    assert_eq!(
        gate["candidate_summary"]["ineligible_by_reason"]["output_missing"],
        1
    );
    assert_eq!(gate["candidate_summary"]["end_to_end_us"]["n"], 3);
    assert_eq!(gate["reason"], "insufficient_samples");
    assert_eq!(gate["cross_arm_hash_equality"][0]["equal"], true);
}

#[test]
fn compare_counts_but_excludes_failed_warmups_from_gates_and_hashes() {
    let mut f = comparison_fixture(3, false);
    f.workload["cells"][0]["warmup_runs"] = json!(1);
    fs::write(f.root.path("runner"), b"#!/bin/sh\ncase \"$1\" in */run-000000/*) exit 7;; esac\nsleep \"$DELAY\"\ncp \"$PREPARED\" \"$1\"\n").unwrap();
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 0);
    let gate = &report["gates"][0];
    assert_eq!(gate["reference_summary"]["warmup_runs"], 2);
    assert_eq!(gate["candidate_summary"]["warmup_runs"], 1);
    assert_eq!(gate["reference_summary"]["measured_runs"], 6);
    assert_eq!(gate["candidate_summary"]["eligible"], 3);
    assert_eq!(gate["cross_arm_hash_equality"][0]["equal"], true);
}

#[test]
fn compare_excludes_eligible_slow_warmups_from_envelopes() {
    let mut f = comparison_fixture(3, false);
    f.workload["cells"][0]["warmup_runs"] = json!(1);
    fs::write(f.root.path("runner"), b"#!/bin/sh\ncase \"$1\" in */run-000000/*) sleep 1;; *) sleep \"$DELAY\";; esac\ncp \"$PREPARED\" \"$1\"\n").unwrap();
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 0);
    assert_eq!(
        report["gates"][0]["candidate_summary"]["end_to_end_us"]["n"],
        3
    );
    assert_eq!(report["gates"][0]["reference_summary"]["warmup_runs"], 2);
}

#[test]
fn compare_worst_gate_wins_when_later_cell_regresses() {
    let mut f = comparison_fixture(3, false);
    let mut cell = f.workload["cells"][0].clone();
    cell["id"] = json!("c2");
    cell["request"] = f.workload["request"].clone();
    cell["request"]["steps"] = json!(2);
    f.workload["cells"].as_array_mut().unwrap().push(cell);
    let mut gate = f.workload["gates"][0].clone();
    gate["cell"] = json!("c2");
    f.workload["gates"].as_array_mut().unwrap().push(gate);
    f.runner["args"] = json!(["{output}", "{steps}"]);
    fs::write(f.root.path("runner"), b"#!/bin/sh\nif [ \"$2\" = 1 ]; then sleep \"$DELAY\"; elif [ \"$DELAY\" = 0 ]; then sleep 0.3; fi\ncp \"$PREPARED\" \"$1\"\n").unwrap();
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 3);
    assert_eq!(report["gates"][0]["outcome"], "PASS");
    assert_eq!(report["gates"][1]["outcome"], "REGRESSION");
    assert_eq!(report["outcome"], "REGRESSION");
}

#[test]
fn compare_candidate_model_unavailable_is_an_independent_cap() {
    let mut f = comparison_fixture(3, false);
    f.capture_slot("a", "0.3");
    let deployment = fs::read(f.root.path("deployment.json")).unwrap();
    let mut candidate: Value = serde_json::from_slice(&deployment).unwrap();
    candidate.as_object_mut().unwrap().remove("model_revision");
    fs::write(
        f.root.path("deployment.json"),
        serde_json::to_vec(&candidate).unwrap(),
    )
    .unwrap();
    f.capture_slot("b", "0");
    fs::write(f.root.path("deployment.json"), deployment).unwrap();
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 2);
    assert_eq!(report["gates"][0]["outcome"], "PASS");
    assert_eq!(report["reference_identity"]["status"], "declared_match");
    assert_eq!(report["reasons"], json!(["model_identity_unavailable"]));
}

#[test]
fn compare_reference_identity_uses_each_shared_declaration_reason() {
    for field in ["runtime", "hardware", "settings"] {
        let mut f = comparison_fixture(3, false);
        let mut deployment: Value =
            serde_json::from_slice(&fs::read(f.root.path("deployment.json")).unwrap()).unwrap();
        deployment.as_object_mut().unwrap().remove(field);
        fs::write(
            f.root.path("deployment.json"),
            serde_json::to_vec(&deployment).unwrap(),
        )
        .unwrap();
        f.capture_slot("a", "0.3");
        f.capture_slot("b", "0");
        f.capture_slot("a2", "0.3");
        let report = f.comparison(false, 2);
        assert_eq!(report["axis"], "same-checkpoint");
        assert_eq!(report["gates"][0]["outcome"], "PASS");
        assert_eq!(report["reasons"], json!(["reference_identity_unavailable"]));
        assert_eq!(
            report["reference_identity"]["reasons"],
            json!([
                format!("reference: baseline {field} declaration unavailable"),
                format!("reference: {field} declaration unavailable"),
            ])
        );
    }
}

#[test]
fn compare_admits_equal_wall_clock_boundaries() {
    let mut f = comparison_fixture(3, false);
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let receipt: Value =
        serde_json::from_slice(&fs::read(f.root.path("b/receipt.json")).unwrap()).unwrap();
    rewrite_capture_manifest(&f.root.path("a"), |manifest| {
        manifest["finished_unix_ms"] = receipt["started_unix_ms"].clone();
    });
    assert_eq!(f.comparison(false, 0)["outcome"], "PASS");
}

#[test]
fn inspect_and_compare_reject_unhashed_completion_tampering() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    f.capture_slot("a2", "0");
    let path = f.root.path("a/capture.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for (field, value) in [
        (
            "finished_unix_ms",
            json!(original["finished_unix_ms"].as_u64().unwrap() + 1),
        ),
        ("status", json!("cancelled")),
        ("runs", json!(2)),
        ("files", json!({})),
        ("evidence_sha256", json!("0".repeat(64))),
    ] {
        let mut manifest = original.clone();
        manifest[field] = value;
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let inspected = cli()
            .args(["media", "inspect"])
            .arg(f.root.path("a"))
            .output()
            .unwrap();
        comparison_rejected(&inspected, "capture manifest digest mismatch");
        f.comparison_rejected(false, "capture manifest digest mismatch");
    }
}

#[test]
fn inspect_rejects_rehashed_interval_shorter_than_process_durations() {
    let f = capture_fixture("sleep 0.3; cp \"$PREPARED\" \"$1\"");
    f.captured();
    let receipt: Value =
        serde_json::from_slice(&fs::read(f.root.path("capture/receipt.json")).unwrap()).unwrap();
    rewrite_manifest(&f, |manifest| {
        manifest["finished_unix_ms"] = receipt["started_unix_ms"].clone()
    });
    inspection_rejected(
        &f,
        "capture interval is shorter than its retained process durations",
    );
}

#[test]
fn compare_rejects_mixed_collector_binaries() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    f.capture_slot("a2", "0");
    rewrite_capture_receipt(&f.root.path("b"), |receipt| {
        receipt["collector"]["sha256"] = json!("0".repeat(64));
    });
    f.comparison_rejected(
        false,
        "captures must use the same collector binary and platform",
    );
}

#[test]
fn compare_rejects_mixed_platforms() {
    for field in ["os", "arch"] {
        let mut f = comparison_fixture(1, false);
        f.capture_slot("a", "0");
        f.capture_slot("b", "0.001");
        f.capture_slot("a2", "0");
        rewrite_capture_receipt(&f.root.path("b"), |receipt| {
            receipt["platform"][field] = json!("different");
        });
        f.comparison_rejected(
            false,
            "captures must use the same collector binary and platform",
        );
    }
}

#[test]
fn compare_and_inspect_reject_clone_relabel_with_full_record_and_stream_evidence() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    let copied = std::process::Command::new("cp")
        .arg("-R")
        .arg(f.root.path("a"))
        .arg(f.root.path("a2"))
        .status()
        .unwrap();
    assert!(copied.success());
    let candidate: Value =
        serde_json::from_slice(&fs::read(f.root.path("b/capture.json")).unwrap()).unwrap();
    let started = candidate["finished_unix_ms"].as_u64().unwrap();
    rewrite_capture_file(&f.root.path("a2"), "receipt.json", |receipt| {
        receipt["slot"] = json!({"id":"a2","arm":"a2"});
        receipt["started_unix_ms"] = json!(started);
    });
    rewrite_capture_manifest(&f.root.path("a2"), |manifest| {
        manifest["finished_unix_ms"] = json!(started + 1000);
    });
    let inspected = cli()
        .args(["media", "inspect"])
        .arg(f.root.path("a2"))
        .output()
        .unwrap();
    comparison_rejected(
        &inspected,
        "run record is bound to a different capture receipt",
    );
    f.comparison_rejected(false, "run record is bound to a different capture receipt");
}

#[test]
fn compare_admits_identical_durations_and_outputs_with_distinct_receipts() {
    let mut f = comparison_fixture(3, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    f.capture_slot("a2", "0");
    // Equal synthetic durations exercise the collision boundary without timing luck.
    for slot in ["a", "b", "a2"] {
        for index in 0..3 {
            rewrite_capture_file(
                &f.root.path(slot),
                &format!("run-{index:06}/run.json"),
                |run| {
                    run["process"]["end_to_end_us"] = json!(1000);
                },
            );
        }
    }
    let report = f.comparison(false, 0);
    assert_eq!(
        report["gates"][0]["reference_summary"]["end_to_end_us"]["mean"],
        1000.0
    );
    assert_eq!(
        report["gates"][0]["candidate_summary"]["end_to_end_us"]["mean"],
        1000.0
    );
    assert_eq!(
        report["gates"][0]["cross_arm_hash_equality"][0]["equal"],
        true
    );
}

#[test]
fn compare_rejects_repeated_capture_receipts() {
    let mut f = comparison_fixture(1, false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    let output = cli()
        .args(["media", "compare", "--a"])
        .arg(f.root.path("a"))
        .arg("--b")
        .arg(f.root.path("b"))
        .arg("--a2")
        .arg(f.root.path("a"))
        .output()
        .unwrap();
    comparison_rejected(
        &output,
        "repeated capture receipt is not an independent acquisition",
    );
}

#[test]
fn compare_equal_pin_sets_ignore_declaration_order() {
    let mut f = comparison_fixture(3, false);
    let pins = json!([{"path":"weights","sha256":"a".repeat(64)}, {"path":"config","sha256":"b".repeat(64)}]);
    f.runner["pins"] = pins.clone();
    f.capture_slot("a", "0.3");
    f.runner["pins"].as_array_mut().unwrap().reverse();
    f.capture_slot("b", "0");
    f.runner["pins"] = pins;
    f.capture_slot("a2", "0.3");
    assert_eq!(f.comparison(false, 0)["axis"], "same-checkpoint");
}

#[test]
fn compare_pin_hash_differences_cap_both_gate_directions() {
    for (reference, candidate, direction) in [("0.3", "0", "PASS"), ("0", "0.3", "REGRESSION")] {
        let mut f = comparison_fixture(3, false);
        f.runner["pins"] = json!([{"path":"weights","sha256":"a".repeat(64)}]);
        f.capture_slot("a", reference);
        f.runner["pins"][0]["sha256"] = json!("b".repeat(64));
        f.capture_slot("b", candidate);
        f.runner["pins"][0]["sha256"] = json!("a".repeat(64));
        f.capture_slot("a2", reference);
        let report = f.comparison(false, 2);
        assert_eq!(report["axis"], "changed-model");
        assert_eq!(report["gates"][0]["outcome"], direction);
        assert_eq!(report["reasons"], json!(["changed_model_axis"]));
    }
}

#[test]
fn compare_pin_path_difference_changes_axis() {
    let mut f = comparison_fixture(3, false);
    f.runner["pins"] = json!([{"path":"weights","sha256":"a".repeat(64)}]);
    f.capture_slot("a", "0.3");
    f.runner["pins"][0]["path"] = json!("other-weights");
    f.capture_slot("b", "0");
    f.runner["pins"][0]["path"] = json!("weights");
    f.capture_slot("a2", "0.3");
    assert_eq!(f.comparison(false, 2)["axis"], "changed-model");
}

#[test]
fn compare_one_sided_pins_do_not_override_matching_revision() {
    let mut f = comparison_fixture(3, false);
    f.capture_slot("a", "0.3");
    f.runner["pins"] = json!([{"path":"weights","sha256":"a".repeat(64)}]);
    f.capture_slot("b", "0");
    f.runner["pins"] = json!([]);
    f.capture_slot("a2", "0.3");
    assert_eq!(f.comparison(false, 0)["axis"], "same-checkpoint");
}

fn setup_comparison_fixture(b2: bool) -> Fixture {
    let mut f = comparison_fixture(1, b2);
    let program = f.root.path("setup");
    fs::write(&program, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    f.workload["limits"]["setup_deadline_ms"] = json!(5000);
    f.runner["setup"] = json!({"program":program,"args":[]});
    f
}

#[test]
fn compare_rejects_reference_setup_program_drift() {
    let mut f = setup_comparison_fixture(false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    fs::write(f.root.path("setup"), b"#!/bin/sh\ntrue\n").unwrap();
    f.capture_slot("a2", "0");
    f.comparison_rejected(
        false,
        "A and A2 runner bytes, program or setup hash, or deployment differ",
    );
}

#[test]
fn compare_rejects_reference_setup_argument_drift() {
    let mut f = setup_comparison_fixture(false);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    f.runner["setup"]["args"] = json!(["changed"]);
    f.capture_slot("a2", "0");
    f.comparison_rejected(
        false,
        "A and A2 runner bytes, program or setup hash, or deployment differ",
    );
}

#[test]
fn compare_admits_candidate_changed_only_in_setup_program_bytes() {
    let mut f = setup_comparison_fixture(false);
    f.capture_slot("a", "0");
    fs::write(f.root.path("setup"), b"#!/bin/sh\ntrue\n").unwrap();
    f.capture_slot("b", "0");
    fs::write(f.root.path("setup"), b"#!/bin/sh\nexit 0\n").unwrap();
    f.capture_slot("a2", "0");
    assert_eq!(
        f.comparison(false, 2)["gates"][0]["reason"],
        "insufficient_samples"
    );
}

#[test]
fn compare_rejects_candidate_return_setup_program_drift() {
    let mut f = setup_comparison_fixture(true);
    f.capture_slot("a", "0");
    f.capture_slot("b", "0.001");
    fs::write(f.root.path("setup"), b"#!/bin/sh\ntrue\n").unwrap();
    f.capture_slot("b2", "0.001");
    fs::write(f.root.path("setup"), b"#!/bin/sh\nexit 0\n").unwrap();
    f.capture_slot("a2", "0");
    f.comparison_rejected(
        true,
        "B2 must match B runner, program, setup and deployment",
    );
}

#[test]
fn compare_cross_arm_hash_difference_is_only_a_byte_fact() {
    let mut f = comparison_fixture(3, false);
    fs::write(
        f.root.path("alternate"),
        png_with_text(1, 1, b"fixture\0alternate"),
    )
    .unwrap();
    fs::write(f.root.path("runner"), b"#!/bin/sh\nsleep \"$DELAY\"\nif [ \"$DELAY\" = 0 ]; then cp \"$ROOT/alternate\" \"$1\"; else cp \"$PREPARED\" \"$1\"; fi\n").unwrap();
    f.capture_slot("a", "0.3");
    f.capture_slot("b", "0");
    f.capture_slot("a2", "0.3");
    let report = f.comparison(false, 0);
    assert_eq!(
        report["gates"][0]["cross_arm_hash_equality"][0]["equal"],
        false
    );
    assert_eq!(
        report["gates"][0]["candidate_summary"]["determinism_by_seed"][0]["deterministic"],
        true
    );
}

#[test]
fn compare_cross_arm_hash_equality_is_unavailable_for_nondeterministic_arms() {
    for delay in ["0", "0.3"] {
        let mut f = comparison_fixture(3, false);
        f.runner["env"]["ALTER_DELAY"] = json!(delay);
        fs::write(
            f.root.path("alternate"),
            png_with_text(1, 1, b"fixture\0alternate"),
        )
        .unwrap();
        fs::write(f.root.path("runner"), b"#!/bin/sh\nsleep \"$DELAY\"\nif [ \"$DELAY\" = \"$ALTER_DELAY\" ] && [ ! -f \"$ROOT/altered\" ]; then touch \"$ROOT/altered\"; cp \"$ROOT/alternate\" \"$1\"; else cp \"$PREPARED\" \"$1\"; fi\n").unwrap();
        f.capture_slot("a", "0.3");
        f.capture_slot("b", "0");
        f.capture_slot("a2", "0.3");
        let report = f.comparison(false, 0);
        let gate = &report["gates"][0];
        assert!(gate["cross_arm_hash_equality"][0]["equal"].is_null());
        let arm = if delay == "0" {
            "candidate_summary"
        } else {
            "reference_summary"
        };
        assert_eq!(gate[arm]["determinism_by_seed"][0]["deterministic"], false);
    }
}
