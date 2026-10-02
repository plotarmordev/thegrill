use crate::evidence::{self, Loaded};
use crate::model::*;
use serde::Serialize;
use std::path::{Path, PathBuf};

const EXCERPT_CHARS: usize = 24;
const SCOPE: &str = "Completion text replayed from verified response evidence of measured lanes, answer and reasoning channels compared separately. Text identity only: token ids are not retained, and identical text does not show identical logits.";

#[derive(Serialize)]
pub struct Report {
    pub version: u32,
    pub kind: &'static str,
    pub scope: &'static str,
    pub a: PathBuf,
    pub b: PathBuf,
    pub workload_sha256: String,
    pub identical: usize,
    pub differs: usize,
    pub unavailable: usize,
    pub lanes: Vec<Lane>,
}
impl Report {
    pub fn exit(&self) -> u8 {
        if self.differs == 0 && self.unavailable == 0 {
            0
        } else {
            2
        }
    }
}
#[derive(Serialize)]
pub struct Lane {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<usize>,
    pub wave: u32,
    pub cell: String,
    pub trial: u32,
    pub lane: u32,
    #[serde(flatten)]
    pub outcome: Outcome,
}
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Identical {
        answer_chars: usize,
        reasoning_chars: usize,
    },
    Differs {
        channel: Channel,
        char_offset: usize,
        byte_offset: usize,
        a_excerpt: String,
        b_excerpt: String,
    },
    Unavailable {
        reasons: Vec<String>,
    },
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Reasoning,
    Answer,
}
impl Channel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::Answer => "answer",
        }
    }
}

type Run = Option<(PathBuf, Loaded)>;

/// A run directory, or a capture's fixed acquisitions in order (`None` when unstarted).
fn runs(root: &Path, capture: bool) -> Result<Vec<Run>> {
    let roots = if capture {
        crate::study::acquisition_runs(root)?
    } else {
        vec![Some(root.to_owned())]
    };
    roots
        .into_iter()
        .map(|root| {
            root.map(|root| evidence::load(&root).map(|loaded| (root, loaded)))
                .transpose()
        })
        .collect()
}

/// Pairs two run directories, or two captures acquisition by acquisition.
pub fn compare(a: &Path, b: &Path) -> Result<Report> {
    let captures = crate::study::is_capture(a);
    if captures != crate::study::is_capture(b) {
        return Err("compare two run directories or two capture roots, not one of each".into());
    }
    let (a_runs, b_runs) = (runs(a, captures)?, runs(b, captures)?);
    let plan = &a_runs
        .iter()
        .chain(&b_runs)
        .flatten()
        .next()
        .ok_or("neither capture has acquisition evidence to compare")?
        .1
        .plan;
    admit(
        plan,
        a_runs
            .iter()
            .chain(&b_runs)
            .flatten()
            .map(|(_, run)| &run.plan),
    )?;
    let mut report = Report {
        version: 1,
        kind: "performance-output-identity-v1",
        scope: SCOPE,
        a: a.to_owned(),
        b: b.to_owned(),
        workload_sha256: plan.workload_sha256.clone(),
        identical: 0,
        differs: 0,
        unavailable: 0,
        lanes: Vec::new(),
    };
    for (acquisition, (a_run, b_run)) in a_runs.iter().zip(&b_runs).enumerate() {
        for spec in plan.waves.iter().filter(|s| s.phase == Phase::Measured) {
            for lane in 0..spec.concurrency {
                let outcome = match (text(a_run, spec, lane), text(b_run, spec, lane)) {
                    (Ok(a), Ok(b)) => identity(&a, &b),
                    (a, b) => Outcome::Unavailable {
                        reasons: [("a", a.err()), ("b", b.err())]
                            .into_iter()
                            .filter_map(|(side, reason)| Some(format!("{side}: {}", reason?)))
                            .collect(),
                    },
                };
                match outcome {
                    Outcome::Identical { .. } => report.identical += 1,
                    Outcome::Differs { .. } => report.differs += 1,
                    Outcome::Unavailable { .. } => report.unavailable += 1,
                }
                report.lanes.push(Lane {
                    acquisition: captures.then_some(acquisition),
                    wave: spec.index,
                    cell: spec.cell.clone(),
                    trial: spec.trial,
                    lane,
                    outcome,
                });
            }
        }
    }
    Ok(report)
}

