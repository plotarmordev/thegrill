use super::*;

/// Measured trial 1, lane 1 of `workload(2, 1, 2)`: seed 42 + 64 * trial + lane.
const LATE_LANE_SEED: i64 = 42 + 64 + 1;
const ANSWER: [&str; 6] = ["Caf\u{e9} ", "one, ", "two, ", "three, ", "four, ", "five."];

/// Every lane's deltas: reasoning through both provider fields, then `ANSWER`.
fn deltas() -> Vec<Value> {
    let mut deltas = vec![
        json!({"reasoning":"Plan. "}),
        json!({"reasoning_content":"Check. "}),
    ];
    deltas.extend(ANSWER.iter().map(|token| json!({"content":token})));
    deltas
}

/// Rewrites the late lane's deltas before they are streamed.
type Edit = fn(&mut Vec<Value>);

fn unchanged(_: &mut Vec<Value>) {}

/// Streams `deltas()`, edited by `late` on the late lane only.
fn story(late: Edit) -> impl Fn(TcpStream, usize, Value) + Send + Sync {
    move |mut s, _, request| {
        header(&mut s, "text/event-stream");
        let mut deltas = deltas();
        if request["seed"] == LATE_LANE_SEED {
            late(&mut deltas);
        }
        for delta in deltas {
            frame(
                &mut s,
                json!({"id":"fixture","choices":[{"index":0,"delta":delta}]}),
            );
        }
        finish(&mut s, Some(8), Some(0));
    }
}

fn outputs(temp: &Temp, a: &str, b: &str) -> Output {
    cli()
        .arg("outputs")
        .arg(temp.path(a))
        .arg(temp.path(b))
        .arg("--json")
        .output()
        .unwrap()
}

fn report(output: &Output, code: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn refused(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(reason), "{stderr}");
}

#[test]
fn identical_runs_pass_and_each_first_difference_is_located() {
    let temp = Temp::new();
    let a = Server::new(story(unchanged));
    for name in ["a", "a2"] {
        successful(&run(&temp, &a, name, &workload(2, 1, 2)));
    }
    let same = report(&outputs(&temp, "a", "a2"), 0);
    assert_eq!(same["kind"], "performance-output-identity-v1");
    assert_eq!(
        [&same["identical"], &same["differs"], &same["unavailable"]],
        [4, 0, 0]
    );
    assert_eq!(same["lanes"][0]["answer_chars"], 33);
    assert_eq!(same["lanes"][0]["reasoning_chars"], 13);

    // "é" is one character and two UTF-8 bytes before each answer difference.
    let cases: [(&str, Edit, _); 3] = [
        (
            "last",
            |d| *d.last_mut().unwrap() = json!({"content":"six."}),
            (
                "answer",
                28,
                29,
                " one, two, three, four, five.",
                " one, two, three, four, six.",
            ),
        ),
        (
            "prefix",
            |d| drop(d.pop()),
            (
                "answer",
                28,
                29,
                " one, two, three, four, five.",
                " one, two, three, four, ",
            ),
        ),
        (
            "reasoning",
            |d| d[1] = json!({"reasoning_content":"Cheek. "}),
            ("reasoning", 9, 9, "Plan. Check. ", "Plan. Cheek. "),
        ),
    ];
    for (name, late, (channel, chars, bytes, a_excerpt, b_excerpt)) in cases {
        let b = Server::new(story(late));
        successful(&run(&temp, &b, name, &workload(2, 1, 2)));
        let changed = report(&outputs(&temp, "a", name), 2);
        assert_eq!(
            [
                &changed["identical"],
                &changed["differs"],
                &changed["unavailable"]
            ],
            [3, 1, 0],
            "{name}"
        );
        let lanes = changed["lanes"].as_array().unwrap();
        assert!(lanes.iter().all(|l| l["wave"] != 0), "warmups are excluded");
        let lane = lanes.iter().find(|l| l["status"] == "differs").unwrap();
        assert_eq!([&lane["wave"], &lane["trial"], &lane["lane"]], [2, 1, 1]);
        assert_eq!(lane["channel"], channel, "{name}");
        assert_eq!([&lane["char_offset"], &lane["byte_offset"]], [chars, bytes]);
        assert_eq!(
            [&lane["a_excerpt"], &lane["b_excerpt"]],
            [a_excerpt, b_excerpt]
        );
    }
}

#[test]
fn sampled_salted_or_different_workloads_are_refused() {
    let temp = Temp::new();
    let server = Server::new(story(unchanged));
    for (name, temperature) in [("hot", json!(700)), ("default", Value::Null)] {
        let mut work = workload(1, 0, 1);
        work["request"]["temperature_milli"] = temperature;
        successful(&run(&temp, &server, name, &work));
        refused(&outputs(&temp, name, name), "greedy decoding");
    }
    let mut salted = workload(1, 0, 1);
    salted["cases"][0]["messages"][0]["content"] = json!("{salt} {fill}");
    salted["cases"][0]["fill"] = json!({"unit":"x","repeat":1});
    successful(&run(&temp, &server, "salted", &salted));
    refused(&outputs(&temp, "salted", "salted"), "{salt}");
    successful(&run(&temp, &server, "one", &workload(1, 0, 1)));
    successful(&run(&temp, &server, "two", &workload(1, 0, 2)));
    refused(&outputs(&temp, "one", "two"), "same normalized workload");
}

#[test]
fn generated_prose_without_header_salt_cannot_claim_output_identity() {
    let temp = Temp::new();
    let server = Server::new(story(unchanged));
    let mut work = workload(1, 0, 1);
    work["cases"][0]["messages"][0]["content"] = json!("{fill}");
    work["cases"][0]["fill"] = json!({"kind":"generated-prose-v1","characters":256});
    successful(&run(&temp, &server, "prose", &work));
    refused(&outputs(&temp, "prose", "prose"), "generated prose");
}

#[test]
fn incomplete_lane_is_unavailable_never_identical() {
    let temp = Temp::new();
    let whole = Server::new(story(unchanged));
    let cut = Server::new(|mut s, _, request| {
        if request["seed"] == LATE_LANE_SEED {
            header(&mut s, "text/event-stream");
            frame(
                &mut s,
                json!({"id":"fixture","choices":[{"index":0,"delta":{"content":ANSWER[0]}}]}),
            );
        } else {
            story(unchanged)(s, 0, request);
        }
    });
    successful(&run(&temp, &whole, "a", &workload(2, 1, 2)));
    run(&temp, &cut, "b", &workload(2, 1, 2));
    let changed = report(&outputs(&temp, "a", "b"), 2);
    assert_eq!(
        [
            &changed["identical"],
            &changed["differs"],
            &changed["unavailable"]
        ],
        [3, 0, 1]
    );
    let lane = changed["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["status"] == "unavailable")
        .unwrap();
    assert_eq!([&lane["wave"], &lane["lane"]], [2, 1]);
    let reasons = lane["reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), 1);
    assert!(
        reasons[0].as_str().unwrap().starts_with("b: "),
        "{reasons:?}"
    );
}
