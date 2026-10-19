use super::*;
use crate::model::Deployment;
use serde_json::{Value, json};
use std::{
    fs,
    time::{Duration, UNIX_EPOCH},
};

const CLAIM: &str = "collector-observed-fresh-process-wall-time-and-container-checked-outputs-not-attested-execution-or-quality";
const CACHE_ENTRIES: usize = 100_000;

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct CacheObservation {
    path: PathBuf,
    exists: bool,
    files: u64,
    bytes: u64,
    newest_mtime_unix_ms: Option<u128>,
    complete: bool,
    error: Option<String>,
}

fn observe(paths: &[PathBuf]) -> Vec<CacheObservation> {
    paths
        .iter()
        .map(|path| {
            let mut observation = CacheObservation {
                path: path.clone(),
                exists: false,
                files: 0,
                bytes: 0,
                newest_mtime_unix_ms: None,
                complete: true,
                error: None,
            };
            let mut remaining = CACHE_ENTRIES;
            if let Err(error) = walk_cache(path, &mut observation, &mut remaining, 0) {
                observation.complete = false;
                observation.error = Some(error);
            }
            observation
        })
        .collect()
}

fn walk_cache(
    path: &Path,
    observation: &mut CacheObservation,
    remaining: &mut usize,
    depth: usize,
) -> Result<()> {
    if *remaining == 0 || depth > 64 {
        return Err("cache observation walk bound exceeded".into());
    }
    *remaining -= 1;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && depth == 0 => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    observation.exists = true;
    if metadata.file_type().is_symlink() {
        return Err("cache observation does not follow symlinks".into());
    }
    let modified = metadata
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    observation.newest_mtime_unix_ms =
        Some(observation.newest_mtime_unix_ms.unwrap_or(0).max(modified));
    if metadata.is_file() {
        observation.files += 1;
        observation.bytes = observation
            .bytes
            .checked_add(metadata.len())
            .ok_or("cache byte count overflow")?;
    } else if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            walk_cache(
                &entry.map_err(|e| e.to_string())?.path(),
                observation,
                remaining,
                depth + 1,
            )?;
        }
    } else {
        return Err("cache path is not a regular file or directory".into());
    }
    Ok(())
}

fn grew(before: &[CacheObservation], after: &[CacheObservation]) -> bool {
    before
        .iter()
        .zip(after)
        .any(|(a, b)| b.bytes > a.bytes || b.files > a.files || (!a.exists && b.exists))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedOutput {
    path: String,
    original_name: Vec<u8>,
    declaration: usize,
    sha256: String,
    bytes: u64,
    validation: std::result::Result<Value, String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Run {
    cell: String,
    phase: String,
    seed: u64,
    end_to_end_us: u64,
    exit_status: Option<i32>,
    status: String,
    eligible: bool,
    failure: Option<String>,
    stdout_truncated: bool,
    stderr_truncated: bool,
    stdout_sha256: String,
    stderr_sha256: String,
    stdout_bytes: usize,
    stderr_bytes: usize,
    outputs: Vec<RetainedOutput>,
    reasons: Vec<String>,
    cache_after: Vec<CacheObservation>,
    cache_mutated_during_measurement: bool,
}

fn render(
    args: &[String],
    request: &Request,
    seed: u64,
    dir: &Path,
    w: &Workload,
) -> Result<Vec<String>> {
    let dir = dir.to_str().ok_or("capture path must be UTF-8")?;
    let ext = w
        .outputs
        .iter()
        .find(|o| o.placeholder == OutputPlaceholder::Output)
        .map(|o| o.container.extension());
    args.iter()
        .map(|arg| substitute(arg, request, seed, dir, ext))
        .collect()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&evidence::read(path, FILE_CAP)?).map_err(|e| e.to_string())
}

fn output_sources(dir: &Path, declaration: &Output, count: u32) -> Result<Vec<PathBuf>> {
    if declaration.placeholder == OutputPlaceholder::Output {
        return Ok(vec![
            dir.join(format!("output-00.{}", declaration.container.extension())),
        ]);
    }
    let directory = dir.join("out");
    evidence::directory(&directory).map_err(|_| "output_entry_type".to_string())?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("output_entry_type".into());
        }
        paths.push(path);
        if paths.len() > count as usize {
            return Err("output_count".into());
        }
    }
    if paths.len() != count as usize {
        return Err("output_count".into());
    }
    paths.sort();
    Ok(paths)
}

