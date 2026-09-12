use crate::{bundle, evidence, model::*};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

pub const CAP: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationScope {
    Normal,
    Stress,
    Unknown,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub id: String,
    pub workload: String,
    pub source_sha256: String,
    pub workload_sha256: String,
    pub scope: String,
    pub operation_scope: OperationScope,
}

pub struct Selected {
    pub manifest: Manifest,
    pub bytes: Vec<u8>,
    pub source: Vec<u8>,
    pub workload: Workload,
}

pub fn parse(bytes: &[u8], source: &[u8]) -> Result<Manifest> {
    let manifest: Manifest =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid selection manifest: {e}"))?;
    if manifest.version != 1
        || manifest.id.is_empty()
        || manifest.id.len() > 64
        || !manifest
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        || manifest.scope.trim().is_empty()
        || manifest.scope.len() > 256
        || manifest.scope.chars().any(char::is_control)
    {
        return Err("selection requires version 1, a short ASCII id and nonempty control-free scope within 256 bytes".into());
    }
    let mut components = Path::new(&manifest.workload).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || manifest.workload.len() > 255
        || manifest.workload.chars().any(char::is_control)
        || manifest.workload.contains(['/', '\\'])
    {
        return Err("selection workload must be a leaf filename beside the manifest".into());
    }
    if !bundle::sha256(&manifest.source_sha256) || !bundle::sha256(&manifest.workload_sha256) {
        return Err("selection digests must be lowercase SHA256 hex".into());
    }
    if evidence::digest(source) != manifest.source_sha256 {
        return Err("selection workload source digest mismatch; restore the pinned bytes or approve a new prospective selection".into());
    }
    let workload = workload(source)?;
    if evidence::digest(&serde_json::to_vec(&workload).map_err(|e| e.to_string())?)
        != manifest.workload_sha256
    {
        return Err("selection normalized workload digest mismatch".into());
    }
    Ok(manifest)
}

pub fn workload(source: &[u8]) -> Result<Workload> {
    let workload: Workload =
        serde_json::from_slice(source).map_err(|e| format!("invalid selected workload: {e}"))?;
    workload.validate()?;
    Ok(workload)
}

pub fn load(path: &Path) -> Result<Selected> {
    let root = bundle::root(path)?;
    let bytes = evidence::read(path, CAP)?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid selection manifest: {e}"))?;
    // Validate the leaf before opening any manifest-controlled path.
    let leaf = Path::new(&manifest.workload);
    if leaf.file_name().and_then(|n| n.to_str()) != Some(manifest.workload.as_str())
        || manifest.workload.contains(['/', '\\'])
    {
        return Err("selection workload must be a leaf filename beside the manifest".into());
    }
    let source = evidence::read(&root.join(leaf), FILE_CAP)?;
    let manifest = parse(&bytes, &source)?;
    let workload = workload(&source)?;
    Ok(Selected {
        manifest,
        bytes,
        source,
        workload,
    })
}

pub fn budgets(workload: &Workload, acquisitions: usize) -> Result<(usize, usize, usize)> {
    let (mut warmup, mut measured) = (0usize, 0usize);
    for cell in &workload.cells {
        let lanes = (cell.concurrency as usize)
            .checked_mul(acquisitions)
            .ok_or("capture request ceiling overflows")?;
        warmup = (cell.warmup_trials as usize)
            .checked_mul(lanes)
            .and_then(|n| warmup.checked_add(n))
            .ok_or("capture warmup request ceiling overflows")?;
        measured = (cell.trials as usize)
            .checked_mul(lanes)
            .and_then(|n| measured.checked_add(n))
            .ok_or("capture measured request ceiling overflows")?;
    }
    let tokens = warmup
        .checked_add(measured)
        .and_then(|n| n.checked_mul(workload.request.output.tokens as usize))
        .ok_or("capture output token ceiling overflows")?;
    Ok((warmup, measured, tokens))
}
