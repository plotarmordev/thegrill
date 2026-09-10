use super::*;
use sha2::{Digest, Sha256};

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")
}
fn fixture(temp: &Temp) -> Value {
    let manifest = value(examples().join("recipes-v1.json"));
    for entry in manifest["entries"].as_array().unwrap() {
        let file = entry["file"].as_str().unwrap();
        fs::copy(examples().join(file), temp.path(file)).unwrap();
    }
    save_manifest(temp, &manifest);
    manifest
}
fn save_manifest(temp: &Temp, manifest: &Value) {
    fs::write(
        temp.path("recipes-v1.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
}
fn verify(path: &Path) -> Output {
    cli()
        .args(["bundle", "verify"])
        .arg(path)
        .arg("--json")
        .output()
        .unwrap()
}
fn rejected(output: Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}
// These fixtures retain the checked-in typed field order and literal strings.
// Removing only insignificant whitespace yields their Workload serialization.
fn compact(source: &str) -> Vec<u8> {
    let mut quoted = false;
    let mut escaped = false;
    source
        .bytes()
        .filter(|&byte| {
            let keep = quoted || !byte.is_ascii_whitespace();
            if escaped {
                escaped = false;
            } else if quoted && byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = !quoted;
            }
            keep
        })
        .collect()
}
fn replace_source(temp: &Temp, manifest: &mut Value, index: usize, source: &str) {
    let entry = &mut manifest["entries"][index];
    fs::write(temp.path(entry["file"].as_str().unwrap()), source).unwrap();
    entry["source_sha256"] = json!(digest(source.as_bytes()));
    entry["workload_sha256"] = json!(digest(&compact(source)));
    save_manifest(temp, manifest);
}

#[test]
fn bundle_checked_in_data_verifies_from_another_cwd_without_network() {
    let temp = Temp::new();
    let server = Server::new(normal);
    let output = cli()
        .current_dir(&temp.0)
        .env("HTTP_PROXY", &server.endpoint)
        .env("HTTPS_PROXY", &server.endpoint)
        .env("ALL_PROXY", &server.endpoint)
        .env_remove("MODEL_API_KEY")
        .args(["bundle", "verify"])
        .arg(examples().join("recipes-v1.json"))
        .arg("--json")
        .output()
        .unwrap();
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let manifest = value(examples().join("recipes-v1.json"));
    assert_eq!(
        report["manifest_sha256"],
        digest(&fs::read(examples().join("recipes-v1.json")).unwrap())
    );
    assert_eq!(report["recipes"], manifest["recipes"]);
    for entry in report["entries"].as_array().unwrap() {
        let work = value(examples().join(entry["file"].as_str().unwrap()));
        let requests: u64 = work["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cell| {
                (cell["warmup_trials"].as_u64().unwrap() + cell["trials"].as_u64().unwrap())
                    * cell["concurrency"].as_u64().unwrap()
            })
            .sum();
        assert_eq!(entry["budgets"]["total_requests"], requests);
        assert_eq!(
            entry["budgets"]["total_output_token_ceiling"],
            requests * work["request"]["output"]["tokens"].as_u64().unwrap()
        );
        assert_ne!(entry["source_sha256"], entry["workload_sha256"]);
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}

#[test]
fn bundle_rejects_unknown_schema_versions_mappings_and_entry_counts() {
    let temp = Temp::new();
    let original = fixture(&temp);
    let mut invalid = Vec::new();
    let mut manifest = original.clone();
    manifest["version"] = json!(2);
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["source_git"] = json!("not-a-manifest-field");
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["entries"][0]["unknown"] = json!(true);
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["recipes"]["glm"]["unknown"] = json!(true);
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["recipes"]["other"] = manifest["recipes"]["glm"].clone();
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["recipes"]["glm"]["decode"] = json!("sparkdash-decode-v1");
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["entries"].as_array_mut().unwrap().pop();
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["entries"]
        .as_array_mut()
        .unwrap()
        .push(original["entries"][0].clone());
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["entries"][1] = manifest["entries"][0].clone();
    invalid.push(manifest);
    let mut manifest = original.clone();
    manifest["entries"][2]["base"] = json!("sparkdash-prefill-v1");
    invalid.push(manifest);
    for manifest in invalid {
        save_manifest(&temp, &manifest);
        rejected(verify(&temp.path("recipes-v1.json")));
    }
    let source = serde_json::to_string(&original).unwrap();
    fs::write(
        temp.path("recipes-v1.json"),
        source.replacen("{", "{\"version\":1,", 1),
    )
    .unwrap();
    rejected(verify(&temp.path("recipes-v1.json")));
}

#[test]
fn bundle_rejects_paths_symlinks_and_oversized_files() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let original = fixture(&temp);
    for file in [
        "../sparkdash-decode-v1.json",
        "/sparkdash-decode-v1.json",
        "nested/sparkdash-decode-v1.json",
        "./sparkdash-decode-v1.json",
        "sparkdash-prefill-v1.json",
    ] {
        let mut manifest = original.clone();
        manifest["entries"][0]["file"] = json!(file);
        save_manifest(&temp, &manifest);
        rejected(verify(&temp.path("recipes-v1.json")));
    }
    save_manifest(&temp, &original);
    let links = Temp::new();
    symlink(&temp.0, links.path("root")).unwrap();
    rejected(verify(&links.path("root/recipes-v1.json")));
    rejected(verify(&links.path("root/./recipes-v1.json")));
    symlink(temp.path("recipes-v1.json"), links.path("manifest.json")).unwrap();
    rejected(verify(&links.path("manifest.json")));
    let workload_path = temp.path("sparkdash-decode-v1.json");
    fs::remove_file(&workload_path).unwrap();
    symlink(examples().join("sparkdash-decode-v1.json"), &workload_path).unwrap();
    rejected(verify(&temp.path("recipes-v1.json")));
    fs::remove_file(&workload_path).unwrap();
    fs::write(&workload_path, vec![b' '; 4 * 1024 * 1024 + 1]).unwrap();
    rejected(verify(&temp.path("recipes-v1.json")));
    fs::write(temp.path("recipes-v1.json"), vec![b' '; 64 * 1024 + 1]).unwrap();
    rejected(verify(&temp.path("recipes-v1.json")));
}