fn collect_outputs(
    dir: &Path,
    w: &Workload,
    request: &Request,
) -> Result<(Vec<RetainedOutput>, Vec<String>)> {
    use std::os::unix::ffi::OsStrExt;
    let cap =
        usize::try_from(w.limits.output_bytes).map_err(|_| "output_bytes exceeds platform size")?;
    let retained = dir.join("retained");
    evidence::fresh(&retained)?;
    let mut outputs = Vec::new();
    let mut reasons = Vec::new();
    for (index, declaration) in w.outputs.iter().enumerate() {
        let sources = match output_sources(dir, declaration, request.images_per_run) {
            Ok(paths) => paths,
            Err(reason) => {
                reasons.push(reason);
                continue;
            }
        };
        for source in sources {
            let bytes = match evidence::read(&source, cap) {
                Ok(bytes) => bytes,
                Err(error) => {
                    reasons.push(
                        if source.try_exists().unwrap_or(true) {
                            "output_invalid"
                        } else {
                            "output_missing"
                        }
                        .into(),
                    );
                    reasons.push(error);
                    continue;
                }
            };
            let name = format!(
                "retained/output-{:02}.{}",
                outputs.len(),
                declaration.container.extension()
            );
            evidence::write(&dir.join(&name), &bytes)?;
            let validation = validate_output(&bytes, declaration);
            if validation.is_err() {
                reasons.push("output_invalid".into());
            }
            outputs.push(RetainedOutput {
                path: name,
                original_name: source
                    .file_name()
                    .ok_or("output filename missing")?
                    .as_bytes()
                    .to_vec(),
                declaration: index,
                sha256: evidence::digest(&bytes),
                bytes: bytes.len() as u64,
                validation,
            });
        }
    }
    Ok((outputs, reasons))
}

fn validate_output(bytes: &[u8], declaration: &Output) -> Result<Value> {
    let checked = crate::imagefile::check(bytes, declaration.container)
        .map_err(|error| error.as_str().to_string())?;
    if checked.width != declaration.expect.width || checked.height != declaration.expect.height {
        return Err("output_dimensions".into());
    }
    Ok(
        json!({"label":"container_checked", "container":checked.kind,
        "width":checked.width, "height":checked.height}),
    )
}

pub(super) fn capture(
    workload: &Path,
    runner: &Path,
    deployment: &Path,
    slot: &str,
    out: &Path,
) -> Result<Value> {
    let admitted = preflight(workload, runner)?;
    let wb = evidence::read(workload, FILE_CAP)?;
    let rb = evidence::read(runner, FILE_CAP)?;
    if admitted.workload_sha256 != evidence::digest(&wb)
        || admitted.runner_sha256 != evidence::digest(&rb)
    {
        return Err("declarations changed during admission".into());
    }
    let w: Workload = parse(&wb)?;
    let r: Runner = parse(&rb)?;
    let selected = w
        .slots
        .iter()
        .find(|s| s.id == slot)
        .ok_or("undeclared capture slot")?;
    let db = evidence::read(deployment, FILE_CAP)?;
    let deployment: Deployment = serde_json::from_slice(&db).map_err(|e| e.to_string())?;
    deployment.validate()?;
    let _signals = crate::process::program_signals::Guard::install()?;
    evidence::fresh(out)?;
    let root = fs::canonicalize(out).map_err(|e| e.to_string())?;
    evidence::write(&root.join("workload.json"), &wb)?;
    evidence::write(&root.join("runner.json"), &rb)?;
    evidence::write(&root.join("deployment.json"), &db)?;
    let receipt = json!({
        "version":1, "kind":"media-capture-v1", "claim":CLAIM,
        "support":"experimental-extension-first-released-v0.9", "slot":selected,
        "workload_sha256":admitted.workload_sha256, "workload_typed_sha256":admitted.workload_typed_sha256,
        "runner_sha256":admitted.runner_sha256, "deployment_sha256":evidence::digest(&db),
        "program":admitted.program, "setup_program":admitted.setup_program,
        "execution":r.execution, "env":r.env, "cwd":admitted.cwd,
        "cache_declared_state":r.cache.declared_state, "cache_paths":admitted.cache_paths,
        "collector":{"version":env!("CARGO_PKG_VERSION"), "source":env!("GRILL_PERF_SOURCE"), "sha256":evidence::binary_digest()?},
        "clock":"grill-monotonic", "platform":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},
        "started_unix_ms":crate::metrics::unix_ms(), "projection_root":root,
    });
    evidence::publish(&root, "receipt.json", &receipt)?;
    let before = observe(&admitted.cache_paths);
    evidence::publish(&root, "cache-before.json", &before)?;
    if let Some(setup) = &r.setup {
        run_setup(&root, &w, &r, &admitted, setup)?;
    }
    let after_setup = observe(&admitted.cache_paths);
    evidence::publish(&root, "cache-after-setup.json", &after_setup)?;
    let mut runs: Vec<Run> = Vec::new();
    for cell in &w.cells {
        let request = cell.request.as_ref().unwrap_or(&w.request);
        for k in 0..cell.warmup_runs + cell.measured_runs {
            let seed = request.seeds[k as usize % request.seeds.len()];
            let phase = if k < cell.warmup_runs {
                "warmup"
            } else {
                "measured"
            };
            let dir = root.join(format!("run-{:06}", runs.len()));
            let previous = runs
                .last()
                .map_or(after_setup.as_slice(), |run| &run.cache_after);
            let context = SampleContext {
                w: &w,
                r: &r,
                admitted: &admitted,
                cell,
            };
            let run = sample(&dir, &context, seed, phase, previous)?;
            runs.push(run);
            if !crate::process::program_signals::cancelled() {
                std::thread::sleep(Duration::from_millis(u64::from(cell.pause_between_runs_ms)));
            }
        }
    }
    evidence::publish(&root, "cache-after.json", &observe(&admitted.cache_paths))?;
    publish_manifest(&root, &runs, r.setup.is_some())?;
    inspect(&root)
}

