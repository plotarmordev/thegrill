# Serving-performance companion

`grill-perf` measures requests to an already-running Chat Completions server.
It does not start servers, download models, grade answers or require quality
packs. Its `run`, `pause`, `resume` and offline `compare` workflows are separate from `grill`.

## Build and use

Requirements match the workspace: Linux, Rust/Cargo 1.98 and the native build
tools needed by the existing rustls/AWS-LC dependency.

From the workspace root:

```sh
cargo build -p grill-perf --release --locked
mkdir -p results

target/release/grill-perf run crates/grill-perf/examples/quick.json \
  --endpoint https://your-server.example/v1/chat/completions \
  --model your-model --auth-env MODEL_API_KEY --out results/baseline

# Change the serving configuration yourself, then use the same workload/binary.
target/release/grill-perf run crates/grill-perf/examples/quick.json \
  --endpoint https://your-server.example/v1/chat/completions \
  --model your-model --auth-env MODEL_API_KEY --out results/candidate

target/release/grill-perf compare results/baseline results/candidate --json
```

Set `MODEL_API_KEY` in your environment if required; omit `--auth-env` for an
unauthenticated endpoint. Credentials are not retained in evidence or error
messages. Plain HTTP requires `--local-http` and a literal loopback IP, for
example `http://127.0.0.1:8000/v1/chat/completions`. Use HTTPS or an independently
managed local forward for remote servers. TLS verification is never disabled.
Endpoint text is bounded to 4,096 bytes and rejects control characters.

Every run directory must be new and its parent must exist. `--help` documents
the commands. Optional installation is:

```sh
cargo install --path crates/grill-perf --locked
```

The supplied quick workload uses concurrency 1 and 6, one warmup trial per
cell, and three measured trials. It caps output at 1,024 tokens; it does not
pretend that a cap forces every server to emit exactly that many tokens.
All warmup waves finish before any measured wave begins.

## Pause and continue

```sh
target/release/grill-perf pause results/baseline
# Wait for the original collector to exit and its session outcome to say paused.
target/release/grill-perf resume results/baseline --json
```

`pause` requests a cooperative stop; it does not cancel any lane. The active
whole wave settles and publishes all its evidence before the next admission
boundary observes the request. A request arriving after the final admission can
finish as completed instead of paused. Exit 0 from `pause` means the request was
recorded, not that draining finished. If no cooperating collector holds ownership,
the command refuses without creating a marker and directs you to inspect the retained session.
Pause publication and wave reservation share an admission lock, so a recorded
request cannot be overtaken by a new reservation. The lock is not held during
network collection, and the control command performs no evidence fsync.
Read `session-NNNNNN/run.json` for the authoritative session outcome.

`resume` accepts only a settled cooperative pause from the original collector
binary. It takes exclusive filesystem ownership, runs the ordinary offline
evidence checks, and continues the original schedule from its first never-started
wave. It takes no replacement workload, endpoint, model or sampling options.
The original named credential environment variable must still be available;
its value is never retained. Failed/interrupted sessions, missing session outcomes,
reserved-but-unsettled waves, damaged evidence, and legacy plans are refused.
There is no recovery override, replay, retry or overwrite.

Every continuation creates a numbered execution session and a fresh HTTP client
pool. It neither repeats completed warmups nor invents new warmups outside the
plan. Cache salts and request bytes follow the original plan, but the intervening
server/cache state and warmup continuity are unverified. **Any run with more than
one execution session is ineligible for matched timing medians/changes**, even
when every wave independently meets the output gates. Per-wave observations
remain available; JSON and human comparison output explain this exclusion.
To obtain an uninterrupted timing comparison, collect a separate fresh run.

The root `run.json` remains the first session's immutable receipt, not a mutable
latest-status file. Session summaries count waves collected in that session,
with `session` and `first_wave` identifying their range. A paused run exits 2;
a fully collected resumed session also exits 2 because it cannot establish an
uninterrupted timing comparison. Completed eligible first sessions remain exit 0.
SIGINT/SIGTERM retain their immediate cancellation behavior, distinct from pause.

The offline loader checks that root receipt against session zero. Missing root
publication or a local failure with a reserved/unsettled next wave remains
inspectable through `compare --json`, with missingness visible and medians
withheld; neither state permits continuation. Unequal or damaged receipts fail
verification rather than being repaired.

## Interpret the result

