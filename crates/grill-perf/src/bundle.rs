use crate::{evidence, model::*};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

const MANIFEST_CAP: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum EntryId {
    SparkdashDecodeV1,
    SparkdashPrefillV1,
    GlmDecodeV1,
    GlmPrefillV1,
}
impl EntryId {
    fn name(self) -> &'static str {
        match self {
            Self::SparkdashDecodeV1 => "sparkdash-decode-v1",
            Self::SparkdashPrefillV1 => "sparkdash-prefill-v1",
            Self::GlmDecodeV1 => "glm-decode-v1",
            Self::GlmPrefillV1 => "glm-prefill-v1",
        }
    }
    fn filename(self) -> &'static str {
        match self {
            Self::SparkdashDecodeV1 => "sparkdash-decode-v1.json",
            Self::SparkdashPrefillV1 => "sparkdash-prefill-v1.json",
            Self::GlmDecodeV1 => "glm-decode-v1.json",
            Self::GlmPrefillV1 => "glm-prefill-v1.json",
        }
    }
    fn base(self) -> Option<Self> {
        match self {
            Self::SparkdashDecodeV1 | Self::SparkdashPrefillV1 => None,
            Self::GlmDecodeV1 => Some(Self::SparkdashDecodeV1),
            Self::GlmPrefillV1 => Some(Self::SparkdashPrefillV1),
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: EntryId,
    file: String,
    source_sha256: String,
    workload_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<EntryId>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    decode: EntryId,
    prefill: EntryId,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Recipes {
    deepseek: Recipe,
    glm: Recipe,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    entries: [Entry; 4],
    recipes: Recipes,
}
#[derive(Serialize)]
struct Budgets {
    warmup_requests: u64,
    measured_requests: u64,
    total_requests: u64,
    total_output_token_ceiling: u64,
    request_bytes_cap: usize,
    limits: Limits,
}
#[derive(Serialize)]
struct VerifiedEntry {
    #[serde(flatten)]
    identity: Entry,
    name: String,
    request: RequestSettings,
    budgets: Budgets,
}
#[derive(Serialize)]
pub struct Verification {
    version: u32,
    claim: &'static str,
    manifest_sha256: String,
    entries: Vec<VerifiedEntry>,
    recipes: Recipes,
}
fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn root(manifest: &Path) -> Result<PathBuf> {
    let parent = manifest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Check components before resolution so a symlink followed by /.. cannot hide.
    let mut checked = PathBuf::new();
    for component in parent.components() {
        checked.push(component.as_os_str());
        evidence::directory(&checked)?;
    }
    evidence::directory(parent)?;
    Ok(parent.to_path_buf())
}
pub fn verify(path: &Path) -> Result<Verification> {
    let root = root(path)?;
    let bytes = evidence::read(path, MANIFEST_CAP)?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid recipe manifest: {e}"))?;
    if manifest.version != 1 {
        return Err("unsupported recipe manifest version".into());
    }
    let recipes = &manifest.recipes;
    if recipes.deepseek.decode != EntryId::SparkdashDecodeV1
        || recipes.deepseek.prefill != EntryId::SparkdashPrefillV1
        || recipes.glm.decode != EntryId::GlmDecodeV1
        || recipes.glm.prefill != EntryId::GlmPrefillV1
    {
        return Err("recipe mappings must select their explicit decode and prefill entries".into());
    }
    let mut workloads = Vec::with_capacity(manifest.entries.len());
    for (index, entry) in manifest.entries.iter().enumerate() {
        if manifest.entries[..index]
            .iter()
            .any(|other| other.id == entry.id)
            || entry.base != entry.id.base()
        {
            return Err("duplicate entry or incorrect base relationship".into());
        }
        let mut components = Path::new(&entry.file).components();
        if !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || entry.file != entry.id.filename()
        {
            return Err("entry filename must be the declared identity's leaf filename".into());
        }
        if !sha256(&entry.source_sha256) || !sha256(&entry.workload_sha256) {
            return Err("entry digests must be lowercase SHA256 hex".into());
        }
        let source = evidence::read(&root.join(&entry.file), FILE_CAP)?;
        if evidence::digest(&source) != entry.source_sha256 {
            return Err("workload source digest mismatch".into());
        }
        let workload: Workload =
            serde_json::from_slice(&source).map_err(|e| format!("invalid workload: {e}"))?;
        workload.validate()?;
        let normalized = serde_json::to_vec(&workload).map_err(|e| e.to_string())?;
        if evidence::digest(&normalized) != entry.workload_sha256
            || workload.name != entry.id.name()
        {
            return Err("workload normalized digest or name mismatch".into());
        }
        workloads.push(workload);
    }
    for (entry, variant) in manifest.entries.iter().zip(&workloads) {
        if let Some(base_id) = entry.base {
            let index = manifest
                .entries
                .iter()
                .position(|other| other.id == base_id)
                .ok_or("missing base workload")?;
            let base = &workloads[index];
            if base.request.thinking != Some(false) || base.request.thinking_control.is_some() {
                return Err("base workload must declare legacy thinking false".into());
            }
            let mut request = base.request.clone();
            request.thinking = None;
            request.thinking_control =
                Some(ThinkingControl::VllmEnableThinkingV1 { enabled: false });
            let Workload {
                version,
                name: _,
                request: variant_request,
                limits,
                cases,
                cells,
            } = variant;
            if *version != base.version
                || *variant_request != request
                || *limits != base.limits
                || *cases != base.cases
                || *cells != base.cells
            {
                return Err("GLM variant differs beyond its name and thinking control".into());
            }
        }
    }
    let entries = manifest
        .entries
        .into_iter()
        .zip(workloads)
        .map(|(identity, workload)| {
            // Workload admission bounds trials, concurrency and output tokens.
            let warmup_requests = workload
                .cells
                .iter()
                .map(|cell| u64::from(cell.warmup_trials) * u64::from(cell.concurrency))
                .sum::<u64>();
            let measured_requests = workload
                .cells
                .iter()
                .map(|cell| u64::from(cell.trials) * u64::from(cell.concurrency))
                .sum::<u64>();
            let total_requests = warmup_requests + measured_requests;
            let total_output_token_ceiling =
                total_requests * u64::from(workload.request.output.tokens);
            VerifiedEntry {
                identity,
                name: workload.name,
                request: workload.request,
                budgets: Budgets {
                    warmup_requests,
                    measured_requests,
                    total_requests,
                    total_output_token_ceiling,
                    request_bytes_cap: REQUEST_CAP,
                    limits: workload.limits,
                },
            }
        })
        .collect();
    Ok(Verification {
        version: 1,
        claim: "verified-declared-bundle-not-live-qualification-or-cross-recipe-equivalence",
        manifest_sha256: evidence::digest(&bytes),
        entries,
        recipes: manifest.recipes,
    })
}
pub fn show(verification: &Verification) {
    println!("Verified offline bundle {}", verification.manifest_sha256);
    for entry in &verification.entries {
        println!(
            "{}: source {}; workload {}; {} requests; {} output tokens ceiling; thinking {:?}; thinking_control {:?}",
            entry.name,
            entry.identity.source_sha256,
            entry.identity.workload_sha256,
            entry.budgets.total_requests,
            entry.budgets.total_output_token_ceiling,
            entry.request.thinking,
            entry.request.thinking_control,
        );
    }
    println!("Declared controls only; no live qualification or cross-recipe equivalence.");
}