fn launch(
    program: &Program,
    args: &[String],
    r: &Runner,
    admitted: &Preflight,
    deadline_ms: u32,
    limits: &Limits,
) -> crate::process::Output {
    let argv = args
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    let env = r
        .env
        .iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect::<Vec<_>>();
    crate::process::run(&crate::process::Request {
        program: &program.path,
        argv: &argv,
        env: &env,
        cwd: admitted.cwd.as_deref(),
        deadline: Duration::from_millis(u64::from(deadline_ms)),
        stdout_cap: limits.stdout_bytes,
        stderr_cap: limits.stderr_bytes,
        truncation_fatal: false,
    })
}

fn process_status(run: &crate::process::Output) -> &'static str {
    if run.cancelled {
        "cancelled"
    } else if run.timed_out {
        "deadline_exceeded"
    } else if run.status.is_none() {
        "spawn_failed"
    } else if !run.status.is_some_and(|s| s.success()) {
        "nonzero_exit"
    } else if run.failure.is_some() {
        "capture_failed"
    } else {
        "complete"
    }
}

fn run_setup(
    root: &Path,
    w: &Workload,
    r: &Runner,
    admitted: &Preflight,
    setup: &Setup,
) -> Result<()> {
    let dir = root.join("setup");
    evidence::fresh(&dir)?;
    evidence::fresh(&dir.join("out"))?;
    let args = render(&setup.args, &w.request, w.request.seeds[0], &dir, w)?;
    evidence::publish(&dir, "reservation.json", &json!({"args":args,"env":r.env}))?;
    let run = launch(
        admitted
            .setup_program
            .as_ref()
            .ok_or("setup program absent")?,
        &args,
        r,
        admitted,
        setup.deadline_ms.unwrap_or(w.limits.setup_deadline_ms),
        &w.limits,
    );
    evidence::write(&dir.join("stdout.bin"), &run.stdout)?;
    evidence::write(&dir.join("stderr.bin"), &run.stderr)?;
    evidence::publish(
        &dir,
        "setup.json",
        &json!({
            "end_to_end_us":run.elapsed_us, "exit_status":run.status.and_then(|s| s.code()),
            "status":process_status(&run), "failure":run.failure,
            "stdout_truncated":run.stdout_truncated, "stderr_truncated":run.stderr_truncated,
            "stdout_sha256":evidence::digest(&run.stdout), "stderr_sha256":evidence::digest(&run.stderr),
            "stdout_bytes":run.stdout.len(), "stderr_bytes":run.stderr.len(),
        }),
    )
}

