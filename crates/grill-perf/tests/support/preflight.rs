// Offline preflight admission: the same declaration checks as `run`, without
// dispatch, capture output or execution identity. Every fixture binds loopback
// listeners and asserts they stay untouched, so the capability is proven
// network-free rather than merely assumed.
use super::*;
use sha2::{Digest, Sha256};

const CREDENTIAL_ENV: &str = "GRILL_PREFLIGHT_TEST_KEY";
const SENTINEL: &str = "preflight-sentinel-credential";

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn save(path: impl AsRef<Path>, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
// Sorted relative paths, so a nested write is visible without knowing its name.
fn entries(root: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            names.push(path.strip_prefix(root).unwrap().display().to_string());
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            }
        }
    }
    names.sort();
    names
}
fn preflight_command(temp: &Temp, input: &Path, endpoint: &str) -> Command {
    let mut command = cli();
    command
        .arg("preflight")
        .arg(input)
        .args([
            "--endpoint",
            endpoint,
            "--model",
            "fixture-model",
            "--local-http",
        ])
        .current_dir(&temp.0);
    command
}
fn run_command(input: &Path, endpoint: &str, out: &Path) -> Command {
    let mut command = cli();
    command
        .arg("run")
        .arg(input)
        .args([
            "--endpoint",
            endpoint,
            "--model",
            "fixture-model",
            "--local-http",
            "--json",
            "--out",
        ])
        .arg(out);
    command
}
// `run` and `preflight` share admission, so a rejection must be byte-identical
// and must not create the output directory `run` would have published.
fn rejected_pair(
    temp: &Temp,
    input: &Path,
    endpoint: &str,
    configure: impl Fn(&mut Command),
) -> String {
    let out = temp.path("rejected-run");
    let mut capture = run_command(input, endpoint, &out);
    let mut offline = preflight_command(temp, input, endpoint);
    configure(&mut capture);
    configure(&mut offline);
    let (capture, offline) = (capture.output().unwrap(), offline.output().unwrap());
    assert_eq!(
        capture.status.code(),
        Some(1),
        "run stderr={}",
        String::from_utf8_lossy(&capture.stderr)
    );
    assert_eq!(
        offline.status.code(),
        Some(1),
        "preflight stderr={}",
        String::from_utf8_lossy(&offline.stderr)
    );
    assert!(capture.stdout.is_empty() && offline.stdout.is_empty());
    assert_eq!(
        capture.stderr, offline.stderr,
        "run and preflight must share one admission error"
    );
    assert!(
        !out.exists(),
        "rejected run must not create its output directory"
    );
    String::from_utf8(offline.stderr).unwrap()
}
fn policy_declaration(work: &Value, source: &[u8]) -> Value {
    let metrics = [
        "wave_latency_us",
        "achieved_completion_tokens_per_second",
        "decode_tokens_per_second",
        "prefill_tokens_per_second",
    ];
    json!({
        "version": 1,
        "method": "observed-envelope-v1",
        "id": "preflight-fixture",
        "collector_sha256": digest(&fs::read(env!("CARGO_BIN_EXE_grill-perf")).unwrap()),
        "workload_source_sha256": digest(source),
        "min_trials": 3,
        "cells": work["cells"].as_array().unwrap().iter().map(|cell| json!({
            "cell": cell["id"],
            "metrics": metrics.iter().map(|metric| json!({
                "metric": metric,
                "max_regression_bps": 0,
                "max_reference_spread_bps": 0
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}

#[test]
fn preflight_admits_valid_declaration_offline_and_matches_bundle_inspect() {
    let temp = Temp::new();
    let model = Server::new(normal);
    let metrics = Server::new(normal);
    let work = workload(1, 1, 3);
    let source = serde_json::to_vec(&work).unwrap();
    let input = temp.path("work.json");
    fs::write(&input, &source).unwrap();
    let declaration = temp.path("deployment.json");
    save(&declaration, &deployment());
    let policy = temp.path("policy.json");
    save(&policy, &policy_declaration(&work, &source));
    let before = entries(&temp.0);
    let output = preflight_command(&temp, &input, &model.endpoint)
        .arg("--deployment")
        .arg(&declaration)
        .arg("--policy")
        .arg(&policy)
        .arg("--metrics-url")
        .arg(&metrics.endpoint)
        .arg("--auth-env")
        .arg(CREDENTIAL_ENV)
        .env(CREDENTIAL_ENV, SENTINEL)
        .output()
        .unwrap();
    successful(&output);
    assert_eq!(
        model.count.load(Ordering::SeqCst),
        0,
        "preflight dispatched to the model listener"
    );
    assert_eq!(
        metrics.count.load(Ordering::SeqCst),
        0,
        "preflight scraped the metrics listener"
    );
    assert_eq!(entries(&temp.0), before, "preflight created files");
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(
        !text.contains(SENTINEL),
        "credential value leaked into stdout"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SENTINEL));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let claim = report["claim"].as_str().unwrap();
    assert!(
        claim.contains("offline") && claim.contains("not-backend-qualification"),
        "{claim}"
    );
    let inspect = cli()
        .args(["bundle", "inspect"])
        .arg(&input)
        .output()
        .unwrap();
    successful(&inspect);
    let pins: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    for key in [
        "source_sha256",
        "workload_sha256",
        "warmup_requests",
        "measured_requests",
        "total_output_token_ceiling",
    ] {
        assert_eq!(
            report[key], pins[key],
            "preflight {key} must agree with bundle inspect"
        );
    }
    assert_eq!(
        report["policy_sha256"].as_str(),
        Some(digest(&fs::read(&policy).unwrap()).as_str())
    );
    assert_eq!(report["warmup_requests"], 1);
    assert_eq!(report["measured_requests"], 3);
    assert_eq!(report["total_output_token_ceiling"], 32);
    assert_eq!(report["planned_waves"], 4);
    let settings = deployment()["settings"].as_str().unwrap().to_owned();
    assert_eq!(
        report["deployment_bytes"]["settings"].as_u64(),
        Some(settings.len() as u64),
        "declarations are reported as observed byte counts"
    );
    assert!(!text.contains(&settings), "declaration contents leaked");
}

#[test]
fn preflight_accepts_workload_without_optional_run_inputs() {
    let temp = Temp::new();
    let model = Server::new(normal);
    let work = workload(1, 0, 1);
    let input = temp.path("work.json");
    save(&input, &work);
    let before = entries(&temp.0);
    let output = preflight_command(&temp, &input, &model.endpoint)
        .output()
        .unwrap();
    successful(&output);
    assert_eq!(model.count.load(Ordering::SeqCst), 0);
    assert_eq!(entries(&temp.0), before);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["source_sha256"], digest(&fs::read(&input).unwrap()));
    assert_eq!(report["total_output_token_ceiling"], 8);
}

#[test]
fn preflight_and_run_share_declaration_field_and_observed_byte_errors() {
    let temp = Temp::new();
    let model = Server::new(normal);
    let work = workload(1, 0, 1);
    let input = temp.path("work.json");
    save(&input, &work);
    let declaration = temp.path("deployment.json");
    // Multibyte text separates the observed byte count from its character count.
    let cases: [(&str, Value, u64); 4] = [
        ("settings", Value::String("\u{e9}".repeat(5940)), 11_880),
        ("runtime", Value::String("r".repeat(4097)), 4097),
        ("model_revision", Value::String(String::new()), 0),
        ("hardware", Value::String("line\nbreak".into()), 10),
    ];
    for (field, value, bytes) in cases {
        let mut declared = deployment();
        declared[field] = value;
        save(&declaration, &declared);
        let error = rejected_pair(&temp, &input, &model.endpoint, |command| {
            command.arg("--deployment").arg(&declaration);
        });
        assert!(error.contains(&format!("deployment.{field}")), "{error}");
        assert!(
            error.contains(&bytes.to_string()),
            "observed byte count {bytes} missing: {error}"
        );
    }
    // The bound itself is unchanged: exactly 4096 bytes is admitted.
    let mut boundary = deployment();
    boundary["settings"] = Value::String("s".repeat(4096));
    save(&declaration, &boundary);
    let output = preflight_command(&temp, &input, &model.endpoint)
        .arg("--deployment")
        .arg(&declaration)
        .output()
        .unwrap();
    successful(&output);
    assert_eq!(model.count.load(Ordering::SeqCst), 0);
}

#[test]
fn preflight_failure_after_credentials_echoes_no_declared_values() {
    let temp = Temp::new();
    let model = Server::new(normal);
    let work = workload(1, 0, 3);
    let source = serde_json::to_vec(&work).unwrap();
    let input = temp.path("work.json");
    fs::write(&input, &source).unwrap();
    let declaration = temp.path("deployment.json");
    save(&declaration, &deployment());
    // A syntactically valid declaration with a mismatched collector pin fails
    // after the credential has already been read and validated.
    let mut policy = policy_declaration(&work, &source);
    policy["collector_sha256"] = Value::String("0".repeat(64));
    let policy_path = temp.path("policy.json");
    save(&policy_path, &policy);
    let before = entries(&temp.0);
    let error = rejected_pair(&temp, &input, &model.endpoint, |command| {
        command
            .arg("--deployment")
            .arg(&declaration)
            .arg("--policy")
            .arg(&policy_path)
            .arg("--auth-env")
            .arg(CREDENTIAL_ENV)
            .env(CREDENTIAL_ENV, SENTINEL);
    });
    assert!(error.contains("policy_collector_mismatch"), "{error}");
    let settings = deployment()["settings"].as_str().unwrap().to_owned();
    for declared in [SENTINEL, settings.as_str(), "Say cafe."] {
        assert!(
            !error.contains(declared),
            "{declared} leaked into the failure: {error}"
        );
    }
    assert_eq!(
        entries(&temp.0),
        before,
        "a rejected preflight created files"
    );
    assert_eq!(model.count.load(Ordering::SeqCst), 0);
}

#[test]
fn preflight_and_run_share_workload_policy_and_request_bound_admission() {
    let temp = Temp::new();
    let model = Server::new(normal);

    let mut profile = workload(1, 0, 1);
    profile["request"]["profile"] = Value::String("unsupported-profile-v1".into());
    let input = temp.path("profile.json");
    save(&input, &profile);
    let error = rejected_pair(&temp, &input, &model.endpoint, |_| {});
    assert!(error.contains("invalid workload"), "{error}");

    // Conversation declarations refuse an advanced policy before that file is read.
    let conversation = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/conversation-v2.json");
    let unbound = temp.path("unbound.json");
    fs::write(&unbound, br#"{"version":2}"#).unwrap();
    let error = rejected_pair(&temp, &conversation, &model.endpoint, |command| {
        command.arg("--policy").arg(&unbound);
    });
    assert!(error.contains("conversation"), "{error}");
    assert!(!error.contains("invalid_policy"), "{error}");

    let scoped = workload(1, 0, 3);
    let source = serde_json::to_vec(&scoped).unwrap();
    let input = temp.path("scope.json");
    fs::write(&input, &source).unwrap();
    let mut policy = policy_declaration(&scoped, &source);
    policy["cells"][0]["cell"] = Value::String("unknown-cell".into());
    let policy_path = temp.path("scope-policy.json");
    save(&policy_path, &policy);
    let error = rejected_pair(&temp, &input, &model.endpoint, |command| {
        command.arg("--policy").arg(&policy_path);
    });
    assert!(error.contains("policy_scope_mismatch"), "{error}");

    // The rendered request bound is checked at admission, not at dispatch. Tabs
    // keep the declaration under the raw bound while the encoded body exceeds it.
    let mut filled = workload(1, 0, 1);
    filled["cases"][0]["messages"][0]["content"] = Value::String("{fill}".into());
    filled["cases"][0]["fill"] = json!({"unit": "\t".repeat(64), "repeat": 32767});
    let input = temp.path("filled.json");
    save(&input, &filled);
    let error = rejected_pair(&temp, &input, &model.endpoint, |_| {});
    assert!(error.contains("encoded request exceeds 2 MiB"), "{error}");

    assert_eq!(
        model.count.load(Ordering::SeqCst),
        0,
        "admission must never dispatch"
    );
}

#[test]
fn preflight_parser_requires_its_inputs_and_rejects_run_output_flag() {
    let temp = Temp::new();
    let work = workload(1, 0, 1);
    let input = temp.path("work.json");
    save(&input, &work);
    let before = entries(&temp.0);

    let missing = cli().arg("preflight").arg(&input).output().unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    let error = String::from_utf8(missing.stderr).unwrap();
    assert!(
        error.contains("--endpoint") && error.contains("--model"),
        "{error}"
    );

    let refused = cli()
        .arg("preflight")
        .arg(&input)
        .args([
            "--endpoint",
            "https://fixture.invalid/v1/chat/completions",
            "--model",
            "fixture-model",
            "--out",
        ])
        .arg(temp.path("out"))
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(refused.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--out"),
        "preflight must not accept a capture destination"
    );

    let run_missing = cli()
        .arg("run")
        .arg(&input)
        .args([
            "--endpoint",
            "https://fixture.invalid/v1/chat/completions",
            "--model",
            "fixture-model",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(run_missing.status.code(), Some(1));
    assert!(run_missing.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&run_missing.stderr).contains("--out"),
        "run must still require its output directory"
    );

    assert_eq!(entries(&temp.0), before);
}
