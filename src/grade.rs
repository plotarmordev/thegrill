use crate::contract::{Acceptance, Grids, Outcome};
use serde::Deserialize;

pub(crate) mod atlas;

pub(crate) const RELATION: &str = "json-answer-set-v1";
// Bump for every grading behavior change, independently of task gold.
pub(crate) const IMPLEMENTATION: &str = "grill-json-answer-set-impl-v1";
pub(crate) const STRING_RELATION: &str = "json-string-set-v2";
pub(crate) const STRING_IMPLEMENTATION: &str = "grill-json-string-set-impl-v2";
pub(crate) const GRID_RELATION: &str = "json-grid-set-v2";
pub(crate) const GRID_IMPLEMENTATION: &str = "grill-json-grid-set-impl-v2";
pub(crate) const FINAL_STRING_RELATION: &str = "final-marker-json-string-set-v1";
pub(crate) const FINAL_STRING_IMPLEMENTATION: &str = "grill-final-marker-json-string-set-impl-v1";
pub(crate) const FINAL_GRID_RELATION: &str = "final-marker-json-grid-set-v1";
pub(crate) const FINAL_GRID_IMPLEMENTATION: &str = "grill-final-marker-json-grid-set-impl-v1";

pub(crate) fn revision(acceptance: &Acceptance) -> (&'static str, &'static str) {
    match acceptance {
        Acceptance::ArchivalV1 { .. } => (RELATION, IMPLEMENTATION),
        Acceptance::StringSet { .. } => (STRING_RELATION, STRING_IMPLEMENTATION),
        Acceptance::GridSet { .. } => (GRID_RELATION, GRID_IMPLEMENTATION),
        Acceptance::FinalStringSet { .. } => (FINAL_STRING_RELATION, FINAL_STRING_IMPLEMENTATION),
        Acceptance::FinalGridSet { .. } => (FINAL_GRID_RELATION, FINAL_GRID_IMPLEMENTATION),
        Acceptance::FinalTextSet { .. } => (
            "final-marker-unicode-lowercase-word-set-v1",
            "grill-final-marker-unicode-lowercase-word-set-impl-v1",
        ),
        Acceptance::FinalNumber { .. } => (
            "final-marker-finite-number-v1",
            "grill-final-marker-finite-number-impl-v2",
        ),
        Acceptance::JsonExact { .. } => (
            "raw-json-exact-decimal-v1",
            "grill-raw-json-exact-decimal-impl-v1",
        ),
        Acceptance::TextConstraints { .. } => (
            "raw-text-constraints-v1",
            "grill-raw-text-constraints-impl-v1",
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    answer: String,
}

/// The frozen final-answer boundary of the final-marker relations.
const MARKER: &[u8] = b"===FINAL===";

/// Borrowed suffix after the single standalone marker line, or None when the
/// boundary is missing or ambiguous. Lines are separated by LF; one CR
/// immediately before an LF is termination, not content (CRLF); a bare CR is
/// content. The scan is deliberately structure-blind over the whole artifact,
/// so a marker line rendered anywhere - including inside a multi-line JSON
/// string - counts toward the exactly-one requirement. A final unterminated
/// marker line leaves an empty suffix, which the strict parser then rejects.
fn suffix(artifact: &[u8]) -> Option<&[u8]> {
    let mut found = None;
    let mut offset = 0;
    for piece in artifact.split(|b| *b == b'\n') {
        let end = offset + piece.len() + 1;
        if piece.strip_suffix(b"\r").unwrap_or(piece) == MARKER {
            if found.is_some() {
                return None;
            }
            found = Some(end);
        }
        offset = end;
    }
    found.map(|end| &artifact[end.min(artifact.len())..])
}

// The graded bytes go directly to a closed typed record. In particular, a
// Value/map intermediate would silently discard duplicate object keys. Only
// trailing JSON whitespace may follow the answer object.
fn strings(artifact: &[u8], accepted: &[String]) -> Outcome {
    match crate::record::parse::<Answer>(artifact) {
        Ok(a) if accepted.contains(&a.answer) => Outcome::Success,
        Ok(_) => Outcome::Wrong,
        Err(_) => Outcome::Malformed,
    }
}

fn grids(artifact: &[u8], outputs: &Grids) -> Outcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GridAnswer {
        answer: Grids,
    }
    match crate::record::parse::<GridAnswer>(artifact) {
        Ok(answer) if answer.answer == *outputs => Outcome::Success,
        Ok(_) => Outcome::Wrong,
        Err(_) => Outcome::Malformed,
    }
}

/// Decode the semantic string answer under this acceptance's envelope; used by
/// pack qualification to prove accepted-alternative coverage.
pub(crate) fn decode(artifact: &[u8], acceptance: &Acceptance) -> Option<String> {
    let bytes = match acceptance {
        Acceptance::FinalStringSet { .. }
        | Acceptance::FinalGridSet { .. }
        | Acceptance::FinalTextSet { .. } => suffix(artifact)?,
        _ => artifact,
    };
    crate::record::parse::<Answer>(bytes).ok().map(|a| a.answer)
}

pub(crate) fn grade(artifact: &[u8], acceptance: &Acceptance) -> Outcome {
    if artifact.len() > crate::contract::ARTIFACT_CAP
        && matches!(
            acceptance,
            Acceptance::FinalTextSet { .. }
                | Acceptance::FinalNumber { .. }
                | Acceptance::JsonExact { .. }
                | Acceptance::TextConstraints { .. }
        )
    {
        return Outcome::Malformed;
    }
    match acceptance {
        Acceptance::ArchivalV1 { accepted } | Acceptance::StringSet { accepted } => {
            strings(artifact, accepted)
        }
        Acceptance::GridSet { outputs } => grids(artifact, outputs),
        Acceptance::FinalStringSet { accepted } => match suffix(artifact) {
            Some(answer) => strings(answer, accepted),
            None => Outcome::Malformed,
        },
        Acceptance::FinalGridSet { outputs } => match suffix(artifact) {
            Some(answer) => grids(answer, outputs),
            None => Outcome::Malformed,
        },
        Acceptance::FinalTextSet { accepted } => match suffix(artifact) {
            Some(answer) => atlas::texts(answer, accepted),
            None => Outcome::Malformed,
        },
        Acceptance::FinalNumber { number } => match suffix(artifact) {
            Some(answer) => atlas::number(answer, number),
            None => Outcome::Malformed,
        },
        Acceptance::JsonExact { expected } => atlas::json(artifact, expected),
        Acceptance::TextConstraints { rules } => atlas::constraints(artifact, &rules.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_artifact_contract() {
        let accepted = Acceptance::ArchivalV1 {
            accepted: vec!["mint".into(), " sea ".into(), "".into()],
        };
        for bytes in [
            br#" {"answer":"m\u0069nt"} "#.as_slice(),
            br#"{"answer":" sea "}"#,
            br#"{"answer":""}"#,
        ] {
            assert_eq!(grade(bytes, &accepted), Outcome::Success);
        }
        for bytes in [br#"{"answer":"Mint"}"#.as_slice(), br#"{"answer":"sea"}"#] {
            assert_eq!(grade(bytes, &accepted), Outcome::Wrong);
        }
        for bytes in [
            b"".as_slice(),
            br#"{"answer":null,"answer":"mint"}"#,
            br#"{"answer":"mint","answer":"mint"}"#,
            br#"{"answer":"mint","x":1}"#,
            br#"{"answer":1}"#,
            br#"{"answer":null}"#,
            br#"{"answer":"mint"}{}"#,
            br#"{"answer":"mint"} prose"#,
            b"```json\n{\"answer\":\"mint\"}\n```",
            b"{\"answer\":\"\xff\"}",
            br#"["mint"]"#,
        ] {
            assert_eq!(grade(bytes, &accepted), Outcome::Malformed, "{bytes:?}");
        }
    }

    #[test]
    fn grids_preserve_order_and_distinguish_wrong_from_malformed() {
        let acceptance: Acceptance =
            crate::record::parse(br#"{"outputs":[[[0,1],[2,3]],[[9]]],"kind":"grid-set"}"#)
                .unwrap();
        assert_eq!(
            grade(br#"{"answer":[[[0,1],[2,3]],[[9]]]}"#, &acceptance),
            Outcome::Success
        );
        for artifact in [
            r#"{"answer":[[[9]],[[0,1],[2,3]]]}"#,
            r#"{"answer":[[[0,1],[2,3]]]}"#,
            r#"{"answer":[[[0,1,2]],[[9]]]}"#,
            r#"{"answer":[[[0,1],[2,4]],[[9]]]}"#,
        ] {
            assert_eq!(
                grade(artifact.as_bytes(), &acceptance),
                Outcome::Wrong,
                "{artifact}"
            );
        }
        for artifact in [
            r#"{"answer":[]}"#,
            r#"{"answer":[[]]}"#,
            r#"{"answer":[[[]]]}"#,
            r#"{"answer":[[[0],[1,2]]]}"#,
            r#"{"answer":[[[10]]]}"#,
            r#"{"answer":[[[-1]]]}"#,
            r#"{"answer":[[[1.0]]]}"#,
            r#"{"answer":[[[true]]]}"#,
            r#"{"answer":"[[[0]]]]"}"#,
            r#"{"answer":null,"answer":[[[0]]]}"#,
            r#"{"answer":[[[0]]],"\u0061nswer":[[[0]]]}"#,
            r#"{"answer":[[[0]]],"extra":0}"#,
            r#"[[[[0]]]]"#,
            r#"{"answer":[[[0]]]}{}"#,
        ] {
            assert_eq!(
                grade(artifact.as_bytes(), &acceptance),
                Outcome::Malformed,
                "{artifact}"
            );
        }
        for outputs in [
            vec![vec![vec![0u8]]; 9],
            vec![vec![vec![0u8]; 31]],
            vec![vec![vec![0u8; 31]]],
        ] {
            let artifact = serde_json::to_vec(&serde_json::json!({"answer": outputs})).unwrap();
            assert_eq!(grade(&artifact, &acceptance), Outcome::Malformed);
        }
        let maximum = vec![vec![vec![9u8; 30]; 30]; 8];
        let acceptance: Acceptance = crate::record::parse(
            &serde_json::to_vec(&serde_json::json!({"kind": "grid-set", "outputs": maximum}))
                .unwrap(),
        )
        .unwrap();
        let artifact = serde_json::to_vec(&serde_json::json!({"answer": maximum})).unwrap();
        assert_eq!(grade(&artifact, &acceptance), Outcome::Success);
    }

    #[test]
    fn acceptance_is_a_closed_typed_object() {
        for wire in [
            r#"{"kind":"grid-set","kind":"grid-set","outputs":[[[0]]]}"#,
            r#"{"kind":"grid-set","outputs":null,"outputs":[[[0]]]}"#,
            r#"{"kind":"grid-set","outputs":[[[0]]],"outputs":[[[0]]]}"#,
            r#"{"kind":"grid-set","outputs":[[[0]]],"accepted":["0"]}"#,
            r#"{"kind":"grid-set","outputs":[[[0]]],"unknown":0}"#,
            r#"{"kind":"grid-set","outputs":[[[0.0]]]}"#,
            r#"{"kind":{"grid-set":null},"outputs":[[[0]]]}"#,
            r#"["grid-set",[[[0]]]]"#,
            r#"{"kind":"string-set","accepted":["a"],"accepted":["b"]}"#,
            r#"{"kind":"final-string-set","outputs":[[[0]]]}"#,
            r#"{"kind":"final-grid-set","accepted":["a"]}"#,
            r#"{"kind":"final-string-set","accepted":["a"],"outputs":[[[0]]]}"#,
        ] {
            assert!(
                crate::record::parse::<Acceptance>(wire.as_bytes()).is_err(),
                "{wire}"
            );
        }
        let strings: Acceptance =
            crate::record::parse(br#"{"kind":"string-set","accepted":[" sea ",""]}"#).unwrap();
        assert_eq!(grade(br#"{"answer":" sea "}"#, &strings), Outcome::Success);
        assert_eq!(grade(br#"{"answer":"sea"}"#, &strings), Outcome::Wrong);
        assert_eq!(grade(br#"{"answer":[]}"#, &strings), Outcome::Malformed);
    }

    #[test]
    fn final_marker_boundary_contract() {
        let acceptance: Acceptance =
            crate::record::parse(br#"{"kind":"final-string-set","accepted":["mint"," sea "]}"#)
                .unwrap();
        // Preamble may be empty or prose; CRLF terminates the marker line like
        // LF; only JSON whitespace may trail the strict suffix object.
        for artifact in [
            b"===FINAL===\n{\"answer\":\"mint\"}".as_slice(),
            b"reasoning first...\nsecond thought\n===FINAL===\n{\"answer\":\"mint\"}",
            b"prose\r\n===FINAL===\r\n{\"answer\":\"mint\"}\r\n",
            b"===FINAL===\n {\"answer\": \"m\\u0069nt\"} \n\t\r\n",
            b"unterminated preamble then\n===FINAL===\n{\"answer\":\" sea \"}",
        ] {
            assert_eq!(
                grade(artifact, &acceptance),
                Outcome::Success,
                "{artifact:?}"
            );
        }
        // Protocol-compliant but semantically wrong stays wrong, not malformed.
        assert_eq!(
            grade(b"p\n===FINAL===\n{\"answer\":\"Mint\"}", &acceptance),
            Outcome::Wrong
        );
        for artifact in [
            // Missing boundary: no marker line anywhere, or decorated/indented
            // lookalikes, or a bare-CR "terminator" merging it into the preamble.
            b"{\"answer\":\"mint\"}".as_slice(),
            b"",
            b"preamble only\n",
            b" ===FINAL===\n{\"answer\":\"mint\"}",
            b"===FINAL=== \n{\"answer\":\"mint\"}",
            b"===FINAL===\r\r\n{\"answer\":\"mint\"}",
            b"p\r===FINAL===\n{\"answer\":\"mint\"}",
            // Unterminated marker line: empty suffix, nothing to parse.
            b"===FINAL===",
            b"p\n===FINAL===",
            // Ambiguous boundary: more than one marker line, wherever rendered.
            b"===FINAL===\n===FINAL===\n{\"answer\":\"mint\"}",
            b"===FINAL===\n{\"answer\":\"mint\"}\n===FINAL===",
            b"p\n===FINAL===\n{\"answer\":\"a\n===FINAL===\nb\"}",
            // Suffix violates the strict typed object contract.
            b"===FINAL===\n{\"answer\":null,\"answer\":\"mint\"}",
            b"===FINAL===\n{\"answer\":\"mint\",\"answer\":\"mint\"}",
            b"===FINAL===\n{\"answer\":\"mint\"}{}",
            b"===FINAL===\n{\"answer\":\"mint\"} trailing prose",
            b"===FINAL===\n```json\n{\"answer\":\"mint\"}\n```",
            b"===FINAL===\n{\"answer\":1}",
            b"===FINAL===\n{\"answer\":\"mint\"",
        ] {
            assert_eq!(
                grade(artifact, &acceptance),
                Outcome::Malformed,
                "{artifact:?}"
            );
        }
        // Grid answers keep their typed equality and strictness behind the marker.
        let grids: Acceptance =
            crate::record::parse(br#"{"kind":"final-grid-set","outputs":[[[0,1]]]}"#).unwrap();
        assert_eq!(
            grade(b"plan...\n===FINAL===\n{\"answer\":[[[0,1]]]}", &grids),
            Outcome::Success
        );
        assert_eq!(
            grade(b"===FINAL===\n{\"answer\":[[[1,0]]]}", &grids),
            Outcome::Wrong
        );
        for artifact in [
            b"===FINAL===\n{\"answer\":[[[0,1]]],\"answer\":[[[0,1]]]}".as_slice(),
            b"===FINAL===\n{\"answer\":[[[0.0,1]]]}",
            b"===FINAL===\n{\"answer\":\"[[[0,1]]]\"}",
            b"{\"answer\":[[[0,1]]]}",
        ] {
            assert_eq!(grade(artifact, &grids), Outcome::Malformed, "{artifact:?}");
        }
        // The envelope-aware qualification decode follows the same boundary.
        assert_eq!(
            decode(b"p\n===FINAL===\n{\"answer\":\"mint\"}", &acceptance).as_deref(),
            Some("mint")
        );
        assert_eq!(decode(b"{\"answer\":\"mint\"}", &acceptance), None);
    }
}