struct SampleContext<'a> {
    w: &'a Workload,
    r: &'a Runner,
    admitted: &'a Preflight,
    cell: &'a Cell,
}

fn sample(
    dir: &Path,
    context: &SampleContext<'_>,
    seed: u64,
    phase: &str,
    previous: &[CacheObservation],
) -> Result<Run> {
    let SampleContext {
        w,
        r,
        admitted,
        cell,
    } = context;
    evidence::fresh(dir)?;
    evidence::fresh(&dir.join("out"))?;
    let request = cell.request.as_ref().unwrap_or(&w.request);
    let args = render(&r.args, request, seed, dir, w)?;
    evidence::publish(
        dir,
        "reservation.json",
        &json!({
            "cell":cell.id, "phase":phase, "seed":seed, "args":args, "env":r.env,
        }),
    )?;
    let result = launch(
        &admitted.program,
        &args,
        r,
        admitted,
        w.limits.run_deadline_ms,
        &w.limits,
    );
    evidence::write(&dir.join("stdout.bin"), &result.stdout)?;
    evidence::write(&dir.join("stderr.bin"), &result.stderr)?;
    let (outputs, reasons) = collect_outputs(dir, w, request)?;
    let cache_after = observe(&admitted.cache_paths);
    let mut status = process_status(&result).to_string();
    if status == "complete" && result.elapsed_us >= u64::from(w.limits.run_deadline_ms) * 1000 {
        status = "deadline_exceeded".into();
    }
    if status == "complete" && !reasons.is_empty() {
        status = if reasons.iter().any(|r| r == "output_missing") {
            "output_missing"
        } else {
            "output_invalid"
        }
        .into();
    }
    let run = Run {
        cell: cell.id.clone(),
        phase: phase.into(),
        seed,
        end_to_end_us: result.elapsed_us,
        exit_status: result.status.and_then(|s| s.code()),
        eligible: status == "complete",
        status,
        failure: result.failure,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        stdout_sha256: evidence::digest(&result.stdout),
        stderr_sha256: evidence::digest(&result.stderr),
        stdout_bytes: result.stdout.len(),
        stderr_bytes: result.stderr.len(),
        outputs,
        reasons,
        cache_mutated_during_measurement: phase == "measured" && grew(previous, &cache_after),
        cache_after,
    };
    evidence::publish(dir, "run.json", &run)?;
    Ok(run)
}

fn publish_manifest(root: &Path, runs: &[Run], setup: bool) -> Result<()> {
    let mut files = BTreeMap::new();
    for name in [
        "workload.json",
        "runner.json",
        "deployment.json",
        "receipt.json",
        "cache-before.json",
        "cache-after-setup.json",
        "cache-after.json",
    ] {
        files.insert(
            name.to_string(),
            evidence::hash_file(&root.join(name), FILE_CAP)?,
        );
    }
    if setup {
        for name in [
            "setup/reservation.json",
            "setup/setup.json",
            "setup/stdout.bin",
            "setup/stderr.bin",
        ] {
            files.insert(
                name.to_string(),
                evidence::hash_file(&root.join(name), usize::MAX - 1)?,
            );
        }
    }
    for (index, run) in runs.iter().enumerate() {
        let prefix = format!("run-{index:06}");
        for name in ["reservation.json", "run.json", "stdout.bin", "stderr.bin"] {
            let path = format!("{prefix}/{name}");
            files.insert(
                path.clone(),
                evidence::hash_file(&root.join(path), usize::MAX - 1)?,
            );
        }
        for output in &run.outputs {
            files.insert(format!("{prefix}/{}", output.path), output.sha256.clone());
        }
    }
    let digest = evidence::digest(&serde_json::to_vec(&files).map_err(|e| e.to_string())?);
    evidence::publish(
        root,
        "capture.json",
        &json!({
            "kind":"media-capture-complete-v1", "version":1, "runs":runs.len(),
            "files":files, "evidence_sha256":digest,
        }),
    )
}

