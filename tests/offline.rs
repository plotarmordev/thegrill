#![cfg(target_os = "linux")]
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "grill-offline-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("scratch directory: {e}"),
            }
        }
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn cli(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grill"))
        .args(args)
        .output()
        .unwrap()
}
fn grade(pack: &Path, submission: &Path, out: &Path) -> Output {
    cli(&[
        "grade".as_ref(),
        pack.as_os_str(),
        submission.as_os_str(),
        "--out".as_ref(),
        out.as_os_str(),
    ])
}
fn compare(a: &Path, b: &Path) -> Output {
    cli(&[
        "compare".as_ref(),
        a.as_os_str(),
        b.as_os_str(),
        "--json".as_ref(),
    ])
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
const PACK: &[u8] = include_bytes!("../examples/synthetic-pack.json");
const A: &[u8] = include_bytes!("../examples/submission-a.json");
const B: &[u8] = include_bytes!("../examples/submission-b.json");

#[derive(Deserialize)]
struct Bounds {
    lower: i64,
    upper: i64,
    denominator: u32,
}
#[derive(Deserialize)]
struct Coverage {
    success: u32,
    failure: u32,
    unknown: u32,
}
#[derive(Deserialize)]
struct Paired {
    gains: u32,
    losses: u32,
    either_unknown: u32,
}
#[derive(Deserialize)]
struct Compared {
    n: u32,
    left: Coverage,
    right: Coverage,
    paired: Paired,
    delta_b_minus_a: Bounds,
}

#[test]
fn relocated_submitted_evidence_keeps_fixed_denominator() {
    let s = Scratch::new();
    let p = s.write("input.json", PACK);
    let a = s.write("a.json", A);
    let b = s.write("b.json", B);
    let av = s.path("av");
    let bv = s.path("bv");
    success(&grade(&p, &a, &av));
    success(&grade(&p, &b, &bv));
    fs::remove_file(p).unwrap();
    fs::remove_file(a).unwrap();
    fs::remove_file(b).unwrap();
    let relocated = s.path("relocated");
    fs::rename(av, &relocated).unwrap();
    let result = compare(&relocated, &bv);
    success(&result);
    let c: Compared = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(c.n, 4);
    assert_eq!((c.left.success, c.left.failure, c.left.unknown), (1, 2, 1));
    assert_eq!(
        (c.right.success, c.right.failure, c.right.unknown),
        (3, 1, 0)
    );
    assert_eq!(
        (c.paired.gains, c.paired.losses, c.paired.either_unknown),
        (2, 1, 1)
    );
    assert_eq!(
        (
            c.delta_b_minus_a.lower,
            c.delta_b_minus_a.upper,
            c.delta_b_minus_a.denominator
        ),
        (1, 2, 4)
    );
}

#[test]
fn v4_views_preserve_adapted_grades_and_bind_provenance() {
    let s = Scratch::new();
    let fixtures = [
        (
            serde_json::json!({"kind":"final-text-set","accepted":["North Star"]}),
            "===FINAL===\n{\"answer\":\"North Star\"}",
            "===FINAL===\n{\"answer\":\"North Star!\"}",
        ),
        (
            serde_json::json!({"kind":"final-number","number":{"expected":1,"absolute_tolerance":0,"relative_tolerance":0}}),
            // Exactly halfway between 1 and its successor: ties round to even.
            "===FINAL===\n{\"answer\":1.00000000000000011102230246251565404236316680908203125}",
            "===FINAL===\n{\"answer\":1.0000000000000002220446049250313080847263336181640625}",
        ),
        (
            serde_json::json!({"kind":"json-exact","expected":{"n":9007199254740993u64,"ok":true}}),
            "{\"ok\":true,\"n\":90071992547409930e-1}",
            "{\"n\":9007199254740992,\"ok\":true}",
        ),
        (
            serde_json::json!({"kind":"text-constraints","rules":[{"kind":"contains","text":"BEGIN"},{"kind":"words","min":2,"max":2}]}),
            "BEGIN here",
            "BEGIN too many words",
        ),
    ];
    let cases: Vec<_> = fixtures
        .iter()
        .enumerate()
        .map(|(i, (acceptance, valid, wrong))| {
            serde_json::json!({
                "id":format!("c{i}"),"world":format!("w{i}"),"group":"adapted",
                "messages":[{"role":"user","content":"Synthetic offline regression; no model call."}],
                "acceptance":acceptance,
                "qualification":{"valid":[valid],"wrong":[wrong]},
                "provenance":{
                    "repository":"https://example.org/tests","revision":"a".repeat(40),
                    "path":"synthetic.json","sha256":"b".repeat(64),
                    "item_id":format!("fixture-{i}"),"license":"Synthetic regression fixture",
                    "changes":["Original test fixture, not an upstream dataset"]
                }
            })
        })
        .collect();
    let mut pack = serde_json::json!({
        "version":4,"label":"Synthetic v4","worlds":["w0","w1","w2","w3"],
        "groups":["adapted"],"cases":cases
    });
    let p = s.write("v4.json", &serde_json::to_vec(&pack).unwrap());
    success(&cli(&["check".as_ref(), p.as_os_str(), "--json".as_ref()]));
    let mut left: serde_json::Value = serde_json::from_slice(A).unwrap();
    left["answers"] = fixtures
        .iter()
        .enumerate()
        .map(|(i, (_, valid, _))| serde_json::json!({"case_id":format!("c{i}"),"artifact":valid}))
        .collect();
    left["answers"][0]["artifact"] = "===FINAL===\n{\"answer\":\" north  STAR \"}".into();
    let mut right = left.clone();
    for (i, (_, _, wrong)) in fixtures.iter().enumerate() {
        right["answers"][i]["artifact"] = (*wrong).into();
    }
    right["answers"][1]["artifact"] = serde_json::Value::Null;
    let a = s.write("left.json", &serde_json::to_vec(&left).unwrap());
    let b = s.write("right.json", &serde_json::to_vec(&right).unwrap());
    let av = s.path("left-view");
    let bv = s.path("right-view");
    success(&grade(&p, &a, &av));
    success(&grade(&p, &b, &bv));

    // Identical answers and prompts cannot mask a changed source declaration.
    pack["cases"][0]["provenance"]["revision"] = "c".repeat(40).into();
    let changed = s.write("changed.json", &serde_json::to_vec(&pack).unwrap());
    let changed_view = s.path("changed-view");
    success(&grade(&changed, &a, &changed_view));
    assert!(!compare(&av, &changed_view).status.success());
    pack["cases"][3]["acceptance"]["rules"][0]["text"] = "".into();
    let vacuous = s.write("vacuous.json", &serde_json::to_vec(&pack).unwrap());
    assert!(
        !cli(&["check".as_ref(), vacuous.as_os_str()])
            .status
            .success()
    );

    fs::remove_file(p).unwrap();
    fs::remove_file(a).unwrap();
    fs::remove_file(b).unwrap();
    let inspected = cli(&["inspect".as_ref(), av.as_os_str(), "--json".as_ref()]);
    success(&inspected);
    let coverage: Coverage = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(
        (coverage.success, coverage.failure, coverage.unknown),
        (4, 0, 0)
    );
    let compared = compare(&av, &bv);
    success(&compared);
    let result: Compared = serde_json::from_slice(&compared.stdout).unwrap();
    assert_eq!(
        (
            result.right.success,
            result.right.failure,
            result.right.unknown
        ),
        (0, 3, 1)
    );
    assert_eq!(
        (
            result.paired.gains,
            result.paired.losses,
            result.paired.either_unknown
        ),
        (0, 3, 1)
    );
    assert_eq!(
        (
            result.delta_b_minus_a.lower,
            result.delta_b_minus_a.upper,
            result.delta_b_minus_a.denominator
        ),
        (-4, -3, 4)
    );
}

#[test]
fn forged_or_missing_evidence_and_unsafe_inputs_are_rejected() {
    use std::os::unix::fs::symlink;
    let s = Scratch::new();
    let p = s.write("pack.json", PACK);
    let a = s.write("a.json", A);
    let av = s.path("av");
    success(&grade(&p, &a, &av));
    let receipt = fs::read_to_string(av.join("view.json")).unwrap();
    let forged = receipt.replacen("\"outcome\": \"success\"", "\"outcome\": \"wrong\"", 1);
    fs::write(av.join("view.json"), forged).unwrap();
    assert!(!compare(&av, &av).status.success());
    fs::write(av.join("view.json"), receipt).unwrap();
    fs::remove_file(av.join("submission.json")).unwrap();
    assert!(!compare(&av, &av).status.success());
    symlink(&a, av.join("submission.json")).unwrap();
    assert!(!compare(&av, &av).status.success());
    fs::remove_file(av.join("submission.json")).unwrap();
    fs::write(av.join("submission.json"), B).unwrap();
    assert!(!compare(&av, &av).status.success());
    let sentinel = av.join("owner-file");
    fs::write(&sentinel, b"preserve").unwrap();
    assert!(!grade(&p, &a, &av).status.success());
    assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");
    let huge = fs::File::create(s.path("huge.json")).unwrap();
    huge.set_len(16 * 1024 * 1024 + 1).unwrap();
    assert!(
        !grade(&s.path("huge.json"), &a, &s.path("oversize-view"))
            .status
            .success()
    );
    assert!(!s.path("oversize-view").exists());
    symlink(&p, s.path("pack-link")).unwrap();
    assert!(
        !grade(&s.path("pack-link"), &a, &s.path("symlink-view"))
            .status
            .success()
    );
}

#[test]
fn protocol_changes_do_not_hide_behind_system_contrasts() {
    let s = Scratch::new();
    let p = s.write("pack.json", PACK);
    let a = s.write("a.json", A);
    let av = s.path("av");
    success(&grade(&p, &a, &av));
    let changed =
        std::str::from_utf8(B)
            .unwrap()
            .replacen("\"stream\": false", "\"stream\": true", 1);
    let b = s.write("b.json", changed.as_bytes());
    let bv = s.path("bv");
    success(&grade(&p, &b, &bv));
    assert!(!compare(&av, &bv).status.success());
    let changed = std::str::from_utf8(PACK).unwrap().replacen(
        "In this invented tile code",
        "In this revised invented tile code",
        1,
    );
    let changed_pack = s.write("changed-pack.json", changed.as_bytes());
    let cv = s.path("cv");
    success(&grade(&changed_pack, &a, &cv));
    assert!(!compare(&av, &cv).status.success());
}

#[test]
fn diagnostics_are_bounded_and_json_controls_round_trip() {
    let s = Scratch::new();
    let huge_key = format!("{{\"{}\":0}}", "x".repeat(2 * 1024 * 1024));
    let p = s.write("huge-key.json", huge_key.as_bytes());
    let error = cli(&["check".as_ref(), p.as_os_str()]);
    assert_eq!(error.status.code(), Some(1));
    assert!(error.stderr.len() <= 1032);

    let argument = format!("\u{1b}]0;{}\u{7}", "x".repeat(16 * 1024));
    let error = cli(&[argument.as_ref()]);
    assert_eq!(error.status.code(), Some(2));
    assert!(error.stderr.len() <= 1032);
    assert!(!error.stderr.contains(&0x1b) && !error.stderr.contains(&0x07));

    #[derive(Deserialize)]
    struct Label {
        label: String,
    }
    let label = "label\u{7f}\u{202e}\u{1b}";
    let source = std::str::from_utf8(PACK).unwrap().replacen(
        &serde_json::to_string("Original synthetic contract exercises; not a capability benchmark")
            .unwrap(),
        &serde_json::to_string(label).unwrap(),
        1,
    );
    let p = s.write("control-label.json", source.as_bytes());
    let output = cli(&["check".as_ref(), p.as_os_str(), "--json".as_ref()]);
    success(&output);
    assert!(!output.stdout.contains(&0x7f) && !output.stdout.contains(&0x1b));
    assert!(output.stdout.is_ascii());
    assert_eq!(
        serde_json::from_slice::<Label>(&output.stdout)
            .unwrap()
            .label,
        label
    );
}

#[test]
fn mixed_v2_pack_grades_and_verifies_offline() {
    let s = Scratch::new();
    let mut pack: serde_json::Value = serde_json::from_slice(PACK).unwrap();
    pack["version"] = 2.into();
    for case in pack["cases"].as_array_mut().unwrap() {
        let accepted = case.as_object_mut().unwrap().remove("accepted").unwrap();
        case["acceptance"] = serde_json::json!({"kind": "string-set", "accepted": accepted});
    }
    pack["cases"][0]["acceptance"] = serde_json::json!({"kind": "grid-set", "outputs": [[[0, 9]]]});
    pack["cases"][0]["qualification"] = serde_json::json!({
        "valid": [r#"{"answer":[[[0,9]]]}"#],
        "wrong": [r#"{"answer":[[[9,0]]]}"#]
    });
    let p = s.write("v2.json", &serde_json::to_vec(&pack).unwrap());
    success(&cli(&["check".as_ref(), p.as_os_str(), "--json".as_ref()]));
    let mut submission: serde_json::Value = serde_json::from_slice(B).unwrap();
    submission["answers"][0]["artifact"] = r#"{"answer":[[[0,9]]]}"#.into();
    let a = s.write("answers.json", &serde_json::to_vec(&submission).unwrap());
    let good = s.path("good");
    success(&grade(&p, &a, &good));
    let inspected = cli(&["inspect".as_ref(), good.as_os_str(), "--json".as_ref()]);
    success(&inspected);
    let coverage: Coverage = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(
        (coverage.success, coverage.failure, coverage.unknown),
        (4, 0, 0)
    );
    for (index, artifact, expected) in [
        (0, r#"{"answer":[[[0]]]}"#, "wrong"),
        (1, r#"{"answer":[[[0.0,9]]]}"#, "malformed"),
    ] {
        submission["answers"][0]["artifact"] = artifact.into();
        let a = s.write("answers.json", &serde_json::to_vec(&submission).unwrap());
        let out = s.path(&format!("changed-{index}"));
        success(&grade(&p, &a, &out));
        let view: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("view.json")).unwrap()).unwrap();
        assert_eq!(view["cases"][0]["outcome"], expected);
        success(&compare(&good, &out));
    }
    fs::remove_file(&p).unwrap();
    fs::remove_file(&a).unwrap();
    success(&compare(&good, &good));
}

#[test]
fn final_marker_pack_v3_grades_and_v2_refuses() {
    let s = Scratch::new();
    let mut pack: serde_json::Value = serde_json::from_slice(PACK).unwrap();
    pack["version"] = 3.into();
    for case in pack["cases"].as_array_mut().unwrap() {
        let accepted = case.as_object_mut().unwrap().remove("accepted").unwrap();
        case["acceptance"] = serde_json::json!({"kind": "final-string-set", "accepted": accepted});
        for list in ["valid", "wrong"] {
            let artifacts: Vec<String> = case["qualification"][list]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| format!("thinking...\n===FINAL===\n{}", a.as_str().unwrap()))
                .collect();
            case["qualification"][list] = artifacts.into();
        }
    }
    let p = s.write("v3.json", &serde_json::to_vec(&pack).unwrap());
    success(&cli(&["check".as_ref(), p.as_os_str(), "--json".as_ref()]));
    pack["version"] = 2.into();
    let frozen = s.write("v2.json", &serde_json::to_vec(&pack).unwrap());
    assert!(
        !cli(&["check".as_ref(), frozen.as_os_str()])
            .status
            .success()
    );
    let mut submission: serde_json::Value = serde_json::from_slice(B).unwrap();
    submission["answers"][0]["artifact"] =
        "I considered the tile names.\n===FINAL===\n{\"answer\":\"T\"}\n".into();
    submission["answers"][1]["artifact"] = "{\"answer\":\" oak \"}".into();
    submission["answers"][2]["artifact"] =
        "===FINAL===\n===FINAL===\n{\"answer\":\"\\u03bb\"}".into();
    submission["answers"][3]["artifact"] = "===FINAL===\n{\"answer\":\"\"}".into();
    let a = s.write("answers.json", &serde_json::to_vec(&submission).unwrap());
    let out = s.path("marked");
    success(&grade(&p, &a, &out));
    let view: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("view.json")).unwrap()).unwrap();
    assert_eq!(view["cases"][0]["outcome"], "success");
    assert_eq!(view["cases"][1]["outcome"], "malformed", "missing marker");
    assert_eq!(view["cases"][2]["outcome"], "malformed", "duplicate marker");
    assert_eq!(view["cases"][3]["outcome"], "success");
}

