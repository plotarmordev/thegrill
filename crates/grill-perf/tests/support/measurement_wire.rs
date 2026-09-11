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
        };
        close(crate::evidence::text_decode_rate(&attempt), text_rate);
        close(
            crate::evidence::decode_sample(&attempt)
                .map(|(n, d)| n as f64 * 1_000_000.0 / d as f64),
            settlement_rate,
        );
        if attempt.status == Status::Complete {
            verify_complete(&attempt, &body, true, true).unwrap();
        } else {
            assert_eq!(attempt.status, Status::Unsupported);
            verify_partial_arrivals(&attempt, &body, true).unwrap();
        }
    }
}
