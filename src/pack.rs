use crate::contract::*;
use crate::grade;
use std::collections::HashSet;

// Select the archival reader without buffering case values into untyped maps.
// The complete second pass rejects all unknown/duplicate fields and wrong types.
#[derive(serde::Deserialize)]
struct Version {
    version: u32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePack<C> {
    version: u32,
    label: String,
    worlds: Vec<String>,
    groups: Vec<String>,
    #[serde(
        deserialize_with = "crate::record::objects",
        bound(deserialize = "C: serde::Deserialize<'de>")
    )]
    cases: Vec<C>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchivalCaseV1 {
    id: String,
    world: String,
    group: String,
    #[serde(deserialize_with = "crate::record::objects")]
    messages: Vec<Message>,
    accepted: Vec<String>,
    #[serde(deserialize_with = "crate::record::object")]
    qualification: Qualification,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseV2 {
    id: String,
    world: String,
    group: String,
    #[serde(deserialize_with = "crate::record::objects")]
    messages: Vec<Message>,
    acceptance: Acceptance,
    #[serde(deserialize_with = "crate::record::object")]
    qualification: Qualification,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseV4 {
    id: String,
    world: String,
    group: String,
    #[serde(deserialize_with = "crate::record::objects")]
    messages: Vec<Message>,
    acceptance: Acceptance,
    #[serde(deserialize_with = "crate::record::object")]
    qualification: Qualification,
    #[serde(deserialize_with = "crate::record::object")]
    provenance: CaseProvenance,
}

impl From<CaseV4> for Case {
    fn from(c: CaseV4) -> Self {
        Self {
            id: c.id,
            world: c.world,
            group: c.group,
            messages: c.messages,
            acceptance: c.acceptance,
            qualification: c.qualification,
            provenance: Some(c.provenance),
        }
    }
}

impl From<ArchivalCaseV1> for Case {
    fn from(c: ArchivalCaseV1) -> Self {
        Self {
            id: c.id,
            world: c.world,
            group: c.group,
            messages: c.messages,
            acceptance: Acceptance::ArchivalV1 {
                accepted: c.accepted,
            },
            qualification: c.qualification,
            provenance: None,
        }
    }
}

impl From<CaseV2> for Case {
    fn from(c: CaseV2) -> Self {
        Self {
            id: c.id,
            world: c.world,
            group: c.group,
            messages: c.messages,
            acceptance: c.acceptance,
            qualification: c.qualification,
            provenance: None,
        }
    }
}

impl<C: Into<Case>> From<WirePack<C>> for Pack {
    fn from(p: WirePack<C>) -> Self {
        Self {
            version: p.version,
            label: p.label,
            worlds: p.worlds,
            groups: p.groups,
            cases: p.cases.into_iter().map(Into::into).collect(),
        }
    }
}

pub(crate) fn text(value: &str, cap: usize, empty: bool, what: &str) -> Result<()> {
    if (!empty && value.is_empty()) || value.len() > cap {
        return Err(format!("{what}: invalid text length (limit {cap} bytes)"));
    }
    Ok(())
}

fn unique<'a>(items: impl Iterator<Item = &'a str>, what: &str) -> Result<HashSet<&'a str>> {
    let mut seen = HashSet::new();
    for item in items {
        text(item, 256, false, what)?;
        if !seen.insert(item) {
            return Err(format!("duplicate {what}: {item}"));
        }
        if seen.len() > CASE_CAP {
            return Err(format!("too many {what}"));
        }
    }
    Ok(seen)
}

