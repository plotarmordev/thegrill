use crate::contract::{ARTIFACT_CAP, Acceptance, NumberAcceptance, Outcome, Result, TextRule};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

// Preserve raw number spelling for identities, but compare exact decimal values.
// Metadata points into the retained token: no f64, bigint, copied digit buffer,
// or exponent-sized expansion. The parser validates JSON syntax before this.
#[derive(Debug)]
pub(crate) struct NumberToken {
    raw: Box<RawValue>,
    negative: bool,
    digits: std::ops::Range<usize>,
    exponent: i64,
}

impl NumberToken {
    fn new(raw: &RawValue) -> std::result::Result<Self, serde_json::Error> {
        let token = raw.get();
        let negative = token.starts_with('-');
        let offset = usize::from(negative);
        let unsigned = &token[offset..];
        let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
            Some(at) => {
                let exponent = unsigned[at + 1..].parse::<i64>().map_err(|_| {
                    <serde_json::Error as de::Error>::custom(
                        "JSON number exponent exceeds i64 range",
                    )
                })?;
                (&unsigned[..at], exponent)
            }
            None => (unsigned, 0),
        };
        let bytes = mantissa.as_bytes();
        let first = bytes.iter().position(|b| matches!(b, b'1'..=b'9'));
        let Some(first) = first else {
            // Signed and exponent-decorated zeros share one mathematical value.
            // Parse the explicit exponent first, so even zero rejects overflow.
            return Ok(Self {
                raw: raw.to_owned(),
                negative: false,
                digits: 0..0,
                exponent: 0,
            });
        };
        let last = bytes
            .iter()
            .rposition(|b| matches!(b, b'1'..=b'9'))
            .expect("nonzero mantissa");
        let fraction = bytes
            .iter()
            .position(|b| *b == b'.')
            .map_or(0, |dot| bytes.len() - dot - 1);
        let trailing = bytes[last + 1..].iter().filter(|b| **b != b'.').count();
        // Both counts are bounded by ARTIFACT_CAP. Combine the adjustment
        // before checked addition so 1.0e<i64::MIN> does not spuriously overflow.
        let adjustment = trailing as i64 - fraction as i64;
        let exponent = exponent.checked_add(adjustment).ok_or_else(|| {
            <serde_json::Error as de::Error>::custom(
                "normalized JSON number exponent exceeds i64 range",
            )
        })?;
        Ok(Self {
            raw: raw.to_owned(),
            negative,
            digits: offset + first..offset + last + 1,
            exponent,
        })
    }

    fn digits(&self) -> impl Iterator<Item = u8> + '_ {
        self.raw.get()[self.digits.clone()]
            .bytes()
            .filter(|b| *b != b'.')
    }
}

impl PartialEq for NumberToken {
    fn eq(&self, other: &Self) -> bool {
        self.negative == other.negative
            && self.exponent == other.exponent
            && self.digits().eq(other.digits())
    }
}

impl Serialize for NumberToken {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.raw.serialize(s)
    }
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum ExactJson {
    Null,
    Bool(bool),
    Number(NumberToken),
    String(String),
    Array(Vec<ExactJson>),
    Object(BTreeMap<String, ExactJson>),
}

// Borrow raw subvalues instead of copying buffers. The explicit depth ceiling
// is necessary because each subvalue starts a fresh serde JSON deserializer.
const JSON_DEPTH: usize = 64;

struct JsonSeed(usize);

impl<'de> DeserializeSeed<'de> for JsonSeed {
    type Value = ExactJson;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<ExactJson, D::Error> {
        let raw = <&RawValue>::deserialize(d)?;
        parse_json(raw, self.0).map_err(de::Error::custom)
    }
}