fn check_manifest(root: &Path) -> Result<(Value, BTreeMap<String, String>)> {
    evidence::directory(root)?;
    let manifest: Value = read_json(&root.join("capture.json"))?;
    if manifest["kind"] != "media-capture-complete-v1" || manifest["version"] != 1 {
        return Err("invalid media capture manifest".into());
    }
    let files: BTreeMap<String, String> =
        serde_json::from_value(manifest["files"].clone()).map_err(|e| e.to_string())?;
    if manifest["evidence_sha256"]
        != evidence::digest(&serde_json::to_vec(&files).map_err(|e| e.to_string())?)
    {
        return Err("capture manifest digest mismatch".into());
    }
    for (name, digest) in &files {
        let path = Path::new(name);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("invalid retained evidence path".into());
        }
        let mut ancestor = root.to_path_buf();
        for part in path.parent().into_iter().flat_map(Path::components) {
            ancestor.push(part);
            evidence::directory(&ancestor)?;
        }
        if evidence::hash_file(&root.join(path), usize::MAX - 1)? != *digest {
            return Err(format!("retained evidence hash mismatch: {name}"));
        }
    }
    Ok((manifest, files))
}

pub(super) fn inspect(root: &Path) -> Result<Value> {
    let (manifest, files) = check_manifest(root)?;
    let wb = evidence::read(&root.join("workload.json"), FILE_CAP)?;
    let rb = evidence::read(&root.join("runner.json"), FILE_CAP)?;
    let w: Workload = parse(&wb)?;
    let r: Runner = parse(&rb)?;
    validate(&w, &r)?;
    let receipt: Value = read_json(&root.join("receipt.json"))?;
    if receipt["kind"] != "media-capture-v1"
        || receipt["workload_sha256"] != evidence::digest(&wb)
        || receipt["runner_sha256"] != evidence::digest(&rb)
        || receipt["workload_typed_sha256"]
            != evidence::digest(&serde_json::to_vec(&w).map_err(|e| e.to_string())?)
    {
        return Err("media receipt declaration mismatch".into());
    }
    let db = evidence::read(&root.join("deployment.json"), FILE_CAP)?;
    let deployment: Deployment = serde_json::from_slice(&db).map_err(|e| e.to_string())?;
    deployment.validate()?;
    if receipt["deployment_sha256"] != evidence::digest(&db) {
        return Err("media deployment hash mismatch".into());
    }
    let projection = Path::new(
        receipt["projection_root"]
            .as_str()
            .ok_or("missing projection root")?,
    );
    let mut runs = Vec::new();
    let mut summaries = Vec::new();
    let after_setup: Vec<CacheObservation> = read_json(&root.join("cache-after-setup.json"))?;
    for cell in &w.cells {
        let first = runs.len();
        let request = cell.request.as_ref().unwrap_or(&w.request);
        for k in 0..cell.warmup_runs + cell.measured_runs {
            let name = format!("run-{:06}", runs.len());
            let dir = root.join(&name);
            evidence::directory(&dir)?;
            let run: Run = read_json(&dir.join("run.json"))?;
            let phase = if k < cell.warmup_runs {
                "warmup"
            } else {
                "measured"
            };
            let seed = request.seeds[k as usize % request.seeds.len()];
            let args = render(&r.args, request, seed, &projection.join(&name), &w)?;
            let reservation: Value = read_json(&dir.join("reservation.json"))?;
            if reservation
                != json!({"cell":cell.id,"phase":phase,"seed":seed,"args":args,"env":r.env})
                || run.cell != cell.id
                || run.phase != phase
                || run.seed != seed
            {
                return Err("sample reservation projection mismatch".into());
            }
            verify_run(&dir, &name, &run, &w, request, &files)?;
            let previous = runs
                .last()
                .map_or(after_setup.as_slice(), |r: &Run| &r.cache_after);
            if run.cache_mutated_during_measurement
                != (phase == "measured" && grew(previous, &run.cache_after))
            {
                return Err("cache growth observation mismatch".into());
            }
            runs.push(run);
        }
        summaries.push(summarize(cell, &runs[first..]));
    }
    if manifest["runs"] != runs.len() {
        return Err("capture run count mismatch".into());
    }
    Ok(json!({
        "version":1, "kind":"media-inspection-v1", "claim":CLAIM,
        "support":"experimental-extension-first-released-v0.9", "execution":r.execution,
        "slot":receipt["slot"], "cells":summaries, "runs":runs,
        "setup":if r.setup.is_some() { read_json::<Value>(&root.join("setup/setup.json"))? } else { Value::Null },
        "cache_mutated_during_measurement":runs.iter().any(|r| r.cache_mutated_during_measurement),
        "evidence_sha256":manifest["evidence_sha256"],
    }))
}

