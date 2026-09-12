use crate::model::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::rc::Rc;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub history: String,
    pub parent: Option<String>,
    pub cache: Cache,
    pub expect: Expected,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    fact: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expected {
    Json { value: Fact, strict: Option<String> },
    Tool { key: String, result: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub history: String,
    pub parent: Option<String>,
    pub correct: bool,
    pub canonical_match: Option<bool>,
    pub strict_match: Option<bool>,
    pub error: Option<String>,
}

pub fn passed(check: &Check) -> bool {
    check.correct && check.strict_match != Some(false) && check.error.is_none()
}

pub fn validate(workload: &Workload) -> Result<()> {
    let sequence = workload.version == 2;
    if !sequence {
        if workload.cases.iter().any(|case| case.step.is_some())
            || workload.request.profile == Profile::VllmConversationV2
        {
            return Err(
                "sequence steps and conversation profile require workload version 2".into(),
            );
        }
        return Ok(());
    }
    if workload.request.profile != Profile::VllmConversationV2
        || !workload.request.stream
        || workload.request.output.mode != OutputMode::Cap
        || workload.request.cache != Cache::Observe
        || workload.cases.len() > 16
        || workload.cells.len() != workload.cases.len()
        || workload.limits.response_bytes > 64 * 1024
    {
        return Err("workload v2 requires bounded vllm-conversation-v2 factual streaming, capped output, per-step cache declarations and at most 16 ordered C1 steps".into());
    }
    for (index, (case, cell)) in workload.cases.iter().zip(&workload.cells).enumerate() {
        let step = case.step.as_ref().ok_or("every v2 case requires a step")?;
        if !identifier(&step.history)
            || cell.case != case.id
            || cell.concurrency != 1
            || cell.trials != 1
            || cell.warmup_trials != 0
            || case
                .messages
                .iter()
                .any(|m| m.role != Role::User && m.role != Role::System)
            || case.messages.iter().any(|m| m.content.contains("{salt}"))
        {
            return Err("sequence requires ordered single-use C1 cases, declared history and user/system inputs without attempt salt".into());
        }
        if let Some(parent) = &step.parent {
            let prior = workload.cases[..index]
                .iter()
                .find(|prior| &prior.id == parent)
                .ok_or("sequence parent must reference an earlier step")?;
            if prior.step.as_ref().map(|step| &step.history) != Some(&step.history)
                || case.messages.iter().any(|m| m.role != Role::User)
                || step.cache == Cache::ReportedPrefixZero
                || case.fill.is_some()
            {
                return Err("sequence parent must belong to the same history; continuations append user messages only".into());
            }
        }
        if step.cache == Cache::ReportedPrefixHit
            && !workload.cases[..index].iter().any(|prior| {
                prior
                    .step
                    .as_ref()
                    .is_some_and(|s| s.history == step.history)
            })
        {
            return Err(
                "required prefix hit needs an earlier explicit prime in that history".into(),
            );
        }
        match &step.expect {
            Expected::Json { value, strict } => {
                if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > 4096 {
                    return Err("sequence expected JSON must be a bounded object".into());
                }
                if let Some(text) = strict
                    && (text.len() > 4096
                        || serde_json::from_str::<Fact>(text).ok().as_ref() != Some(value))
                {
                    return Err("strict expected text must encode the declared JSON object".into());
                }
            }
            Expected::Tool { key, result } => {
                if !identifier(key) || result.len() > 4096 {
                    return Err("fixed lookup_fact fixture exceeds key/result bounds".into());
                }
                if !workload.cases[index + 1..].iter().any(|child| {
                    child.step.as_ref().is_some_and(|next| {
                        next.parent.as_deref() == Some(&case.id)
                            && matches!(next.expect, Expected::Json { .. })
                    })
                }) {
                    return Err("tool fixture requires a later factual follow-up parented to its actual call".into());
                }
            }
        }
    }
    Ok(())
}

pub fn settings(workload: &Workload, spec: &WaveSpec) -> RequestSettings {
    let mut settings = workload.request.clone();
    if let Some(step) = workload
        .cases
        .iter()
        .find(|c| c.id == spec.case)
        .and_then(|c| c.step.as_ref())
    {
        settings.cache = step.cache;
        settings.stream = matches!(step.expect, Expected::Json { .. });
    }
    settings
}

struct Messages<'a>(&'a [Rc<Value>]);
impl Serialize for Messages<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(Rc::as_ref))
    }
}

