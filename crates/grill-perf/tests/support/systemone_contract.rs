use super::*;

fn noul() -> Question {
    Question {
        kind: QuestionKind::Noul,
        instructions: "Does the fixture state hold?".into(),
        criteria: None,
    }
}

fn choice() -> Question {
    Question {
        kind: QuestionKind::Choice,
        instructions: "Which fixture label applies?".into(),
        criteria: Some(serde_json::json!({"alpha": "First label", "beta": "Second label"})),
    }
}

fn score() -> Question {
    Question {
        kind: QuestionKind::Score,
        instructions: "Which fixture level applies?".into(),
        criteria: Some(serde_json::json!(["low", "mid", "high"])),
    }
}

fn decl(questions: Vec<(&str, Question)>) -> RequestDecl {
    RequestDecl {
        id: "one".into(),
        state: Some(serde_json::json!("neutral fixture state")),
        questions: questions
            .into_iter()
            .map(|(id, question)| (id.to_string(), question))
            .collect(),
        images: Vec::new(),
    }
}

/// The contract violation for a response that must be rejected.
fn violation(request: &RequestDecl, body: &[u8]) -> (Status, Vec<String>) {
    match validate_response(request, body) {
        Ok(_) => panic!("expected a System One contract violation"),
        Err(violation) => violation,
    }
}

