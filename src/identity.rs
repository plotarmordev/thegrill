use crate::contract::*;
use crate::grade;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn start(domain: &str) -> Sha256 {
    let mut hash = Sha256::new();
    hash.update(b"the-grill\0");
    hash.update(domain.as_bytes());
    hash.update(b"\0v1\0");
    hash
}

fn finish(hash: Sha256) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(64);
    for byte in hash.finalize() {
        write!(&mut result, "{byte:02x}").expect("String write");
    }
    result
}

pub(crate) fn bytes(domain: &str, value: &[u8]) -> String {
    let mut hash = start(domain);
    hash.update(value);
    finish(hash)
}

pub(crate) fn typed(domain: &str, value: &impl Serialize) -> Result<String> {
    let mut writer = HashWriter(start(domain));
    serde_json::to_writer(&mut writer, value).map_err(|e| format!("identity encoding: {e}"))?;
    Ok(finish(writer.0))
}

// Plain (undomained) SHA-256 for retained evidence files so external tools can
// recheck them; identities above stay domain-separated.
pub(crate) struct Hasher(Sha256);

impl Hasher {
    pub(crate) fn new() -> Self {
        Self(Sha256::new())
    }
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
    pub(crate) fn finish(self) -> String {
        finish(self.0)
    }
}

pub(crate) fn sha256(value: &[u8]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(value);
    hasher.finish()
}

pub(crate) fn input(case: &Case) -> Result<String> {
    if let Some(tool) = &case.tool {
        return typed(
            "case-input-v5",
            &(
                5u32,
                grade::revision(&case.acceptance),
                (grade::TOOL_RELATION, grade::TOOL_IMPLEMENTATION),
                &case.id,
                &case.messages,
                &case.acceptance,
                &case.provenance,
                tool,
            ),
        );
    }
    if let Some(provenance) = &case.provenance {
        return typed(
            "case-input-v4",
            &(
                4u32,
                grade::revision(&case.acceptance),
                &case.id,
                &case.messages,
                &case.acceptance,
                provenance,
            ),
        );
    }
    match &case.acceptance {
        Acceptance::ArchivalV1 { accepted } => typed(
            "case-input",
            &(grade::RELATION, &case.id, &case.messages, accepted),
        ),
        acceptance => typed(
            "case-input-v2",
            &(
                2u32,
                grade::revision(acceptance),
                &case.id,
                &case.messages,
                acceptance,
            ),
        ),
    }
}

pub(crate) fn pack(pack: &Pack, source: &[u8]) -> Result<(PackIdentities, Vec<String>)> {
    let inputs = pack.cases.iter().map(input).collect::<Result<Vec<_>>>()?;
    let selection: Vec<_> = pack
        .cases
        .iter()
        .map(|c| (&c.id, &c.world, &c.group))
        .collect();
    let qualification = if pack.version == 5 {
        let entries: Vec<_> = pack
            .cases
            .iter()
            .map(|c| (&c.id, &c.qualification, &c.call_qualification))
            .collect();
        typed("qualification-v5", &entries)?
    } else {
        let entries: Vec<_> = pack
            .cases
            .iter()
            .map(|c| (&c.id, &c.qualification))
            .collect();
        let domain = match pack.version {
            1 => "qualification",
            4 => "qualification-v4",
            _ => "qualification-v2",
        };
        typed(domain, &entries)?
    };
    let grading = if pack.version == 1 {
        typed(
            "grading",
            &(grade::RELATION, grade::IMPLEMENTATION, &qualification),
        )?
    } else if pack.version == 5 {
        let relations: Vec<_> = pack
            .cases
            .iter()
            .map(|c| {
                (
                    &c.id,
                    grade::revision(&c.acceptance),
                    (grade::TOOL_RELATION, grade::TOOL_IMPLEMENTATION),
                )
            })
            .collect();
        typed("grading-v5", &(5u32, relations, &qualification))?
    } else {
        let relations: Vec<_> = pack
            .cases
            .iter()
            .map(|c| (&c.id, grade::revision(&c.acceptance)))
            .collect();
        if pack.version == 4 {
            typed("grading-v4", &(4u32, relations, &qualification))?
        } else {
            typed("grading-v2", &(2u32, relations, &qualification))?
        }
    };
    let identities = PackIdentities {
        source: bytes("pack-source", source),
        tasks: typed("tasks", &inputs)?,
        target: typed(
            "target",
            &(
                pack.version,
                "equal-weight",
                "one-attempt",
                &pack.worlds,
                &pack.groups,
                selection,
            ),
        )?,
        grading,
        qualification,
    };
    Ok((identities, inputs))
}

