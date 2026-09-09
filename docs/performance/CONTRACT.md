# Performance measurement contract

Workloads and wave receipts remain v1. New execution plans are v2, adding mandatory
execution-session provenance without changing request rendering or wave timing.
Legacy v1 plans remain readable offline but cannot be paused or resumed.

## Workload admission

The workload has `version`, `name`, `request`, `limits`, `cases` and `cells`.
See the complete [portable example](../../crates/grill-perf/examples/quick.json).
Unknown workload fields and invalid controls are rejected before dispatch.

`request` declares `profile`, `stream`, `output: {tokens, mode}`, `cache`, and
nullable `temperature_milli`, `top_p_milli`, `seed`. Thousandths are encoded as
decimal sampling values. Nullable `thinking` requires `vllm-fixed-v1` when declared
and is sent as `chat_template_kwargs.thinking`; null leaves the provider default.
No other generation fields are sent or inferred.
Model-side thinking defaults are not overridden unless declared.
Streaming requests explicitly request usage with `stream_options.include_usage`.

Cases may declare `fill: {unit, repeat}`. The unit is nonempty and at most 64
bytes; repeat is 1..1,000,000. Such a case requires exactly one `{fill}` and at
most one `{salt}` across its message contents. Rendering replaces `{fill}` with
the repeated unit and `{salt}` with `namespace[..16]-wave.index-lane`.
Without `fill`, both placeholders are ordinary text. A random 64-hex
`cache_namespace` exists exactly when cache is not `observe` or any case has
fill; text salts differ per attempt even under `observe`.

Each cell names a case, concurrency, warmup trial count and measured trial count.
For a trial, each lane receives the same case with its attempt's text salt. When a seed is declared,
its lane seed is `seed + trial*64 + lane`. Warmup and measured trials are separate
phases; seeds are paired but stochastic responses need not be identical.

Bounds: at most 128 cases, 64 cells, 64 concurrent requests, 100 measured and 20
warmup trials per cell, 1,024 total waves and 10,000 attempts. Case messages are
bounded to 128 KiB declared decoded content. Declared content bytes plus
`unit.len()*repeat` must fit 2 MiB, as must each serialized request including
JSON escaping and controls. Output budgets are 1..32,768 tokens.
IDs are unique short ASCII identifiers.

Response limits are 1 KiB..8 MiB per request. SSE line and event caps are 256 KiB,
and JSON nesting is limited to 64. Per-request total/idle deadlines are explicit,
positive, at most ten minutes, with idle no greater than total.

Admission checks the wave-buffer allowance against concurrency times
`2*response_bytes + 6*256KiB + 512KiB + fill_bytes` for streaming, or
`6*response_bytes + 512KiB + fill_bytes` for nonstreaming, where `fill_bytes` is
the cell's case `unit.len()*repeat` (zero when absent). The latter budgets
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
| `settle_us` | Dispatch to completion or failure settlement, including collection/parse work. |
| `capture_parse_us` | Accumulated elapsed intervals inside capture/parse sections; not process CPU time or server time. |
| Derived per-stream decode tokens/s | For a complete streaming attempt with `completion_tokens = n >= 2`, `(n - 1) * 1_000_000 / (settle_us - first_generated_text_us)` when the first generated text time is present and settlement is later; otherwise null. Derived offline, not persisted in wave receipts. Settlement includes final `[DONE]`/usage frame parsing, making this slightly conservative relative to last-token timing. |
| Derived per-stream prefill tokens/s | For a complete streaming attempt with `prompt_tokens = p >= 1`, `first_generated_text_us = t > 0`, and provider-reported cached prompt tokens absent or zero, `p * 1_000_000 / t`; otherwise null. Matches sparkDash prompt_tokens/TTFT, including queueing and first-token generation. Derived offline, not persisted in wave receipts. |

Fragmented events become observable when their framing boundary arrives. Multiple
events in one received chunk share its arrival observation. Gateway buffering,
coalescing and client scheduling remain in these observations. Nonstreaming
responses have completion latency but no fabricated first-text observation.
A per-stream decode rate from first generated text to settlement is derived offline; no GPU/token timestamps are produced.

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
This initial performance profile is text-only; tools and refusals are retained
as unsupported results rather than graded or silently ignored.

A streaming response needs a supported `stop`/`length` finish followed by a framed
`[DONE]`. EOF does not finish an unclosed event. Usage-only events after finish are
accepted, but further generated text, a changed response ID, duplicate finish,
malformed JSON or invalid UTF-8 are failures. `[DONE]` belongs to this performance
response handler; shared `grill-sse` remains unaware of its meaning.

Bytes co-read after semantic completion are retained within the response cap;
the completion offset and observed surplus are recorded. Bytes never read are not
claimed as retained. Missing usage stays null. Reported lengths exceeding the cap,
exact-length mismatches and unmet reported-prefix requirements make the wave
ineligible. Usage is provider evidence, not verified billing or engine attestation.
Contradictory reported reasoning counts greater than completion counts are
ineligible; they are not repaired or silently included in a rate.

All peers settle before the next wave. A response failure stops later admission;
metadata-only ineligibility does not cause a retry. SIGINT/SIGTERM cancel active
network waits, retain partial bytes, and prevent new waves. A hard kill can leave
reserved-but-unsettled evidence; it cannot be turned into a successful sample.

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
Absent wave directories and reserved-but-unsettled waves remain distinguishable.

Hashes bind retained bytes, not truthful execution. Plans record the executing
collector binary fingerprint and optional deployment declarations. These do not
prove which model or hardware an untrusted server used. Raw evidence is not a
public-safe export.

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
`compare A B --reference A2` checks the reference against the same workload,
collector identity and transport controls, and includes its cell summaries in
`reference`. `drift` reports raw median changes from A to A2 for wave latency,
achieved throughput, decode and prefill, without overlap gating. An A-to-B change
whose absolute value is no greater than that metric's absolute reference drift
is additionally withheld with the change and drift percentages in `withheld`.
Reference drift is a noise floor, not a performance claim; absent reference and
drift fields are explicit nulls, and human output prints absent drift as `n/a`.

## Validation

Run the real CLI fixture regressions and strict package checks:

```sh
cargo test -p grill-perf --locked
cargo clippy -p grill-perf --locked --all-targets -- -D warnings
cargo fmt --all --check
```

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
