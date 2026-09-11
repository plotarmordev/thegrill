use super::*;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bounded_capture_preserves_invalid_prefix_facts_but_not_missing_usage_guesses() {
    // Hashing is outside these synthetic request-deadline scenarios.
    evidence::binary_digest().unwrap();
    for (name, headers, prefix, status, attempt_status) in [
        (
            "http",
            "HTTP/1.1 503 Unavailable\r\nContent-Type: text/plain\r\n\r\n",
            "oops",
            "stopped-after-response-failure",
            Status::HttpError,
        ),
        (
            "media",
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n",
            "oops",
            "stopped-after-response-failure",
            Status::Unsupported,
        ),
        (
            "over-cap",
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":401}}\n\n",
            "stopped-after-ineligible-response",
            Status::Unsupported,
        ),
        (
            "missing-usage",
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
            "budget-exhausted",
            Status::Interrupted,
        ),
        (
            "short-prefix",
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":4}}\n\n",
            "budget-exhausted",
            Status::Interrupted,
        ),
    ] {
        let temp = Temp(
            std::env::temp_dir().join(format!("grill-perf-bounded-{}-{name}", std::process::id())),
        );
        std::fs::create_dir(&temp.0).unwrap();
        let input = temp.0.join("workload.json");
        std::fs::write(&input, serde_json::to_vec(&json!({
            "version":1,"name":"bounded-fixture","request":{"profile":"vllm-fixed-v1","stream":true,"output":{"tokens":400,"mode":"exact"},"cache":"observe","temperature_milli":0,"top_p_milli":1000},
            "limits":{"total_ms":60000,"idle_ms":30000,"response_bytes":65536,"wave_buffer_bytes":33554432},
            "cases":[{"id":"one","messages":[{"role":"user","content":"Synthetic output."}]}],
            "cells":[{"id":"cell","case":"one","concurrency":1,"warmup_trials":0,"trials":2}]
        })).unwrap()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let server = thread::spawn(move || {
            let mut requests = 0;
            while !stopped.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let headers_text = String::from_utf8(request).unwrap();
                let length: usize = headers_text
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                requests += 1;
                stream.write_all(headers.as_bytes()).unwrap();
                stream.write_all(prefix.as_bytes()).unwrap();
                stream.flush().unwrap();
                // Leave the response incomplete until rejection or the enclosing deadline.
                assert_eq!(stream.read(&mut byte).unwrap(), 0);
            }
            requests
        });
        let options = Options {
            workload: input,
            endpoint,
            model: "fixture-model".into(),
            out: temp.0.join("run"),
            deployment: None,
            policy: None,
            metrics_url: None,
            auth_env: None,
            local_http: true,
            json: true,
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let result = execute_bounded(&options, deadline);
        let returned_before_deadline = Instant::now() < deadline;
        stop.store(true, Ordering::SeqCst);
        assert_eq!(server.join().unwrap(), 1, "{name}");
        let summary = result.unwrap();
        assert_eq!(summary.status, status, "{name}");
        assert_eq!(summary.published_waves, 1, "{name}");
        assert!(!options.out.join("wave-000001").exists(), "{name}");
        let loaded = evidence::load(&options.out).unwrap();
        let attempt = &loaded.waves[0].as_ref().unwrap().attempts[0];
        assert_eq!(attempt.status, attempt_status, "{name}");
        assert!(attempt.terminal_offset.is_none(), "{name}");
        if name == "over-cap" {
            assert!(
                returned_before_deadline,
                "over-cap prefix waited for the enclosing deadline"
            );
            assert_eq!(attempt.usage.completion_tokens, Some(401));
            assert!(
                attempt
                    .eligibility_errors
                    .iter()
                    .any(|error| error == "reported_output_exceeds_cap")
            );
        }
        if matches!(name, "over-cap" | "missing-usage") {
            let path = options.out.join("wave-000000/wave.json");
            let original = std::fs::read(&path).unwrap();
            let mut receipt: Wave = serde_json::from_slice(&original).unwrap();
            receipt.attempts[0].usage.completion_tokens = Some(402);
            receipt.attempts[0].eligibility_errors = eligibility(
                &receipt.attempts[0],
                &loaded.plan.workload.request,
                receipt.spec.phase,
            );
            std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            assert!(
                evidence::load(&options.out).is_err(),
                "{name}: edited partial usage"
            );
            let mut receipt: Wave = serde_json::from_slice(&original).unwrap();
            receipt.attempts[0].finish_reason = Some("length".into());
            std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            assert!(
                evidence::load(&options.out).is_err(),
                "{name}: invented partial finish"
            );
            std::fs::write(&path, original).unwrap();
        }
    }
}