- **Wave latency** spans first dispatch to last request settlement in a fixed
  wave. It includes dispatch spread and transport/collection effects.
- **Achieved completion throughput** uses complete provider-reported completion
  counts divided by that entire wave interval. It is not an answer-only token
  rate or a claim of steady-state server capacity.
- **First body**, **first generated text** and **first answer text** are separate
  observations. Role-only events do not count as text; reasoning is generated
  text but not answer text. These are client observations, not GPU token times.
- Nonstreaming runs do not invent first-text timestamps.
- Medians require all declared warmups and every measured trial in the cell
  to have eligible evidence. Warmup timings are excluded.
- Detailed per-request status, timing, usage and failure reasons remain in each
  `wave.json`; comparison JSON summarizes trial accounting and eligibility.
- A matched change is withheld if observed output amounts differ in any paired
  trial/lane, even when wave totals match. Shorter answers must not masquerade
  as a speed improvement.
- A compatible comparison requires the same normalized workload, collector
  binary fingerprint and transport controls. Model/endpoint deployments may
  differ; this is a descriptive deployment comparison, not causal attribution.

Exit codes: `0` means a complete eligible run/comparison, `2` means retained but
ineligible/incomplete evidence, and `1` means CLI usage, admission, integrity or local I/O
failure. No automatic retry, redirect, proxy, parameter fallback or winner
selection occurs. SIGINT and SIGTERM stop further admission and settle active
attempts. A response failure stops admission after its wave; metadata-only
ineligibility is retained without a retry.

## Exact output and cache observations

For a consenting vLLM-compatible server, use request profile `vllm-fixed-v1`.
`output.mode: "exact"` explicitly sends `min_tokens` and `ignore_eos`, and checks
reported completion length. These extensions are never silently sent under
`portable-chat-v1` or stripped and retried after an error.

Cache modes:

| Mode | Meaning |
|---|---|
| `observe` | No cache-state control; retain reported counts if available. |
| `reported-prefix-zero` | Unique per-request salt; require reported prefix-cache count zero. |
| `reported-prefix-hit` | Stable salt per cell/lane; require explicit warmup and reported hits in measured waves. |

Required prefix modes need `vllm-fixed-v1`. Plans name the declared mechanism
and the provider observation source; the response record retains the actual
reported count. A flag plus a reported zero is **not universal proof of cold
cache state**. Model weights, operating-system/JIT/GPU caches and engine state
are not inspected or flushed. Missing usage remains unknown, never zero.

## Deployment declarations and privacy

`--deployment FILE` optionally records a closed JSON object with nullable
`model_revision`, `runtime`, `hardware` and `settings` strings. These are operator
declarations, not verification that a server loaded those bytes. Keep secrets
out of declarations. Use exact public model/runtime references where available.

Runs retain prompts, request bodies and raw provider responses. They are private
evidence, **not automatically safe public exports**. There is no upload command.
Review content and rights before sharing. Comparisons are offline and do not
read credential values or contact endpoints.

## Loopback lifecycle smoke plan

After all concurrent source writers have stopped, run from the workspace root:

```sh
cargo test -p grill-perf --locked --test cli \
  pause_drains_wave_resume_is_exclusive_and_preserves_evidence -- --exact
cargo test -p grill-perf --locked --test cli crashed_and_unstarted_waves_remain_distinguishable -- --exact
cargo test -p grill-perf --locked --test cli local_failure_reserved_suffix_remains_inspectable_but_not_resumable -- --exact
cargo test -p grill-perf --locked --test cli pause_and_wave_admission_share_serialization -- --exact
cargo test -p grill-perf --locked
```

These tests launch the actual CLI against ephemeral literal-loopback fixtures,
never a configured model endpoint. The lifecycle fixture holds both warmup lanes,
requests pause, verifies neither cancellation nor next-wave admission, then
releases the wave and checks exit 2/paused. It damages retained response bytes and
checks that resume refuses before dispatch, restores the synthetic fixture, and
starts a valid continuation. While that continuation is collecting, a second
resume must refuse. Exactly the remaining declared lanes are collected, all prior
raw bytes and receipts remain identical, no warmup is replayed, and comparison
after fixture shutdown returns exit 2 with the session/cache-continuity reason.
The other regressions cover reserved uncertainty and immediate interruption.
Passing these is local lifecycle evidence, not production performance calibration.

See [the measurement/evidence contract](CONTRACT.md) for timing boundaries,
buffering, crash semantics, limits and validation commands.
