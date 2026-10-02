use super::*;

fn workload() -> Workload {
    serde_json::from_str(include_str!(
        "../../examples/prefill-prose-portable-v1.json"
    ))
    .unwrap()
}

#[test]
fn prose_request_replays_from_recorded_namespace_wave_and_lane() {
    let workload = workload();
    workload.validate().unwrap();
    let context = BodyContext {
        workload: &workload,
        model: "fixture",
        cache_namespace: Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
    };
    let wave = &workload.waves()[0];
    let first = request_body(&context, wave, 0).unwrap();
    let mut bodies = std::collections::HashSet::new();
    for spec in workload.waves() {
        for lane in 0..2 {
            let encoded = request_body(&context, &spec, lane).unwrap();
            let body: serde_json::Value = serde_json::from_str(&encoded).unwrap();
            let content = body["messages"][0]["content"].as_str().unwrap();
            let fill = content.lines().nth(2).unwrap();
            assert!(bodies.insert(fill.to_owned()));
            let case = workload
                .cases
                .iter()
                .find(|c| Some(&c.id) == spec.case.as_ref())
                .unwrap();
            assert_eq!(fill.chars().count(), case.fill.as_ref().unwrap().bytes());
        }
    }
    let saved_workload: Workload =
        serde_json::from_slice(&serde_json::to_vec(&workload).unwrap()).unwrap();
    let saved_wave: WaveSpec = serde_json::from_slice(&serde_json::to_vec(wave).unwrap()).unwrap();
    let saved_namespace: String =
        serde_json::from_str(&serde_json::to_string(context.cache_namespace.unwrap()).unwrap())
            .unwrap();
    let restored = BodyContext {
        workload: &saved_workload,
        model: "fixture",
        cache_namespace: Some(&saved_namespace),
    };
    assert_eq!(request_body(&restored, &saved_wave, 0).unwrap(), first);
    let other = BodyContext {
        cache_namespace: Some("fedcba98765432100123456789abcdef0123456789abcdef0123456789abcdef"),
        ..context
    };
    let other: serde_json::Value =
        serde_json::from_str(&request_body(&other, wave, 0).unwrap()).unwrap();
    assert!(
        !bodies.contains(
            other["messages"][0]["content"]
                .as_str()
                .unwrap()
                .lines()
                .nth(2)
                .unwrap()
        )
    );
}

#[test]
fn prose_size_and_sentence_boundaries_are_exact() {
    let mut full = String::new();
    prose_v1(&mut full, REQUEST_CAP, "0123456789abcdef-0-0");
    assert_eq!(full.len(), REQUEST_CAP);
    assert!(full.is_ascii());
    // An independently regenerated vector pins saved-request replay across releases.
    assert_eq!(
        crate::evidence::digest(&full.as_bytes()[..256]),
        "f9642873558c666e967ef7c64fe36433e34e7b0c07472e3309ca100871f5cc18"
    );
    let mut lengths = std::collections::HashSet::new();
    for sentence in full.split('.').rev().skip(1) {
        let words = sentence.split_whitespace().count();
        assert!((6..=16).contains(&words));
        lengths.insert(words);
        assert!(
            sentence
                .trim_start()
                .starts_with(|c: char| c.is_ascii_uppercase())
        );
    }
    assert_eq!(lengths.len(), 11);
    for length in [1, 2, 5, 6, 7, 31, 256, 4096] {
        let mut rendered = "header".to_owned();
        prose_v1(&mut rendered, length, "0123456789abcdef-0-0");
        assert_eq!(rendered, format!("header{}", &full[..length]));
    }
}

#[test]
fn prose_admission_rejects_ambiguous_unbounded_and_underbudget_inputs() {
    for fill in [
        r#"{"kind":"generated-prose-v2","characters":100}"#,
        r#"{"kind":"generated-prose-v1","characters":100,"unit":" the","repeat":1}"#,
        r#"{"kind":"generated-prose-v1","characters":100,"characters":101}"#,
        r#"{"kind":"generated-prose-v1","characters":-1}"#,
        r#"{"kind":"generated-prose-v1","characters":100,"seed":1}"#,
    ] {
        assert!(serde_json::from_str::<Fill>(fill).is_err(), "{fill}");
    }
    let mut workload = workload();
    workload.cases.truncate(1);
    workload.cells.truncate(1);
    for characters in [0, REQUEST_CAP as u32, u32::MAX] {
        workload.cases[0].fill = Some(Fill::GeneratedProse {
            kind: ProseKind::V1,
            characters,
        });
        assert!(workload.validate().is_err());
    }
    workload.cases[0].fill = Some(Fill::GeneratedProse {
        kind: ProseKind::V1,
        characters: 400,
    });
    let required = 2 * workload.limits.response_bytes + 6 * FRAME_CAP + 512 * 1024 + 400;
    workload.limits.wave_buffer_bytes = required - 1;
    assert!(workload.validate().is_err());
    workload.limits.wave_buffer_bytes = required;
    workload.validate().unwrap();
}
