use super::*;
use std::{fs, time::UNIX_EPOCH};

const ENTRY_CAP: usize = 100_000;

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Link {
    path: PathBuf,
    target: PathBuf,
    bytes: u64,
    mtime_unix_ms: Option<u64>,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    pub path: PathBuf,
    pub exists: bool,
    pub files: u64,
    pub bytes: u64,
    pub symlinks: u64,
    links: Vec<Link>,
    newest_mtime_unix_ms: Option<u64>,
    pub complete: bool,
    reason: Option<String>,
    detail: Option<String>,
}

pub(super) fn observe(paths: &[PathBuf]) -> Vec<Observation> {
    paths
        .iter()
        .map(|path| {
            let mut observation = Observation {
                path: path.clone(),
                exists: false,
                files: 0,
                bytes: 0,
                symlinks: 0,
                links: Vec::new(),
                newest_mtime_unix_ms: None,
                complete: true,
                reason: None,
                detail: None,
            };
            let mut remaining = ENTRY_CAP;
            let mut link_bytes = FILE_CAP / 4 / paths.len().max(1);
            if let Err(error) = walk(path, &mut observation, &mut remaining, &mut link_bytes, 0) {
                observation.complete = false;
                observation.reason = Some("cache_observation_incomplete".into());
                observation.detail = Some(error);
            }
            observation.links.sort_by(|a, b| a.path.cmp(&b.path));
            observation
        })
        .collect()
}

fn walk(
    path: &Path,
    observation: &mut Observation,
    remaining: &mut usize,
    link_bytes: &mut usize,
    depth: usize,
) -> Result<()> {
    if *remaining == 0 || depth > 64 {
        return Err("cache walk bound exceeded".into());
    }
    *remaining -= 1;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && depth == 0 => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    observation.exists = true;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|time| u64::try_from(time.as_millis()).ok());
    observation.newest_mtime_unix_ms = observation.newest_mtime_unix_ms.max(mtime);
    if metadata.file_type().is_symlink() {
        observation.symlinks += 1;
        let target = fs::read_link(path).map_err(|error| error.to_string())?;
        let relative = path.strip_prefix(&observation.path).unwrap_or(path);
        let size = relative
            .as_os_str()
            .len()
            .saturating_add(target.as_os_str().len())
            .saturating_add(128);
        if size > *link_bytes || relative.to_str().is_none() || target.to_str().is_none() {
            observation.complete = false;
            observation.reason = Some("cache_link_record_bound".into());
        } else {
            *link_bytes -= size;
            observation.links.push(Link {
                path: relative.into(),
                target,
                bytes: metadata.len(),
                mtime_unix_ms: mtime,
            });
        }
    } else if metadata.is_file() {
        observation.files += 1;
        observation.bytes = observation
            .bytes
            .checked_add(metadata.len())
            .ok_or("cache byte count overflow")?;
    } else if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            walk(
                &entry.map_err(|error| error.to_string())?.path(),
                observation,
                remaining,
                link_bytes,
                depth + 1,
            )?;
        }
    } else {
        observation.complete = false;
        observation.reason = Some("cache_entry_type".into());
    }
    Ok(())
}

pub(super) fn grew(before: &[Observation], after: &[Observation]) -> Option<bool> {
    if before.len() != after.len()
        || before
            .iter()
            .zip(after)
            .any(|(a, b)| !a.complete || !b.complete || a.path != b.path)
    {
        return None;
    }
    Some(before.iter().zip(after).any(|(a, b)| {
        b.bytes > a.bytes || b.files > a.files || b.symlinks > a.symlinks || (!a.exists && b.exists)
    }))
}