fn parse_json(raw: &RawValue, depth: usize) -> std::result::Result<ExactJson, serde_json::Error> {
    if depth > JSON_DEPTH || raw.get().len() > ARTIFACT_CAP {
        return Err(de::Error::custom("exact JSON exceeds byte/depth limit"));
    }
    if matches!(raw.get().as_bytes().first(), Some(b'-' | b'0'..=b'9')) {
        return NumberToken::new(raw).map(ExactJson::Number);
    }
    struct JsonVisitor(usize);
    impl<'de> Visitor<'de> for JsonVisitor {
        type Value = ExactJson;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a duplicate-free JSON value")
        }
        fn visit_unit<E: de::Error>(self) -> std::result::Result<ExactJson, E> {
            Ok(ExactJson::Null)
        }
        fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<ExactJson, E> {
            Ok(ExactJson::Bool(value))
        }
        fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<ExactJson, E> {
            Ok(ExactJson::String(value.to_owned()))
        }
        fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<ExactJson, E> {
            Ok(ExactJson::String(value))
        }
        fn visit_seq<S: SeqAccess<'de>>(
            self,
            mut seq: S,
        ) -> std::result::Result<ExactJson, S::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element_seed(JsonSeed(self.0 + 1))? {
                values.push(value);
            }
            Ok(ExactJson::Array(values))
        }
        fn visit_map<M: MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<ExactJson, M::Error> {
            let mut values = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if values.contains_key(&key) {
                    return Err(de::Error::custom("duplicate JSON object key"));
                }
                values.insert(key, map.next_value_seed(JsonSeed(self.0 + 1))?);
            }
            Ok(ExactJson::Object(values))
        }
    }
    let mut d = serde_json::Deserializer::from_str(raw.get());
    let value = serde::Deserializer::deserialize_any(&mut d, JsonVisitor(depth))?;
    d.end()?;
    Ok(value)
}

impl<'de> Deserialize<'de> for ExactJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        JsonSeed(0).deserialize(d)
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
pub(crate) struct NumberObject(
    #[serde(deserialize_with = "crate::record::object")] pub NumberAcceptance,
);