#[derive(Serialize)]
struct HistoryRequest<'a> {
    #[serde(flatten)]
    body: Value,
    messages: Messages<'a>,
}

#[derive(Default)]
pub struct State {
    histories: Vec<(String, Vec<Rc<Value>>)>,
    pending: Option<(String, Vec<Rc<Value>>)>,
    next_wave: u32,
    stopped: bool,
}

impl State {
    pub fn request(&mut self, plan: &Plan, spec: &WaveSpec, lane: u32) -> Result<String> {
        if plan.workload.version != 2 {
            return crate::wire::request_body(plan, spec, lane);
        }
        if self.stopped {
            return Err("sequence cannot admit a step after invalid or incomplete evidence".into());
        }
        if spec.index != self.next_wave || self.pending.is_some() || lane != 0 {
            return Err(
                "sequence admission must follow each settled declared step exactly once".into(),
            );
        }
        let case = plan
            .workload
            .cases
            .iter()
            .find(|c| c.id == spec.case)
            .ok_or("unknown sequence case")?;
        let step = case.step.as_ref().ok_or("missing step")?;
        let mut body: Value = serde_json::from_str(&crate::wire::request_body(plan, spec, lane)?)
            .map_err(|e| e.to_string())?;
        let inputs = body
            .as_object_mut()
            .and_then(|body| body.remove("messages"))
            .ok_or("missing sequence inputs")?;
        let Value::Array(inputs) = inputs else {
            return Err("sequence inputs must be messages".into());
        };
        let mut messages = if let Some(parent) = &step.parent {
            self.histories
                .iter()
                .find(|(id, _)| id == parent)
                .map(|(_, messages)| messages.clone())
                .ok_or("sequence parent lacks valid retained output")?
        } else {
            Vec::new()
        };
        messages.extend(inputs.into_iter().map(Rc::new));
        if messages.len() > 64 {
            return Err("sequence accumulated history exceeds 64 messages".into());
        }
        let namespace = plan
            .cache_namespace
            .as_deref()
            .ok_or("sequence needs a private cache namespace")?;
        body["cache_salt"] = Value::String(format!("{namespace}-{}", step.history));
        if let Expected::Tool { .. } = step.expect {
            body["tools"] = json!([{"type":"function","function":{"name":"lookup_fact","description":"Return the declared local fixture value for one key.","parameters":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}}}]);
            body["tool_choice"] = json!({"type":"function","function":{"name":"lookup_fact"}});
        }
        let encoded = serde_json::to_string(&HistoryRequest {
            body,
            messages: Messages(&messages),
        })
        .map_err(|e| e.to_string())?;
        if encoded.len() > 128 * 1024 {
            return Err("sequence accumulated input exceeds 128 KiB".into());
        }
        self.pending = Some((case.id.clone(), messages));
        Ok(encoded)
    }

    pub fn observe(
        &mut self,
        plan: &Plan,
        spec: &WaveSpec,
        attempt: &Attempt,
        body: &[u8],
    ) -> Result<Option<Check>> {
        if plan.workload.version != 2 {
            return Ok(None);
        }
        let case = plan
            .workload
            .cases
            .iter()
            .find(|c| c.id == spec.case)
            .ok_or("unknown sequence case")?;
        let step = case.step.as_ref().ok_or("missing step")?;
        let (pending_case, mut messages) = self
            .pending
            .take()
            .ok_or("sequence response lacks admitted history")?;
        if pending_case != case.id || spec.index != self.next_wave {
            return Err("sequence response differs from its admitted step".into());
        }
        self.next_wave += 1;
        let mut check = Check {
            history: step.history.clone(),
            parent: step.parent.clone(),
            correct: false,
            canonical_match: None,
            strict_match: None,
            error: None,
        };
        let observed = (|| {
            if attempt.status != Status::Complete {
                return Err("sequence response did not complete".to_string());
            }
            let message = if settings(&plan.workload, spec).stream {
                let content = crate::wire::sequence_answer(attempt, body, plan.version == 3)?;
                json!({"role":"assistant","content":content})
            } else {
                let mut response: Value =
                    serde_json::from_slice(body).map_err(|_| "sequence response is not JSON")?;
                response
                    .pointer_mut("/choices/0/message")
                    .ok_or("sequence response lacks message")?
                    .take()
            };
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                return Err("sequence response role must be assistant".into());
            }
            match &step.expect {
                Expected::Json { value, strict } => {
                    if message.get("tool_calls").is_some_and(|v| !v.is_null()) {
                        return Err("unexpected tool call in factual response".into());
                    }
                    let content = message
                        .get("content")
                        .and_then(Value::as_str)
                        .ok_or("factual response requires text")?;
                    let parsed: Fact = serde_json::from_str(content).map_err(|_| "factual response requires one fact string without duplicate or unknown fields")?;
                    check.strict_match = strict.as_ref().map(|expected| content == expected);
                    check.canonical_match =
                        Some(content == serde_json::to_string(value).map_err(|e| e.to_string())?);
                    check.correct = &parsed == value;
                    if &parsed != value {
                        return Err(
                            "factual response differs from prospective expected object".into()
                        );
                    }
                    if check.strict_match == Some(false) {
                        return Err(
                            "declared strict-output check failed; semantic JSON matched".into()
                        );
                    }
                    messages.push(Rc::new(json!({"role":"assistant","content":content})));
                }
                Expected::Tool { key, result } => {
                    if attempt.finish_reason.as_deref() != Some("tool_calls") {
                        return Err("tool response requires tool_calls finish reason".into());
                    }
                    let calls = message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .ok_or("expected lookup_fact tool call")?;
                    if calls.len() != 1
                        || message
                            .get("content")
                            .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
                    {
                        return Err("tool step requires exactly one call and no answer text".into());
                    }
                    let call = &calls[0];
                    let id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| identifier(id))
                        .ok_or("invalid tool call ID")?;
                    if call.get("type").and_then(Value::as_str) != Some("function")
                        || call.pointer("/function/name").and_then(Value::as_str)
                            != Some("lookup_fact")
                    {
                        return Err("unexpected tool type or name".into());
                    }
                    let raw = call
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .ok_or("tool arguments must be encoded JSON")?;
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Arguments {
                        key: String,
                    }
                    let args: Arguments = serde_json::from_str(raw)
                        .map_err(|_| "malformed, duplicate or unknown tool arguments")?;
                    if &args.key != key {
                        return Err("tool arguments differ from declared fixture key".into());
                    }
                    if messages.iter().any(|m| {
                        m.get("tool_calls")
                            .and_then(Value::as_array)
                            .is_some_and(|calls| {
                                calls
                                    .iter()
                                    .any(|c| c.get("id").and_then(Value::as_str) == Some(id))
                            })
                    }) {
                        return Err("tool call ID reused within history".into());
                    }
                    messages.push(Rc::new(
                        json!({"role":"assistant","content":null,"tool_calls":calls}),
                    ));
                    messages.push(Rc::new(
                        json!({"role":"tool","tool_call_id":id,"content":result}),
                    ));
                    check.correct = true;
                }
            }
            if messages.len() > 64
                || serde_json::to_vec(&Messages(&messages))
                    .map_err(|e| e.to_string())?
                    .len()
                    > 128 * 1024
            {
                return Err("retained sequence history exceeds 128 KiB".into());
            }
            Ok(messages)
        })();
        match observed {
            Ok(history) => {
                check.correct = true;
                if eligibility(attempt, &settings(&plan.workload, spec), spec.phase).is_empty() {
                    self.histories.push((case.id.clone(), history));
                } else {
                    self.stopped = true;
                }
            }
            Err(error) => {
                check.error = Some(error);
                self.stopped = true;
            }
        }
        Ok(Some(check))
    }
}
