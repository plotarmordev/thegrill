use super::*;

fn close(actual: Option<f64>, expected: Option<f64>) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => {
            assert!((actual - expected).abs() <= expected.abs() * 1e-9)
        }
        (None, None) => (),
        pair => panic!("undefined-rate mismatch: {pair:?}"),
    }
}

#[test]
fn shared_arrival_traces_preserve_chunk_clocks_and_distinct_rate_arithmetic() {
    // Eight reported tokens give seven decode intervals. Text spans 0.02s;
    // ordinary settlement spans 0.05s, and terminal-tail settlement spans 0.09s.
    let expected = [
        (
            "paced",
            Some(30_000),
            Some(50_000),
            Some(8),
            60_000,
            Some(350.0),
            Some(140.0),
        ),
        (
            "fragmented",
            Some(30_000),
            Some(50_000),
            Some(8),
            60_000,
            Some(350.0),
            Some(140.0),
        ),
        (
            "coalesced",
            Some(10_000),
            Some(10_000),
            Some(8),
            60_000,
            None,
            Some(140.0),
        ),
        (
            "terminal-tail",
            Some(30_000),
            Some(90_000),
            Some(8),
            100_000,
            Some(350.0),
            Some(700.0 / 9.0),
        ),
        (
            "missing-usage",
            Some(30_000),
            Some(50_000),
            None,
            60_000,
            None,
            None,
        ),
        (
            "early-output",
            Some(30_000),
            Some(50_000),
            Some(4),
            60_000,
            Some(150.0),
            Some(60.0),
        ),
        ("error", Some(10_000), None, None, 40_000, None, None),
    ];
    for (trace, (name, last, terminal, tokens, settle, text_rate, settlement_rate)) in
        measurement_fixtures::traces().into_iter().zip(expected)
    {
        assert_eq!(trace.name, name);
        let mut parser = Parser::new(SseLimits {
            line_bytes: FRAME_CAP,
            event_bytes: FRAME_CAP,
        })
        .unwrap();
        let mut semantic = Semantic::default();
        let mut timing = Timing {
            headers_us: Some(0),
            first_body_us: Some(trace.chunks[0].0),
            settle_us: settle,
            ..Timing::default()
        };
        let mut status = Status::Incomplete;
        let mut terminal_offset = None;
        let mut body = Vec::new();
        for (observed, chunk) in trace.chunks {
            let previous = body.len();
            body.extend_from_slice(&chunk);
            match parser.feed(&chunk, |event| {
                semantic.event(event, true, observed, &mut timing)
            }) {
                Ok(Some(consumed)) => {
                    status = Status::Complete;
                    terminal_offset = Some(previous + consumed);
                    break;
                }
                Ok(None) => (),
                Err(grill_sse::Error::Handler((error, _))) => {
                    status = error;
                    break;
                }
                Err(error) => panic!("{name}: {error:?}"),
            }
        }
        assert_eq!(timing.first_generated_text_us, Some(10_000), "{name}");
        assert_eq!(timing.last_generated_text_us, last, "{name}");
        assert_eq!(timing.terminal_us, terminal, "{name}");
        assert_eq!(
            timing.first_generated_channel,
            Some(TextChannel::Reasoning),
            "{name}"
        );
        assert_eq!(semantic.usage.completion_tokens, tokens, "{name}");
        timing.validate(true).unwrap();
        let attempt = Attempt {
            lane: 0,
            dispatched: true,
            status,
            detail: String::new(),
            http_status: Some(200),
            finish_reason: semantic.finish,
            usage: semantic.usage,
            timing,
            response_bytes: body.len(),
            response_sha256: crate::evidence::digest(&body),
            terminal_offset,
            surplus_observed_bytes: 0,
            eligibility_errors: Vec::new(),
            sequence: None,
        };
        close(crate::evidence::text_decode_rate(&attempt), text_rate);
        close(
            crate::evidence::decode_sample(&attempt)
                .map(|(n, d)| n as f64 * 1_000_000.0 / d as f64),
            settlement_rate,
        );
        if attempt.status == Status::Complete {
            verify_complete(&attempt, &body, true, true, Profile::VllmFixedV1).unwrap();
        } else {
            assert_eq!(attempt.status, Status::Unsupported);
            verify_partial_arrivals(&attempt, &body, true, Profile::VllmFixedV1).unwrap();
        }
        let mut peer = attempt.clone();
        peer.lane = 1;
        peer.timing.dispatch_offset_us = 5_000;
        peer.timing.settle_us += 10_000;
        let lanes = [attempt, peer];
        let start = lanes
            .iter()
            .map(|a| a.timing.dispatch_offset_us)
            .min()
            .unwrap();
        let end = lanes
            .iter()
            .map(|a| a.timing.dispatch_offset_us + a.timing.settle_us)
            .max()
            .unwrap();
        assert_eq!(end - start, settle + 15_000);
        let (eligible, total, rate) = crate::evidence::throughput(&lanes, end - start);
        assert_eq!(total, tokens.map(|n| n * 2));
        assert_eq!(
            eligible,
            tokens.is_some() && lanes.iter().all(|a| a.status == Status::Complete)
        );
        close(
            rate,
            tokens.map(|n| n as f64 * 2_000_000.0 / (settle + 15_000) as f64),
        );
    }
}

