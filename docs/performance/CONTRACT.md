# Performance measurement contract

Existing workloads and wave receipts retain v1 meaning. Execution plans are v3, declaring
`metric_contract: "generated-text-arrival-v2"` and retaining last generated-text
and terminal observations. Execution-session provenance remains mandatory.
Legacy v1/v2 plans stay readable with their original timing semantics and no
invented timestamps. Legacy v1 plans cannot be paused or resumed.
Cross-contract raw comparisons are rejected, not silently harmonized.

Plans may additionally bind `policy.json` through optional `policy_sha256`.
The field is omitted when unused; existing unbound plans retain their bytes.
Policy-bearing plans require a reader that supports this declaration.

The separate [baseline/check contract](README.md#baseline-change-check) wraps
verified native runs in versioned capture manifests. Its capture-period
assessment is not the captured observed-envelope policy below. Historical
policies, gates and verdicts retain their meanings.

Explicit [selected captures](README.md#explicit-selected-captures) pin their
workload/control selection and include the final timing receipt in comparison
identity. They do not inherit structured-C1 inference. New captures use
[capture v3](#capture-v3-and-deployment-comparison); earlier built-in captures
are v1 and earlier selected captures v2.
[Conversation workload v2](CONVERSATIONS.md) adds bounded ordered case steps and
the sole `vllm-conversation-v2` profile (streamed factual steps, nonstream tool
steps); wave receipts remain v1 with an
optional `Attempt.sequence` check omitted for ordinary workloads. Its stable
history salts and actual parent-output replay do not reinterpret v1 filled prompts.
Semantic/strict checks are separate from performance eligibility; either failed
admission requirement stops the sequence. Native per-cell observations from a
valid prefix remain descriptive, not qualification of a failed whole sequence.
Conversation workloads do not accept captured policies or continuation.

## Captured policy decisions

`run --policy FILE` captures and validates the same exact pre-dispatch policy
for baseline, candidate and repeat. It changes neither generation requests nor
ordinary measurement eligibility. Offline `decide` verifies the saved evidence
and policy bindings; it never applies a new policy to unbound historical runs.

Policy1 emits decision envelope version 1; explicit policy2 emits version 2.
These are separate from comparison JSON version 3.
PASS means complete observed evidence satisfies every required policy gate;
REGRESSION means a qualified observed adverse bound exceeds the declared
tolerance. INCONCLUSIVE preserves missing or insufficient evidence and excessive
reference variability; ERROR denotes invalid or unverifiable input. These are
descriptive policy outcomes, not significance, causality or universal guarantees.
An eligible comparison or successful process exit alone is not a decision.

The [policy guide](README.md#captured-observed-envelope-policy) defines the closed
schema, exact checked rational arithmetic, complete-coverage requirements,
reference-spread gate, outcome precedence and command-specific exits.

Policy2 (`observed-envelope-v2`) admits homogeneous flat workloads 1 and 3,
and explicit named-lane gates on workload4 schedules. It adds first-generated
text, first-answer text and client-settlement latencies, plus one fairness sample per
repetition. All are lower-better and use lane-relative service clocks. Fairness
requires every admitted lane complete and positive, with at least two lanes;
missing/zero peers never disappear from its population. Per-lane observations
do not enlarge the independent repeated-wave count. The
[policy2 definitions and limits](README.md#policy2-first-output-and-finite-fairness-gates)
specify exact rational units and coverage. Shared `envelope.rs` rejects zero
denominators, reversed bounds and checked-product overflow; floating diagnostics
do not select outcomes. Policy1 results, reason precedence and exits are
unchanged. Schedule gates additionally require complete matched solo controls
and actual required decode/prefill overlap; unlike-lane rates are not pooled.

Policy3 (`observed-envelope-v3`) is exclusive to workload6. It adds measured
step/tool gates, complete whole-conversation wall time and explicitly declared
empirical p95/p99 point gates. Required controls and warmups remain qualifying
evidence; missing acquisitions are not filtered into a favorable percentile.
The [acquisition contract](README.md#workload6-acquisitions-and-policy3) specifies
the independent population, clocks, finite history/input limits and minimum
200/1,000 complete acquisitions for p95/p99.

Prospective policy2/3 `required_telemetry` uses explicit metrics2 evidence.
Selector-bearing series retain their exact source/position identity, but
accounting consistency is checked on the corresponding engine/model identity,
without those selectors. Contradictory accounting withholds that identity's
required claims and does not contaminate a different engine/model. Missing or
incomplete sources are unavailable, never zero. Counter attribution still needs
the declared isolation contract; declaration is not authentication.

## Workload admission

Workload v1 retains its flat schedule and profiles. Workload v2 adds the explicit
conversation restrictions linked above rather than weakening v1 profiles.
Workload v3 adds phase-specific output budgets to the flat workload; it does not
admit conversation steps. These workload versions are separate from native plan
versions: a v3 workload requires a native version 3 plan.

The workload has `version`, `name`, `request`, `limits`, `cases` and `cells`.
See the complete [portable example](../../crates/grill-perf/examples/quick.json).
Unknown workload fields and invalid controls are rejected before dispatch.

`request` declares `profile`, `stream`, `output: {tokens, mode}`, `cache`, and
nullable `temperature_milli`, `top_p_milli`, `seed`. Thousandths are encoded as
decimal sampling values. Output mode `cap` sends `max_tokens`; `exact` also sends
`min_tokens` and `ignore_eos` and requires `vllm-fixed-v1`; `cap-reached` sends
only `max_tokens` under either flat profile and checks equal work from the
response instead (see [eligibility](#completion-failures-and-eligibility)).
Nullable `thinking` requires `vllm-fixed-v1` when declared
and is sent as `chat_template_kwargs.thinking`; null leaves the provider default.
Alternatively, nullable `thinking_control: {"kind":"vllm-enable-thinking-v1",
"enabled":false}` declares `chat_template_kwargs.enable_thinking` under the same
explicit profile, and `{"kind":"chat-template-thinking-v1","enabled":false}`
sends both `chat_template_kwargs.thinking` and `enable_thinking` with that value
under either flat profile; `enabled` accepts either boolean. Unknown kinds and nested
fields are rejected. Both controls cannot be non-null, even when their booleans
agree. No key is inferred from a model name.
Absent or null controls are omitted from normalized workloads and leave provider
defaults unchanged; legacy `thinking` request bytes remain unchanged.
`portable-chat-v1` rejects `thinking` and `vllm-enable-thinking-v1`. No generic `extra_body` or
other generation fields are sent or inferred.

These controls record requested behavior, not evidence that a template honored
it. Inspect reported reasoning tokens and observed generated/answer channels;
an answer-first event does not prove reasoning was absent elsewhere. A successful
response does not qualify a provider's template support. Only
`chat-template-thinking-v1` with `enabled: false` changes eligibility: a lane
whose usage reports reasoning tokens, or whose first generated event carries
reasoning text, is ineligible with `reasoning_reported_with_thinking_disabled`.
The check depends on the server reporting reasoning; servers without a reasoning
parser cannot be checked this way. Other thinking declarations do not change
eligibility or impose an answer-only requirement.
Streaming requests explicitly request usage with `stream_options.include_usage`.

Only workload v3 may declare `request.warmup_output: {tokens, mode}`. When absent,
`request.output` applies to both phases; when present, it overrides warmup only.
Explicit null is rejected, including in legacy versions. Both declared budgets
must satisfy token and profile bounds even when a phase has no planned requests.
An absent override is omitted from typed serialization, preserving old workload
identities. An explicit override participates in workload identity, native request
generation, usage eligibility and offline evidence verification. Different phase
controls are not compatible matched workloads.

Request/output ceilings are phase-weighted with checked arithmetic. Local
admission bounds both active, distinct phase encodings, including exact-output
extensions and reservation JSON escaping. Failure expectations use the failing
wave's budget. Full-capture minimum output rates require every planned phase to
fix its output count (`exact` or `cap-reached`); a capped phase has none.

Cases may declare `fill: {unit, repeat}`. The unit is nonempty and at most 64
bytes; repeat is 1..1,000,000. Such a case requires exactly one `{fill}` and at
most one `{salt}` across its message contents. Rendering replaces `{fill}` with
the repeated unit and `{salt}` with `namespace[..16]-wave.index-lane`.
Without `fill`, both placeholders are ordinary text. A random 64-hex
`cache_namespace` exists exactly when cache is not `observe` or any case has
fill; text salts differ per attempt even under `observe`.

Alternatively, `fill: {"kind":"generated-prose-v1","characters":16280}` selects
synthetic random-word prose. `characters` is positive and bounded by the request
byte cap; the same placeholder rules apply. The compiled ordered list contains
472 common English words authored for this generator, all ASCII. Each sentence
draws a length of 6 to 16 words inclusive, selects words with replacement, capitalizes its
first letter, and ends with a period. Words and sentences use single spaces;
the stream is truncated at exactly `characters`, possibly within its last word.
The word order and generator are immutable under this kind.

The seed is the first eight bytes, interpreted little-endian, of
`SHA256("grill-perf/generated-prose-v1\0" || text_salt)`, where `\0` denotes a
single zero byte and `text_salt` is the existing rendered salt above. SplitMix64
draws the sentence length first (`6 + draw % 11`), then each word
(`draw % word_count`). No new seed or wall-clock field is needed: saved
`cache_namespace`, wave index and lane regenerate the body even without a
`{salt}` placeholder. The generated alphabet needs no JSON escaping and each
character occupies one byte. Different request identities vary bodies, not
only headers; neither pseudorandom text nor a salted prefix proves a cold cache.
The old repeated-unit typed serialization, rendering and identities are unchanged.

Each cell names a case, concurrency, warmup trial count and measured trial count.
For a trial, each lane receives the same case with its attempt's text salt. When a seed is declared,
its lane seed is `seed + trial*64 + lane`. Warmup and measured trials are separate
phases; seeds are paired but stochastic responses need not be identical.

Bounds: at most 128 cases, 64 cells, 64 concurrent requests, 100 measured and 20
warmup trials per cell, 1,024 total waves and 10,000 attempts. Case messages are
bounded to 128 KiB declared decoded content. Declared content bytes plus
the fill byte count must fit 2 MiB, as must each serialized request including
JSON escaping and controls. Output budgets are 1..32,768 tokens.
IDs are unique short ASCII identifiers.

Response limits are 1 KiB..8 MiB per request. SSE line and event caps are 256 KiB,
and JSON nesting is limited to 64. Per-request total/idle deadlines are explicit,
positive, with `total_ms` at most 3,600,000 (one hour) and idle no greater than
total. This finite ceiling is an admission policy, not a runtime guarantee.
Selected budgets remain explicit workload identity; existing declarations are
not increased automatically. Admission errors identify the failed deadline or
buffer field and relation, before dispatch.

Both clocks start at dispatch. Only nonempty body chunks reset idle; headers,
empty chunks and server-side progress do not. SSE comments can reset idle without
generated text. Total never resets, and completion parsing/settlement must fit
it. Cancellation takes precedence over total expiry, then idle expiry. Raising
total alone cannot prevent a shorter idle timeout during quiet prefill.
Longer selected budgets can lengthen active-wave drain and cooperative pause
latency. No retry, hidden override or buffer expansion accompanies the ceiling.

Admission checks the wave-buffer allowance against concurrency times
`2*response_bytes + 6*256KiB + 512KiB + fill_bytes` for streaming, or
`6*response_bytes + 512KiB + fill_bytes` for nonstreaming, where `fill_bytes` is
the cell's case `unit.len()*repeat` or generated-prose `characters` (zero when absent). The latter budgets
body-sized JSON scratch and decoded fields rather than assuming frame-sized parsing.
Each request body, JSON-string-escaped as it is embedded in the reservation receipt
and rendered at the admission bound widths, times concurrency must not exceed
32 MiB, within the 40 MiB reservation receipt limit. Fill bytes are held in rendered,
encoded and escaped-receipt copies while a wave is reserved; the allowance counts them once.
Serialized reservation buffers are released before dispatch. These are
conservative owned-buffer allowances, **not** RSS or kernel/socket-memory
guarantees. The allowance cannot exceed 512 MiB. HTTP-library, TLS and process
overhead require measurement, not claims from this formula.

## Scheduling and timing

One reusable HTTP/1 client has verified TLS, no automatic retry, redirect, proxy,
compression or fallback, and an idle-pool maximum equal to the largest declared
concurrency. That pool maximum does not cap active requests: the fixed-wave
scheduler does. The server can close connections; no forced reconnect is added.

Before a wave, requests are serialized, reservations are durably published and
HTTP requests are constructed. Each collector records a monotonic dispatch
observation immediately before its request is executed. It records:

| Observation | Definition |
|---|---|
| `headers_us` | Dispatch to response-header observation. |
| `first_body_us` | Dispatch to first nonempty body chunk. |
| `first_generated_text_us` | Dispatch to arrival of the first complete SSE event containing nonempty answer or recognized reasoning text. |
| `first_generated_channel` | Answer, reasoning, or both in the same event; not a guessed server token order. |
| `first_answer_text_us` | Dispatch to the first complete SSE event with nonempty `delta.content`. |
| `last_generated_text_us` | New-contract streaming observation: arrival of the last complete SSE event with nonempty answer or recognized reasoning text. Same-chunk events share an observation; this is not a GPU token timestamp. |
| `terminal_us` | Arrival of the successful completion marker: SSE `[DONE]`, or complete nonstreaming JSON observation. It is distinct from a `finish_reason` event and from settlement. |
| `settle_us` | Dispatch to completion or failure settlement, including collection/parse work. |
| `capture_parse_us` | Accumulated elapsed intervals inside capture/parse sections; not process CPU time or server time. |
| `decode_tokens_per_second` | For a complete stream with reported `n >= 2`, `(n - 1) * 1_000_000 / (settle_us - first_generated_text_us)` when settlement is later than first text; otherwise null. This original policy metric includes terminal delay/parsing. There is no promised bound on its difference from the text-window rate. |
| `text_decode_tokens_per_second` | For a complete stream with reported `n >= 2`, `(n - 1) * 1_000_000 / (last_generated_text_us - first_generated_text_us)`, only for a positive observed text span. Null for legacy/missing/coincident observations; never estimated from SSE event counts. |
| Derived per-stream prefill tokens/s | For a complete streaming attempt with `prompt_tokens = p >= 1`, `first_generated_text_us = t > 0`, and provider-reported cached prompt tokens absent or zero, `p * 1_000_000 / t`; otherwise null. Matches sparkDash prompt_tokens/TTFT, including queueing and first-token generation. Derived offline, not persisted in wave receipts. |

Fragmented events become observable when their framing boundary arrives. Multiple
events in one received chunk share its arrival observation. Gateway buffering,
coalescing and client scheduling remain in these observations. Nonstreaming
responses have completion latency but no fabricated first-text observation.
A per-stream decode rate from first generated text to settlement is derived offline; no GPU/token timestamps are produced.
The text-window formula agrees with sparkDash's formula for identical reported
usage and supplied text-event timestamps. This is not native parser/framing or
aggregate-throughput equivalence; an undefined interval remains null here.
The conventional `n - 1` numerator does not mean the first SSE event held exactly
one token: an event may contain several. Neither event-window rate reconstructs
individual model-token generation times.
Deterministic shared traces cover split frames/UTF-8, coalesced events and delayed
terminal data, with independent arithmetic checked at relative tolerance `1e-9`.
Raw bodies support semantic/presence checks, not independent recovery of missing
chunk timestamps. New observations are checked for order and consistency with
the retained response; they are not execution attestations.

Wave elapsed time is the span from the first collector dispatch to the last
collector settlement. Dispatch spread is recorded. Only a fully eligible wave
gets achieved completion throughput: the sum of provider-reported completion
counts divided by that entire interval. Completion counts may include reasoning
according to provider accounting; the result never calls them answer-only tokens.
The wave-level completion total is null if any response is incomplete. Such a
response's provider-reported usage remains in its individual attempt record.
This is achieved fixed-wave throughput, not maximum or steady-state capacity.

## Completion, failures and eligibility

Only HTTP 200 with the declared JSON/SSE content type and identity encoding is
parsed. Records must be objects, choices must contain at most one choice and
only index zero is supported. Text/refusal/tool/audio ambiguity is not repaired.
The original `portable-chat-v1`/`vllm-fixed-v1` response profiles remain text-only;
tools and refusals are retained as unsupported results, not silently ignored.
Only explicit conversation profiles permit the bounded full nonstreaming
tool-call object described in their contract. No streamed tool-delta
assembly or arbitrary tool execution is supported.
Within conversation workloads, a tool expectation may declare `shared: true`
to send the fixed tool array on every step of its history with
`tool_choice: "none"` on factual steps; the forced call is unchanged. The flag
is opt-in, requires workload version 5 or 6, and all tool steps in one history
must agree. Workloads without it keep their exact request bytes.

A streaming response needs a supported `stop`/`length` finish followed by a framed
`[DONE]`. EOF does not finish an unclosed event. Usage-only events after finish are
accepted, but further generated text, a changed response ID, duplicate finish,
malformed JSON or invalid UTF-8 are failures. `[DONE]` belongs to this performance
response handler; shared `grill-sse` remains unaware of its meaning.

Bytes co-read after semantic completion are retained within the response cap;
the completion offset and observed surplus are recorded. Bytes never read are not
claimed as retained. Missing usage stays null. Reported lengths exceeding the cap,
exact-length mismatches and unmet reported-prefix requirements make the wave
ineligible. A `cap-reached` lane is eligible only when reported completion
tokens equal the cap (`reported_output_below_cap` otherwise) and the finish
reason is `length` (`cap_not_reported_as_length` otherwise), so a backend that
ignores `min_tokens` cannot make lanes silently unequal.
Usage is provider evidence, not verified billing or engine attestation.
In particular, a reported prefix miss after warmup can reflect server-specific
cache-block alignment rather than collector malfunction; priming alone does not
establish reusable cache blocks. The reported-hit requirement is not relaxed.
Contradictory reported reasoning counts greater than completion counts are
ineligible; they are not repaired or silently included in a rate.

All admitted peers settle before the next wave. In new v3 runs, a response
failure or measurement ineligibility stops later admission without retries.
An observed irrevocable violation, such as over-cap usage, remains invalid even
if the request later reaches a deadline. Missing final usage on a genuinely
interrupted prefix remains incomplete, not an invented failure or zero count.
SIGINT/SIGTERM cancel active waits, retain partial bytes and prevent new waves.
A hard kill can leave reserved-but-unsettled evidence, never a successful sample.

## Evidence barrier and layout

```text
RUN/
  workload.json
  plan.json
  wave-000000/
    reservation.json
    response-0000.bin
    response-0001.bin
    wave.json
  ...
  run.json
  session-000000/
    session.json       plan hash, collector hash, start index and timestamp
    pause.request/     optional cooperative control marker
    run.json           immutable session outcome and session-local counters
  session-000001/       present only after validated continuation
    session.json
    run.json
```

Output must be fresh. Files use exclusive creation and restricted permissions.
All active wave responses remain in bounded memory. A completed lane cannot
write or fsync its response while another lane is active. After the entire wave
settles, bodies are hashed, written and synced, then the wave receipt is published
without clobbering. `body_publication_us` covers the body-publication interval;
run-level `wave_publication_us` also includes wave-receipt publication.
`preparation_us` covers per-wave preparation before dispatch and includes
`reservation_publication_us`, which records reservation serialization, hashing
and durable publication. Run-level counters sum those intervals; do not add
the nested reservation interval to preparation a second time. These intervals
are outside response timing. Capture overhead remains measured and visible;
it is not subtracted as a universal correction.

An I/O failure stops admission and returns an error. Partial files and reservations
remain; no fallback drops evidence in order to finish a benchmark. Offline loading
rejects corrupt evidence, verifies request rendering and response hashes, replays
complete response semantics/usage, and rederives wave rates and elapsed spans.
New-contract partial responses also have retained usage and finish facts checked
against their raw bytes; legacy partial-evidence interpretation is unchanged.
Absent wave directories and reserved-but-unsettled waves remain distinguishable.

Hashes bind retained bytes, not truthful execution. Plans record the executing
collector binary fingerprint and optional deployment declarations. These do not
prove which model or hardware an untrusted server used. Raw evidence is not a
public-safe export.

## Provider snapshot protocol

Opt-in plan field `metrics` is a closed version-1 configuration containing
`endpoint`, `allowlist`, `scope`, `cadence`, `deadline_us`, `body_bytes`,
`line_bytes`, `series`, `labels_per_series`, `label_bytes_per_series`,
`run_budget_us`, `retained_raw_bytes` and `max_requests`. The collector freezes
the endpoint and bounds described in the usage guide; the loader rejects
modified policies. The plan-byte hash binds them to every reservation and wave.
Absent `metrics` serializes absent, preserving opted-out plan/request/receipt
shapes. Legacy plans cannot acquire telemetry without execution-session
provenance. Comparison requires equal optional configurations, including endpoint.

Each opted-in published wave requires `metrics: {file, sha256, overhead_us}`;
`file` is exactly `metrics.json` within that wave directory. The bounded companion
contains `{version, plan_sha256, wave, before, after, measured_origin_unix_ms,
measured_duration_us}`. The measurement duration spans the collector origin
through all lane settlement, not the subsequent scrape or publication.
`overhead_us` covers both telemetry operations including raw and companion
publication; it is excluded from wave preparation/body-publication intervals.
Wall timestamps are observations, not an assumed monotonic clock.

Each snapshot contains `{started_unix_ms, scrape_us, duration_us, charged_us,
allowance_us, status, error, http_status, raw_bytes, raw_sha256}`.
`scrape_us` covers capture/parse; `duration_us` also includes raw hashing and
publication. Its raw entity or retained
prefix is `metrics-before.bin` or `metrics-after.bin`; skipped snapshots bind
an empty file. Status is `complete`, `parse_error`, `transport_error`,
`http_error`, `unsupported`, `deadline`, `body_limit` or `skipped_budget`.
Only successful, completely captured, identity-encoded HTTP 200 entities supply
samples. Errors remain diagnostics, not zeros or performance-ineligibility
reasons. The loader verifies companion lineage/hash, raw bounds/hashes, policy,
budget arithmetic and complete/parse-error raw semantics. Derived series,
deltas and ratios are recomputed offline, not trusted from serialized values.

The parser accepts UTF-8 Prometheus text samples, comments, optional integer
timestamps and quoted labels with newline, quote and backslash escapes. It
rejects duplicate selected identities, duplicate selected labels, nonfinite selected
values and negative supported counters. Unselected names are skipped before label/value
parsing; whole-body UTF-8, byte and line bounds still apply. Label bytes include
the encoded braces, names, separators and values of each selected series.
The allowlist is fixed; no histogram expansion or OpenMetrics exemplars are
interpreted. Empty/missing samples remain unavailable, not zero.

Counter identity is metric name plus canonical complete labels. A delta requires
both finite samples and a nondecreasing value. A decrease reports
`reset_observed`; a missing side reports `missing`, with null delta.
Nondecreasing deltas explicitly leave restart identity unverified. Acceptance
ratios require matching label sets, usable draft and accepted deltas, positive
draft delta and accepted delta no greater than draft delta. Gauges are snapshots,
not deltas. All diagnostics remain server-scoped, not workload-attributed.

Each scrape deadline is the lesser of 2 seconds and remaining 30-second
whole-run telemetry allowance. Its persisted elapsed duration, including raw
publication and at least a microsecond, is the charge. Remaining telemetry
overhead from the wave reference is charged before the next wave. Scheduler
and filesystem overrun is retained and prevents further
admission once exhausted. No completed scrape gets a fresh full-deadline charge
on resume. Every published companion is reloaded in schedule order to reconstruct
requests, elapsed charges and retained bytes before continuation. Existing
lifecycle rules refuse continuation of any reserved/unsettled scrape or wave;
uncommitted reservations therefore cannot reset the allowance.

Raw bodies are bounded by 1 MiB each and 16 MiB across the run. Admission reserves
a full body cap before making another request. Companions are bounded by 16 KiB
each and labels are retained only in raw bodies, not JSON receipts. Thus escaped
labels cannot inflate companion publication. Offline JSON can expand raw labels
through escaping and diagnostic projections; its input remains bounded by the
whole-run raw cap and per-scrape series/label limits, not a claim of constant
output size or RSS. Filesystem latency, kernel/TLS buffering and scheduler
starvation are not bounded by the network deadline. Published telemetry overhead
exposes local work rather than claiming zero perturbation.

Raw before evidence is written outside the measured origin, after reservation.
The after scrape and companion publication follow all lane settlement and precede
wave publication. A failed companion publication leaves the reservation
unsettled; it is inspectable as missing performance evidence, never silently
recovered or resumed. A published wave missing its required companion fails
integrity verification. Telemetry has no separate inspector or recovery workflow.

## Lifecycle ownership and continuation

Each collector/resumer holds a nonblocking exclusive `flock` on the nonsymlink
run-directory inode from validation through final publication. There is no PID
existence guess, stale lock-file removal, lock-stealing or background controller.
The operating system releases the lock on process exit; a crash does not make
uncertain evidence safe to resume. Local Linux filesystems with working advisory
locking are required. Cooperating collectors are excluded; hostile directory
replacement or an actor ignoring advisory locks is not a supported threat model.

Before network admission, a fresh numbered session directory durably records its
first wave and original plan/collector bindings. Session directories are
contiguous and bounded to 2,048. Its immutable outcome records the count of
published waves. The existing offline loader validates session ranges alongside
the existing reservation, request, raw-body and wave checks. A continuation
requires the latest session to end in `paused`, every prior range to be published,
and every remaining wave directory to be absent. Open sessions and ambiguous
reservations are blocked; failed or interrupted work is never retried.
A settled `local-failure` may retain its next wave's reservation without a wave
receipt. Offline comparison preserves that reserved/unsettled state and the
never-started suffix, but withholds medians and continuation. Published evidence
outside the settled range or noncontiguous reservations remain errors.

The pause command creates an idempotent session-local directory marker without
fsync or raw-evidence hashing while peer collectors may be active. It is a
cooperative request, not a durable claim that draining has completed. Pause
publication and the runner's pause-check-through-reservation use a blocking
exclusive `flock` on the session-directory inode, separate from run ownership.
The admission lock is released before network collection. After the active wave
settles, the runner publishes its evidence before reaching that boundary.
Immediate signals still cancel network waits. A request after final admission
may complete the run instead of pausing it.
Session publication and all collector evidence synchronization remain outside
active peer collection. Failure to publish a session outcome leaves uncertainty.

The first root `run.json` is retained unchanged and verified byte-for-byte against
the settled session-zero outcome. A missing root receipt leaves publication
uncertain, inspectable but not resumable or eligible for medians; a damaged or
unequal receipt fails verification. The loader never regenerates it.
No snapshot, request, response, wave, or prior session receipt is replaced.
Resume uses only frozen configuration and the original credential variable name,
and requires the same collector binary/version. Changing the original external
input file is irrelevant: the retained snapshot, not that external path, is used.

## Comparison

`compare` is offline. It requires equal normalized workloads, collector version
and binary fingerprint, and transport controls. It retains planned measured
trial slots and separately exposes warmup states. All declared warmups must
have eligible completed evidence before any matched medians/changes are granted;
their timings remain excluded. Changes are withheld when ordered per-lane
completion counts differ in paired trials, even if wave totals match.
Complete per-attempt status, detail, usage and timing observations are in the
original `wave.json` receipts; comparison JSON summarizes trial accounting,
token counts and eligibility issues. Equal caps alone do not establish equal work.
Model and optional deployment declarations are shown separately. There is no
combined quality/performance score, automatic winner, confidence interval or
p95 estimate from three trials.
Per-stream decode medians use eligible measured lanes under the same completeness gates; decode-rate changes also require matched ordered lane counts.
Per-stream prefill medians use the same completeness gates; prefill-rate changes require matched ordered lane completion counts like the other changes, and each side's ordered lane prompt counts are reported so tokenizer or salt differences stay visible.
A second execution session resets the client connection pool and introduces an
unmeasured gap in server/cache state. Original warmup waves remain evidence but
cannot certify resumed-session warmup continuity. No warmup is added or replayed,
including under reported-prefix-hit mode. All multi-session run medians and
matched changes are withheld, even if one cell lies entirely within one session;
per-wave observations and declared warmup completion remain visible. Open session
outcomes also withhold medians. `changes[].ineligibility_reasons` explains side
specific evidence issues and missing/unequal paired lane counts in both CLI and
JSON, rather than requiring users to infer why a change is absent.

Each complete cell reports min/max ranges alongside its medians: trial-level
wave latency and achieved throughput, and all eligible measured lanes for decode
and prefill rates. A matched percentage is reported only when the two ranges do
not overlap (`a.max < b.min || b.max < a.min`); touching endpoints overlap.
`changes[].withheld` records each overlap with both ranges, using seconds with
two decimals for latency and tokens/s with one decimal for rates. Withholding
for spread does not change `eligible` or `ineligibility_reasons`, which describe
run comparability. Absent scalars and ranges are explicit JSON nulls.
Disjoint observed ranges are a descriptive filter, not a significance test,
bootstrap confidence interval or equivalence test. A withheld change is not
evidence of equality.

`compare A B --reference A2` verifies the supplied reference against the same
workload, collector identity and transport controls. Corrupt or structurally
incompatible reference evidence is a command error with reference context.
Valid but incomplete or ineligible reference evidence makes the affected cell
ineligible: changes and drift are null, and reference-specific reasons are
reported. This includes incomplete declared warmups and unsettled or continued
execution sessions. All run summaries and ordered raw observations remain
available; no supplied reference silently becomes an ordinary A/B comparison.

`reference_model`, `reference_deployment` and `reference_identity` disclose
baseline-repeat declarations. Identity status is `declared_match` only when
model, endpoint and every deployment field (`model_revision`, `runtime`,
`hardware`, `settings`) are present and equal. A known differing declaration
is `declared_mismatch`, even if another field is missing; otherwise missing
declarations are `unavailable`. Both nonmatching statuses make reference-aware
cells ineligible, with reasons, without discarding any run's summaries.
Matching declarations do not verify physical server restoration, cache state,
temporal A/B/A ordering or causal effect. JSON and human output state this
limitation; the result is only a descriptive declared-repeat comparison.

`observed_output_amounts_match` retains its A/B meaning.
`reference_output_amounts_match` reports the A/A2 ordered trial/lane completion
count check and is null without a reference. Complete equal ordered counts
across A, B and A2 are required for both changes and drift; equal totals or
permutations are insufficient.

For a qualified repeat, the baseline range becomes the union of A's and A2's
ranges before the existing overlap test. Missing lane observations on A, B or A2
withhold that stream metric and its drift with a reference-specific reason;
surviving lanes cannot silently stand in for a complete paired metric.
A missing reference metric range likewise never permits A-only fallback.
Other metrics remain usable; absent decode/prefill observations do not alone
make a nonstreaming cell ineligible. `drift` reports qualified A-to-A2 median
changes without an overlap filter; its `withheld` array explains null metrics
and failed qualification. Drift is descriptive, not a validated noise bound.

Comparison JSON advances to `version` 3 for reference qualification and identity
disclosure. Reference metadata, summaries and drift are explicit nulls when
no reference is supplied. Ordinary no-reference numerical results and eligibility
retain the previous range method. Run, plan, reservation and wave receipt
formats and legacy readers are unchanged. Exit status follows cell eligibility:
supplied ineligible reference evidence cannot produce an eligible A/B exit;
metric-specific absence or overlap alone does not change cell eligibility.

## Capture v3 and deployment comparison

New `baseline` captures are `performance-capture-v3`. Besides the v1/v2 fields
they record `collector: {version, source_commit, target}` (the binary
fingerprint stays in `collector_sha256`), the declared `client_placement`
(`same-host` or `network`, from the required `baseline --client-placement`) and,
without a selection, the built-in `workload` name (`baseline-v1`, `baseline-v2`
or `portable-v1`, from `baseline --workload`, default `baseline-v2`). The
loader verifies the retained workload bytes against that built-in and the
collector version against every native plan; v1 captures hold `baseline-v1`.
A `check` keeps its baseline's capture version, so v1 and v2 baselines check and
compare as before. Older readers reject v3 captures and the `deployment` change.

`check A --change deployment` records a candidate B served by another
deployment. `--endpoint`, `--model`, `--auth-env`, `--local-http` (with
`--endpoint`) and `--client-placement` (required here) are accepted only for this
change; unset values are inherited from A. Before any request, A must be a v3
capture of a built-in workload, the parsed declaration must differ in at least
one field, the client placement must be equal, and the collector version and
source commit must be equal and recorded (not `unrecorded`). Endpoint, model and
binary fingerprint may differ and are recorded, not admitted on. Every other
`--change` keeps requiring the baseline's binary. A complete deployment check
reports `PENDING` (exit 0); it has no verdict on its own.

`compare A B --reference A2` gives the verdict, where A2 is an unchanged control
(`check A --change none`) of A captured after B. By recorded timestamps, B must
start at least 60 s after A finished and A2 at least 60 s after B finished. With
same-host placement on two client hosts this ordering relies on each client's
clock. The unchanged C1 interval model is computed for A→B, A2→B and A→A2. The
result is `IMPROVED` or `REGRESSED` only when A→B and A2→B are directional in the
same direction; otherwise it is `INCONCLUSIVE` with the reason "candidate is not
directional against both baseline capture periods". A directional A→A2 adds the
reason "baseline deployment shifted between capture periods" without gating the
result. Without A2 a deployment pair is `INCONCLUSIVE` without an interval; a
reference with any other capture pair is `INVALID`.

The report kind is `performance-deployment-comparison-v1`. Its top-level interval
fields hold A→B; `deployment` holds each side's endpoint, model, declaration,
collector identity, binary fingerprint, client placement and median reported
`usage.prompt_tokens` over measured requests, the reference path and identity,
and the A2→B (`candidate_against_reference`) and A→A2
(`reference_against_baseline`) intervals. Differing prompt-token medians add a
reason, because the servers then do different prefill work. The scope is a
whole-deployment comparison of request throughput at C1 including prefill:
hardware, runtime, model build, settings, endpoint and placement are confounded,
so no single factor such as a quantization format is attributed; only the
baseline deployment has a drift control; and the synthetic count prompt lets
speculative decoding on either side dominate.

## Output identity

`outputs A B` is offline. A and B are both native run directories or both
capture roots; captures pair acquisition `i` of A with acquisition `i` of B.
Each run and capture is fully verified by the ordinary loaders first.
Admission requires, else exit 1: equal normalized workloads (`workload_sha256`,
which also fixes the waves and lanes), `temperature_milli` 0 on every measured
lane (absent means the provider default and is refused), no generated-prose fill
or `{salt}` text in a filled case (its prompt differs per capture), and at least
one measured lane.
Endpoint, model, deployment declaration and collector may differ.

For each measured lane (warmups excluded) the retained response, rechecked
against its recorded digest, is replayed with the completion parser into two
channels: answer (`content`) and reasoning (`reasoning`, else
`reasoning_content`, per event). A lane needs a `complete` attempt on both
sides; performance eligibility errors do not affect it. The channels are
compared separately, reasoning first. The result is `identical` (with each
channel's character count), `differs` (the channel, the first differing
Unicode character and UTF-8 byte offset, and up to 24 characters on each side
of it from A and B; a proper prefix differs where it ends) or `unavailable`
(`reasons` per side: acquisition not started, wave not published, response not
complete, or evidence that does not replay as completion text).

The report kind is `performance-output-identity-v1` (`version` 1) with the
paths, `workload_sha256`, counts and per-lane results; `acquisition` is present
only for captures. Exit 0 requires every lane identical; any `differs` or
`unavailable` lane exits 2. This is text identity only: token IDs are not
retained, and equal text does not show equal logits.

## Validation

Run the real CLI fixture regressions and strict package checks:

```sh
cargo test -p grill-perf --locked -- --test-threads=1
cargo clippy -p grill-perf --locked --all-targets -- -D warnings
cargo fmt --all --check
```

CI likewise serializes the test harness (`cargo test --workspace --locked --
--test-threads=1`). These socket fixtures assert short timeout and timing
relationships; running unrelated fixtures simultaneously can exhaust deadlines
or overlap ranges through host contention. Request lanes inside a fixture remain
concurrent, so overlap, peer settlement and publication barriers are still
exercised. This changes neither collector limits nor benchmark scheduling.

The explicit release-mode overhead experiment is:

```sh
cargo test -p grill-perf --release --locked --test cli paced_collection_overhead \
  -- --ignored --nocapture
```

It compares raw loopback draining with collection at concurrency 1/6, 512 paced
SSE text events per request, three measured trials, and a prespecified 5% median
wave-latency margin. It prints all timings plus parsing and publication intervals.
Passing bounds this particular paced fixture, not arbitrary rates, machines, TLS
proxies, OS cache states or production capacity. Real adoption/measurement checks
on Mia and a second independent recipe require separate execution authorization;
fixture success is not a substitute for those checks.
