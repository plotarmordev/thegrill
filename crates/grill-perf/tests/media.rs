use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    workload: Value,
    runner: Value,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "grill-media-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let program = root.join("runner");
        fs::write(&program, b"#!/bin/sh\nexit 99\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&program, root.join("link")).unwrap();
        let workload = json!({
            "kind":"media-workload-v1", "version":1,"name":"synthetic", "modality":"image",
            "request":{"prompt":"red bicycle {unknown}","negative_prompt":null,"seeds":[42,43,44],"width":1,"height":1,"steps":1,"guidance_milli":null,"images_per_run":1,"frames":null,"fps":null,"duration_us":null,"sample_rate":null},
            "outputs":[{"placeholder":"output","container":"png","expect":{"width":1,"height":1}}],
            "cells":[{"id":"c1","warmup_runs":1,"measured_runs":3,"pause_between_runs_ms":10}],
            "slots":[{"id":"a","arm":"a"},{"id":"b","arm":"b"},{"id":"b2","arm":"b2"},{"id":"a2","arm":"a2"}],
            "limits":{"run_deadline_ms":100,"setup_deadline_ms":200,"output_bytes":1024,"stdout_bytes":1024,"stderr_bytes":1024},
            "gates":[{"cell":"c1","metric":"end_to_end_us","max_adverse_bps":500,"max_reference_spread_bps":2000}]
        });
        let runner = json!({"kind":"media-runner-v1","version":1,"id":"synthetic","execution":"in-process","program":root.join("link"),"args":["{prompt}","{seed}","{width}x{height}","{steps}","{negative_prompt}","{output}","{output_dir}","{marker_file}","; touch NEVER | $(exit 1)"],"env":{},"cwd":null,"setup":{"args":["{output}"],"deadline_ms":150},"cache":{"declared_state":"unknown","paths":[]},"markers":null,"pins":[],"probe":null});
        Self {
            root,
            workload,
            runner,
        }
    }
    fn save(&self) {
        fs::write(
            self.root.join("workload.json"),
            serde_json::to_vec(&self.workload).unwrap(),
        )
        .unwrap();
        fs::write(
            self.root.join("runner.json"),
            serde_json::to_vec(&self.runner).unwrap(),
        )
        .unwrap();
    }
    fn invoke(&self, json: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_grill-perf"));
        cmd.args(["media", "preflight", "--workload"])
            .arg(self.root.join("workload.json"))
            .arg("--runner")
            .arg(self.root.join("runner.json"));
        if json {
            cmd.arg("--json");
        }
        cmd.output().unwrap()
    }
    fn reject(&self, expected: &str) {
        self.save();
        let out = self.invoke(true);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn preflight_projects_without_running_and_hashes_resolved_bytes() {
    let f = Fixture::new();
    f.save();
    let output = f.invoke(true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["program"]["path"],
        f.root.join("runner").to_str().unwrap()
    );
    assert_eq!(
        report["program"]["sha256"],
        Sha256::digest(fs::read(f.root.join("runner")).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    assert_eq!(report["warmup_runs"], 1);
    assert_eq!(report["measured_runs"], 3);
    assert_eq!(report["total_runs"], 4);
    assert_eq!(report["wall_clock_ceiling_ms"], 590);
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
            "run-000000/out/",
            "run-000000/markers.ndjson",
            "; touch NEVER | $(exit 1)"
        ])
    );
    assert!(!f.root.join("run-000000").exists());
    let text = f.invoke(false);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    for expected in [
        "no process started",
        "measured runs: 3",
        "wall-clock ceiling ms: 590",
        "c1 measured seed 43",
        "; touch NEVER",
    ] {
        assert!(text.contains(expected));
    }
}
#[test]
fn strict_schemas_and_image_only_gate() {
    let mut f = Fixture::new();
    for modality in ["audio", "video"] {
        f.workload["modality"] = json!(modality);
        f.reject("not supported yet");
    }
    f.workload["modality"] = json!("image");
    f.workload["request"]["frames"] = json!(1);
    f.reject("invalid image request");
    f.workload["request"]["frames"] = Value::Null;
    for pointer in [
        "",
        "/request",
        "/outputs/0",
        "/outputs/0/expect",
        "/cells/0",
        "/slots/0",
        "/limits",
        "/gates/0",
    ] {
        f.workload.pointer_mut(pointer).unwrap()["unexpected"] = json!(true);
        f.reject("unknown field");
        f.workload
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("unexpected");
    }
    for pointer in ["", "/setup", "/cache"] {
        f.runner.pointer_mut(pointer).unwrap()["unexpected"] = json!(true);
        f.reject("unknown field");
        f.runner
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("unexpected");
    }
    f.workload["request"] = json!([]);
    f.reject("JSON objects");
}
#[test]
fn rejects_duplicates_templates_and_missing_programs() {
    let mut f = Fixture::new();
    f.runner["args"] = json!(["{oops}"]);
    f.reject("unknown placeholder");
    f.runner["args"] = json!(["{frames}"]);
    f.reject("unavailable for image");
    f.runner["args"] = json!(["{prompt"]);
    f.reject("unclosed placeholder");
    f.runner["args"] = json!([]);
    f.runner["program"] = json!(f.root.join("absent"));
    f.reject("resolve program");
    f.save();
    fs::write(
        f.root.join("runner.json"),
        b"{\"kind\":null,\"kind\":\"media-runner-v1\"}",
    )
    .unwrap();
    let out = f.invoke(true);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("duplicate field"));
}

#[test]
fn multiple_images_require_directory_output() {
    let mut f = Fixture::new();
    f.workload["request"]["images_per_run"] = json!(2);
    f.reject("multiple images require output_dir");
    f.workload["outputs"][0]["placeholder"] = json!("output_dir");
    f.runner["args"] = json!(["{output_dir}"]);
    f.save();
    assert!(f.invoke(true).status.success());
    f.workload["request"]["images_per_run"] = json!(1);
    f.runner["args"] = json!(["{output}", "{output}"]);
    f.reject("output may appear at most once");
}