#[test]
fn conversation_stream_lineage_uses_only_accepted_text_before_delayed_terminal() {
    let content = "\n{\"fact\":\"copper\"}\n";
    let event = |delta, finish| {
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "choices":[{"index":0,"delta":delta,"finish_reason":finish}]
            })
        )
        .into_bytes()
    };
    let mut chunks = vec![
        (
            10_000,
            event(
                serde_json::json!({"role":"assistant","reasoning_content":"checking"}),
                None::<&str>,
            ),
        ),
        (
            20_000,
            event(serde_json::json!({"content": &content[..5]}), None),
        ),
        (
            30_000,
            event(serde_json::json!({"content": &content[5..]}), Some("stop")),
        ),
    ];
    let mut terminal = b"data: [DONE]\n\n".to_vec();
    terminal.extend(event(serde_json::json!({"content":"not retained"}), None));
    chunks.push((90_000, terminal));
    let mut parser = Parser::new(SseLimits {
        line_bytes: FRAME_CAP,
        event_bytes: FRAME_CAP,
    })
    .unwrap();
    let mut semantic = Semantic::default();
    let mut timing = Timing {
        headers_us: Some(0),
        first_body_us: Some(10_000),
        settle_us: 100_000,
        ..Timing::default()
    };
    let mut body = Vec::new();
    let mut terminal_offset = None;
    for (observed, chunk) in chunks {
        let previous = body.len();
        body.extend_from_slice(&chunk);
        if let Some(consumed) = parser
            .feed(&chunk, |bytes| {
                semantic.event(bytes, true, observed, &mut timing)
            })
            .unwrap()
        {
            terminal_offset = Some(previous + consumed);
        }
    }
    let attempt = Attempt {
        lane: 0,
        dispatched: true,
        status: Status::Complete,
        detail: String::new(),
        http_status: Some(200),
        finish_reason: semantic.finish,
        usage: semantic.usage,
        timing,
        response_bytes: body.len(),
        response_sha256: crate::evidence::digest(&body),
        terminal_offset,
        surplus_observed_bytes: body.len() - terminal_offset.unwrap(),
        eligibility_errors: Vec::new(),
        sequence: None,
    };
    assert_eq!(attempt.timing.first_generated_text_us, Some(10_000));
    assert_eq!(attempt.timing.first_answer_text_us, Some(20_000));
    assert_eq!(attempt.timing.last_generated_text_us, Some(30_000));
    assert_eq!(attempt.timing.terminal_us, Some(90_000));
    assert_eq!(sequence_answer(&attempt, &body, true).unwrap(), content);
    let mut tampered = attempt.clone();
    tampered.terminal_offset = Some(body.len());
    assert!(sequence_answer(&tampered, &body, true).is_err());
}
