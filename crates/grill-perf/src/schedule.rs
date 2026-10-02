use crate::model::*;
use serde::{Deserialize, Serialize};

/// Offsets are microseconds on the admitted scenario's single monotonic origin.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Arrival {
    FixedOffset { offset_us: u64 },
    AfterFirstGenerated { lane: String, offset_us: u64 },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Solo,
    Overlap,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub scenario: String,
    pub lane: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lane {
    pub id: String,
    pub case: String,
    pub arrival: Arrival,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub request: Option<RequestSettings>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub control: Option<Control>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub kind: Kind,
    pub warmup_trials: u32,
    pub trials: u32,
    pub lanes: Vec<Lane>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedLane {
    pub id: String,
    pub case: String,
    pub arrival: Arrival,
    pub request: RequestSettings,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub control: Option<Control>,
}

pub fn validate(workload: &Workload) -> Result<()> {
    let scenarios = workload
        .schedule
        .as_ref()
        .ok_or("workload4 requires schedule")?;
    if scenarios.is_empty() || scenarios.len() > 64 || !workload.cells.is_empty() {
        return Err("schedule requires 1..64 scenarios and an empty cells list".into());
    }
    let mut names = std::collections::HashSet::new();
    let mut attempts = 0u64;
    let mut repetitions = 0u64;
    for scenario in scenarios {
        if !identifier(&scenario.id)
            || !names.insert(&scenario.id)
            || scenario.trials == 0
            || scenario.trials > 100
            || scenario.warmup_trials > 20
            || scenario.lanes.is_empty()
            || scenario.lanes.len() > 64
            || (scenario.kind == Kind::Solo && scenario.lanes.len() != 1)
            || (scenario.kind == Kind::Overlap && scenario.lanes.len() < 2)
        {
            return Err("invalid scenario identity, kind, lanes or repetition bounds".into());
        }
        let mut lanes = std::collections::HashSet::new();
        let triggered = scenario
            .lanes
            .iter()
            .any(|l| matches!(l.arrival, Arrival::AfterFirstGenerated { .. }));
        let mut memory = 0usize;
        for (index, lane) in scenario.lanes.iter().enumerate() {
            if !identifier(&lane.id) || !lanes.insert(&lane.id) {
                return Err(
                    "lane IDs must be unique short ASCII identifiers within a scenario".into(),
                );
            }
            let case = workload
                .cases
                .iter()
                .find(|c| c.id == lane.case)
                .ok_or("unknown schedule case")?;
            let settings = lane.request.as_ref().unwrap_or(&workload.request);
            settings.validate()?;
            if !matches!(
                settings.profile,
                Profile::PortableChatV1 | Profile::VllmFixedV1
            ) || (triggered && !settings.stream)
                || (settings.cache == Cache::ReportedPrefixHit && scenario.warmup_trials == 0)
            {
                return Err("schedule requires flat profiles, streaming trigger scenarios and explicit hit priming".into());
            }
            let offset = match &lane.arrival {
                Arrival::FixedOffset { offset_us } => *offset_us,
                Arrival::AfterFirstGenerated {
                    lane: source,
                    offset_us,
                } => {
                    if !scenario.lanes[..index].iter().any(|l| &l.id == source) {
                        return Err("generated trigger must name an earlier declared lane".into());
                    }
                    *offset_us
                }
            };
            if offset >= u64::from(workload.limits.total_ms) * 1000 {
                return Err("arrival offset must precede the whole-scenario deadline".into());
            }
            match scenario.kind {
                Kind::Solo => {
                    if lane.control.is_some()
                        || lane.arrival != (Arrival::FixedOffset { offset_us: 0 })
                    {
                        return Err(
                            "solo requires fixed offset zero and no control reference".into()
                        );
                    }
                }
                Kind::Overlap => {
                    let control = lane
                        .control
                        .as_ref()
                        .ok_or("each overlap lane requires a matched solo")?;
                    let solo = scenarios
                        .iter()
                        .find(|s| s.id == control.scenario && s.kind == Kind::Solo)
                        .ok_or("control must name a solo in this workload")?;
                    let peer = solo
                        .lanes
                        .first()
                        .filter(|p| p.id == control.lane)
                        .ok_or("unknown control lane")?;
                    if peer.case != lane.case
                        || peer.request.as_ref().unwrap_or(&workload.request) != settings
                        || solo.trials != scenario.trials
                        || solo.warmup_trials != scenario.warmup_trials
                    {
                        return Err("solo control must match case, resolved request and repetitions exactly".into());
                    }
                }
            }
            let fill = case.fill.as_ref().map_or(0, Fill::bytes);
            let per_lane = if settings.stream {
                2 * workload.limits.response_bytes + 6 * FRAME_CAP + 512 * 1024
            } else {
                6 * workload.limits.response_bytes + 512 * 1024
            };
            memory = memory
                .checked_add(per_lane)
                .and_then(|n| n.checked_add(fill))
                .ok_or("schedule memory overflow")?;
        }
        if memory > workload.limits.wave_buffer_bytes {
            return Err(format!(
                "scenario {} requires wave_buffer_bytes >= {memory}",
                scenario.id
            ));
        }
        let n = u64::from(scenario.trials) + u64::from(scenario.warmup_trials);
        repetitions += n;
        attempts += n * scenario.lanes.len() as u64;
    }
    if attempts > MAX_ATTEMPTS || repetitions > 1024 {
        return Err("schedule exceeds 10,000 attempts or 1,024 scenarios including warmups".into());
    }
    Ok(())
}

pub fn waves(workload: &Workload) -> Vec<WaveSpec> {
    let mut waves = Vec::new();
    for phase in [Phase::Warmup, Phase::Measured] {
        for scenario in workload.schedule.iter().flatten() {
            let count = if phase == Phase::Warmup {
                scenario.warmup_trials
            } else {
                scenario.trials
            };
            for trial in 0..count {
                waves.push(WaveSpec {
                    index: waves.len() as u32,
                    phase,
                    cell: scenario.id.clone(),
                    case: None,
                    trial,
                    concurrency: scenario.lanes.len() as u32,
                    acquisition: None,
                    lanes: Some(
                        scenario
                            .lanes
                            .iter()
                            .map(|lane| ResolvedLane {
                                id: lane.id.clone(),
                                case: lane.case.clone(),
                                arrival: lane.arrival.clone(),
                                request: lane.request.as_ref().unwrap_or(&workload.request).clone(),
                                control: lane.control.clone(),
                            })
                            .collect(),
                    ),
                });
            }
        }
    }
    waves
}

/// Full lane replacement first, then the existing phase-output selection.
pub fn settings(workload: &Workload, spec: &WaveSpec, lane: u32) -> Result<RequestSettings> {
    let Some(lanes) = &spec.lanes else {
        return Ok(crate::sequence::settings(workload, spec));
    };
    let request = &lanes
        .get(lane as usize)
        .ok_or("unknown scheduled lane")?
        .request;
    let mut effective = request.clone();
    effective.output = request.effective_output(spec.phase).clone();
    effective.warmup_output = None;
    Ok(effective)
}

pub fn case(spec: &WaveSpec, lane: u32) -> Result<&str> {
    match &spec.lanes {
        Some(lanes) => lanes
            .get(lane as usize)
            .map(|l| l.case.as_str())
            .ok_or_else(|| "unknown scheduled lane".into()),
        None => spec
            .case
            .as_deref()
            .ok_or_else(|| "missing flat wave case".into()),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    MissingTrigger,
    SourceSettled,
    LaneFailed,
    Cancelled,
    Deadline,
    NotificationFailed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LaneObservation {
    pub id: String,
    pub trigger_offset_us: Option<u64>,
    pub not_dispatched: Option<Reason>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Overlap {
    pub left: String,
    pub right: String,
    pub request_inflight: bool,
    pub generated_text: bool,
    pub left_decode_right_prefill: bool,
    pub right_decode_left_prefill: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub lanes: Vec<LaneObservation>,
    pub overlap: Vec<Overlap>,
    pub fatal: Option<Reason>,
    pub settled_offset_us: u64,
}

pub fn observation(
    spec: &WaveSpec,
    attempts: &[Attempt],
    not_dispatched: &[Option<Reason>],
    fatal: Option<Reason>,
    settled_offset_us: u64,
) -> Result<Observation> {
    let lanes = spec
        .lanes
        .as_ref()
        .ok_or("schedule observation requires resolved lanes")?;
    if attempts.len() != lanes.len() || not_dispatched.len() != lanes.len() {
        return Err("schedule lane cardinality mismatch".into());
    }
    let mut observations = Vec::with_capacity(lanes.len());
    let mut overlap = Vec::with_capacity(lanes.len() * lanes.len().saturating_sub(1) / 2);
    for (index, lane) in lanes.iter().enumerate() {
        let trigger_offset_us = match &lane.arrival {
            Arrival::FixedOffset { .. } => None,
            Arrival::AfterFirstGenerated { lane, .. } => {
                let source = lanes
                    .iter()
                    .position(|l| &l.id == lane)
                    .ok_or("unknown trigger lane")?;
                attempts[source]
                    .timing
                    .first_generated_text_us
                    .map(|first| add(attempts[source].timing.dispatch_offset_us, first))
                    .transpose()?
            }
        };
        observations.push(LaneObservation {
            id: lane.id.clone(),
            trigger_offset_us,
            not_dispatched: not_dispatched[index],
        });
        for right in index + 1..lanes.len() {
            overlap.push(Overlap {
                left: lane.id.clone(),
                right: lanes[right].id.clone(),
                request_inflight: intersects(
                    inflight(&attempts[index])?,
                    inflight(&attempts[right])?,
                ),
                generated_text: intersects(
                    generated(&attempts[index])?,
                    generated(&attempts[right])?,
                ),
                left_decode_right_prefill: intersects(
                    generated(&attempts[index])?,
                    prefill(&attempts[right])?,
                ),
                right_decode_left_prefill: intersects(
                    generated(&attempts[right])?,
                    prefill(&attempts[index])?,
                ),
            });
        }
    }
    Ok(Observation {
        lanes: observations,
        overlap,
        fatal,
        settled_offset_us,
    })
}

pub fn verify(
    spec: &WaveSpec,
    attempts: &[Attempt],
    observed: &Observation,
    total_ms: u32,
) -> Result<()> {
    let lanes = spec.lanes.as_ref().ok_or("missing schedule lanes")?;
    let reasons: Vec<_> = observed.lanes.iter().map(|l| l.not_dispatched).collect();
    if observation(
        spec,
        attempts,
        &reasons,
        observed.fatal,
        observed.settled_offset_us,
    )? != *observed
    {
        return Err("schedule observations differ from retained timing evidence".into());
    }
    bounds(attempts.iter(), Some(observed.settled_offset_us))?;
    if observed.fatal.is_none() && observed.settled_offset_us >= u64::from(total_ms) * 1000 {
        return Err("successful scenario barrier exceeds its total deadline".into());
    }
    for (index, lane) in lanes.iter().enumerate() {
        let a = &attempts[index];
        if a.dispatched != reasons[index].is_none() || (!a.dispatched && observed.fatal.is_none()) {
            return Err("schedule dispatch status contradicts retained failure".into());
        }
        if reasons[index].is_some() && reasons[index] != observed.fatal {
            return Err("undispatched lane reason differs from the scenario fatal outcome".into());
        }
        if a.status == Status::Complete
            && add(a.timing.dispatch_offset_us, a.timing.settle_us)? >= u64::from(total_ms) * 1000
        {
            return Err("completed lane exceeds the whole-scenario deadline".into());
        }
        let due = match &lane.arrival {
            Arrival::FixedOffset { offset_us } => Some(*offset_us),
            Arrival::AfterFirstGenerated {
                lane: source,
                offset_us,
            } => {
                let source = lanes
                    .iter()
                    .position(|l| &l.id == source)
                    .ok_or("unknown trigger")?;
                if a.dispatched
                    && a.timing.dispatch_offset_us
                        >= add(
                            attempts[source].timing.dispatch_offset_us,
                            attempts[source].timing.settle_us,
                        )?
                {
                    return Err("dependent dispatched after its source settled".into());
                }
                observed.lanes[index]
                    .trigger_offset_us
                    .map(|trigger| add(trigger, *offset_us))
                    .transpose()?
            }
        };
        if a.dispatched
            && (due.is_none_or(|due| a.timing.dispatch_offset_us < due)
                || a.timing.dispatch_offset_us >= u64::from(total_ms) * 1000)
        {
            return Err("dispatch violates intended arrival or scenario deadline".into());
        }
        if !a.dispatched
            && (a.timing.dispatch_offset_us != 0
                || a.timing.first_generated_text_us.is_some()
                || a.timing.settle_us != 0
                || a.response_bytes != 0
                || a.usage != Usage::default()
                || a.finish_reason.is_some()
                || a.surplus_observed_bytes != 0)
        {
            return Err("undispatched lane contains service observations".into());
        }
    }
    if observed.fatal.is_none()
        && attempts
            .iter()
            .any(|a| a.status != Status::Complete || !a.eligibility_errors.is_empty())
    {
        return Err("failed schedule lacks fatal outcome".into());
    }
    if observed.fatal == Some(Reason::MissingTrigger)
        && !lanes.iter().enumerate().any(|(i, l)| {
            !attempts[i].dispatched
                && matches!(l.arrival, Arrival::AfterFirstGenerated { .. })
                && observed.lanes[i].trigger_offset_us.is_none()
        })
    {
        return Err("missing-trigger failure has no missing dependent trigger".into());
    }
    if observed.fatal == Some(Reason::SourceSettled)
        && !lanes.iter().enumerate().any(|(i, l)| {
            !attempts[i].dispatched
                && matches!(l.arrival, Arrival::AfterFirstGenerated { .. })
                && observed.lanes[i].trigger_offset_us.is_some()
        })
    {
        return Err("source-settled failure has no undispatched triggered dependent".into());
    }
    Ok(())
}

/// Finite admission state only; transport remains wire::collect in run's existing collector.
pub(crate) struct State<'a> {
    lanes: &'a [ResolvedLane],
    started: Vec<bool>,
    settled: Vec<bool>,
    first: Vec<Option<u64>>,
    pub fatal: Option<Reason>,
}

impl<'a> State<'a> {
    pub fn new(lanes: &'a [ResolvedLane]) -> Self {
        Self {
            lanes,
            started: vec![false; lanes.len()],
            settled: vec![false; lanes.len()],
            first: vec![None; lanes.len()],
            fatal: None,
        }
    }

    pub fn fail(&mut self, reason: Reason) {
        self.fatal.get_or_insert(reason);
    }

    pub fn notify(&mut self, event: FirstGenerated) {
        let lane = event.lane as usize;
        if lane >= self.lanes.len() || !self.started[lane] || self.first[lane].is_some() {
            self.fail(Reason::NotificationFailed);
        } else {
            self.first[lane] = Some(event.offset_us);
        }
    }

    pub fn settle(&mut self, a: &Attempt, settings: &RequestSettings, phase: Phase) {
        let lane = a.lane as usize;
        self.settled[lane] = true;
        let first = a
            .timing
            .first_generated_text_us
            .and_then(|first| a.timing.dispatch_offset_us.checked_add(first));
        if first != self.first[lane] {
            self.fail(Reason::NotificationFailed);
        }
        // Missing promised text is a distinct retained scheduling failure, even
        // when the provider's textless completion is itself unsupported.
        if self.lanes.iter().enumerate().any(|(i, l)| !self.started[i]
            && matches!(&l.arrival, Arrival::AfterFirstGenerated { lane: source, .. } if source == &self.lanes[lane].id))
        {
            self.fail(if first.is_none() { Reason::MissingTrigger } else { Reason::SourceSettled });
        }
        if a.status != Status::Complete || !eligibility(a, settings, phase).is_empty() {
            self.fail(if a.status == Status::Interrupted {
                Reason::Cancelled
            } else {
                Reason::LaneFailed
            });
        }
    }

    pub fn pending(&self) -> bool {
        self.started.iter().any(|started| !started)
    }

    pub fn due(&self, index: usize) -> Option<u64> {
        match &self.lanes[index].arrival {
            Arrival::FixedOffset { offset_us } => Some(*offset_us),
            Arrival::AfterFirstGenerated { lane, offset_us } => {
                let source = self.lanes.iter().position(|l| &l.id == lane)?;
                self.first[source]?.checked_add(*offset_us)
            }
        }
    }

    pub fn next_due(&self) -> Option<u64> {
        if self.fatal.is_some() {
            return None;
        }
        (0..self.lanes.len())
            .filter(|&i| !self.started[i])
            .filter_map(|i| self.due(i))
            .min()
    }

    pub fn ready(&mut self, now: u64) -> Option<usize> {
        if self.fatal.is_some() {
            return None;
        }
        for i in 0..self.lanes.len() {
            if self.started[i] {
                continue;
            }
            if let Arrival::AfterFirstGenerated { lane, offset_us } = &self.lanes[i].arrival {
                let source = self
                    .lanes
                    .iter()
                    .position(|l| &l.id == lane)
                    .expect("validated earlier lane");
                if self.settled[source] {
                    self.fail(if self.first[source].is_some() {
                        Reason::SourceSettled
                    } else {
                        Reason::MissingTrigger
                    });
                    return None;
                }
                if self.first[source].is_some_and(|first| first.checked_add(*offset_us).is_none()) {
                    self.fail(Reason::NotificationFailed);
                    return None;
                }
            }
            if self.due(i).is_some_and(|due| due <= now) {
                self.started[i] = true;
                return Some(i);
            }
        }
        None
    }
}

pub(crate) fn undispatched(lane: u32, reason: Reason) -> crate::wire::Collected {
    crate::wire::Collected {
        attempt: Attempt {
            lane,
            dispatched: false,
            status: Status::Interrupted,
            detail: format!("schedule admission stopped: {reason:?}"),
            http_status: None,
            finish_reason: None,
            usage: Usage::default(),
            timing: Timing::default(),
            response_bytes: 0,
            response_sha256: String::new(),
            terminal_offset: None,
            surplus_observed_bytes: 0,
            eligibility_errors: Vec::new(),
            sequence: None,
        },
        body: Vec::new(),
    }
}

pub(crate) fn bounds<'a>(
    attempts: impl Iterator<Item = &'a Attempt>,
    barrier: Option<u64>,
) -> Result<(u64, u64)> {
    let (mut first, mut last, mut last_dispatch) = (u64::MAX, 0, 0);
    for a in attempts {
        if barrier.is_some() && !a.dispatched {
            continue;
        }
        first = first.min(a.timing.dispatch_offset_us);
        last = last.max(add(a.timing.dispatch_offset_us, a.timing.settle_us)?);
        last_dispatch = last_dispatch.max(a.timing.dispatch_offset_us);
    }
    let first = if first == u64::MAX { 0 } else { first };
    if barrier.is_some_and(|end| end < last) {
        return Err("scenario barrier precedes lane settlement".into());
    }
    Ok((barrier.unwrap_or(last - first), last_dispatch - first))
}

pub fn throughput(
    attempts: &[Attempt],
    elapsed_us: u64,
    schedule: Option<&Observation>,
) -> (bool, Option<u64>, Option<f64>) {
    if schedule.is_some_and(|s| s.fatal.is_some()) {
        (false, None, None)
    } else {
        crate::evidence::throughput(attempts, elapsed_us)
    }
}

#[derive(Serialize)]
pub struct LaneSample {
    pub state: String,
    pub dispatched: bool,
    pub dispatch_offset_us: Option<u64>,
    pub first_generated_text_us: Option<u64>,
    pub first_answer_text_us: Option<u64>,
    pub last_generated_text_us: Option<u64>,
    pub completion_latency_us: Option<u64>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub eligibility_errors: Vec<String>,
    pub matched_solo_complete: bool,
}

#[derive(Serialize)]
pub struct LaneReport {
    pub lane: String,
    pub case: String,
    pub request: RequestSettings,
    pub control: Option<Control>,
    pub repetitions: Vec<LaneSample>,
}

#[derive(Serialize)]
pub struct ScenarioReport {
    pub scenario: String,
    pub kind: Kind,
    pub warmup_states: Vec<String>,
    pub lanes: Vec<LaneReport>,
    pub elapsed_us: Vec<Option<u64>>,
    pub complete_schedule_completion_tokens_per_second: Vec<Option<f64>>,
    pub observations: Vec<Option<Observation>>,
}

#[derive(Serialize)]
pub struct Comparison {
    pub scope: &'static str,
    pub complete_eligible: bool,
    pub baseline: Vec<ScenarioReport>,
    pub candidate: Vec<ScenarioReport>,
    pub reference: Option<Vec<ScenarioReport>>,
}

pub fn complete_eligible(run: &crate::evidence::Loaded) -> bool {
    run.history.count == 1
        && !run.history.open
        && run
            .waves
            .iter()
            .all(|wave| wave.as_ref().is_some_and(|wave| wave.eligible))
}

pub fn summarize(run: &crate::evidence::Loaded) -> Vec<ScenarioReport> {
    let pairs = |id: &str, phase| {
        run.plan
            .waves
            .iter()
            .zip(&run.waves)
            .filter(|(spec, _)| spec.cell == id && spec.phase == phase)
            .map(|(_, wave)| wave.as_ref())
            .collect::<Vec<_>>()
    };
    run.plan
        .workload
        .schedule
        .iter()
        .flatten()
        .map(|scenario| {
            let measured = pairs(&scenario.id, Phase::Measured);
            let warmups = pairs(&scenario.id, Phase::Warmup);
            ScenarioReport {
                scenario: scenario.id.clone(),
                kind: scenario.kind,
                warmup_states: warmups
                    .iter()
                    .map(|wave| {
                        match wave {
                            None => "missing",
                            Some(w) if w.eligible => "complete",
                            Some(_) => "ineligible",
                        }
                        .into()
                    })
                    .collect(),
                lanes: scenario
                    .lanes
                    .iter()
                    .enumerate()
                    .map(|(index, lane)| {
                        let solo = lane
                            .control
                            .as_ref()
                            .map(|c| pairs(&c.scenario, Phase::Measured));
                        let solo_warmups_complete = lane.control.as_ref().is_none_or(|c| {
                            pairs(&c.scenario, Phase::Warmup)
                                .iter()
                                .all(|w| w.is_some_and(|w| w.eligible))
                        });
                        LaneReport {
                            lane: lane.id.clone(),
                            case: lane.case.clone(),
                            request: lane
                                .request
                                .as_ref()
                                .unwrap_or(&run.plan.workload.request)
                                .clone(),
                            control: lane.control.clone(),
                            repetitions: measured
                                .iter()
                                .enumerate()
                                .map(|(trial, wave)| {
                                    let a = wave.map(|w| &w.attempts[index]);
                                    LaneSample {
                                        state: a.map_or("missing".into(), |a| {
                                            format!("{:?}", a.status)
                                        }),
                                        dispatched: a.is_some_and(|a| a.dispatched),
                                        dispatch_offset_us: a
                                            .filter(|a| a.dispatched)
                                            .map(|a| a.timing.dispatch_offset_us),
                                        first_generated_text_us: a
                                            .and_then(|a| a.timing.first_generated_text_us),
                                        first_answer_text_us: a
                                            .and_then(|a| a.timing.first_answer_text_us),
                                        last_generated_text_us: a
                                            .and_then(|a| a.timing.last_generated_text_us),
                                        completion_latency_us: a
                                            .filter(|a| a.status == Status::Complete)
                                            .map(|a| a.timing.settle_us),
                                        prompt_tokens: a.and_then(|a| a.usage.prompt_tokens),
                                        completion_tokens: a
                                            .and_then(|a| a.usage.completion_tokens),
                                        eligibility_errors: a.map_or_else(
                                            || vec!["missing_attempt".into()],
                                            |a| a.eligibility_errors.clone(),
                                        ),
                                        matched_solo_complete: solo.as_ref().is_none_or(|s| {
                                            s.get(trial)
                                                .is_some_and(|s| s.is_some_and(|w| w.eligible))
                                        }) && solo_warmups_complete,
                                    }
                                })
                                .collect(),
                        }
                    })
                    .collect(),
                elapsed_us: measured.iter().map(|w| w.map(|w| w.elapsed_us)).collect(),
                complete_schedule_completion_tokens_per_second: measured
                    .iter()
                    .map(|w| w.and_then(|w| w.achieved_completion_tokens_per_second))
                    .collect(),
                observations: measured
                    .iter()
                    .map(|w| w.and_then(|w| w.schedule.clone()))
                    .collect(),
            }
        })
        .collect()
}
/// One bounded notification per admitted lane; never a per-delta queue.
#[derive(Clone, Copy, Debug)]
pub struct FirstGenerated {
    pub lane: u32,
    pub offset_us: u64,
}

fn add(dispatch: u64, event: u64) -> Result<u64> {
    dispatch
        .checked_add(event)
        .ok_or_else(|| "schedule timestamp overflow".into())
}

fn intersects(a: Option<(u64, u64)>, b: Option<(u64, u64)>) -> bool {
    matches!((a, b), (Some((a0, a1)), Some((b0, b1))) if a0.max(b0) < a1.min(b1))
}

fn inflight(a: &Attempt) -> Result<Option<(u64, u64)>> {
    if !a.dispatched {
        return Ok(None);
    }
    let start = a.timing.dispatch_offset_us;
    Ok(Some((start, add(start, a.timing.settle_us)?)))
}

fn generated(a: &Attempt) -> Result<Option<(u64, u64)>> {
    if !a.dispatched {
        return Ok(None);
    }
    match (
        a.timing.first_generated_text_us,
        a.timing.last_generated_text_us,
    ) {
        (Some(first), Some(last)) => Ok(Some((
            add(a.timing.dispatch_offset_us, first)?,
            add(a.timing.dispatch_offset_us, last)?,
        ))),
        _ => Ok(None),
    }
}

fn prefill(a: &Attempt) -> Result<Option<(u64, u64)>> {
    if !a.dispatched {
        return Ok(None);
    }
    a.timing
        .first_generated_text_us
        .map(|first| {
            Ok((
                a.timing.dispatch_offset_us,
                add(a.timing.dispatch_offset_us, first)?,
            ))
        })
        .transpose()
}

#[cfg(test)]
#[path = "../tests/support/schedule_model.rs"]
mod tests;