fn provenance(source: &CaseProvenance) -> Result<()> {
    fn meaningful(value: &str, cap: usize, what: &str) -> Result<()> {
        text(value, cap, false, what)?;
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(format!("{what}: requires meaningful text without controls"));
        }
        Ok(())
    }
    meaningful(&source.repository, 4096, "provenance repository")?;
    let url = reqwest::Url::parse(&source.repository)
        .map_err(|_| "provenance repository requires an HTTPS URL")?;
    if !source.repository.starts_with("https://")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || source.repository.chars().any(char::is_whitespace)
    {
        return Err(
            "provenance repository requires an HTTPS URL without credentials/query/fragment".into(),
        );
    }
    for (value, length, what) in [
        (&source.revision, 40, "revision"),
        (&source.sha256, 64, "sha256"),
    ] {
        if value.len() != length
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!(
                "provenance {what} requires {length} lowercase hex digits"
            ));
        }
    }
    meaningful(&source.path, 4096, "provenance path")?;
    meaningful(&source.item_id, 256, "provenance item_id")?;
    meaningful(&source.license, 1024, "provenance license")?;
    for change in &source.changes.0 {
        meaningful(change, 4096, "provenance change")?;
    }
    Ok(())
}

pub(crate) fn admit(bytes: &[u8]) -> Result<Pack> {
    if bytes.len() > PACK_CAP {
        return Err("pack exceeds byte limit".into());
    }
    let version: Version = crate::record::parse(bytes).map_err(|e| format!("pack JSON: {e}"))?;
    let pack: Pack = match version.version {
        1 => crate::record::parse::<WirePack<ArchivalCaseV1>>(bytes).map(Into::into),
        2 | 3 => crate::record::parse::<WirePack<CaseV2>>(bytes).map(Into::into),
        4 => crate::record::parse::<WirePack<CaseV4>>(bytes).map(Into::into),
        _ => return Err("unsupported pack version".into()),
    }
    .map_err(|e| format!("pack JSON: {e}"))?;
    // Archived readers retain their original acceptance sets.
    if pack.version == 2
        && pack.cases.iter().any(|c| {
            matches!(
                c.acceptance,
                Acceptance::FinalStringSet { .. } | Acceptance::FinalGridSet { .. }
            )
        })
    {
        return Err("final-marker acceptance requires pack version 3".into());
    }
    if pack.version < 4
        && pack.cases.iter().any(|c| {
            matches!(
                c.acceptance,
                Acceptance::FinalTextSet { .. }
                    | Acceptance::FinalNumber { .. }
                    | Acceptance::JsonExact { .. }
                    | Acceptance::TextConstraints { .. }
            )
        })
    {
        return Err("adapted acceptance requires pack version 4".into());
    }
    text(&pack.label, 256, false, "pack label")?;
    if pack.cases.is_empty() || pack.cases.len() > CASE_CAP {
        return Err("pack requires 1..=1024 cases".into());
    }
    let worlds = unique(pack.worlds.iter().map(String::as_str), "world ID")?;
    let groups = unique(pack.groups.iter().map(String::as_str), "group ID")?;
    unique(pack.cases.iter().map(|c| c.id.as_str()), "case ID")?;
    let mut used_worlds = HashSet::new();
    let mut used_groups = HashSet::new();
    for case in &pack.cases {
        if let Some(source) = &case.provenance {
            provenance(source)?;
        }
        grade::atlas::admit(&case.acceptance)?;
        if !worlds.contains(case.world.as_str()) || !groups.contains(case.group.as_str()) {
            return Err(format!("case {} has undeclared world/group", case.id));
        }
        used_worlds.insert(case.world.as_str());
        used_groups.insert(case.group.as_str());
        if case.messages.is_empty() || case.messages.len() > 64 {
            return Err(format!("case {} requires 1..=64 messages", case.id));
        }
        let mut message_bytes = 0usize;
        for message in &case.messages {
            text(&message.content, MESSAGE_CAP, true, "message")?;
            message_bytes = message_bytes
                .checked_add(message.content.len())
                .ok_or("message length overflow")?;
        }
        if message_bytes > MESSAGE_CAP {
            return Err("case messages exceed 256 KiB".into());
        }
        let accepted = match &case.acceptance {
            Acceptance::ArchivalV1 { accepted }
            | Acceptance::StringSet { accepted }
            | Acceptance::FinalStringSet { accepted }
            | Acceptance::FinalTextSet { accepted } => Some(accepted),
            Acceptance::GridSet { .. }
            | Acceptance::FinalGridSet { .. }
            | Acceptance::FinalNumber { .. }
            | Acceptance::JsonExact { .. }
            | Acceptance::TextConstraints { .. } => None,
        };
        if let Some(accepted) = accepted {
            if accepted.is_empty() || accepted.len() > 64 {
                return Err("accepted set requires 1..=64 alternatives".into());
            }
            let mut alternatives = HashSet::new();
            for answer in accepted {
                text(answer, ARTIFACT_CAP, true, "accepted answer")?;
                if !alternatives.insert(answer) {
                    return Err("duplicate accepted answer".into());
                }
            }
        }
        let q = &case.qualification;
        if q.valid.is_empty() || q.valid.len() > 128 || q.wrong.is_empty() || q.wrong.len() > 128 {
            return Err("qualification requires 1..=128 valid and wrong artifacts".into());
        }
        for artifact in q.valid.iter().chain(&q.wrong) {
            text(artifact, ARTIFACT_CAP, true, "qualification artifact")?;
        }
        let mut qualified = HashSet::new();
        for artifact in &q.valid {
            if grade::grade(artifact.as_bytes(), &case.acceptance) != Outcome::Success {
                return Err(format!("valid qualification failed for case {}", case.id));
            }
            if accepted.is_some() {
                qualified.insert(
                    grade::decode(artifact.as_bytes(), &case.acceptance)
                        .expect("successful string grade"),
                );
            }
        }
        if q.wrong
            .iter()
            .any(|a| grade::grade(a.as_bytes(), &case.acceptance) == Outcome::Success)
        {
            return Err(format!("wrong qualification failed for case {}", case.id));
        }
        if accepted.is_some_and(|accepted| accepted.iter().any(|a| !qualified.contains(a))) {
            return Err(format!(
                "unqualified accepted alternative in case {}",
                case.id
            ));
        }
    }
    if used_worlds.len() != worlds.len() || used_groups.len() != groups.len() {
        return Err("unused world/group declaration".into());
    }
    Ok(pack)
}