fn body(value: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

fn plan() -> Plan {
    Plan {
        version: 1,
        kind: PLAN_KIND.into(),
        name: "contract".into(),
        protocol: PROTOCOL.into(),
        requests: vec![decl(vec![("q", noul())])],
        cells: vec![Cell {
            id: "cell".into(),
            requests: vec!["one".into()],
            concurrency: 1,
            warmup_bursts: 1,
            measured_bursts: 3,
        }],
        limits: Limits {
            request_timeout_ms: 1_000,
            burst_timeout_ms: 2_000,
            response_bytes: 65_536,
        },
        slots: vec![
            Slot {
                id: "warm".into(),
                arm: Arm::A,
                warmup: true,
                index: 0,
            },
            Slot {
                id: "a".into(),
                arm: Arm::A,
                warmup: false,
                index: 1,
            },
            Slot {
                id: "b".into(),
                arm: Arm::B,
                warmup: false,
                index: 2,
            },
            Slot {
                id: "a2".into(),
                arm: Arm::A2,
                warmup: false,
                index: 3,
            },
        ],
        gates: vec![Gate {
            cell: "cell".into(),
            metric: Metric::LatencyUs,
            max_adverse_bps: 500,
            max_reference_spread_bps: 2_000,
        }],
    }
}

#[test]
fn body_is_deterministic_and_omits_absent_images() {
    let body = render_body(&decl(vec![("q", noul())])).unwrap();
    assert_eq!(
        body,
        concat!(
            r#"{"questions":{"q":{"instructions":"Does the fixture state hold?","#,
            r#""type":"noul"}},"state":"neutral fixture state"}"#
        )
    );
    assert_eq!(body, render_body(&decl(vec![("q", noul())])).unwrap());
}

#[test]
fn body_carries_null_state_and_data_url_images() {
    let mut request = decl(vec![("q", choice())]);
    request.state = None;
    request.images = vec!["data:image/png;base64,AA==".into()];
    let body = render_body(&request).unwrap();
    assert_eq!(
        body,
        concat!(
            r#"{"images":["data:image/png;base64,AA=="],"#,
            r#""questions":{"q":{"criteria":{"alpha":"First label","beta":"Second label"},"#,
            r#""instructions":"Which fixture label applies?","type":"choice"}},"state":null}"#
        )
    );
}

#[test]
fn accepts_every_declared_question_type() {
    let request = decl(vec![("asked", noul()), ("label", choice()), ("level", score())]);
    let answers = validate_response(
        &request,
        &body(serde_json::json!({
            "answers": {
                "asked": {"type": "noul", "noul": 0.75},
                "label": {
                    "type": "choice",
                    "choice": "alpha",
                    "confidence": 0.9,
                    "probabilities": {"alpha": 0.9, "beta": 0.1},
                },
                "level": {
                    "type": "score",
                    "score": 1.0,
                    "confidence": 0.5,
                    "probabilities": {"0": 0.2, "1": 0.4, "2": 0.4},
                    "legend": {"0": "low", "1": "mid", "2": "high"},
                },
            },
            "usage": {"input_tokens": 41, "output_tokens": 0},
        })),
    )
    .unwrap();
    assert_eq!(answers.decisions, 3);
    assert_eq!(answers.input_tokens, Some(41));
    assert_eq!(answers.output_tokens, Some(0));
}

#[test]
fn rejects_missing_and_unknown_answers() {
    let request = decl(vec![("asked", noul())]);
    let (status, errors) = violation(
        &request,
        &body(serde_json::json!({
            "answers": {},
            "usage": {"output_tokens": 0},
        })),
    );
    assert_eq!(status, Status::Unsupported);
    assert!(errors.contains(&"missing_answer:asked".to_string()));

    let (_, errors) = violation(
        &request,
        &body(serde_json::json!({
            "answers": {
                "asked": {"type": "noul", "noul": 0.5},
                "other": {"type": "noul", "noul": 0.5},
            },
            "usage": {"output_tokens": 0},
        })),
    );
    assert!(errors.contains(&"unknown_answer:other".to_string()));
}

#[test]
fn rejects_probabilities_outside_the_unit_interval() {
    for bad in [
        serde_json::json!(1.5),
        serde_json::json!(-0.25),
        serde_json::json!("0.5"),
    ] {
        let request = decl(vec![("asked", noul())]);
        let (status, errors) = violation(
            &request,
            &body(serde_json::json!({
                "answers": {"asked": {"type": "noul", "noul": bad}},
                "usage": {"output_tokens": 0},
            })),
        );
        assert_eq!(status, Status::Unsupported);
        assert!(errors.contains(&"invalid_probability:asked".to_string()));
    }
}

#[test]
fn rejects_distribution_and_choice_mismatches() {
    let request = decl(vec![("label", choice())]);
    let (_, errors) = violation(
        &request,
        &body(serde_json::json!({
            "answers": {
                "label": {
                    "type": "choice",
                    "choice": "alpha",
                    "confidence": 0.9,
                    "probabilities": {"alpha": 0.9, "beta": 0.1, "extra": 0.0},
                },
            },
            "usage": {"output_tokens": 0},
        })),
    );
    assert!(errors.contains(&"probability_keys_mismatch:label".to_string()));

    let (_, errors) = violation(
        &request,
        &body(serde_json::json!({
            "answers": {
                "label": {
                    "type": "choice",
                    "choice": "nothing",
                    "confidence": 0.9,
                    "probabilities": {"alpha": 0.9, "beta": 0.1},
                },
            },
            "usage": {"output_tokens": 0},
        })),
    );
    assert!(errors.contains(&"unknown_choice:label".to_string()));

    let (_, errors) = violation(
        &request,
        &body(serde_json::json!({
            "answers": {
                "label": {
                    "type": "score",
                    "score": 1.0,
                    "confidence": 0.5,
                    "probabilities": {"0": 0.5, "1": 0.5},
                },
            },
            "usage": {"output_tokens": 0},
        })),
    );
    assert!(errors.contains(&"answer_type_mismatch:label".to_string()));
}

#[test]
fn rejects_generated_tokens_and_absent_usage() {
    let request = decl(vec![("asked", noul())]);
    let (status, errors) = violation(
        &request,
        &body(serde_json::json!({
            "answers": {"asked": {"type": "noul", "noul": 0.5}},
            "usage": {"input_tokens": 9, "output_tokens": 8},
        })),
    );
    assert_eq!(status, Status::Unsupported);
    assert!(errors.contains(&"output_tokens_nonzero".to_string()));

    let (_, errors) = violation(
        &request,
        &body(serde_json::json!({
            "answers": {"asked": {"type": "noul", "noul": 0.5}},
        })),
    );
    assert!(errors.contains(&"missing_usage".to_string()));
}

#[test]
fn rejects_malformed_bodies() {
    let request = decl(vec![("asked", noul())]);
    let (status, errors) = violation(&request, b"{\"answers\":");
    assert_eq!(status, Status::Malformed);
    assert_eq!(errors, vec!["malformed_response".to_string()]);

    let (status, errors) = violation(&request, b"[]");
    assert_eq!(status, Status::Malformed);
    assert_eq!(errors, vec!["malformed_response".to_string()]);
}

#[test]
fn plan_accepts_the_declared_shape() {
    assert_eq!(plan().validate(), Ok(()));
}

#[test]
fn plan_rejects_identity_and_corpus_deviations() {
    let mut wrong_kind = plan();
    wrong_kind.kind = "systemone-plan-v2".into();
    assert!(wrong_kind.validate().is_err());

    let mut wrong_protocol = plan();
    wrong_protocol.protocol = "systemone-v2".into();
    assert!(wrong_protocol.validate().is_err());

    let mut stateless = plan();
    stateless.requests[0].state = None;
    assert!(stateless.validate().is_err());

    let mut criteria = plan();
    criteria.requests[0].questions.insert(
        "q".into(),
        Question {
            kind: QuestionKind::Choice,
            instructions: "Which?".into(),
            criteria: None,
        },
    );
    assert!(criteria.validate().is_err());

    let mut unknown = plan();
    unknown.cells[0].requests = vec!["other".into()];
    assert!(unknown.validate().is_err());
}

#[test]
fn plan_rejects_budget_slot_and_gate_deviations() {
    let mut bursts = plan();
    bursts.cells[0].measured_bursts = 0;
    assert!(bursts.validate().is_err());

    let mut concurrency = plan();
    concurrency.cells[0].concurrency = 4;
    assert!(concurrency.validate().is_err());

    let mut timeout = plan();
    timeout.limits.request_timeout_ms = 0;
    assert!(timeout.validate().is_err());

    let mut burst_timeout = plan();
    burst_timeout.limits.burst_timeout_ms = 10;
    assert!(burst_timeout.validate().is_err());

    let mut arm = plan();
    arm.slots.retain(|slot| slot.arm != Arm::A2);
    assert!(arm.validate().is_err());

    let mut duplicate_arm = plan();
    duplicate_arm.slots[3].arm = Arm::A;
    assert!(duplicate_arm.validate().is_err());

    let mut cell = plan();
    cell.gates[0].cell = "other".into();
    assert!(cell.validate().is_err());

    let mut bounds = plan();
    bounds.gates[0].max_adverse_bps = 10_000;
    assert!(bounds.validate().is_err());
}