/// Identity is meaningful only for one greedy workload whose prompts do not vary per capture.
fn admit<'a>(plan: &Plan, plans: impl Iterator<Item = &'a Plan>) -> Result<()> {
    for other in plans {
        if other.workload_sha256 != plan.workload_sha256 {
            return Err(
                "output identity requires the same normalized workload on both sides".into(),
            );
        }
    }
    let workload = &plan.workload;
    if workload.cases.iter().any(|case| {
        case.fill.as_ref().is_some_and(|fill| {
            matches!(fill, Fill::GeneratedProse { .. })
                || case.messages.iter().any(|m| m.content.contains("{salt}"))
        })
    }) {
        return Err(
            "output identity requires prompts without per-capture {salt} text or generated prose"
                .into(),
        );
    }
    let mut lanes = 0;
    for spec in plan.waves.iter().filter(|s| s.phase == Phase::Measured) {
        for lane in 0..spec.concurrency {
            if crate::schedule::settings(workload, spec, lane)?.temperature_milli != Some(0) {
                return Err(
                    "output identity requires greedy decoding (temperature_milli 0) on every measured lane".into(),
                );
            }
            lanes += 1;
        }
    }
    if lanes == 0 {
        return Err("workload has no measured lanes to compare".into());
    }
    Ok(())
}

/// One side's (answer, reasoning) text, or why it cannot be compared.
fn text(run: &Run, spec: &WaveSpec, lane: u32) -> Result<(String, String)> {
    let (root, run) = run.as_ref().ok_or("acquisition not started")?;
    let wave = run.waves[spec.index as usize]
        .as_ref()
        .ok_or_else(|| format!("wave {}", run.states[spec.index as usize]))?;
    let attempt = &wave.attempts[lane as usize];
    if attempt.status != Status::Complete {
        return Err(format!("response not complete: {}", attempt.detail));
    }
    let settings = crate::schedule::settings(&run.plan.workload, spec, lane)?;
    let body = evidence::response(
        &evidence::wave_dir(root, spec.index),
        attempt,
        run.plan.workload.limits.response_bytes,
    )?;
    crate::wire::completion_text(
        attempt,
        &body,
        settings.stream,
        matches!(run.plan.version, 3..=5),
        settings.profile,
    )
}

fn identity(
    (a_answer, a_reasoning): &(String, String),
    (b_answer, b_reasoning): &(String, String),
) -> Outcome {
    for (channel, a, b) in [
        (Channel::Reasoning, a_reasoning, b_reasoning),
        (Channel::Answer, a_answer, b_answer),
    ] {
        if let Some((char_offset, byte_offset)) = difference(a, b) {
            return Outcome::Differs {
                channel,
                char_offset,
                byte_offset,
                a_excerpt: excerpt(a, byte_offset),
                b_excerpt: excerpt(b, byte_offset),
            };
        }
    }
    Outcome::Identical {
        answer_chars: a_answer.chars().count(),
        reasoning_chars: a_reasoning.chars().count(),
    }
}

/// First differing (character, byte) offset; a proper prefix differs where it ends.
fn difference(a: &str, b: &str) -> Option<(usize, usize)> {
    let mut chars = 0;
    for ((byte, x), y) in a.char_indices().zip(b.chars()) {
        if x != y {
            return Some((chars, byte));
        }
        chars += 1;
    }
    (a.len() != b.len()).then_some((chars, a.len().min(b.len())))
}

/// Up to `EXCERPT_CHARS` characters on each side of a character boundary.
fn excerpt(text: &str, byte: usize) -> String {
    let start = text[..byte]
        .char_indices()
        .rev()
        .nth(EXCERPT_CHARS - 1)
        .map_or(0, |(i, _)| i);
    let end = text[byte..]
        .char_indices()
        .nth(EXCERPT_CHARS)
        .map_or(text.len(), |(i, _)| byte + i);
    text[start..end].to_owned()
}

pub fn human(report: &Report) -> String {
    let mut text = format!(
        "Output identity: {} identical, {} differs, {} unavailable of {} measured lanes.\n{}\na: {}\nb: {}\n",
        report.identical,
        report.differs,
        report.unavailable,
        report.lanes.len(),
        report.scope,
        report.a.display(),
        report.b.display()
    );
    for lane in &report.lanes {
        let acquisition = lane
            .acquisition
            .map(|n| format!("acquisition {n} "))
            .unwrap_or_default();
        let place = format!(
            "{acquisition}wave {} lane {} (cell {} trial {})",
            lane.wave, lane.lane, lane.cell, lane.trial
        );
        match &lane.outcome {
            Outcome::Identical { .. } => (),
            Outcome::Differs {
                channel,
                char_offset,
                byte_offset,
                a_excerpt,
                b_excerpt,
            } => text.push_str(&format!(
                "{place}: differs in {} at character {char_offset} (byte {byte_offset})\n  a: \"{}\"\n  b: \"{}\"\n",
                channel.as_str(),
                a_excerpt.escape_debug(),
                b_excerpt.escape_debug()
            )),
            Outcome::Unavailable { reasons } => text.push_str(&format!(
                "{place}: unavailable: {}\n",
                reasons.join("; ").escape_debug()
            )),
        }
    }
    text
}
