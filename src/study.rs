use crate::contract::*;
use crate::{identity, pack, record, report};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(crate) const MANIFEST_CAP: usize = 1024 * 1024;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Purpose {
    Pilot,
    Screening,
    Confirmation,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExposureStatus {
    Fresh,
    Exposed,
    Unknown,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Family {
    pub group: String,
    pub construct: String,
    pub source: String,
    pub revision: String,
    pub rights: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Exposure {
    pub case_id: String,
    #[serde(deserialize_with = "record::string_enum")]
    pub left: ExposureStatus,
    #[serde(deserialize_with = "record::string_enum")]
    pub right: ExposureStatus,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub version: u32,
    pub label: String,
    #[serde(deserialize_with = "record::string_enum")]
    pub purpose: Purpose,
    pub construct: String,
    pub sampling: String,
    pub exclusions: String,
    pub stopping_rule: String,
    pub pack_source: String,
    #[serde(deserialize_with = "record::object")]
    pub protocol: Protocol,
    #[serde(deserialize_with = "record::object")]
    pub left_system: System,
    #[serde(deserialize_with = "record::object")]
    pub right_system: System,
    #[serde(deserialize_with = "record::objects")]
    pub families: Vec<Family>,
    #[serde(deserialize_with = "record::objects")]
    pub exposure: Vec<Exposure>,
}

#[derive(Serialize)]
pub(crate) struct Study {
    pub identity: String,
    pub manifest: Manifest,
    pub pack: PackIdentities,
    pub protocol: String,
    pub left_system: String,
    pub right_system: String,
    #[serde(skip)]
    inputs: Vec<String>,
}

fn prose(value: &str, cap: usize, what: &str) -> Result<()> {
    pack::text(value, cap, false, what)?;
    if value.trim().is_empty() {
        return Err(format!("{what} must not be blank"));
    }
    Ok(())
}

/// Bind declarations to exact inputs; hashes do not establish when they were frozen.
pub(crate) fn admit(bytes: &[u8], pack: &Pack, pack_bytes: &[u8]) -> Result<Study> {
    if bytes.len() > MANIFEST_CAP {
        return Err("study manifest exceeds byte limit".into());
    }
    let manifest: Manifest = record::parse(bytes).map_err(|e| format!("study JSON: {e}"))?;
    if manifest.version != 1 {
        return Err("unsupported study version".into());
    }
    prose(&manifest.label, 256, "study label")?;
    for (value, what) in [
        (&manifest.construct, "study construct"),
        (&manifest.sampling, "study sampling"),
        (&manifest.stopping_rule, "study stopping rule"),
    ] {
        prose(value, 4096, what)?;
    }
    pack::text(&manifest.exclusions, 4096, true, "study exclusions")?;
    pack::protocol(&manifest.protocol)?;
    pack::system(&manifest.left_system)?;
    pack::system(&manifest.right_system)?;
    let (identities, inputs) = identity::pack(pack, pack_bytes)?;
    if manifest.pack_source != identities.source {
        return Err("study pack source does not match exact pack bytes".into());
    }
    if manifest.families.len() != pack.groups.len()
        || manifest
            .families
            .iter()
            .zip(&pack.groups)
            .any(|(family, group)| family.group != *group)
    {
        return Err("study families must cover every pack group in declared order".into());
    }
    for family in &manifest.families {
        for (value, what) in [
            (&family.construct, "family construct"),
            (&family.source, "family source"),
            (&family.revision, "family revision"),
            (&family.rights, "family rights"),
        ] {
            prose(value, 4096, what)?;
        }
    }
    if manifest.exposure.len() != pack.cases.len() {
        return Err("study exposure must cover every pack case".into());
    }
    let mut units = HashMap::with_capacity(pack.worlds.len());
    for (case, exposure) in pack.cases.iter().zip(&manifest.exposure) {
        if case.id != exposure.case_id {
            return Err("study exposure must follow exact pack case order".into());
        }
        if manifest.purpose == Purpose::Confirmation
            && (exposure.left != ExposureStatus::Fresh || exposure.right != ExposureStatus::Fresh)
        {
            return Err("confirmation requires fresh exposure declarations on both sides".into());
        }
        if let Some(group) = units.insert(case.world.as_str(), case.group.as_str())
            && group != case.group
        {
            return Err("study problem unit spans multiple families".into());
        }
    }
    Ok(Study {
        identity: identity::bytes("study-source", bytes),
        protocol: identity::protocol(&manifest.protocol)?,
        left_system: identity::system(&manifest.left_system)?,
        right_system: identity::system(&manifest.right_system)?,
        pack: identities,
        inputs,
        manifest,
    })
}

/// The caller supplies receipt-verified views, never unverified outcome arrays.
pub(crate) fn bind(study: &Study, left: &report::Verified, right: &report::Verified) -> Result<()> {
    for (side, view, system) in [
        ("left", left, &study.left_system),
        ("right", right, &study.right_system),
    ] {
        for (name, actual, expected) in [
            ("source", &view.source, &study.pack.source),
            ("tasks", &view.tasks, &study.pack.tasks),
            ("target", &view.target, &study.pack.target),
            (
                "qualification",
                &view.qualification,
                &study.pack.qualification,
            ),
            ("grading", &view.grading, &study.pack.grading),
            ("protocol", &view.protocol, &study.protocol),
        ] {
            if actual != expected {
                return Err(format!("study {side} {name} identity mismatch"));
            }
        }
        if identity::system(&view.system)? != *system {
            return Err(format!("study {side} system identity mismatch"));
        }
        if view.cases.len() != study.inputs.len() {
            return Err(format!("study {side} case count mismatch"));
        }
        for ((case, input), exposure) in view
            .cases
            .iter()
            .zip(&study.inputs)
            .zip(&study.manifest.exposure)
        {
            if case.input != *input || case.case_id != exposure.case_id {
                return Err(format!("study {side} ordered case input mismatch"));
            }
        }
    }
    Ok(())
}