const STUDY: &[u8] = include_bytes!("../examples/synthetic-study.json");

fn study_check(manifest: &Path, pack: &Path) -> Output {
    cli(&[
        "study".as_ref(),
        "check".as_ref(),
        manifest.as_os_str(),
        pack.as_os_str(),
        "--json".as_ref(),
    ])
}

fn study_compare(manifest: &Path, pack: &Path, left: &Path, right: &Path) -> Output {
    cli(&[
        "study".as_ref(),
        "compare".as_ref(),
        manifest.as_os_str(),
        pack.as_os_str(),
        left.as_os_str(),
        right.as_os_str(),
        "--json".as_ref(),
    ])
}

fn pack_source(path: &Path) -> serde_json::Value {
    let checked = cli(&["check".as_ref(), path.as_os_str(), "--json".as_ref()]);
    success(&checked);
    let checked: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
    checked["identities"]["source"].clone()
}

#[test]
fn study_keeps_unknowns_and_paired_variants_in_their_problem_units() {
    let s = Scratch::new();
    let generated = cli(&[
        "pilot".as_ref(),
        "--seed".as_ref(),
        "7".as_ref(),
        "--units".as_ref(),
        "1".as_ref(),
    ]);
    success(&generated);
    let pack: serde_json::Value = serde_json::from_slice(&generated.stdout).unwrap();
    let p = s.write("pack.json", &generated.stdout);
    let cases = pack["cases"].as_array().unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(STUDY).unwrap();
    manifest["pack_source"] = pack_source(&p);
    for (family, group) in manifest["families"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(pack["groups"].as_array().unwrap())
    {
        family["group"] = group.clone();
    }
    manifest["exposure"] = cases
        .iter()
        .map(|case| {
            serde_json::json!({
                "case_id": case["id"], "left": "unknown", "right": "unknown"
            })
        })
        .collect::<Vec<_>>()
        .into();
    let m = s.write("study.json", &serde_json::to_vec(&manifest).unwrap());
    success(&study_check(&m, &p));
    let mut left: serde_json::Value = serde_json::from_slice(A).unwrap();
    let mut right: serde_json::Value = serde_json::from_slice(B).unwrap();
    left["answers"] = cases
        .iter()
        .enumerate()
        .map(|(i, case)| {
            serde_json::json!({
                "case_id": case["id"],
                "artifact": match i {
                    0 => case["qualification"]["valid"][0].clone(),
                    1 => case["qualification"]["wrong"][0].clone(),
                    2 => serde_json::Value::Null,
                    _ => "".into()
                }
            })
        })
        .collect::<Vec<_>>()
        .into();
    right["answers"] = cases
        .iter()
        .enumerate()
        .map(|(i, case)| {
            serde_json::json!({
                "case_id": case["id"],
                "artifact": match i {
                    0 => case["qualification"]["wrong"][0].clone(),
                    1 | 2 => case["qualification"]["valid"][1].clone(),
                    _ => serde_json::Value::Null
                }
            })
        })
        .collect::<Vec<_>>()
        .into();
    let a = s.write("a.json", &serde_json::to_vec(&left).unwrap());
    let b = s.write("b.json", &serde_json::to_vec(&right).unwrap());
    let av = s.path("av");
    let bv = s.path("bv");
    success(&grade(&p, &a, &av));
    success(&grade(&p, &b, &bv));
    let result = study_compare(&m, &p, &av, &bv);
    success(&result);
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let plain = compare(&av, &bv);
    success(&plain);
    assert_eq!(
        report["comparison"],
        serde_json::from_slice::<serde_json::Value>(&plain.stdout).unwrap()
    );
    let families = report["families"].as_array().unwrap();
    assert_eq!(families.len(), 2);
    for family in families {
        assert_eq!(
            family["units"].as_array().unwrap().len(),
            1,
            "variants are not independent units"
        );
        assert_eq!(family["summary"]["n"], 2);
        assert_eq!(family["units"][0]["summary"], family["summary"]);
    }
    assert_eq!(families[0]["summary"]["paired"]["gains"], 1);
    assert_eq!(families[0]["summary"]["paired"]["losses"], 1);
    assert_eq!(
        families[0]["summary"]["delta_b_minus_a"],
        serde_json::json!({"lower":0,"upper":0,"denominator":2})
    );
    assert_eq!(families[1]["summary"]["paired"]["either_unknown"], 2);
    assert_eq!(
        families[1]["summary"]["delta_b_minus_a"],
        serde_json::json!({"lower":0,"upper":2,"denominator":2})
    );
    assert_eq!(families[1]["summary"]["left"]["malformed"], 1);
    assert_eq!(families[1]["units"][0]["cases"][0]["left"], "unknown");
    assert_eq!(families[1]["units"][0]["cases"][1]["right"], "unknown");
}

#[test]
fn study_rejects_unbound_views_even_when_plain_comparison_is_compatible() {
    let s = Scratch::new();
    let p = s.write("pack.json", PACK);
    let a = s.write("a.json", A);
    let b = s.write("b.json", B);
    let av = s.path("av");
    let bv = s.path("bv");
    success(&grade(&p, &a, &av));
    success(&grade(&p, &b, &bv));
    let m = s.write("study.json", STUDY);
    success(&study_compare(&m, &p, &av, &bv));
    assert!(
        !study_compare(&m, &p, &bv, &av).status.success(),
        "system order is binding"
    );
    let mut manifest: serde_json::Value = serde_json::from_slice(STUDY).unwrap();
    manifest["protocol"]["token_cap"]["value"] = 129.into();
    let changed = s.write(
        "changed-protocol.json",
        &serde_json::to_vec(&manifest).unwrap(),
    );
    success(&study_check(&changed, &p));
    assert!(!study_compare(&changed, &p, &av, &bv).status.success());
    // A label-only pack edit is compatible for ordinary comparison, but it is
    // not the exact source snapshot committed to by this study.
    let mut pack: serde_json::Value = serde_json::from_slice(PACK).unwrap();
    pack["label"] = "Relabeled task source".into();
    let changed_pack = s.write("relabeled.json", &serde_json::to_vec(&pack).unwrap());
    assert!(!study_check(&m, &changed_pack).status.success());
    manifest = serde_json::from_slice(STUDY).unwrap();
    manifest["pack_source"] = pack_source(&changed_pack);
    let changed = s.write(
        "changed-source.json",
        &serde_json::to_vec(&manifest).unwrap(),
    );
    success(&study_check(&changed, &changed_pack));
    success(&compare(&av, &bv));
    assert!(
        !study_compare(&changed, &changed_pack, &av, &bv)
            .status
            .success()
    );
    fs::write(av.join("view.json"), b"{}").unwrap();
    assert!(
        !study_compare(&m, &p, &av, &bv).status.success(),
        "receipts are reverified"
    );
}

#[test]
fn study_admission_is_closed_and_confirmation_cannot_hide_exposure() {
    let s = Scratch::new();
    let p = s.write("pack.json", PACK);
    let original: serde_json::Value = serde_json::from_slice(STUDY).unwrap();
    let mut invalid = Vec::new();
    let mut v = original.clone();
    v["families"].as_array_mut().unwrap().pop();
    invalid.push(v);
    let mut v = original.clone();
    v["exposure"].as_array_mut().unwrap().swap(0, 1);
    invalid.push(v);
    let mut v = original.clone();
    v["exposure"][0]["left"] = serde_json::json!({"fresh": null});
    invalid.push(v);
    let mut v = original.clone();
    v["left_system"] = serde_json::json!(["Synthetic submission A", "declared-model-a", null]);
    invalid.push(v);
    let mut v = original.clone();
    v["construct"] = "   ".into();
    invalid.push(v);
    let mut v = original.clone();
    v["ignored"] = true.into();
    invalid.push(v);
    let mut confirmation = original.clone();
    confirmation["purpose"] = "confirmation".into();
    invalid.push(confirmation.clone());
    for exposure in confirmation["exposure"].as_array_mut().unwrap() {
        exposure["left"] = "fresh".into();
        exposure["right"] = "fresh".into();
    }
    let fresh = s.write("fresh.json", &serde_json::to_vec(&confirmation).unwrap());
    success(&study_check(&fresh, &p));
    confirmation["exposure"][0]["right"] = "unknown".into();
    invalid.push(confirmation);
    for (i, value) in invalid.into_iter().enumerate() {
        let m = s.write(
            &format!("invalid-{i}.json"),
            &serde_json::to_vec(&value).unwrap(),
        );
        let result = study_check(&m, &p);
        assert!(!result.status.success(), "accepted invalid manifest {i}");
        assert!(
            result.stdout.is_empty(),
            "no partial report on invalid input"
        );
    }
    let duplicate = String::from_utf8(STUDY.to_vec()).unwrap().replacen(
        "\"version\": 1",
        "\"version\": 1, \"version\": 1",
        1,
    );
    let m = s.write("duplicate.json", duplicate.as_bytes());
    assert!(!study_check(&m, &p).status.success());
    let mut pack: serde_json::Value = serde_json::from_slice(PACK).unwrap();
    pack["cases"][1]["world"] = pack["cases"][0]["world"].clone();
    pack["worlds"].as_array_mut().unwrap().remove(1);
    let mixed = s.write("mixed-world.json", &serde_json::to_vec(&pack).unwrap());
    let mut manifest = original;
    manifest["pack_source"] = pack_source(&mixed);
    let m = s.write("mixed-study.json", &serde_json::to_vec(&manifest).unwrap());
    assert!(
        !study_check(&m, &mixed).status.success(),
        "one problem unit cannot span families"
    );
}
