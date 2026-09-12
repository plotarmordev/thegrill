use super::*;

#[test]
fn output_parent_failures_do_not_create_or_overwrite_evidence() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let declaration = deployment(&temp, "serving.json", "unchanged");
    let missing = baseline(&temp, &server, &declaration, "missing/before");
    assert_eq!(missing.status.code(), Some(1));
    let error = String::from_utf8(missing.stderr).unwrap();
    assert!(error.contains(temp.path("missing").to_str().unwrap()));
    assert!(error.contains("create"));
    assert!(!temp.path("missing").exists());

    fs::create_dir(temp.path("before")).unwrap();
    fs::write(temp.path("before/private-evidence"), b"retain exactly").unwrap();
    let existing = baseline(&temp, &server, &declaration, "before");
    assert_eq!(existing.status.code(), Some(1));
    assert_eq!(
        fs::read(temp.path("before/private-evidence")).unwrap(),
        b"retain exactly"
    );
    assert!(!temp.path("before/report.json").exists());

    let candidate = check(&temp, "before", &declaration, "missing/after");
    assert_eq!(candidate.status.code(), Some(1));
    let error = String::from_utf8(candidate.stderr).unwrap();
    assert!(error.contains(temp.path("missing").to_str().unwrap()));
    assert!(error.contains("create"));
    assert!(!temp.path("missing").exists());

    std::os::unix::fs::symlink(&temp.0, temp.path("linked-parent")).unwrap();
    let linked = baseline(&temp, &server, &declaration, "linked-parent/after");
    assert_eq!(linked.status.code(), Some(1));
    assert!(!temp.path("after").exists());
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}

#[test]
fn absent_or_blank_deployment_identity_is_identified_before_dispatch() {
    let temp = Temp::new();
    let server = Server::new(|stream, _, _| response(stream, Some(400), false));
    let declaration = deployment(&temp, "serving.json", "unchanged");
    let original = read_json(&declaration);
    for (field, invalid) in [
        ("model_revision", None),
        ("runtime", Some(Value::Null)),
        ("hardware", Some(json!(""))),
        ("settings", Some(json!("   "))),
    ] {
        let mut input = original.clone();
        match invalid {
            Some(value) => input[field] = value,
            None => {
                input.as_object_mut().unwrap().remove(field);
            }
        }
        write_json(&declaration, &input);
        let output = baseline(&temp, &server, &declaration, field);
        assert_eq!(output.status.code(), Some(1));
        let report = decoded(&output);
        assert_eq!(report["result"], "INVALID");
        assert_eq!(report["baseline_ready"], false);
        assert!(report["baseline_accounting"].is_null());
        assert!(report["reasons"].as_array().unwrap().iter().any(|reason| {
            reason
                .as_str()
                .unwrap()
                .contains(&format!("nonempty {field}"))
        }));
        assert_eq!(
            read_json(&temp.path(&format!("{field}/report.json"))),
            report
        );
        assert!(!temp.path(&format!("{field}/capture.json")).exists());
        assert!(!temp.path(&format!("{field}/acquisition-00")).exists());
    }
    assert_eq!(server.count.load(Ordering::SeqCst), 0);
}
