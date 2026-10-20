//! Admission and retained evidence for operator-declared media processes.
mod cache;
mod capture;
use crate::{
    evidence,
    model::{FILE_CAP, Phase, Result, identifier, present},
};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const MAX_CELLS: usize = 64;
const MAX_GATES: usize = 16;
const MAX_SEEDS: usize = 16;
const MAX_CANVAS: u32 = 16384;
const MAX_STEPS: u32 = 1000;
const MAX_IMAGES: u32 = 64;
const MAX_FRAMES: u64 = 100_000;
const MAX_FPS: u64 = 240;
const MAX_DURATION_US: u64 = 3_600_000_000;
const MAX_WARMUPS: u32 = 20;
const MAX_MEASURED: u32 = 100;
const PROMPT_CAP: usize = 16 * 1024;
const PROGRAM_CAP: usize = 1024 * 1024 * 1024;
const SUPPORT: &str = "experimental-extension-first-released-v0.9";

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Modality {
    Image,
    Audio,
    Video,
}
#[derive(Deserialize, Serialize, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum OutputPlaceholder {
    Output,
    OutputDir,
}
impl OutputPlaceholder {
    fn name(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::OutputDir => "output_dir",
        }
    }
}
#[derive(Debug, Deserialize, Serialize, Copy, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Container {
    Png,
    Jpeg,
    Wav,
    Mp4,
}
impl Container {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Wav => "wav",
            Self::Mp4 => "mp4",
        }
    }
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
enum Arm {
    A,
    B,
    B2,
    A2,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Metric {
    EndToEndUs,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Execution {
    InProcess,
    DelegatesToResidentService,
    Unknown,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CacheState {
    Cold,
    Warm,
    Unknown,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Workload {
    kind: String,
    version: u32,
    name: String,
    modality: Modality,
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
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    negative_prompt: Option<String>,
    seeds: Vec<u64>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    width: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    height: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    steps: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    guidance_milli: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    images_per_run: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    frames: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    fps: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    duration_us: Option<u64>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    sample_rate: Option<u32>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Output {
    placeholder: OutputPlaceholder,
    container: Container,
    expect: Expect,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    width: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    height: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    frames: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    fps: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    duration_us: Option<u64>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    sample_rate: Option<u32>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    channels: Option<u16>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    audio: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cell {
    id: String,
    warmup_runs: u32,
    measured_runs: u32,
    #[serde(default)]
    pause_between_runs_ms: u32,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    request: Option<Request>,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Slot {
    id: String,
    arm: Arm,
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
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    cell: String,
    metric: Metric,
    max_adverse_bps: u32,
    max_reference_spread_bps: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Runner {
    kind: String,
    version: u32,
    id: String,
    execution: Execution,
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "present")]
    cwd: Option<String>,
    #[serde(default, deserialize_with = "present")]
    setup: Option<Setup>,
    cache: Cache,
    pins: Vec<Pin>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Setup {
    #[serde(default, deserialize_with = "present")]
    program: Option<String>,
    args: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    deadline_ms: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cache {
    declared_state: CacheState,
    paths: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    path: String,
    sha256: String,
}

// Value decoding also rejects duplicate env keys, which a BTreeMap would overwrite.
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
                    if map.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!("duplicate field `{key}`")));
                    }
                    map.insert(key, value);
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
fn reject_positional_records(value: &serde_json::Value) -> Result<()> {
    let map = value.as_object().ok_or("records must be JSON objects")?;
    for (key, v) in map {
        match key.as_str() {
            "env" if !v.is_object() => return Err("environment must be a JSON object".into()),
            "request" | "limits" | "expect" | "setup" | "cache" if !v.is_null() => {
                reject_positional_records(v)?
            }
            "outputs" | "cells" | "slots" | "gates" | "pins" => {
                for item in v.as_array().ok_or("record list must be an array")? {
                    reject_positional_records(item)?;
                }
            }
            _ => (),
        }
    }
    Ok(())
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let Strict(value) = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    reject_positional_records(&value)?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}
fn nul_free(s: &str) -> bool {
    !s.contains('\0')
}
#[derive(Clone, Copy)]
enum Presence {
    Required,
    Optional,
    Forbidden,
}

struct FieldRule {
    name: &'static str,
    min: u64,
    max: u64,
    request: [Presence; 3],
    expect: [Presence; 3],
}

fn validate_fields(
    modality: Modality,
    context: &str,
    expect: bool,
    values: &[(&str, Option<u64>)],
) -> Result<()> {
    use Presence::{Forbidden as F, Optional as O, Required as R};
    const RULES: [FieldRule; 10] = [
        FieldRule {
            name: "width",
            min: 1,
            max: MAX_CANVAS as u64,
            request: [R, F, R],
            expect: [R, F, R],
        },
        FieldRule {
            name: "height",
            min: 1,
            max: MAX_CANVAS as u64,
            request: [R, F, R],
            expect: [R, F, R],
        },
        FieldRule {
            name: "steps",
            min: 1,
            max: MAX_STEPS as u64,
            request: [R, O, O],
            expect: [F, F, F],
        },
        FieldRule {
            name: "images_per_run",
            min: 1,
            max: MAX_IMAGES as u64,
            request: [R, F, F],
            expect: [F, F, F],
        },
        FieldRule {
            name: "frames",
            min: 1,
            max: MAX_FRAMES,
            request: [F, F, R],
            expect: [F, F, R],
        },
        FieldRule {
            name: "fps",
            min: 1,
            max: MAX_FPS,
            request: [F, F, R],
            expect: [F, F, R],
        },
        FieldRule {
            name: "duration_us",
            min: 1,
            max: MAX_DURATION_US,
            request: [F, R, O],
            expect: [F, R, O],
        },
        FieldRule {
            name: "sample_rate",
            min: 1,
            max: u32::MAX as u64,
            request: [F, R, F],
            expect: [F, R, F],
        },
        FieldRule {
            name: "channels",
            min: 1,
            max: u16::MAX as u64,
            request: [F, F, F],
            expect: [F, R, F],
        },
        FieldRule {
            name: "audio",
            min: 0,
            max: 1,
            request: [F, F, F],
            expect: [F, F, O],
        },
    ];
    let index = match modality {
        Modality::Image => 0,
        Modality::Audio => 1,
        Modality::Video => 2,
    };
    for rule in &RULES {
        let value = values
            .iter()
            .find(|(name, _)| *name == rule.name)
            .and_then(|(_, value)| *value);
        let presence = if expect {
            rule.expect[index]
        } else {
            rule.request[index]
        };
        match (presence, value) {
            (Presence::Required, None) => {
                return Err(format!("{context}: missing field `{}`", rule.name));
            }
            (Presence::Forbidden, Some(_)) => {
                return Err(format!(
                    "{context}.{} is forbidden for this modality",
                    rule.name
                ));
            }
            (_, Some(value)) if !(rule.min..=rule.max).contains(&value) => {
                return Err(format!(
                    "{context}.{} must be {}..={}",
                    rule.name, rule.min, rule.max
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_expect(output: &Output, modality: Modality) -> Result<()> {
    let name = output.placeholder.name();
    let compatible = match modality {
        Modality::Image => matches!(output.container, Container::Png | Container::Jpeg),
        Modality::Audio => output.container == Container::Wav,
        Modality::Video => output.container == Container::Mp4,
    };
    if !compatible {
        return Err(format!("outputs.{name}.container does not match modality"));
    }
    let e = &output.expect;
    validate_fields(
        modality,
        &format!("outputs.{name}.expect"),
        true,
        &[
            ("width", e.width.map(u64::from)),
            ("height", e.height.map(u64::from)),
            ("frames", e.frames.map(u64::from)),
            ("fps", e.fps.map(u64::from)),
            ("duration_us", e.duration_us),
            ("sample_rate", e.sample_rate.map(u64::from)),
            ("channels", e.channels.map(u64::from)),
            ("audio", e.audio.map(u64::from)),
        ],
    )
}

fn validate_request(r: &Request, modality: Modality, context: &str) -> Result<()> {
    for (field, value) in [
        ("prompt", Some(r.prompt.as_str())),
        ("negative_prompt", r.negative_prompt.as_deref()),
    ] {
        if value.is_some_and(|v| v.len() > PROMPT_CAP || v.chars().any(char::is_control)) {
            return Err(format!(
                "{context}.{field} must be control-free and at most {PROMPT_CAP} bytes"
            ));
        }
    }
    if !(1..=MAX_SEEDS).contains(&r.seeds.len()) {
        return Err(format!("{context}.seeds needs 1..={MAX_SEEDS} entries"));
    }
    validate_fields(
        modality,
        context,
        false,
        &[
            ("width", r.width.map(u64::from)),
            ("height", r.height.map(u64::from)),
            ("steps", r.steps.map(u64::from)),
            ("images_per_run", r.images_per_run.map(u64::from)),
            ("frames", r.frames.map(u64::from)),
            ("fps", r.fps.map(u64::from)),
            ("duration_us", r.duration_us),
            ("sample_rate", r.sample_rate.map(u64::from)),
        ],
    )
}
fn validate(w: &Workload, r: &Runner) -> Result<()> {
    if w.kind != "media-workload-v1" {
        return Err("workload.kind must be media-workload-v1".into());
    }
    if w.version != 1 {
        return Err("workload.version must be 1".into());
    }
    if !identifier(&w.name) {
        return Err(format!("invalid workload.name: {}", w.name));
    }
    validate_request(&w.request, w.modality, "request")?;
    if !(1..=MAX_CELLS).contains(&w.cells.len()) {
        return Err(format!("cells needs 1..={MAX_CELLS} entries"));
    }
    let mut cells = BTreeSet::new();
    for c in &w.cells {
        if !identifier(&c.id) || !cells.insert(&c.id) {
            return Err(format!("cell {}: invalid or duplicate id", c.id));
        }
        if c.warmup_runs > MAX_WARMUPS {
            return Err(format!(
                "cell {}.warmup_runs must be 0..={MAX_WARMUPS}",
                c.id
            ));
        }
        if !(1..=MAX_MEASURED).contains(&c.measured_runs) {
            return Err(format!(
                "cell {}.measured_runs must be 1..={MAX_MEASURED}",
                c.id
            ));
        }
        if let Some(req) = &c.request {
            validate_request(req, w.modality, &format!("cell {}.request", c.id))?;
        }
    }
    validate_outputs(w, r)?;
    let mut slots = BTreeSet::new();
    let mut arms = BTreeSet::new();
    for s in &w.slots {
        if !identifier(&s.id) || !slots.insert(&s.id) || !arms.insert(&s.arm) {
            return Err(format!("slot {}: invalid or duplicate id/arm", s.id));
        }
    }
    if ![Arm::A, Arm::B, Arm::A2].iter().all(|a| arms.contains(a)) {
        return Err("slots require a, b and a2; b2 is optional".into());
    }
    if !(1..=MAX_GATES).contains(&w.gates.len()) {
        return Err(format!("gates needs 1..={MAX_GATES} entries"));
    }
    let mut gates = BTreeSet::new();
    for g in &w.gates {
        if !cells.contains(&g.cell) || !gates.insert(&g.cell) {
            return Err(format!(
                "gate cell {}: undeclared or duplicate cell",
                g.cell
            ));
        }
        if g.max_adverse_bps > 9999 {
            return Err(format!("gate {}.max_adverse_bps must be <=9999", g.cell));
        }
        if g.max_reference_spread_bps > 1_000_000 {
            return Err(format!(
                "gate {}.max_reference_spread_bps must be <=1000000",
                g.cell
            ));
        }
    }
    let l = &w.limits;
    for (field, value) in [
        ("run_deadline_ms", u64::from(l.run_deadline_ms)),
        ("setup_deadline_ms", u64::from(l.setup_deadline_ms)),
        ("output_bytes", l.output_bytes),
        ("stdout_bytes", l.stdout_bytes as u64),
        ("stderr_bytes", l.stderr_bytes as u64),
    ] {
        if value == 0 {
            return Err(format!("limits.{field} must be positive"));
        }
    }
    validate_runner(r, l)
}
fn validate_outputs(w: &Workload, r: &Runner) -> Result<()> {
    if w.outputs.is_empty() {
        return Err("outputs needs at least one declaration".into());
    }
    let mut outputs = BTreeSet::new();
    for o in &w.outputs {
        let name = o.placeholder.name();
        if !outputs.insert(o.placeholder) {
            return Err(format!("outputs.{name}: duplicate declaration"));
        }
        validate_expect(o, w.modality)?;
    }
    validate_output_args(&outputs, &r.args, "runner.args", true)?;
    if let Some(setup) = &r.setup {
        validate_output_args(&outputs, &setup.args, "runner.setup.args", false)?;
    }
    if outputs.contains(&OutputPlaceholder::Output) {
        for cell in &w.cells {
            if cell
                .request
                .as_ref()
                .unwrap_or(&w.request)
                .images_per_run
                .unwrap_or(1)
                > 1
            {
                return Err(format!(
                    "cell {}.images_per_run: multiple images require only output_dir",
                    cell.id
                ));
            }
        }
    }
    Ok(())
}
fn validate_output_args(
    outputs: &BTreeSet<OutputPlaceholder>,
    args: &[String],
    context: &str,
    require_all: bool,
) -> Result<()> {
    for placeholder in [OutputPlaceholder::Output, OutputPlaceholder::OutputDir] {
        let token = format!("{{{}}}", placeholder.name());
        let count = args
            .iter()
            .map(|arg| arg.matches(&token).count())
            .sum::<usize>();
        if (count > 0 && !outputs.contains(&placeholder))
            || (require_all && count == 0 && outputs.contains(&placeholder))
        {
            return Err(format!(
                "{context}: {token} must match its output declaration"
            ));
        }
        if placeholder == OutputPlaceholder::Output && count > 1 {
            return Err(format!("{context}: {{output}} may appear only once"));
        }
    }
    Ok(())
}
fn validate_runner(r: &Runner, limits: &Limits) -> Result<()> {
    if r.kind != "media-runner-v1" {
        return Err("runner.kind must be media-runner-v1".into());
    }
    if r.version != 1 {
        return Err("runner.version must be 1".into());
    }
    if !identifier(&r.id) {
        return Err(format!("runner.id is invalid: {}", r.id));
    }
    for (i, arg) in r.args.iter().enumerate() {
        if !nul_free(arg) {
            return Err(format!("runner.args[{i}] contains NUL"));
        }
    }
    for (key, value) in &r.env {
        if key.is_empty() || key.contains('=') || !nul_free(key) || !nul_free(value) {
            return Err(format!("runner.env key/value invalid: {key}"));
        }
    }
    if r.cache.paths.len() > 64 || r.cache.paths.iter().any(|path| path.len() > 4096) {
        return Err("cache paths exceed retained observation bounds".into());
    }
    for path in &r.cache.paths {
        validate_path(path, "runner.cache.paths")?;
    }
    if let Some(s) = &r.setup {
        if s.deadline_ms
            .is_some_and(|d| d == 0 || d > limits.setup_deadline_ms)
        {
            return Err(
                "runner.setup.deadline_ms must be positive and <= limits.setup_deadline_ms".into(),
            );
        }
        for (i, arg) in s.args.iter().enumerate() {
            if !nul_free(arg) {
                return Err(format!("runner.setup.args[{i}] contains NUL"));
            }
        }
    }
    for pin in &r.pins {
        validate_path(&pin.path, "runner.pins.path")?;
        if !evidence::is_digest(&pin.sha256) {
            return Err(format!("runner.pins sha256 invalid for {}", pin.path));
        }
    }
    Ok(())
}
fn validate_path(path: &str, field: &str) -> Result<()> {
    if path.is_empty() || !nul_free(path) {
        return Err(format!("{field} must be nonempty and NUL-free"));
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
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Program {
    declared_path: PathBuf,
    resolved_path: PathBuf,
    resolved_sha256: String,
}
fn resolve_program(name: &str) -> Result<Program> {
    validate_path(name, "program")?;
    let declared_path =
        std::path::absolute(expand(name)?).map_err(|e| format!("program {name}: {e}"))?;
    let path = std::fs::canonicalize(&declared_path)
        .map_err(|e| format!("resolve program {name}: {e}"))?;
    let metadata = std::fs::metadata(&path).map_err(|e| format!("program {name}: {e}"))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!("program {name} must be an executable regular file"));
    }
    Ok(Program {
        resolved_sha256: evidence::hash_file(&path, PROGRAM_CAP)?,
        resolved_path: path,
        declared_path,
    })
}
fn substitute(
    arg: &str,
    request: &Request,
    seed: u64,
    dir: &str,
    ext: Option<&str>,
) -> Result<String> {
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
            "width" => request.width.ok_or("width is unavailable")?.to_string(),
            "height" => request.height.ok_or("height is unavailable")?.to_string(),
            "steps" => request.steps.ok_or("steps is unavailable")?.to_string(),
            "frames" => request.frames.ok_or("frames is unavailable")?.to_string(),
            "fps" => request.fps.ok_or("fps is unavailable")?.to_string(),
            "sample_rate" => request
                .sample_rate
                .ok_or("sample_rate is unavailable")?
                .to_string(),
            "duration_s" => {
                let us = request.duration_us.ok_or("duration_s is unavailable")?;
                format!("{}.{:06}", us / 1_000_000, us % 1_000_000)
            }
            "output" => format!(
                "{dir}/output-00.{}",
                ext.ok_or("args uses undeclared output")?
            ),
            "output_dir" => format!("{dir}/out/"),
            _ => return Err(format!("unknown placeholder {{{key}}}")),
        };
        result.push_str(&value);
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}
#[derive(Serialize)]
struct Sample {
    cell: String,
    phase: Phase,
    seed: u64,
    args: Vec<String>,
}
#[derive(Serialize)]
struct Preflight {
    version: u32,
    kind: &'static str,
    claim: &'static str,
    support: &'static str,
    execution: Execution,
    workload_sha256: String,
    workload_typed_sha256: String,
    runner_sha256: String,
    program: Program,
    setup_program: Option<Program>,
    setup_args: Option<Vec<String>>,
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    cache_paths: Vec<PathBuf>,
    cache_declared_state: CacheState,
    warmup_runs: u64,
    measured_runs: u64,
    total_runs: u64,
    wall_clock_allowance_ms: u64,
    samples: Vec<Sample>,
}
struct Plan {
    workload: Workload,
    runner: Runner,
    workload_bytes: Vec<u8>,
    runner_bytes: Vec<u8>,
}

fn load_plan(workload: &Path, runner: &Path) -> Result<Plan> {
    let workload_bytes = evidence::read(workload, FILE_CAP)?;
    let runner_bytes = evidence::read(runner, FILE_CAP)?;
    plan_from_bytes(workload_bytes, runner_bytes)
}

fn plan_from_bytes(workload_bytes: Vec<u8>, runner_bytes: Vec<u8>) -> Result<Plan> {
    let workload = parse(&workload_bytes)?;
    let runner = parse(&runner_bytes)?;
    validate(&workload, &runner)?;
    Ok(Plan {
        workload,
        runner,
        workload_bytes,
        runner_bytes,
    })
}

struct Scheduled<'a> {
    index: usize,
    cell: &'a Cell,
    request: &'a Request,
    phase: Phase,
    seed: u64,
}
impl Scheduled<'_> {
    fn directory(&self) -> String {
        format!("run-{:06}", self.index)
    }
}

fn schedule(workload: &Workload) -> impl Iterator<Item = Scheduled<'_>> {
    workload
        .cells
        .iter()
        .flat_map(|cell| {
            let request = cell.request.as_ref().unwrap_or(&workload.request);
            (0..cell.warmup_runs + cell.measured_runs).map(move |k| (cell, request, k))
        })
        .enumerate()
        .map(|(index, (cell, request, k))| Scheduled {
            index,
            cell,
            request,
            phase: if k < cell.warmup_runs {
                Phase::Warmup
            } else {
                Phase::Measured
            },
            seed: request.seeds[k as usize % request.seeds.len()],
        })
}

fn render(
    args: &[String],
    request: &Request,
    seed: u64,
    dir: &Path,
    workload: &Workload,
) -> Result<Vec<String>> {
    let dir = dir.to_str().ok_or("capture path must be UTF-8")?;
    let ext = workload
        .outputs
        .iter()
        .find(|output| output.placeholder == OutputPlaceholder::Output)
        .map(|output| output.container.extension());
    args.iter()
        .map(|arg| substitute(arg, request, seed, dir, ext))
        .collect()
}

fn preflight(plan: &Plan) -> Result<Preflight> {
    let w = &plan.workload;
    let r = &plan.runner;
    let program = resolve_program(&r.program)?;
    let setup_program = r
        .setup
        .as_ref()
        .map(|s| match s.program.as_deref() {
            Some(name) if name != r.program => resolve_program(name),
            _ => Ok(program.clone()),
        })
        .transpose()?;
    let cwd = r
        .cwd
        .as_deref()
        .map(|p| {
            validate_path(p, "runner.cwd")?;
            let path = expand(p)?;
            evidence::directory(&path).map_err(|e| format!("runner.cwd: {e}"))?;
            Ok::<_, String>(path)
        })
        .transpose()?;
    let setup_args = r
        .setup
        .as_ref()
        .map(|setup| {
            render(
                &setup.args,
                &w.request,
                w.request.seeds[0],
                Path::new("setup"),
                w,
            )
            .map_err(|error| format!("runner.setup.args: {error}"))
        })
        .transpose()?;
    let mut samples = Vec::new();
    let (mut warmup_runs, mut measured_runs, mut wall_clock_allowance_ms) = (0u64, 0u64, 0u64);
    for sample in schedule(w) {
        match sample.phase {
            Phase::Warmup => warmup_runs += 1,
            Phase::Measured => measured_runs += 1,
        }
        samples.push(Sample {
            cell: sample.cell.id.clone(),
            phase: sample.phase,
            seed: sample.seed,
            args: render(
                &r.args,
                sample.request,
                sample.seed,
                Path::new(&sample.directory()),
                w,
            )?,
        });
        wall_clock_allowance_ms +=
            u64::from(w.limits.run_deadline_ms) + u64::from(sample.cell.pause_between_runs_ms);
    }
    if let Some(s) = &r.setup {
        wall_clock_allowance_ms += u64::from(s.deadline_ms.unwrap_or(w.limits.setup_deadline_ms));
    }
    Ok(Preflight {
        version: 1,
        kind: "media-preflight-v1",
        claim: "offline-declared-media-plan-admission-launches-nothing",
        support: SUPPORT,
        execution: r.execution,
        workload_sha256: evidence::digest(&plan.workload_bytes),
        workload_typed_sha256: evidence::digest(
            &serde_json::to_vec(&w).map_err(|e| e.to_string())?,
        ),
        runner_sha256: evidence::digest(&plan.runner_bytes),
        program,
        setup_program,
        setup_args,
        env: r.env.clone(),
        cwd,
        cache_paths: r
            .cache
            .paths
            .iter()
            .map(|p| expand(p))
            .collect::<Result<_>>()?,
        cache_declared_state: r.cache.declared_state,
        warmup_runs,
        measured_runs,
        total_runs: warmup_runs + measured_runs,
        wall_clock_allowance_ms,
        samples,
    })
}
#[derive(Subcommand)]
pub enum Command {
    /// Validate an image workload and runner, hash programs and print sample argv without starting a process.
    Preflight {
        /// Shared image workload document.
        #[arg(long)]
        workload: PathBuf,
        /// Per-arm process runner declaration.
        #[arg(long)]
        runner: PathBuf,
    },
    /// Capture serial fresh-process image samples.
    Capture {
        #[arg(long)]
        workload: PathBuf,
        #[arg(long)]
        runner: PathBuf,
        #[arg(long)]
        deployment: PathBuf,
        #[arg(long)]
        slot: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Recheck retained evidence without launching a runner.
    Inspect { dir: PathBuf },
    /// Compare verified image captures under the declared envelope gates.
    Compare {
        #[arg(long)]
        a: PathBuf,
        #[arg(long)]
        b: PathBuf,
        #[arg(long)]
        a2: PathBuf,
        #[arg(long)]
        b2: Option<PathBuf>,
    },
}
pub fn execute(command: Command) -> Result<u8> {
    match command {
        Command::Preflight { workload, runner } => {
            crate::print_json(&preflight(&load_plan(&workload, &runner)?)?)?;
        }
        Command::Capture {
            workload,
            runner,
            deployment,
            slot,
            out,
        } => {
            let report = capture::capture(&workload, &runner, &deployment, &slot, &out)?;
            let code = report.exit_code();
            crate::print_json(&report)?;
            return Ok(code);
        }
        Command::Inspect { dir } => {
            let report = capture::inspect(&dir)?;
            let code = report.exit_code();
            crate::print_json(&report)?;
            return Ok(code);
        }
        Command::Compare { a, b, a2, b2 } => {
            let report = capture::compare::compare(&a, &b, &a2, b2.as_deref())?;
            let code = report.outcome.exit();
            crate::print_json(&report)?;
            return Ok(code);
        }
    }
    Ok(0)
}