pub(crate) fn system(system: &System) -> Result<()> {
    text(&system.name, 256, false, "system name")?;
    text(&system.model, 4096, false, "model declaration")?;
    if let Some(endpoint) = &system.endpoint {
        text(endpoint, 4096, false, "endpoint declaration")?;
    }
    Ok(())
}

pub(crate) fn protocol(protocol: &Protocol) -> Result<()> {
    // The request controls are bound to the declared profile: v1 records carry
    // neither field; v2 records declare include_usage explicitly and may only
    // request streaming usage in the streaming mode.
    match protocol.profile {
        Profile::DeclaredChatCompletionsV1 => {
            if protocol.reasoning_effort.is_some() || protocol.include_usage.is_some() {
                return Err(
                    "profile declared-chat-completions-v1 carries no reasoning_effort or include_usage".into(),
                );
            }
        }
        Profile::DeclaredChatCompletionsV2 => match protocol.include_usage {
            None => {
                return Err(
                    "profile declared-chat-completions-v2 requires an explicit include_usage declaration".into(),
                );
            }
            Some(true) if !protocol.stream => {
                return Err("include_usage requires the streaming mode".into());
            }
            _ => {}
        },
    }
    if !(1..=1_048_576).contains(&protocol.token_cap.value)
        || protocol.temperature_milli.is_some_and(|t| t > 2000)
        || protocol.top_p_milli.is_some_and(|p| p == 0 || p > 1000)
    {
        return Err("unsupported token cap or sampling range".into());
    }
    let c = &protocol.collection;
    if c.total_ms == 0
        || c.total_ms > 86_400_000
        || c.idle_ms == 0
        || c.idle_ms > c.total_ms
        || c.response_bytes == 0
        || c.response_bytes as usize > RESPONSE_CAP
        || c.artifact_bytes == 0
        || c.artifact_bytes as usize > ARTIFACT_CAP
    {
        return Err("invalid collection protection limits".into());
    }
    let rendering = &protocol.rendering;
    match (&rendering.status, &rendering.template, &rendering.tokenizer) {
        (RenderingStatus::Unknown, None, None) => {}
        (RenderingStatus::Known, Some(template), Some(tokenizer)) => {
            text(template, 4096, false, "rendering template declaration")?;
            text(tokenizer, 4096, false, "rendering tokenizer declaration")?;
        }
        _ => {
            return Err(
                "known rendering requires template/tokenizer; unknown requires null/omitted".into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn submission(bytes: &[u8], pack: &Pack) -> Result<Submission> {
    if bytes.len() > SUBMISSION_CAP {
        return Err("submission exceeds byte limit".into());
    }
    let submission: Submission =
        crate::record::parse(bytes).map_err(|e| format!("submission JSON: {e}"))?;
    if submission.version != 1 {
        return Err("unsupported submission version".into());
    }
    system(&submission.system)?;
    protocol(&submission.protocol)?;
    let known: HashSet<_> = pack.cases.iter().map(|c| c.id.as_str()).collect();
    unique(
        submission.answers.iter().map(|a| a.case_id.as_str()),
        "submission case ID",
    )?;
    for answer in &submission.answers {
        if !known.contains(answer.case_id.as_str()) {
            return Err(format!("unknown case ID: {}", answer.case_id));
        }
        if let Some(artifact) = &answer.artifact {
            text(artifact, ARTIFACT_CAP, true, "submitted artifact")?;
        }
    }
    Ok(submission)
}

#[cfg(test)]
mod tests {
    use super::*;
    const PACK: &[u8] = include_bytes!("../examples/synthetic-pack.json");
    const SUB: &[u8] = include_bytes!("../examples/submission-a.json");

    #[test]
    fn closed_records_and_qualification() {
        let pack = admit(PACK).unwrap();
        // These are otherwise-valid serde positional encodings, not merely
        // incomplete arrays which would already fail required-field checks.
        let positional = serde_json::to_vec(&(
            pack.version,
            &pack.label,
            &pack.worlds,
            &pack.groups,
            &pack.cases,
        ))
        .unwrap();
        assert!(admit(&positional).is_err());
        let message = &pack.cases[0].messages[0];
        let positional_message = serde_json::to_string(&(&message.role, &message.content)).unwrap();
        let nested = serde_json::to_string(&pack).unwrap().replacen(
            &serde_json::to_string(message).unwrap(),
            &positional_message,
            1,
        );
        assert!(admit(nested.as_bytes()).is_err());
        let enum_object = serde_json::to_string(&pack).unwrap().replacen(
            "\"role\":\"user\"",
            "\"role\":{\"user\":null}",
            1,
        );
        assert!(admit(enum_object.as_bytes()).is_err());
        for bad in [
            "{\"version\":1,\"version\":1}",
            "{} {}",
            "{\"version\":4294967296}",
        ] {
            assert!(admit(bad.as_bytes()).is_err());
        }
        let mut changed = admit(PACK).unwrap();
        let valid = changed.cases[0].qualification.valid[0].clone();
        changed.cases[0].qualification.wrong.push(valid);
        assert!(admit(&serde_json::to_vec(&changed).unwrap()).is_err());
        let s = std::str::from_utf8(SUB).unwrap();
        let duplicate = s.replacen(
            "\"endpoint\": null",
            "\"endpoint\": null, \"endpoint\": null",
            1,
        );
        assert!(submission(duplicate.as_bytes(), &pack).is_err());
        let mut sub = submission(SUB, &pack).unwrap();
        sub.answers.push(AnswerEntry {
            case_id: sub.answers[0].case_id.clone(),
            artifact: None,
        });
        assert!(submission(&serde_json::to_vec(&sub).unwrap(), &pack).is_err());
        sub.answers.pop();
        sub.answers[0].case_id = "not-admitted".into();
        assert!(submission(&serde_json::to_vec(&sub).unwrap(), &pack).is_err());
    }

    #[test]
    fn pack_versions_do_not_mix_archival_and_typed_cases() {
        let mut wire: serde_json::Value = serde_json::from_slice(PACK).unwrap();
        wire["version"] = 2.into();
        assert!(admit(&serde_json::to_vec(&wire).unwrap()).is_err());
        for case in wire["cases"].as_array_mut().unwrap() {
            let accepted = case.as_object_mut().unwrap().remove("accepted").unwrap();
            case["acceptance"] = serde_json::json!({"kind": "string-set", "accepted": accepted});
        }
        let typed = serde_json::to_vec(&wire).unwrap();
        let current = admit(&typed).unwrap();
        let legacy = admit(PACK).unwrap();
        let old = crate::identity::pack(&legacy, PACK).unwrap().0;
        let new = crate::identity::pack(&current, &typed).unwrap().0;
        assert_ne!(old.tasks, new.tasks);
        assert_ne!(old.grading, new.grading);
        wire["version"] = 1.into();
        assert!(admit(&serde_json::to_vec(&wire).unwrap()).is_err());
        wire["version"] = 2.into();
        wire["cases"][0]["acceptance"] =
            serde_json::json!({"kind": "grid-set", "outputs": [[[0]]]});
        assert!(
            admit(&serde_json::to_vec(&wire).unwrap()).is_err(),
            "string qualification cannot qualify a grid"
        );
    }

    #[test]
    fn final_marker_kinds_require_version_3_and_bind_identity() {
        let mut wire: serde_json::Value = serde_json::from_slice(PACK).unwrap();
        wire["version"] = 2.into();
        for case in wire["cases"].as_array_mut().unwrap() {
            let accepted = case.as_object_mut().unwrap().remove("accepted").unwrap();
            case["acceptance"] = serde_json::json!({"kind": "string-set", "accepted": accepted});
        }
        let plain_bytes = serde_json::to_vec(&wire).unwrap();
        let mut marked = wire.clone();
        for case in marked["cases"].as_array_mut().unwrap() {
            case["acceptance"]["kind"] = "final-string-set".into();
        }
        // The frozen version 2 refuses the new kinds; version 3 with unmarked
        // qualification artifacts fails its own self-check.
        assert!(admit(&serde_json::to_vec(&marked).unwrap()).is_err());
        marked["version"] = 3.into();
        assert!(admit(&serde_json::to_vec(&marked).unwrap()).is_err());
        for case in marked["cases"].as_array_mut().unwrap() {
            for list in ["valid", "wrong"] {
                let artifacts: Vec<String> = case["qualification"][list]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| format!("preamble\n===FINAL===\n{}", a.as_str().unwrap()))
                    .collect();
                case["qualification"][list] = artifacts.into();
            }
        }
        let marked_bytes = serde_json::to_vec(&marked).unwrap();
        let admitted = admit(&marked_bytes).unwrap();
        // Same golds, distinct envelope: task, grading and target identities all
        // move; the plain v2 reading stays admissible and unchanged.
        let plain = admit(&plain_bytes).unwrap();
        let old = crate::identity::pack(&plain, &plain_bytes).unwrap().0;
        let new = crate::identity::pack(&admitted, &marked_bytes).unwrap().0;
        assert_ne!(old.tasks, new.tasks);
        assert_ne!(old.grading, new.grading);
        assert_ne!(old.target, new.target);
    }

    fn v4_case(acceptance: serde_json::Value, valid: &[&str], wrong: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "version":4, "label":"synthetic v4", "worlds":["w"], "groups":["g"],
            "cases":[{
                "id":"c", "world":"w", "group":"g",
                "messages":[{"role":"user","content":"Synthetic task; follow the declared output contract."}],
                "acceptance":acceptance,
                "qualification":{"valid":valid,"wrong":wrong},
                "provenance":{
                    "repository":"https://example.org/tasks",
                    "revision":"a".repeat(40), "path":"fixture.json",
                    "sha256":"b".repeat(64), "item_id":"synthetic",
                    "license":"CC0-1.0", "changes":["Synthetic regression fixture"]
                }
            }]
        })
    }

    #[test]
    fn v4_kinds_require_provenance_and_reject_archival_versions() {
        for (acceptance, valid, wrong) in [
            (
                serde_json::json!({"kind":"final-text-set","accepted":["Alpha"]}),
                "===FINAL===\n{\"answer\":\"Alpha\"}",
                "===FINAL===\n{\"answer\":\"Beta\"}",
            ),
            (
                serde_json::json!({"kind":"final-number","number":{"expected":1,"absolute_tolerance":0,"relative_tolerance":0}}),
                "===FINAL===\n{\"answer\":1}",
                "===FINAL===\n{\"answer\":2}",
            ),
            (
                serde_json::json!({"kind":"json-exact","expected":{"x":[1,true]}}),
                "{\"x\":[1,true]}",
                "{\"x\":[true,1]}",
            ),
            (
                serde_json::json!({"kind":"text-constraints","rules":[{"kind":"words","min":2,"max":2}]}),
                "two words",
                "one",
            ),
        ] {
            let wire = v4_case(acceptance, &[valid], &[wrong]);
            let pack = admit(&serde_json::to_vec(&wire).unwrap()).unwrap();
            let serialized = serde_json::to_vec(&pack).unwrap();
            let roundtrip = admit(&serialized).unwrap();
            assert_eq!(
                crate::identity::input(&pack.cases[0]).unwrap(),
                crate::identity::input(&roundtrip.cases[0]).unwrap()
            );
            let mut missing = wire.clone();
            missing["cases"][0]
                .as_object_mut()
                .unwrap()
                .remove("provenance");
            assert!(admit(&serde_json::to_vec(&missing).unwrap()).is_err());
            for version in 1..=3 {
                let mut old = wire.clone();
                old["version"] = version.into();
                assert!(admit(&serde_json::to_vec(&old).unwrap()).is_err());
                old["cases"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("provenance");
                assert!(admit(&serde_json::to_vec(&old).unwrap()).is_err());
            }
        }
    }

    #[test]
    fn provenance_is_closed_validated_and_bound_even_for_older_acceptance() {
        let wire = v4_case(
            serde_json::json!({"kind":"string-set","accepted":["ok"]}),
            &["{\"answer\":\"ok\"}"],
            &["{\"answer\":\"no\"}"],
        );
        let admitted = admit(&serde_json::to_vec(&wire).unwrap()).unwrap();
        let before = crate::identity::input(&admitted.cases[0]).unwrap();
        for (field, valid, invalid) in [
            (
                "repository",
                serde_json::json!("https://example.org/other"),
                serde_json::json!("http://example.org"),
            ),
            (
                "revision",
                serde_json::json!("c".repeat(40)),
                serde_json::json!("A".repeat(40)),
            ),
            (
                "path",
                serde_json::json!("other.json"),
                serde_json::json!(" "),
            ),
            (
                "sha256",
                serde_json::json!("d".repeat(64)),
                serde_json::json!("f".repeat(63)),
            ),
            (
                "item_id",
                serde_json::json!("other"),
                serde_json::json!("x".repeat(257)),
            ),
            ("license", serde_json::json!("MIT"), serde_json::json!(null)),
            (
                "changes",
                serde_json::json!(["Different adaptation"]),
                serde_json::json!(["\n"]),
            ),
        ] {
            let mut changed = wire.clone();
            changed["cases"][0]["provenance"][field] = valid;
            let other = admit(&serde_json::to_vec(&changed).unwrap()).unwrap();
            assert_ne!(
                before,
                crate::identity::input(&other.cases[0]).unwrap(),
                "{field}"
            );
            changed["cases"][0]["provenance"][field] = invalid;
            assert!(
                admit(&serde_json::to_vec(&changed).unwrap()).is_err(),
                "{field}"
            );
        }
        let raw = serde_json::to_string(&wire).unwrap();
        for changed in [
            raw.replace(
                "\"item_id\":\"synthetic\"",
                "\"item_id\":\"synthetic\",\"item_id\":\"synthetic\"",
            ),
            raw.replace(
                "\"license\":\"CC0-1.0\"",
                "\"license\":\"CC0-1.0\",\"unknown\":0",
            ),
            raw.replace("\"provenance\":{", "\"provenance\":null,\"provenance\":{"),
        ] {
            assert!(admit(changed.as_bytes()).is_err());
        }
        for version in [2, 3] {
            let mut old = wire.clone();
            old["version"] = version.into();
            assert!(admit(&serde_json::to_vec(&old).unwrap()).is_err());
            old["cases"][0]
                .as_object_mut()
                .unwrap()
                .remove("provenance");
            let old = admit(&serde_json::to_vec(&old).unwrap()).unwrap();
            assert_ne!(before, crate::identity::input(&old.cases[0]).unwrap());
        }
    }

    #[test]
    fn normalized_aliases_still_require_each_listed_positive_fixture() {
        let mut wire = v4_case(
            serde_json::json!({"kind":"final-text-set","accepted":["A","a"]}),
            &["===FINAL===\n{\"answer\":\"A\"}"],
            &["===FINAL===\n{\"answer\":\"b\"}"],
        );
        assert!(admit(&serde_json::to_vec(&wire).unwrap()).is_err());
        wire["cases"][0]["qualification"]["valid"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("===FINAL===\n{\"answer\":\"a\"}"));
        assert!(admit(&serde_json::to_vec(&wire).unwrap()).is_ok());
    }

    #[test]
    fn profile_binds_request_controls() {
        let mut p = Protocol {
            profile: Profile::DeclaredChatCompletionsV1,
            stream: true,
            token_cap: TokenCap {
                field: TokenField::MaxCompletionTokens,
                value: 64,
            },
            temperature_milli: None,
            top_p_milli: None,
            seed: None,
            reasoning_effort: None,
            include_usage: None,
            collection: Collection {
                total_ms: 1000,
                idle_ms: 1000,
                response_bytes: 1024,
                artifact_bytes: 1024,
            },
            rendering: Rendering {
                status: RenderingStatus::Unknown,
                template: None,
                tokenizer: None,
            },
        };
        protocol(&p).unwrap();
        p.reasoning_effort = Some(Effort::High);
        assert!(protocol(&p).is_err(), "v1 rejects reasoning_effort");
        p.reasoning_effort = None;
        p.include_usage = Some(false);
        assert!(protocol(&p).is_err(), "v1 rejects include_usage");
        p.profile = Profile::DeclaredChatCompletionsV2;
        protocol(&p).unwrap();
        p.include_usage = None;
        assert!(protocol(&p).is_err(), "v2 requires explicit include_usage");
        p.include_usage = Some(true);
        p.reasoning_effort = Some(Effort::Xhigh);
        protocol(&p).unwrap();
        p.stream = false;
        assert!(protocol(&p).is_err(), "include_usage needs streaming");
        p.include_usage = Some(false);
        protocol(&p).unwrap();
    }

    #[test]
    fn present_controls_reject_null_spellings_on_the_wire() {
        let pack = admit(PACK).unwrap();
        let s = std::str::from_utf8(SUB).unwrap();
        assert!(s.contains("\"seed\": null"), "fixture anchor moved");
        // The archival v1 protocol refuses a present control in every spelling,
        // exactly like the frozen reader that has no such fields.
        for injected in [
            "\"seed\": null,\n    \"reasoning_effort\": null",
            "\"seed\": null,\n    \"include_usage\": null",
            "\"seed\": null,\n    \"reasoning_effort\": \"high\"",
            "\"seed\": null,\n    \"include_usage\": false",
        ] {
            let wire = s.replacen("\"seed\": null", injected, 1);
            assert!(submission(wire.as_bytes(), &pack).is_err(), "{injected}");
        }
        // Profile v2 declares real values; null and wrong-type spellings stay
        // rejected on the raw wire.
        let v2 = s.replacen(
            "declared-chat-completions-v1",
            "declared-chat-completions-v2",
            1,
        );
        let valid = v2.replacen(
            "\"seed\": null",
            "\"seed\": null,\n    \"reasoning_effort\": \"high\",\n    \"include_usage\": false",
            1,
        );
        assert!(submission(valid.as_bytes(), &pack).is_ok());
        for injected in [
            "\"seed\": null,\n    \"reasoning_effort\": null,\n    \"include_usage\": false",
            "\"seed\": null,\n    \"include_usage\": null",
            "\"seed\": null,\n    \"include_usage\": 1",
        ] {
            let wire = v2.replacen("\"seed\": null", injected, 1);
            assert!(submission(wire.as_bytes(), &pack).is_err(), "{injected}");
        }
    }
}