fn verify_run(
    dir: &Path,
    name: &str,
    run: &Run,
    w: &Workload,
    request: &Request,
    files: &BTreeMap<String, String>,
) -> Result<()> {
    for required in ["reservation.json", "run.json", "stdout.bin", "stderr.bin"] {
        if !files.contains_key(&format!("{name}/{required}")) {
            return Err("incomplete manifest".into());
        }
    }
    for (stream, cap, length, digest) in [
        (
            "stdout.bin",
            w.limits.stdout_bytes,
            run.stdout_bytes,
            &run.stdout_sha256,
        ),
        (
            "stderr.bin",
            w.limits.stderr_bytes,
            run.stderr_bytes,
            &run.stderr_sha256,
        ),
    ] {
        let bytes = evidence::read(&dir.join(stream), cap)?;
        if bytes.len() != length || evidence::digest(&bytes) != *digest {
            return Err("stream evidence mismatch".into());
        }
    }
    let mut counts = vec![0; w.outputs.len()];
    for (index, output) in run.outputs.iter().enumerate() {
        let declaration = w
            .outputs
            .get(output.declaration)
            .ok_or("invalid output declaration index")?;
        if output.path
            != format!(
                "retained/output-{index:02}.{}",
                declaration.container.extension()
            )
        {
            return Err("invalid retained output path".into());
        }
        if !files.contains_key(&format!("{name}/{}", output.path)) {
            return Err("unlisted output".into());
        }
        let cap =
            usize::try_from(w.limits.output_bytes).map_err(|_| "output cap exceeds platform")?;
        let bytes = evidence::read(&dir.join(&output.path), cap)?;
        if evidence::digest(&bytes) != output.sha256
            || bytes.len() as u64 != output.bytes
            || validate_output(&bytes, declaration) != output.validation
        {
            return Err("retained output hash or validation mismatch".into());
        }
        counts[output.declaration] += 1;
    }
    let complete = counts.iter().zip(&w.outputs).all(|(count, output)| {
        *count
            == if output.placeholder == OutputPlaceholder::Output {
                1
            } else {
                request.images_per_run
            }
    });
    let eligible = run.exit_status == Some(0)
        && run.failure.is_none()
        && run.end_to_end_us < u64::from(w.limits.run_deadline_ms) * 1000
        && run.reasons.is_empty()
        && complete
        && run.outputs.iter().all(|o| o.validation.is_ok());
    if run.eligible != (run.status == "complete") || (run.eligible && !eligible) {
        return Err("sample eligibility mismatch".into());
    }
    Ok(())
}

fn summarize(cell: &Cell, runs: &[Run]) -> Value {
    let measured = runs
        .iter()
        .filter(|r| r.phase == "measured")
        .collect::<Vec<_>>();
    let mut times = measured.iter().map(|r| r.end_to_end_us).collect::<Vec<_>>();
    times.sort_unstable();
    let n = times.len();
    let median = if n % 2 == 0 {
        (times[n / 2 - 1] as f64 + times[n / 2] as f64) / 2.0
    } else {
        times[n / 2] as f64
    };
    let mut seeds = BTreeMap::<u64, Vec<Vec<&str>>>::new();
    for run in &measured {
        if run.eligible {
            seeds
                .entry(run.seed)
                .or_default()
                .push(run.outputs.iter().map(|o| o.sha256.as_str()).collect());
        }
    }
    let determinism = seeds.iter().map(|(seed, outputs)| json!({
        "seed":seed, "n":outputs.len(),
        "deterministic":if outputs.len() < 2 { Value::Null } else { json!(outputs.windows(2).all(|w| w[0] == w[1])) },
    })).collect::<Vec<_>>();
    json!({
        "cell":cell.id, "warmup_runs":cell.warmup_runs, "measured_runs":measured.len(),
        "eligible":measured.iter().filter(|r| r.eligible).count(),
        "end_to_end_us":{"n":n,"min":times[0],"median":median,
            "mean":times.iter().map(|t| u128::from(*t)).sum::<u128>() as f64 / n as f64,"max":times[n-1]},
        "determinism_by_seed":determinism,
    })
}
