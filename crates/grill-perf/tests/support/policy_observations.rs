use super::*;

#[test]
fn envelope_failures_preserve_error_precedence() {
    for reason in [
        EnvelopeReason::InvalidRational,
        EnvelopeReason::InvalidBounds,
        EnvelopeReason::ArithmeticOverflow {
            adverse_bounds: None,
        },
    ] {
        assert_eq!(Outcome::from(reason), Outcome::Error);
    }
    for reason in [
        EnvelopeReason::NonpositiveReference,
        EnvelopeReason::ReferenceSpreadExceeded,
        EnvelopeReason::EnvelopeStraddlesTolerance {
            adverse_bounds: [0.0, 1.0],
        },
    ] {
        assert_eq!(Outcome::from(reason), Outcome::Inconclusive);
    }
}

fn attempt(lane: u32) -> Attempt {
    Attempt {
        lane,
        dispatched: true,
        status: Status::Complete,
        detail: String::new(),
        http_status: Some(200),
        finish_reason: Some("stop".into()),
        usage: serde_json::from_value(serde_json::json!({
            "prompt_tokens":4,"completion_tokens":8,"total_tokens":12,
            "cached_prompt_tokens":1,"reasoning_tokens":null
        }))
        .unwrap(),
        timing: Timing {
            dispatch_offset_us: u64::MAX,
            headers_us: Some(1),
            first_body_us: Some(2),
            first_generated_text_us: Some(3),
            first_generated_channel: Some(TextChannel::Reasoning),
            first_answer_text_us: Some(5),
            first_tool_delta_us: None,
            first_validated_tool_call_us: None,
            tool_stream_arrivals: None,
            last_generated_text_us: Some(7),
            terminal_us: Some(11),
            settle_us: 13,
            capture_parse_us: 0,
        },
        response_bytes: 0,
        response_sha256: String::new(),
        terminal_offset: None,
        surplus_observed_bytes: 0,
        eligibility_errors: Vec::new(),
        sequence: None,
    }
}

#[test]
fn distinct_service_events_do_not_borrow_dispatch_or_terminal_clocks() {
    let mut attempt = attempt(0);
    assert_eq!(
        latency_sample(&attempt, Metric::FirstGeneratedTextUs)
            .unwrap()
            .numerator,
        3
    );
    assert_eq!(
        latency_sample(&attempt, Metric::FirstAnswerTextUs)
            .unwrap()
            .numerator,
        5
    );
    assert_eq!(
        latency_sample(&attempt, Metric::CompletionLatencyUs)
            .unwrap()
            .numerator,
        13
    );
    assert!(evidence::prefill_sample(&attempt).is_none());
    attempt.timing.first_answer_text_us = None;
    assert!(latency_sample(&attempt, Metric::FirstAnswerTextUs).is_none());
    assert_eq!(
        latency_sample(&attempt, Metric::CompletionLatencyUs)
            .unwrap()
            .numerator,
        13
    );
    attempt
        .eligibility_errors
        .push("required_hit_unavailable".into());
    assert!(latency_sample(&attempt, Metric::CompletionLatencyUs).is_none());
    attempt.eligibility_errors.clear();
    attempt.dispatched = false;
    assert!(latency_sample(&attempt, Metric::FirstGeneratedTextUs).is_none());
}

#[test]
fn fairness_requires_the_whole_admitted_population() {
    let mut wave = Wave {
        version: 1,
        plan_sha256: String::new(),
        reservation_sha256: String::new(),
        spec: WaveSpec {
            index: 0,
            phase: Phase::Measured,
            cell: "cell".into(),
            case: Some("case".into()),
            trial: 0,
            concurrency: 2,
            lanes: None,
            acquisition: None,
        },
        attempts: vec![attempt(0), attempt(1)],
        elapsed_us: u64::MAX,
        dispatch_spread_us: 0,
        preparation_us: 0,
        reservation_publication_us: 0,
        body_publication_us: 0,
        completion_tokens: Some(16),
        achieved_completion_tokens_per_second: None,
        eligible: true,
        metrics: None,
        schedule: None,
        acquisition_clock: None,
    };
    wave.attempts[1].timing.first_answer_text_us = Some(10);
    wave.attempts[1].timing.last_generated_text_us = Some(10);
    assert_eq!(
        fairness_sample(&wave, Metric::FirstAnswerMaxMinRatio),
        Some((10, 5).into())
    );
    assert_eq!(
        fairness_sample(&wave, Metric::WorstLaneFirstAnswerUs),
        Some((10, 1).into())
    );
    wave.attempts[1].status = Status::Incomplete;
    assert!(fairness_sample(&wave, Metric::FirstAnswerMaxMinRatio).is_none());
    assert!(fairness_sample(&wave, Metric::WorstLaneFirstAnswerUs).is_none());
    wave.attempts[1].status = Status::Complete;
    wave.attempts[1].timing.first_answer_text_us = None;
    assert!(fairness_sample(&wave, Metric::FirstAnswerMaxMinRatio).is_none());
    wave.attempts.pop();
    assert!(fairness_sample(&wave, Metric::WorstLaneFirstAnswerUs).is_none());
}