pub(crate) fn protocol(protocol: &Protocol) -> Result<String> {
    if protocol.profile == Profile::DeclaredChatCompletionsV3 {
        return typed(
            "protocol",
            &(
                1u32,
                "serial-pack-order",
                "one-attempt-no-retry",
                "one-choice-at-most-one-fixture-tool-round-nonstream",
                "tool-choice-auto-then-none",
                "valid-call-replayed-canonical-arguments-null-content",
                "other-settings-omitted",
                "collection-protection-not-utility",
                protocol,
            ),
        );
    }
    typed(
        "protocol",
        &(
            1u32,
            "serial-pack-order",
            "one-attempt-no-retry",
            "text-only-one-choice-no-tools",
            "other-settings-omitted",
            "collection-protection-not-utility",
            protocol,
        ),
    )
}

pub(crate) fn system(system: &System) -> Result<String> {
    typed("system", system)
}

pub(crate) fn all(
    pack: PackIdentities,
    submission: &Submission,
    submission_bytes: &[u8],
) -> Result<Identities> {
    Ok(Identities {
        pack,
        submission: bytes("submission-source", submission_bytes),
        protocol: protocol(&submission.protocol)?,
        system: system(&submission.system)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack as admission;

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Vectors {
        bytes_empty: String,
        typed_example: String,
        pack: PackIdentities,
        submission_a: Identities,
        submission_b: Identities,
    }

    #[test]
    fn frozen_identity_vectors() {
        let vectors: Vectors =
            serde_json::from_slice(include_bytes!("../examples/identity-vectors.json")).unwrap();
        assert_eq!(bytes("test", b""), vectors.bytes_empty);
        assert_eq!(
            typed(
                "test",
                &(1u32, "a\n\u{00e9}", Option::<u32>::None, [true, false])
            )
            .unwrap(),
            vectors.typed_example
        );
        let source = include_bytes!("../examples/synthetic-pack.json");
        assert_eq!(
            pack(&admission::admit(source).unwrap(), source).unwrap().0,
            vectors.pack
        );
        for (bytes, expected) in [
            (
                include_bytes!("../examples/submission-a.json").as_slice(),
                vectors.submission_a,
            ),
            (
                include_bytes!("../examples/submission-b.json").as_slice(),
                vectors.submission_b,
            ),
        ] {
            let p = admission::admit(source).unwrap();
            let s = admission::submission(bytes, &p).unwrap();
            let (pack_identity, _) = pack(&p, source).unwrap();
            assert_eq!(all(pack_identity, &s, bytes).unwrap(), expected);
        }
    }

    #[test]
    fn labels_and_source_are_not_task_gold() {
        let source = include_bytes!("../examples/synthetic-pack.json");
        let mut p = admission::admit(source).unwrap();
        let before = pack(&p, source).unwrap().0;
        p.label = "a different label".into();
        let after = pack(&p, &serde_json::to_vec(&p).unwrap()).unwrap().0;
        assert_ne!(before.source, after.source);
        assert_eq!(before.tasks, after.tasks);
        assert_eq!(before.target, after.target);
        if let Acceptance::ArchivalV1 { accepted } = &mut p.cases[0].acceptance {
            accepted.push("new gold".into());
        }
        assert_ne!(before.tasks, pack(&p, source).unwrap().0.tasks);
        p.cases.swap(0, 1);
        assert_ne!(before.target, pack(&p, source).unwrap().0.target);
    }

    /// The documented H(domain, bytes), restated independently of `start`.
    fn h(domain: &str, bytes: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(b"the-grill\0");
        hash.update(domain.as_bytes());
        hash.update(b"\0v1\0");
        hash.update(bytes.as_bytes());
        hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    // Compact source fragments already in typed projection order, so each
    // expected J below is plain concatenation rather than this encoder.
    const PROVENANCE: &str = r#"{"repository":"https://example.org/tasks","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","path":"fixture.json","sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","item_id":"synthetic","license":"CC0-1.0","changes":["Synthetic regression fixture"]}"#;
    const TOOL: &str = r#"{"functions":[{"name":"lookup_invoice","description":"Invoice by ID.","parameters":{"type":"object"}}],"expected":{"name":"lookup_invoice","arguments":[{"invoice_id":"INV-7"}]},"result":"{\"total\":22}"}"#;
    const ANSWERS: &str = r#"{"valid":["{\"answer\":\"22\"}"],"wrong":["{\"answer\":\"21\"}"]}"#;
    const CALLS: &str = r#"{"valid":["[{\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"lookup_invoice\",\"arguments\":\"{\\\"invoice_id\\\":\\\"INV-7\\\"}\"}}]"],"wrong":["[]"]}"#;
    const RELATIONS: &str = r#"["json-string-set-v2","grill-json-string-set-impl-v2"],["tool-call-exact-arguments-v1","grill-tool-call-exact-arguments-impl-v1"]"#;

    #[test]
    fn v5_identities_bind_tool_gold_and_call_qualification() {
        let source = format!(
            r#"{{"version":5,"label":"synthetic v5","worlds":["w"],"groups":["g"],"cases":[{{"id":"c","world":"w","group":"g","messages":[{{"role":"user","content":"Total INV-7."}}],"acceptance":{{"kind":"string-set","accepted":["22"]}},"qualification":{ANSWERS},"provenance":{PROVENANCE},"tool":{TOOL},"call_qualification":{CALLS}}}]}}"#
        );
        let mut p = admission::admit(source.as_bytes()).unwrap();
        let (ids, inputs) = pack(&p, source.as_bytes()).unwrap();
        let input = h(
            "case-input-v5",
            &format!(
                r#"[5,{RELATIONS},"c",[{{"role":"user","content":"Total INV-7."}}],{{"kind":"string-set","accepted":["22"]}},{PROVENANCE},{TOOL}]"#
            ),
        );
        assert_eq!(inputs.as_slice(), std::slice::from_ref(&input));
        assert_eq!(ids.tasks, h("tasks", &format!(r#"["{input}"]"#)));
        assert_eq!(
            ids.target,
            h(
                "target",
                r#"[5,"equal-weight","one-attempt",["w"],["g"],[["c","w","g"]]]"#
            )
        );
        let qualification = h("qualification-v5", &format!(r#"[["c",{ANSWERS},{CALLS}]]"#));
        assert_eq!(ids.qualification, qualification);
        assert_eq!(
            ids.grading,
            h(
                "grading-v5",
                &format!(r#"[5,[["c",{RELATIONS}]],"{qualification}"]"#)
            )
        );
        // The fixture result and declarations are task gold; call
        // qualification moves only qualification and grading.
        p.cases[0].tool.as_mut().unwrap().result = "{\"total\":23}".into();
        let changed = pack(&p, source.as_bytes()).unwrap().0;
        assert_ne!(changed.tasks, ids.tasks);
        assert_eq!(
            (&changed.qualification, &changed.grading),
            (&ids.qualification, &ids.grading)
        );
        p.cases[0].tool.as_mut().unwrap().result = "{\"total\":22}".into();
        p.cases[0].tool.as_mut().unwrap().functions[0].description = "Other.".into();
        assert_ne!(pack(&p, source.as_bytes()).unwrap().0.tasks, ids.tasks);
        p.cases[0].tool.as_mut().unwrap().functions[0].description = "Invoice by ID.".into();
        p.cases[0]
            .call_qualification
            .as_mut()
            .unwrap()
            .wrong
            .push("{}".into());
        let requalified = pack(&p, source.as_bytes()).unwrap().0;
        assert_eq!(requalified.tasks, ids.tasks);
        assert_ne!(requalified.qualification, ids.qualification);
        assert_ne!(requalified.grading, ids.grading);
    }

    #[test]
    fn tool_profile_states_its_own_protocol_constants() {
        let source = include_bytes!("../examples/synthetic-pack.json");
        let bytes = include_bytes!("../examples/submission-a.json");
        let mut protocol = admission::submission(bytes, &admission::admit(source).unwrap())
            .unwrap()
            .protocol;
        let text_only = super::protocol(&protocol).unwrap();
        protocol.profile = Profile::DeclaredChatCompletionsV3;
        protocol.include_usage = Some(false);
        let record = serde_json::to_string(&protocol).unwrap();
        assert_eq!(
            super::protocol(&protocol).unwrap(),
            h(
                "protocol",
                &format!(
                    r#"[1,"serial-pack-order","one-attempt-no-retry","one-choice-at-most-one-fixture-tool-round-nonstream","tool-choice-auto-then-none","valid-call-replayed-canonical-arguments-null-content","other-settings-omitted","collection-protection-not-utility",{record}]"#
                )
            )
        );
        assert_ne!(super::protocol(&protocol).unwrap(), text_only);
    }
}
