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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseV5 {
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
    #[serde(deserialize_with = "crate::record::object")]
    tool: ToolTask,
    #[serde(deserialize_with = "crate::record::object")]
    call_qualification: Qualification,
}

impl From<CaseV5> for Case {
    fn from(c: CaseV5) -> Self {
        Self {
            id: c.id,
            world: c.world,
            group: c.group,
            messages: c.messages,
            acceptance: c.acceptance,
            qualification: c.qualification,
            provenance: Some(c.provenance),
            tool: Some(c.tool),
            call_qualification: Some(c.call_qualification),
        }
    }
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
            tool: None,
            call_qualification: None,
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
            tool: None,
            call_qualification: None,
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
            tool: None,
            call_qualification: None,
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

/// Shared bounds of the answer and call qualification lists; `what` names the
/// list in errors.
fn qualification_shape(q: &Qualification, what: &str) -> Result<()> {
    if q.valid.is_empty() || q.valid.len() > 128 || q.wrong.is_empty() || q.wrong.len() > 128 {
        return Err(format!("{what} requires 1..=128 valid and wrong artifacts"));
    }
    match q
        .valid
        .iter()
        .chain(&q.wrong)
        .find(|a| a.len() > ARTIFACT_CAP)
    {
        // The label is only built for the error.
        Some(artifact) => text(artifact, ARTIFACT_CAP, true, &format!("{what} artifact")),
        None => Ok(()),
    }
}

/// Version 5 tool round: declared functions, the expected call and its own
/// self-qualification through the call grader.
fn tool(case: &Case) -> Result<()> {
    let (Some(tool), Some(q)) = (&case.tool, &case.call_qualification) else {
        return Ok(());
    };
    let id = &case.id;
    if tool.functions.is_empty() || tool.functions.len() > 8 {
        return Err(format!("case {id}: tool requires 1..=8 functions"));
    }
    let mut names = HashSet::new();
    for function in &tool.functions {
        let name = function.name.as_str();
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || !names.insert(name)
        {
            return Err(format!(
                "case {id}: function names must be unique, 1..=64 bytes of [A-Za-z0-9_-]"
            ));
        }
        text(&function.description, 4096, false, "function description")?;
        if !matches!(function.parameters, grade::atlas::ExactJson::Object(_)) {
            return Err(format!(
                "case {id}: function parameters must be a JSON object"
            ));
        }
    }
    if !names.contains(tool.expected.name.as_str()) {
        return Err(format!(
            "case {id}: expected call names an undeclared function"
        ));
    }
    let alternatives = &tool.expected.arguments;
    if alternatives.is_empty()
        || alternatives.len() > 16
        || alternatives.iter().enumerate().any(|(i, a)| {
            !matches!(a, grade::atlas::ExactJson::Object(_)) || alternatives[..i].contains(a)
        })
    {
        return Err(format!(
            "case {id}: expected arguments require 1..=16 distinct JSON objects"
        ));
    }
    qualification_shape(q, "call qualification")?;
    let mut covered = [false; 16];
    for artifact in &q.valid {
        match grade::tool_call(artifact.as_bytes(), tool) {
            (ToolCall::Valid, Some(alternative)) => covered[alternative] = true,
            _ => return Err(format!("valid call qualification failed for case {id}")),
        }
    }
    if q.wrong
        .iter()
        .any(|a| grade::tool_call(a.as_bytes(), tool).0 == ToolCall::Valid)
    {
        return Err(format!("wrong call qualification failed for case {id}"));
    }
    if covered[..alternatives.len()].contains(&false) {
        return Err(format!("unqualified expected arguments in case {id}"));
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
        5 => crate::record::parse::<WirePack<CaseV5>>(bytes).map(Into::into),
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
        tool(case)?;
        if !worlds.contains(case.world.as_str()) || !groups.contains(case.group.as_str()) {
            return Err(format!("case {} has undeclared world/group", case.id));
        }
        used_worlds.insert(case.world.as_str());
        used_groups.insert(case.group.as_str());
        if case.messages.is_empty() || case.messages.len() > 64 {
            return Err(format!("case {} requires 1..=64 messages", case.id));
        }
        let mut message_bytes = case.tool.as_ref().map_or(0, |t| t.result.len());
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
        qualification_shape(q, "qualification")?;
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
    // request streaming usage in the streaming mode; the v3 tool round is
    // nonstreaming only.
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
        Profile::DeclaredChatCompletionsV3 => {
            if protocol.stream || protocol.include_usage != Some(false) {
                return Err(
                    "profile declared-chat-completions-v3 is nonstreaming and declares include_usage false".into(),
                );
            }
        }
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

/// Tool packs (version 5) and the tool-call profile (v3) only pair with each
/// other, wherever a pack meets a protocol.
pub(crate) fn tool_profile(pack: &Pack, protocol: &Protocol) -> Result<()> {
    if (pack.version == 5) != (protocol.profile == Profile::DeclaredChatCompletionsV3) {
        return Err("tool packs require profile v3 and profile v3 requires a tool pack".into());
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
    // Submission v1 has no call channel; tool cases are graded only from
    // collected run evidence.
    if pack.version == 5 || submission.protocol.profile == Profile::DeclaredChatCompletionsV3 {
        return Err(
            "tool-call cases require collected run evidence, not submitted artifacts".into(),
        );
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
        p.profile = Profile::DeclaredChatCompletionsV3;
        protocol(&p).unwrap();
        p.include_usage = None;
        assert!(protocol(&p).is_err(), "v3 requires explicit include_usage");
        p.include_usage = Some(false);
        p.stream = true;
        assert!(protocol(&p).is_err(), "v3 is nonstreaming");
    }

    fn tool_call(name: &str, arguments: &str) -> String {
        serde_json::json!([{"id":"c1","type":"function",
            "function":{"name":name,"arguments":arguments}}])
        .to_string()
    }

    fn v5_pack() -> serde_json::Value {
        let mut pack = v4_case(
            serde_json::json!({"kind":"string-set","accepted":["22"]}),
            &[r#"{"answer":"22"}"#],
            &[r#"{"answer":"21"}"#],
        );
        pack["version"] = 5.into();
        pack["cases"][0]["tool"] = serde_json::json!({
            "functions":[
                {"name":"lookup_invoice","description":"Invoice by ID.",
                 "parameters":{"type":"object","properties":{"invoice_id":{"type":"string"}}}},
                {"name":"lookup_customer","description":"Customer by ID.",
                 "parameters":{"type":"object"}}],
            "expected":{"name":"lookup_invoice","arguments":[
                {"invoice_id":"INV-7"},{"invoice_id":"INV-7","currency":"EUR"}]},
            "result":"{\"lines\":[{\"quantity\":3,\"unit_price\":4},{\"quantity\":2,\"unit_price\":5}]}"
        });
        pack["cases"][0]["call_qualification"] = serde_json::json!({
            "valid":[
                tool_call("lookup_invoice", r#"{"invoice_id":"INV-7"}"#),
                tool_call("lookup_invoice", r#"{"currency":"EUR","invoice_id":"INV-7"}"#)],
            "wrong":[tool_call("lookup_customer", r#"{"customer_id":"INV-7"}"#), "[]"]
        });
        pack
    }

    #[test]
    fn v5_tool_cases_admit_and_self_qualify() {
        let wire = v5_pack();
        let bytes = serde_json::to_vec(&wire).unwrap();
        let pack = admit(&bytes).unwrap();
        assert!(pack.cases[0].tool.is_some() && pack.cases[0].call_qualification.is_some());
        // Earlier closed readers refuse the tool fields; v5 requires both.
        for version in 1..=4 {
            let mut old = wire.clone();
            old["version"] = version.into();
            assert!(
                admit(&serde_json::to_vec(&old).unwrap()).is_err(),
                "{version}"
            );
        }
        for field in ["tool", "call_qualification", "provenance"] {
            let mut missing = wire.clone();
            missing["cases"][0].as_object_mut().unwrap().remove(field);
            assert!(
                admit(&serde_json::to_vec(&missing).unwrap()).is_err(),
                "{field}"
            );
        }
        // Nine distinct, otherwise valid declarations that keep the expected
        // function, so only the count bound can reject them.
        let mut nine = wire["cases"][0]["tool"]["functions"]
            .as_array()
            .unwrap()
            .clone();
        nine.extend((0..7).map(
            |i| serde_json::json!({"name":format!("decoy{i}"),"description":"d","parameters":{}}),
        ));
        let mut rejected: Vec<(&str, serde_json::Value)> = vec![
            ("/tool/functions", serde_json::json!([])),
            ("/tool/functions", nine.into()),
            ("/tool/functions/1/name", "lookup_invoice".into()),
            ("/tool/functions/1/name", "lookup customer".into()),
            ("/tool/functions/1/name", "".into()),
            ("/tool/functions/1/name", "x".repeat(65).into()),
            ("/tool/functions/1/description", "".into()),
            ("/tool/functions/1/parameters", "object".into()),
            (
                "/tool/functions/1",
                serde_json::json!(["lookup_customer", "d", {}]),
            ),
            ("/tool/functions/1/extra", serde_json::json!(null)),
            ("/tool/expected/name", "lookup_order".into()),
            ("/tool/expected/arguments", serde_json::json!([])),
            ("/tool/expected/arguments", serde_json::json!([["INV-7"]])),
            (
                "/tool/expected/arguments",
                serde_json::json!([{"invoice_id":"INV-7"},{"invoice_id":"INV-7"}]),
            ),
            (
                "/tool/expected/arguments/1",
                serde_json::json!({"invoice_id":"INV-7"}),
            ),
            ("/tool/result", 7.into()),
            ("/tool/result", "x".repeat(MESSAGE_CAP).into()),
            ("/tool/executable", true.into()),
            ("/call_qualification/wrong", serde_json::json!([])),
            ("/call_qualification/valid/1", "[]".into()),
            (
                "/call_qualification/wrong/1",
                tool_call("lookup_invoice", r#"{"invoice_id":"INV-7"}"#).into(),
            ),
        ];
        let alternatives: Vec<_> = (0..17).map(|i| serde_json::json!({"page": i})).collect();
        rejected.push(("/tool/expected/arguments", alternatives.into()));
        for (pointer, value) in rejected {
            let mut bad = wire.clone();
            let case = &mut bad["cases"][0];
            if case.pointer(pointer).is_some() {
                *case.pointer_mut(pointer).unwrap() = value;
            } else {
                let (parent, key) = pointer.rsplit_once('/').unwrap();
                case.pointer_mut(parent).unwrap()[key] = value;
            }
            assert!(
                admit(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "{pointer}"
            );
        }
        // Every argument alternative needs a passing qualification artifact.
        let mut uncovered = wire.clone();
        uncovered["cases"][0]["call_qualification"]["valid"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(admit(&serde_json::to_vec(&uncovered).unwrap()).is_err());
        // Submitted artifacts carry no call channel: v5 packs and the v3
        // profile are refused before any answer is read.
        let error = submission(SUB, &pack).unwrap_err();
        assert!(error.contains("collected run evidence"), "{error}");
        let v3 = std::str::from_utf8(SUB)
            .unwrap()
            .replacen(
                "declared-chat-completions-v1",
                "declared-chat-completions-v3",
                1,
            )
            .replacen(
                "\"seed\": null",
                "\"seed\": null,\n    \"include_usage\": false",
                1,
            );
        let error = submission(v3.as_bytes(), &admit(PACK).unwrap()).unwrap_err();
        assert!(error.contains("collected run evidence"), "{error}");
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
