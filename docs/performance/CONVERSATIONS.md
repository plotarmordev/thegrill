# Bounded conversation checks

Use the normal capture workflow with the explicit selection:

```sh
grill-perf baseline --selection /absolute/path/to/conversation-selection-v2.json \
  --endpoint "$ENDPOINT" --model "$MODEL" --deployment "$DECLARATION" \
  --auth-env MODEL_API_TOKEN --out "$PRIVATE/baseline"
grill-perf check "$PRIVATE/baseline" --change none \
  --deployment "$DECLARATION" --out "$PRIVATE/control"
grill-perf compare "$PRIVATE/baseline" "$PRIVATE/control" --json
```

Keep `conversation-v2.json` alongside its selection manifest. Use `--local-http`
only for an explicitly allowed local HTTP fixture. A serving change is made by
the operator, never TheGrill; select its declared changed field for a candidate.
`check` inherits the pinned workload, controls and endpoint from the baseline.
No agent, cache reset, tool process or experiment coordinator is involved.

## Scope and budget

The pinned scenario has thirteen ordered C1 steps. Its first eleven retain the
original alpha/beta prime, reuse, alternation, edit/restore, fixed tool continuity
and short prime/reuse inputs and checks. After the short history is introduced,
`return-beta` observes cache reuse without requiring a hit; `recover-beta` is
parented to that actual return and requires a reported hit. Existing hit
requirements are not relaxed after an eviction.

Each capture contains eight whole-sequence acquisitions: **104 requests, at most
13,312 requested output tokens**, including all primes and follow-ups. There are
no hidden warmups or replacement requests. The default whole-capture deadline is
300 seconds, including preparation and native evidence publication; each request
is capped at 128 output tokens, 64 KiB response and 10 seconds. A candidate/control
has a separate equal allowance.

Workload v2 bounds a sequence to sixteen steps. Each encoded accumulated request
is limited to 128 KiB and 64 messages; retained histories have a 128 KiB bound per
step. A limit, interruption, missing required evidence or failed declared check
stops later admission and retains the partial sequence. Sequences cannot resume.
A new capture requires a new explicit allowance; failure never triggers one.

The explicit `vllm-conversation-v2` profile declares streaming at workload level.
Effective settings derive from each prospective expected response kind: factual
steps stream with usage requested; tool steps require a full nonstreaming
response. There is no independent per-step streaming knob and no streamed tool
assembly. Requests include the declared `enable_thinking=false` mapping without
retry or field stripping. The provider must support these controls, `cache_salt`,
full tool messages and the declared usage fields; TheGrill cannot attest that
the backend honored them.

Factual steps report client-observed first-generated-text, first-answer-text,
last-generated-text, terminal and settlement timing through the existing
collector. A delayed terminal is not the last answer arrival. Tool
first-output/first-answer latency and decode-only rates remain **unavailable**:
body or header arrival is not a generated-token observation.

The archived `conversation-v1.json`, its selection and `vllm-conversation-v1`
profile remain unchanged and nonstreaming. Their captures retain the original
eleven-step budget and unavailable first-text timing. Text-only profiles still
reject tools; the separate quality evaluator is unchanged.

## Prefix and lineage contract

Each step names a history and optionally an earlier parent in that same history.
Its messages append only declared user inputs to the parent's **actual retained
assistant output**, not an expected or repaired answer. Editing and restoring
parent the same pre-edit state; the restore does not inherit the edited sibling.
Independent histories use different namespace/history cache salts. A new native
acquisition gets a new namespace; no real server cache is flushed.

Root repeated-fill content is deterministic and has no per-attempt text salt.
Continuation steps cannot fill or request cold-zero semantics. Existing workload
v1 filled prompts retain their original salting behavior. Prefix-cache hits are
provider reports: priming does not prove reuse, and token block alignment,
eviction and backend replay can change observed cached tokens. A declared hit
requires a positive reported count; missing telemetry or a zero count cannot
pass that requirement. `observe` permits unknown telemetry without inventing a
hit. The original alternation still prospectively requires retention. A zero
count there fails cache eligibility and stops admission. Only the newly declared
`return-beta` uses `observe`; its warm recovery requires a positive count.

This is a **small bounded retention scenario**, not guaranteed real-backend
eviction or capacity pressure. The CPU fixture explicitly simulates **two
retained history slots**, least-recently-used replacement and one latest request
per retained history. It computes actual common byte prefixes within the matching
salt, maps four bytes to a synthetic token, and rounds down to sixteen-token
blocks. Introducing the short history evicts beta in this declared simulation;
beta's return misses and its parent-linked recovery hits. Those fixture rules are
not a product cache implementation, tokenizer model or claim about a serving
backend's retention policy. A real backend may retain beta and report a hit on
the observation step. No real cache is reset or filled until eviction occurs.

Tool support is deliberately one fixed `lookup_fact` function. A step declares
one expected key and local fixture result; no arbitrary function runs. The
response must contain one full call, the correct name and JSON arguments, a
bounded unique ID and `tool_calls` finish. The following request uses the actual
call/arguments/ID and the declared fixture result. Invalid or duplicate arguments
are rejected, never repaired. Every tool step requires a parent-linked factual
follow-up, so a tool call alone cannot claim continuity.

## Reading the evidence

`Attempt.sequence.correct` records factual/structural correctness.
`canonical_match` records whether factual JSON used its canonical spelling;
`strict_match` applies only where a strict output string was declared beforehand.
Equivalent JSON whitespace is semantically valid and the original bytes remain
in later history. A declared strict failure stops admission without relabeling
it as a factual error. Genuine stale facts and cross-history leakage fail.

Correctness, performance eligibility and comparison outcome are separate.
A semantically wrong response may have valid timing and usage; those observations
remain visible, but it cannot authorize subsequent steps or a successful capture.
Per-step native waves retain request bytes, raw output, parent/history identity,
completion/cache observations, errors and available provider metrics. Offline
replay reconstructs requests from actual parent responses and checks the same
lineage and semantics. Provider-wide snapshots, if collected through advanced
`run`, do not prove a per-step cause without label/reset/traffic accounting;
unsupported preemption telemetry remains unknown.

Streamed factual lineage concatenates only answer-content deltas accepted by the
existing semantic decoder and SSE parser through the exact accepted terminal
boundary. It preserves decoded content formatting, excludes reasoning and
post-terminal bytes, and bounds the reconstructed answer by the response ceiling.

Successful selected comparisons report `DESCRIPTIVE`, displayed as
`COMPLETE - DESCRIPTIVE ONLY`; this means required evidence and declared checks
completed, not a measured speedup or equivalence. Incomplete captures remain
inconclusive and invalid evidence remains invalid.

Selected captures remain descriptive per step/acquisition. They can show improved
long-history reuse alongside slower short follow-ups; no pooled improvement,
model-quality score, C1 confidence interval or production qualification follows.
The controlled CPU fixtures exercise protocol, linkage, simulated eviction,
recovery and failure handling—not real-model correctness or cache capacity.

Issue scope remains partial for first-output latency on tool responses,
guaranteed real-backend eviction/capacity pressure, live backend qualification,
and causal per-step attribution of provider-wide counters or preemptions.
Unsupported telemetry stays unknown. Live qualification needs its own finite
approved window. Raw requests, responses, declarations and endpoints remain
private; publish only a reviewed sanitized summary.