pub(crate) fn admit(acceptance: &Acceptance) -> Result<()> {
    match acceptance {
        Acceptance::FinalNumber { number } => {
            if !number.expected.is_finite()
                || !number.absolute_tolerance.is_finite()
                || !number.relative_tolerance.is_finite()
                || number.absolute_tolerance < 0.0
                || number.relative_tolerance < 0.0
                || !(number.relative_tolerance * number.expected.abs()).is_finite()
            {
                return Err("number acceptance requires finite expected value and nonnegative finite tolerances/bound".into());
            }
        }
        Acceptance::TextConstraints { rules } => {
            for rule in &rules.0 {
                match rule {
                    TextRule::Words { min, max } | TextRule::Characters { min, max } => {
                        if min > max {
                            return Err("text rule requires min <= max".into());
                        }
                    }
                    TextRule::StartsWith { text }
                    | TextRule::EndsWith { text }
                    | TextRule::Contains { text }
                    | TextRule::Excludes { text } => {
                        crate::pack::text(text, ARTIFACT_CAP, false, "text rule literal")?;
                    }
                    TextRule::Lowercase | TextRule::Uppercase => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// Compare word iterators directly: Unicode scalar lowercasing, no allocation,
// punctuation removal, locale rules, full casefold, or suffix extraction.
fn same_text(left: &str, right: &str) -> bool {
    let mut left = left.split_whitespace();
    let mut right = right.split_whitespace();
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(a), Some(b))
                if a.chars()
                    .flat_map(char::to_lowercase)
                    .eq(b.chars().flat_map(char::to_lowercase)) => {}
            _ => return false,
        }
    }
}

pub(super) fn texts(artifact: &[u8], accepted: &[String]) -> Outcome {
    match crate::record::parse::<super::Answer>(artifact) {
        Ok(answer) if accepted.iter().any(|a| same_text(a, &answer.answer)) => Outcome::Success,
        Ok(_) => Outcome::Wrong,
        Err(_) => Outcome::Malformed,
    }
}

pub(super) fn number(artifact: &[u8], expected: &NumberAcceptance) -> Outcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Answer {
        answer: f64,
    }
    match crate::record::parse::<Answer>(artifact) {
        Ok(answer) if answer.answer.is_finite() => {
            let bound = expected
                .absolute_tolerance
                .max(expected.relative_tolerance * expected.expected.abs());
            outcome((answer.answer - expected.expected).abs() <= bound)
        }
        _ => Outcome::Malformed,
    }
}

pub(super) fn json(artifact: &[u8], expected: &ExactJson) -> Outcome {
    match serde_json::from_slice::<ExactJson>(artifact) {
        Ok(answer) => outcome(answer == *expected),
        Err(_) => Outcome::Malformed,
    }
}

fn outcome(success: bool) -> Outcome {
    if success {
        Outcome::Success
    } else {
        Outcome::Wrong
    }
}

pub(super) fn constraints(artifact: &[u8], rules: &[TextRule]) -> Outcome {
    let Ok(text) = std::str::from_utf8(artifact) else {
        return Outcome::Malformed;
    };
    outcome(rules.iter().all(|rule| match rule {
        TextRule::Words { min, max } => {
            let n = text.split_whitespace().count();
            (*min as usize..=*max as usize).contains(&n)
        }
        TextRule::Characters { min, max } => {
            let n = text.chars().count();
            (*min as usize..=*max as usize).contains(&n)
        }
        TextRule::StartsWith { text: prefix } => text.starts_with(prefix),
        TextRule::EndsWith { text: suffix } => text.ends_with(suffix),
        TextRule::Contains { text: needle } => text.contains(needle),
        TextRule::Excludes { text: needle } => !text.contains(needle),
        TextRule::Lowercase => {
            text.chars().any(char::is_lowercase) && !text.chars().any(char::is_uppercase)
        }
        TextRule::Uppercase => {
            text.chars().any(char::is_uppercase) && !text.chars().any(char::is_lowercase)
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acceptance(wire: &str) -> Acceptance {
        crate::record::parse(wire.as_bytes()).unwrap()
    }

    #[test]
    fn adapted_text_preserves_punctuation_and_unicode_lowercase_not_casefold() {
        let a = acceptance(r#"{"kind":"final-text-set","accepted":["ÉLAN vital!","straße"]}"#);
        assert_eq!(
            super::super::grade(
                "reason\n===FINAL===\n{\"answer\":\"  élan\\tVITAL!  \"}".as_bytes(),
                &a
            ),
            Outcome::Success
        );
        for answer in ["élan vital", "élan vital! suffix", "STRASSE"] {
            let artifact = format!("===FINAL===\n{}", serde_json::json!({"answer": answer}));
            assert_eq!(super::super::grade(artifact.as_bytes(), &a), Outcome::Wrong);
        }
        for artifact in [
            "===FINAL===\n{\"answer\":\"straße\",\"answer\":\"straße\"}",
            "===FINAL===\n{\"answer\":\"straße\",\"extra\":0}",
            "===FINAL===\n{\"answer\":1}",
            "===FINAL===\n===FINAL===\n{\"answer\":\"straße\"}",
            "{\"answer\":\"straße\"}",
        ] {
            assert_eq!(
                super::super::grade(artifact.as_bytes(), &a),
                Outcome::Malformed
            );
        }
    }

    #[test]
    fn numeric_tolerance_zero_boundary_relative_and_overflow() {
        let exact = acceptance(
            r#"{"kind":"final-number","number":{"expected":0,"absolute_tolerance":0,"relative_tolerance":1e-9}}"#,
        );
        let absolute = acceptance(
            r#"{"kind":"final-number","number":{"expected":2,"absolute_tolerance":0.25,"relative_tolerance":0}}"#,
        );
        let relative = acceptance(
            r#"{"kind":"final-number","number":{"expected":-100,"absolute_tolerance":0,"relative_tolerance":0.01}}"#,
        );
        for (a, answer, expected) in [
            (&exact, "0", Outcome::Success),
            (&exact, "1e-10", Outcome::Wrong),
            (&absolute, "2.25", Outcome::Success),
            (&absolute, "2.2501", Outcome::Wrong),
            (&relative, "-99", Outcome::Success),
            (&relative, "-98.999", Outcome::Wrong),
            (&exact, "\"0\"", Outcome::Malformed),
            (&exact, "true", Outcome::Malformed),
            (&exact, "1/2", Outcome::Malformed),
            (&exact, "1e999", Outcome::Malformed),
        ] {
            let artifact = format!("===FINAL===\n{{\"answer\":{answer}}}");
            assert_eq!(super::super::grade(artifact.as_bytes(), a), expected);
        }
        let overflow = acceptance(
            r#"{"kind":"final-number","number":{"expected":1e308,"absolute_tolerance":0,"relative_tolerance":2}}"#,
        );
        assert!(admit(&overflow).is_err());
        let negative = acceptance(
            r#"{"kind":"final-number","number":{"expected":0,"absolute_tolerance":-1,"relative_tolerance":0}}"#,
        );
        assert!(admit(&negative).is_err());
    }

    #[test]
    fn exact_json_retains_large_numbers_duplicates_and_whole_value_boundary() {
        let a = acceptance(
            r#"{"kind":"json-exact","expected":{"n":18446744073709551616001,"a":[true,null,"X"]}}"#,
        );
        assert_eq!(
            super::super::grade(
                br#" { "a":[true,null,"\u0058"],"n":18446744073709551616001 } "#,
                &a
            ),
            Outcome::Success
        );
        for artifact in [
            r#"{"n":18446744073709551616000,"a":[true,null,"X"]}"#,
            r#"{"n":18446744073709551616001,"a":[1,null,"X"]}"#,
            r#"{"n":18446744073709551616001,"a":[true,"X",null]}"#,
            r#"{"n":18446744073709551616001,"a":[true,null,"x"]}"#,
        ] {
            assert_eq!(super::super::grade(artifact.as_bytes(), &a), Outcome::Wrong);
        }
        for artifact in [
            r#"{"n":18446744073709551616001,"a":[true,null,"X"],"\u006e":0}"#,
            r#"{"n":0,"a":[{"x":null,"x":1}]}"#,
            "```json\n{}\n```",
            "{} trailing",
            "{} {}",
            "===FINAL===\n{}",
        ] {
            assert_eq!(
                super::super::grade(artifact.as_bytes(), &a),
                Outcome::Malformed
            );
        }
        for wire in [
            r#"{"kind":"json-exact","expected":{"a":null,"a":1}}"#,
            r#"{"kind":"json-exact","expected":null,"expected":null}"#,
        ] {
            assert!(crate::record::parse::<Acceptance>(wire.as_bytes()).is_err());
        }
        let number = acceptance(r#"{"kind":"json-exact","expected":1}"#);
        assert_eq!(super::super::grade(b"1.0", &number), Outcome::Success);
        assert_eq!(super::super::grade(b"1", &number), Outcome::Success);
        let null = acceptance(r#"{"kind":"json-exact","expected":null}"#);
        assert_eq!(super::super::grade(b"null", &null), Outcome::Success);
        let deep = format!(
            "{}0{}",
            "[".repeat(JSON_DEPTH + 2),
            "]".repeat(JSON_DEPTH + 2)
        );
        assert_eq!(
            super::super::grade(deep.as_bytes(), &null),
            Outcome::Malformed
        );
        assert_eq!(
            super::super::grade(&vec![b' '; ARTIFACT_CAP + 1], &null),
            Outcome::Malformed
        );
    }

    #[test]
    fn raw_constraints_count_scalars_words_and_require_cased_letters() {
        let a = acceptance(
            r#"{"kind":"text-constraints","rules":[{"kind":"words","min":2,"max":2},{"kind":"characters","min":5,"max":5},{"kind":"starts-with","text":"é"},{"kind":"ends-with","text":"a"},{"kind":"contains","text":"!"},{"kind":"excludes","text":"?"},{"kind":"lowercase"}]}"#,
        );
        assert_eq!(
            super::super::grade("é!  a".as_bytes(), &a),
            Outcome::Success
        );
        for text in ["é!  A", "é! a", "é!  a\n", "é?  a"] {
            assert_eq!(super::super::grade(text.as_bytes(), &a), Outcome::Wrong);
        }
        let upper = acceptance(r#"{"kind":"text-constraints","rules":[{"kind":"uppercase"}]}"#);
        assert_eq!(
            super::super::grade("É 123!".as_bytes(), &upper),
            Outcome::Success
        );
        assert_eq!(super::super::grade(b"123!", &upper), Outcome::Wrong);
        let empty_literal =
            acceptance(r#"{"kind":"text-constraints","rules":[{"kind":"contains","text":""}]}"#);
        assert!(admit(&empty_literal).is_err());
        assert_eq!(super::super::grade(&[0xff], &upper), Outcome::Malformed);
        for wire in [
            r#"{"kind":"text-constraints","rules":[]}"#,
            r#"{"kind":"text-constraints","rules":[{"kind":"words","min":1,"min":1,"max":2}]}"#,
            r#"{"kind":"text-constraints","rules":[{"kind":"lowercase","text":null}]}"#,
            r#"{"kind":"text-constraints","rules":[{"kind":"contains","text":"x","extra":0}]}"#,
            r#"{"kind":"text-constraints","rules":[["lowercase"]]}"#,
            r#"{"kind":"final-number","number":[0,0,0]}"#,
        ] {
            assert!(
                crate::record::parse::<Acceptance>(wire.as_bytes()).is_err(),
                "{wire}"
            );
        }
        let too_many = format!(
            "{{\"kind\":\"text-constraints\",\"rules\":[{}]}}",
            vec![r#"{"kind":"lowercase"}"#; 17].join(",")
        );
        assert!(crate::record::parse::<Acceptance>(too_many.as_bytes()).is_err());
    }
}

#[cfg(test)]
mod number_tests {
    use super::*;

    fn acceptance(number: &str) -> Acceptance {
        crate::record::parse(format!(r#"{{"kind":"json-exact","expected":{number}}}"#).as_bytes())
            .unwrap()
    }

    #[test]
    fn decimal_equality_normalizes_scale_sign_and_precision_without_changing_serialization() {
        for (left, right) in [
            ("1", "1.0"),
            ("1", "1e0"),
            ("1", "100.000E-0002"),
            ("0", "-0.000e+42"),
            ("0e-9223372036854775808", "-0e9223372036854775807"),
            ("-0.00120", "-12e-4"),
            ("1000.000", "1e3"),
            ("18446744073709551616001", "1844674407370955161600100e-2"),
            ("1e9223372036854775807", "1.0e9223372036854775807"),
            ("1e-9223372036854775808", "1.0e-9223372036854775808"),
            ("10e-9223372036854775808", "1e-9223372036854775807"),
        ] {
            let expected = acceptance(left);
            assert_eq!(
                super::super::grade(right.as_bytes(), &expected),
                Outcome::Success,
                "{left} vs {right}"
            );
            assert_eq!(
                super::super::grade(left.as_bytes(), &acceptance(right)),
                Outcome::Success,
                "{right} vs {left}"
            );
            let parsed: ExactJson = serde_json::from_str(right).unwrap();
            assert_eq!(serde_json::to_string(&parsed).unwrap(), right);
        }
        for (left, right) in [
            ("1", "-1"),
            ("0", "1e-9223372036854775808"),
            ("9007199254740992", "9007199254740993"),
            ("18446744073709551616001", "18446744073709551616000"),
            ("1", "1.0000000000000001"),
            ("1e9223372036854775807", "1e9223372036854775806"),
        ] {
            assert_eq!(
                super::super::grade(right.as_bytes(), &acceptance(left)),
                Outcome::Wrong,
                "{left} vs {right}"
            );
        }
    }

    #[test]
    fn exponent_overflow_is_malformed_for_answers_and_rejected_for_gold() {
        let expected = acceptance("0");
        for number in [
            "1e9223372036854775808",
            "1e-9223372036854775809",
            "0e9223372036854775808",
            "0.1e9223372036854775808",
            "10e9223372036854775807",
            "0.1e-9223372036854775808",
            "1e99999999999999999999999999999999999999999",
        ] {
            assert_eq!(
                super::super::grade(number.as_bytes(), &expected),
                Outcome::Malformed,
                "{number}"
            );
            let wire = format!(r#"{{"kind":"json-exact","expected":{number}}}"#);
            assert!(
                crate::record::parse::<Acceptance>(wire.as_bytes()).is_err(),
                "{number}"
            );
        }
    }
}