#[test]
fn bundle_distinguishes_raw_and_normalized_hashes_and_rejects_bad_pins() {
    let temp = Temp::new();
    let original = fixture(&temp);
    for field in ["source_sha256", "workload_sha256"] {
        for pin in ["0".repeat(64), "A".repeat(64), "a".repeat(63)] {
            let mut manifest = original.clone();
            manifest["entries"][0][field] = json!(pin);
            save_manifest(&temp, &manifest);
            rejected(verify(&temp.path("recipes-v1.json")));
        }
    }
    save_manifest(&temp, &original);
    let source = fs::read_to_string(temp.path("sparkdash-decode-v1.json")).unwrap() + "\n";
    fs::write(temp.path("sparkdash-decode-v1.json"), &source).unwrap();
    rejected(verify(&temp.path("recipes-v1.json")));
    let mut manifest = original.clone();
    manifest["entries"][0]["source_sha256"] = json!(digest(source.as_bytes()));
    save_manifest(&temp, &manifest);
    let output = verify(&temp.path("recipes-v1.json"));
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["entries"][0]["workload_sha256"],
        original["entries"][0]["workload_sha256"]
    );
    assert_ne!(
        report["entries"][0]["source_sha256"],
        original["entries"][0]["source_sha256"]
    );
}

#[test]
fn bundle_rejects_semantic_variant_drift_even_with_recomputed_hashes() {
    let temp = Temp::new();
    let original = fixture(&temp);
    for (index, from, to) in [
        (2, "\"temperature_milli\": 0", "\"temperature_milli\": 1"),
        (2, "\"enabled\": false", "\"enabled\": true"),
        (2, "\"total_ms\": 360000", "\"total_ms\": 360001"),
        (2, "\"trials\": 3", "\"trials\": 4"),
        (2, "Count from 1 to 200.", "Count from 1 to 201."),
        (2, "\"glm-decode-v1\"", "\"other-decode-v1\""),
        (3, "\"repeat\": 4070", "\"repeat\": 4071"),
        (3, "\"version\": 1", "\"version\": 2"),
    ] {
        fixture(&temp);
        let mut manifest = original.clone();
        let source =
            fs::read_to_string(temp.path(manifest["entries"][index]["file"].as_str().unwrap()))
                .unwrap();
        replace_source(&temp, &mut manifest, index, &source.replace(from, to));
        rejected(verify(&temp.path("recipes-v1.json")));
    }
}

#[test]
fn bundle_recipe_paths_collect_both_declared_control_mappings() {
    let temp = Temp::new();
    let output = verify(&examples().join("recipes-v1.json"));
    successful(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    for recipe in ["deepseek", "glm"] {
        for kind in ["decode", "prefill"] {
            let id = &report["recipes"][recipe][kind];
            let entry = report["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["id"] == *id)
                .unwrap();
            let server = Server::new(|mut stream, _, request| {
                header(&mut stream, "text/event-stream");
                frame(
                    &mut stream,
                    json!({"id":"fixture","choices":[{"index":0,"delta":{"content":"synthetic answer"}}]}),
                );
                finish(&mut stream, request["max_tokens"].as_u64(), Some(0));
            });
            let output = cli()
                .arg("run")
                .arg(examples().join(entry["file"].as_str().unwrap()))
                .args([
                    "--endpoint",
                    &server.endpoint,
                    "--model",
                    "fixture-model",
                    "--local-http",
                    "--json",
                    "--out",
                ])
                .arg(temp.path(&format!("{recipe}-{kind}")))
                .output()
                .unwrap();
            successful(&output);
            let requests: Vec<_> = server.seen.try_iter().collect();
            assert_eq!(
                requests.len() as u64,
                entry["budgets"]["total_requests"].as_u64().unwrap()
            );
            for request in requests {
                assert_eq!(
                    request["chat_template_kwargs"],
                    if recipe == "deepseek" {
                        json!({"thinking":false})
                    } else {
                        json!({"enable_thinking":false})
                    }
                );
                assert_eq!(request["max_tokens"], entry["request"]["output"]["tokens"]);
                assert_eq!(request["min_tokens"], entry["request"]["output"]["tokens"]);
                assert_eq!(request["ignore_eos"], true);
            }
        }
    }
}
