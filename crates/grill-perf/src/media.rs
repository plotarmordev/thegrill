//! Offline admission for operator-declared media processes; never launches a runner.
use crate::{evidence, model::Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const CAP: usize = 4 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Workload {
    kind: String,
    version: u32,
    name: String,
    modality: String,
    request: Request,
    outputs: Vec<Output>,
    cells: Vec<Cell>,
    slots: Vec<Slot>,
    limits: Limits,
    gates: Vec<Gate>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    prompt: String,
    negative_prompt: Option<String>,
    seeds: Vec<u64>,
    width: u32,
    height: u32,
    steps: u32,
    guidance_milli: Option<u32>,
    images_per_run: u32,
    frames: Option<u32>,
    fps: Option<u32>,
    duration_us: Option<u64>,
    sample_rate: Option<u32>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Output {
    placeholder: String,
    container: String,
    expect: Expect,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    width: u32,
    height: u32,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cell {
    id: String,
    warmup_runs: u32,
    measured_runs: u32,
    #[serde(default)]
    pause_between_runs_ms: u32,
    request: Option<Request>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Slot {
    id: String,
    arm: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    run_deadline_ms: u32,
    setup_deadline_ms: u32,
    output_bytes: u64,
    stdout_bytes: usize,
    stderr_bytes: usize,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    cell: String,
    metric: String,
    max_adverse_bps: u32,
    max_reference_spread_bps: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Runner {
    kind: String,
    version: u32,
    id: String,
    execution: String,
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<String>,
    setup: Option<Setup>,
    cache: Cache,
    markers: Option<Markers>,
    pins: Vec<Pin>,
    probe: Option<Probe>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Setup {
    program: Option<String>,
    args: Vec<String>,
    deadline_ms: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cache {
    declared_state: String,
    paths: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Markers {
    file: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    path: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    program: String,
    args: Vec<String>,
}

// Deserialize through maps so positional records and duplicate keys cannot disappear.
struct Strict(serde_json::Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON value")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut map = serde_json::Map::new();
                while let Some((key, Strict(value))) = a.next_entry::<String, Strict>()? {
                    if map.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate field"));
                    }
                }
                Ok(Strict(map.into()))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(v)) = a.next_element()? {
                    values.push(v);
                }
                Ok(Strict(values.into()))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(serde_json::Value::Null))
            }
        }
        d.deserialize_any(Visitor)
    }
}
fn shapes(value: &serde_json::Value) -> Result<()> {
    let map = value.as_object().ok_or("records must be JSON objects")?;
    for (key, v) in map {
        match key.as_str() {
            "env" if !v.is_object() => return Err("environment must be a JSON object".into()),
            "request" | "limits" | "expect" | "setup" | "cache" | "markers" | "probe"
                if !v.is_null() =>
            {
                shapes(v)?
            }
            "outputs" | "cells" | "slots" | "gates" | "pins" => {
                for item in v.as_array().ok_or("record list must be an array")? {
                    shapes(item)?;
                }
            }
            _ => (),
        }
    }
    Ok(())
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let Strict(value) = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    shapes(&value)?;
    if let Some(modality) = value.get("modality").and_then(|v| v.as_str())
        && matches!(modality, "audio" | "video")
    {
        return Err(format!("{modality} modality is not supported yet"));
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}
fn text(s: &str) -> bool {
    !s.contains('\0')
}
fn label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn request_valid(r: &Request) -> bool {
    let prompt = |s: &str| s.len() <= 16 * 1024 && !s.chars().any(char::is_control);
    prompt(&r.prompt)
        && r.negative_prompt.as_deref().is_none_or(prompt)
        && (1..=16).contains(&r.seeds.len())
        && (1..=16384).contains(&r.width)
        && (1..=16384).contains(&r.height)
        && (1..=1000).contains(&r.steps)
        && r.images_per_run > 0
        && r.frames.is_none()
        && r.fps.is_none()
        && r.duration_us.is_none()
        && r.sample_rate.is_none()
}
fn validate(w: &Workload, r: &Runner) -> Result<()> {
    if w.kind != "media-workload-v1" || w.version != 1 || !label(&w.name) || w.modality != "image" {
        return Err(
            "expected media-workload-v1 version 1 with image modality and a valid name".into(),
        );
    }
    if !request_valid(&w.request)
        || !(1..=64).contains(&w.cells.len())
        || w.outputs.is_empty()
        || !(1..=16).contains(&w.gates.len())
    {
        return Err("invalid image request or workload list bounds".into());
    }
    let mut cells = BTreeSet::new();
    for c in &w.cells {
        if !label(&c.id)
            || !cells.insert(&c.id)
            || c.warmup_runs > 20
            || !(1..=100).contains(&c.measured_runs)
            || c.request.as_ref().is_some_and(|r| !request_valid(r))
        {
            return Err("invalid or duplicate cell; warmups 0..20, measured runs 1..100".into());
        }
    }
    let mut outputs = BTreeSet::new();
    for o in &w.outputs {
        if !matches!(o.placeholder.as_str(), "output" | "output_dir")
            || !outputs.insert(o.placeholder.as_str())
            || !matches!(o.container.as_str(), "png" | "jpeg")
            || !(1..=16384).contains(&o.expect.width)
            || !(1..=16384).contains(&o.expect.height)
        {
            return Err(
                "image outputs require unique output/output_dir placeholders, png/jpeg and bounded dimensions"
                    .into(),
            );
        }
    }
    let output_args = r
        .args
        .iter()
        .map(|a| a.matches("{output}").count())
        .sum::<usize>();
    for request in
        std::iter::once(&w.request).chain(w.cells.iter().filter_map(|c| c.request.as_ref()))
    {
        if output_args > 1
            || (request.images_per_run > 1
                && (output_args != 0
                    || outputs.contains("output")
                    || !r.args.iter().any(|a| a.contains("{output_dir}"))))
        {
            return Err(
                "output may appear at most once; multiple images require output_dir, not output"
                    .into(),
            );
        }
    }
    let mut slots = BTreeSet::new();
    let mut arms = BTreeSet::new();
    for s in &w.slots {
        if !label(&s.id)
            || !slots.insert(&s.id)
            || !arms.insert(s.arm.as_str())
            || !matches!(s.arm.as_str(), "a" | "b" | "b2" | "a2")
        {
            return Err("invalid or duplicate slot".into());
        }
    }
    if !["a", "b", "a2"].iter().all(|a| arms.contains(a)) {
        return Err("slots require a, b and a2; b2 is optional".into());
    }
    let mut gates = BTreeSet::new();
    for g in &w.gates {
        if !cells.contains(&g.cell) || g.metric != "end_to_end_us" || !gates.insert(&g.cell) {
            return Err("gates require unique declared cells and end_to_end_us".into());
        }
    }
    let l = &w.limits;
    if l.run_deadline_ms == 0
        || l.setup_deadline_ms == 0
        || l.output_bytes == 0
        || l.stdout_bytes == 0
        || l.stderr_bytes == 0
    {
        return Err("limits must be positive".into());
    }
    validate_runner(r, l)
}
fn validate_runner(r: &Runner, limits: &Limits) -> Result<()> {
    if r.kind != "media-runner-v1"
        || r.version != 1
        || !label(&r.id)
        || !matches!(
            r.execution.as_str(),
            "in-process" | "delegates-to-resident-service" | "unknown"
        )
        || !matches!(r.cache.declared_state.as_str(), "cold" | "warm" | "unknown")
    {
        return Err("invalid runner kind, version, id, execution or cache state".into());
    }
    if r.program.is_empty()
        || !text(&r.program)
        || r.args.iter().any(|s| !text(s))
        || r.cwd.as_ref().is_some_and(|s| s.is_empty() || !text(s))
        || r.cache.paths.iter().any(|s| s.is_empty() || !text(s))
        || r.env
            .iter()
            .any(|(k, v)| k.is_empty() || k.contains('=') || !text(k) || !text(v))
    {
        return Err("invalid program, argv, environment or path".into());
    }
    if let Some(s) = &r.setup
        && (s
            .deadline_ms
            .is_some_and(|d| d == 0 || d > limits.setup_deadline_ms)
            || s.program.as_ref().is_some_and(|p| p.is_empty() || !text(p))
            || s.args.iter().any(|s| !text(s)))
    {
        return Err("invalid setup command or deadline".into());
    }
    if r.markers.as_ref().is_some_and(|m| m.file) {
        return Err("markers are not supported yet".into());
    }
    for pin in &r.pins {
        if pin.path.is_empty() || !text(&pin.path) || !evidence::is_digest(&pin.sha256) {
            return Err("invalid pin".into());
        }
    }
    if let Some(p) = &r.probe {
        let _ = (&p.program, &p.args);
        return Err("external probes are not supported yet".into());
    }
    Ok(())
}
fn expand(path: &str) -> Result<PathBuf> {
    if path == "~" || path.starts_with("~/") {
        let home = std::env::var_os("HOME").ok_or("HOME is unavailable for ~ expansion")?;
        Ok(PathBuf::from(home).join(path.strip_prefix("~/").unwrap_or("")))
    } else {
        Ok(path.into())
    }
}
#[derive(Serialize)]
struct Program {
    path: PathBuf,
    sha256: String,
}
fn program(name: &str) -> Result<Program> {
    let path = std::fs::canonicalize(expand(name)?).map_err(|e| format!("resolve program: {e}"))?;
    let bytes = evidence::read(&path, 256 * 1024 * 1024)?;
    Ok(Program {
        path,
        sha256: evidence::digest(&bytes),
    })
}
fn substitute(arg: &str, request: &Request, seed: u64, dir: &str, ext: &str) -> Result<String> {
    let mut result = String::new();
    let mut rest = arg;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        let end = rest[start..].find('}').ok_or("unclosed placeholder")? + start;
        let key = &rest[start + 1..end];
        let value = match key {
            "prompt" => request.prompt.clone(),
            "negative_prompt" => request.negative_prompt.clone().unwrap_or_default(),
            "seed" => seed.to_string(),
            "width" => request.width.to_string(),
            "height" => request.height.to_string(),
            "steps" => request.steps.to_string(),
            "output" => format!("{dir}/output-00.{ext}"),
            "output_dir" => format!("{dir}/out/"),
            "marker_file" => format!("{dir}/markers.ndjson"),
            "frames" | "fps" | "duration_s" | "sample_rate" => {
                return Err(format!("placeholder {{{key}}} is unavailable for image"));
            }
            _ => return Err(format!("unknown placeholder {{{key}}}")),
        };
        result.push_str(&value);
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}
#[derive(Serialize)]
struct Planned {
    cell: String,
    phase: &'static str,
    seed: u64,
    args: Vec<String>,
}
#[derive(Serialize)]
struct Preflight {
    kind: &'static str,
    status: &'static str,
    execution: String,
    workload_sha256: String,
    workload_typed_sha256: String,
    runner_sha256: String,
    program: Program,
    setup_program: Option<Program>,
    setup_args: Option<Vec<String>>,
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    cache_paths: Vec<PathBuf>,
    cache_declared_state: String,
    pins_verified: usize,
    warmup_runs: u64,
    measured_runs: u64,
    total_runs: u64,
    wall_clock_ceiling_ms: u64,
    output_path_basis: &'static str,
    samples: Vec<Planned>,
}
fn preflight(workload: &Path, runner: &Path) -> Result<Preflight> {
    let wb = evidence::read(workload, CAP)?;
    let rb = evidence::read(runner, CAP)?;
    let w: Workload = parse(&wb)?;
    let r: Runner = parse(&rb)?;
    validate(&w, &r)?;
    let program = program(&r.program)?;
    let setup_program = r
        .setup
        .as_ref()
        .map(|s| self::program(s.program.as_deref().unwrap_or(&r.program)))
        .transpose()?;
    let ext = &w.outputs[0].container;
    let render = |args: &[String], req: &Request, seed, dir: &str| -> Result<Vec<String>> {
        args.iter()
            .map(|arg| substitute(arg, req, seed, dir, ext))
            .collect()
    };
    let setup_args = r
        .setup
        .as_ref()
        .map(|s| render(&s.args, &w.request, w.request.seeds[0], "setup"))
        .transpose()?;
    let mut samples = Vec::new();
    let (mut warmup_runs, mut measured_runs, mut ceiling) = (0u64, 0u64, 0u64);
    for cell in &w.cells {
        let req = cell.request.as_ref().unwrap_or(&w.request);
        warmup_runs += u64::from(cell.warmup_runs);
        measured_runs += u64::from(cell.measured_runs);
        for k in 0..cell.warmup_runs + cell.measured_runs {
            let seed = req.seeds[k as usize % req.seeds.len()];
            let dir = format!("run-{:06}", samples.len());
            samples.push(Planned {
                cell: cell.id.clone(),
                phase: if k < cell.warmup_runs {
                    "warmup"
                } else {
                    "measured"
                },
                seed,
                args: render(&r.args, req, seed, &dir)?,
            });
            ceiling += u64::from(w.limits.run_deadline_ms) + u64::from(cell.pause_between_runs_ms);
        }
    }
    if let Some(s) = &r.setup {
        ceiling += u64::from(s.deadline_ms.unwrap_or(w.limits.setup_deadline_ms));
    }
    Ok(Preflight {
        kind: "media-preflight-v1",
        status: "experimental-preflight-only",
        execution: r.execution,
        workload_sha256: evidence::digest(&wb),
        workload_typed_sha256: evidence::digest(
            &serde_json::to_vec(&w).map_err(|e| e.to_string())?,
        ),
        runner_sha256: evidence::digest(&rb),
        program,
        setup_program,
        setup_args,
        env: r.env,
        cwd: r.cwd.as_deref().map(expand).transpose()?,
        cache_paths: r
            .cache
            .paths
            .iter()
            .map(|p| expand(p))
            .collect::<Result<_>>()?,
        cache_declared_state: r.cache.declared_state,
        pins_verified: 0,
        warmup_runs,
        measured_runs,
        total_runs: warmup_runs + measured_runs,
        wall_clock_ceiling_ms: ceiling,
        output_path_basis: "relative to a future capture directory; no directory created",
        samples,
    })
}
#[derive(Subcommand)]
pub enum Command {
    /// Validate image workload and runner, hash programs and plan commands; launch nothing.
    Preflight {
        #[arg(long)]
        workload: PathBuf,
        #[arg(long)]
        runner: PathBuf,
        #[arg(long)]
        json: bool,
    },
}
pub fn execute(command: Command) -> Result<u8> {
    let Command::Preflight {
        workload,
        runner,
        json,
    } = command;
    let report = preflight(&workload, &runner)?;
    if json {
        crate::print_json(&report)?;
    } else {
        println!("media: {} (no process started)", report.status);
        println!("execution: {}", report.execution);
        println!(
            "workload sha256: {}\nworkload typed sha256: {}\nrunner sha256: {}",
            report.workload_sha256, report.workload_typed_sha256, report.runner_sha256
        );
        println!(
            "program: {}\nprogram sha256: {}",
            report.program.path.display(),
            report.program.sha256
        );
        println!(
            "warmup runs: {}\nmeasured runs: {}\ntotal runs: {}\nwall-clock ceiling ms: {}",
            report.warmup_runs,
            report.measured_runs,
            report.total_runs,
            report.wall_clock_ceiling_ms
        );
        println!("output paths: {}", report.output_path_basis);
        println!(
            "env additions: {}\ncwd: {}\ncache state: {}\ncache paths: {}\npins verified: {}",
            serde_json::to_string(&report.env).map_err(|e| e.to_string())?,
            serde_json::to_string(&report.cwd).map_err(|e| e.to_string())?,
            report.cache_declared_state,
            serde_json::to_string(&report.cache_paths).map_err(|e| e.to_string())?,
            report.pins_verified
        );
        if let (Some(p), Some(args)) = (&report.setup_program, &report.setup_args) {
            println!(
                "setup: {} {}\nsetup program sha256: {}",
                p.path.display(),
                serde_json::to_string(args).map_err(|e| e.to_string())?,
                p.sha256
            );
        } else {
            println!("setup: null");
        }
        for sample in &report.samples {
            println!(
                "{} {} seed {}: {} {}",
                sample.cell,
                sample.phase,
                sample.seed,
                report.program.path.display(),
                serde_json::to_string(&sample.args).map_err(|e| e.to_string())?
            );
        }
    }
    Ok(0)
}
