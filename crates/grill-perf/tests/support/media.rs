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
