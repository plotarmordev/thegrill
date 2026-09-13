# Serving-performance companion

`grill-perf` measures requests to an already-running Chat Completions server.
It does not start servers, download models or grade answer quality. Performance
capture and assessment are separate from `grill` quality evaluation.

## Baseline, change, check

Start with **[Install and first capture](INSTALL.md)** for the authoritative
staged-archive/source-fallback workflow, declarations, optional unchanged control,
candidate check, offline replay and actionable failures. There are no published
releases yet. The default path needs no policy file or statistical settings.
This guide is the detailed measurement and evidence reference.

Use the existing declaration from [the first-run guide](INSTALL.md#supply-the-unavoidable-facts-once):
`model_revision`, `runtime`, `hardware`, `settings`. All must be nonempty.
Prefer meaningful revision IDs or configuration fingerprints; never put credentials
in them. Selecting a declaration field with `--change` requires that value to
differ and the others to match. `--change none` instead requires every field to
match. Multi-field migrations are outside this simple path. A settings
fingerprint cannot prove only one internal knob changed.

The candidate inherits the verified baseline's endpoint, model selector, fixed
workload and authentication-environment **name**. Pass `--auth-env NAME` to
`baseline` when needed; the credential value is never saved. The same collector
binary and measurement contract are required. Declarations are not independently
read from or attested by the server.

### Unchanged-deployment control

Use the [optional control in the first-run workflow](INSTALL.md#baseline-optional-control-change-check):
leave the deployment unchanged and check with the same declaration and
`--change none`.

Any declaration mismatch is rejected before control requests. The capture's
`change` and report's `declared_change` serialize as `"none"` for this control,
distinct from a baseline's absent change (`null`). Older readers may reject
the new explicit value; existing captures and historical results are not rewritten
or reinterpreted. Offline replay uses the same `compare` command as a candidate.

The same workload, budgets, metrics, thresholds and exit semantics apply.
The display labels `MEASURED FASTER` and `MEASURED SLOWER` (existing JSON codes
`IMPROVED` and `REGRESSED`) describe a capture-period shift under an unchanged
deployment, not evidence of a serving-change effect. Retain any directional
control result and investigate chance variation, time drift, load, cache effects
and dependence; do not replace it with repeated runs until a preferred verdict.
An inconclusive control does not establish equality or repeatability.
A directional control result can occur by chance even under the model assumptions;
one such result does not itself prove an assumption or software failure.
Directional labels describe sign, not practical significance: a small observed
change is not automatically a useful upgrade.
Matching declarations remain operator claims, not proof of unchanged server state.

### Default scope and budget

The versioned `baseline-v1.json` workload selects the existing structured count
prompt at C1. It requests streaming, thinking off through the legacy
`chat_template_kwargs.thinking` field, temperature zero, top_p one and **exactly
400** output tokens using vLLM controls. A backend rejecting those controls is
an error, not an invitation to retry with weaker controls. Generated text and
server completion usage can include reasoning; channels remain in the receipts.

Each capture makes eight acquisitions with a fresh client per acquisition.
Each acquisition has one warmup and three measured waves. The fixed ceiling is
**32 requests / 12,800 output tokens / 300 seconds per capture**, including
warmups and collection overhead. There are no probes or recovery requests.
`--seconds 1..3600` changes only the whole-capture time allowance, not the sample
count. The default requires more than **42.7 reported output tokens/s** including
overhead to finish; a slow server can exhaust its budget without producing a
directional conclusion. Choose an affordable allowance before collecting.

Request limits are 60 seconds total and 30 seconds idle, bounded by the remaining
capture allowance. An invalid response stops subsequent waves; admitted peers
settle and their evidence is retained. Time exhaustion stops further dispatch,
retains the active partial request and produces insufficient evidence rather
than a fabricated complete sample. Filesystem publication and OS stalls can
outlast a network deadline. No automatic retries, replacement acquisitions,
continuations, or performance-conditioned early stopping occur in this path.
Repeatedly invoking fresh comparisons until a favorable result appears is not
a valid use of the reported model interval.

### Results and evidence

| Display label | Existing JSON `result` code | Supported interpretation |
|---|---|---|
| `MEASURED FASTER` | `IMPROVED` | The model-based uncertainty range lies above zero: higher measured throughput in these capture periods |
| `MEASURED SLOWER` | `REGRESSED` | The interval lies below zero: lower measured throughput in these periods |
| `COMPLETE - DESCRIPTIVE ONLY` | `DESCRIPTIVE` | Explicit selection completed successfully; no default-C1 direction, equivalence or no-regression verdict |
| `INCONCLUSIVE` | `INCONCLUSIVE` | No direction is established, variation cannot be estimated, or coverage is incomplete; this does not establish equivalence |
| `INVALID` | `INVALID` | Corrupt/incompatible evidence, unexplained declarations or an invalid response prevents assessment |

This is a presentation mapping for the capture workflow, not a new verdict
schema. JSON result codes, numeric observations, interval calculations, thresholds
and exit statuses are unchanged. Existing stored `report.json` and `report.txt`
files are never migrated or renamed. New text reports and human-readable
`compare` output use the display labels, with the verified default structured C1
or explicit selected scope immediately below the headline. If capture loading
fails, scope is explicitly unavailable; it never falls back to the default
workload. Machine consumers should continue to use the JSON codes, not parse
the display text.

Reports keep the observed percentage separate from the uncertainty range and
explain that correlated or drifting measurements can make that range too narrow.
Measured direction does not establish its cause or practical importance.
`INCONCLUSIVE` does not mean equivalent performance. Reports also identify the
declared change, artifact paths and verified request/token accounting. Missing
usage stays unknown. Failure reports contain the first failing request's status,
reported usage, expected output count, errors and raw-evidence path.

A failed capture load remains `INVALID`/exit 1, with no performance verdict or
partial `selected` report. Its accounting remains null. The existing JSON
complete-acquisition counters count verified acquisitions; when the corresponding
accounting is null, their zero values are placeholders, not observed zero coverage.
Human output displays those counts as unavailable. Native-wave duration failures
identify the acquisition, the summed wave duration and the declared window in
microseconds, including the unchanged 1 ms resolution allowance. A mismatch does
not establish a clock-adjustment cause or authorize relaxing integrity checks.

Each root contains immutable `capture.json`, exact workload/deployment bytes and
eight indexed native acquisition directories, including empty unstarted slots.
`report.json` and `report.txt` explain the outcome. Offline comparison verifies
the manifests, native plans, raw responses, collector identities and chronology;
it rejects copied acquisition identities and does not modify stored reports.
Those checks establish internal consistency, not an execution attestation.
The path printed by offline `compare` identifies the recorded report location;
it does not promise that the file still exists or was updated. Its newly printed
JSON is the recomputed result.

The primary observation is the median aggregate throughput of an acquisition's
three measured waves. Let `a` and `b` be the eight log observations from each
capture. The estimate is `exp(mean(b)-mean(a))-1`; the nominal model interval uses
standard error `sqrt(var(a)/8+var(b)/8)` and the conservative df7 two-sided
critical value `2.364624251`. The variances are not pooled. Eight complete
acquisitions on each side are required. At zero estimated standard error, the
observed difference remains visible but interval/confidence are withheld.

This model assumes stable, independent acquisition-level variation and an
adequate log-scale mean model. Fresh clients do not establish independence.
Positive autocorrelation can make the interval too narrow. Sequential captures
cannot separate a serving change from time, cache or load effects. These are
**capture-period conclusions**, not causal certificates, equivalence,
noninferiority or guaranteed detection of a particular percentage change.
Concurrent lanes/waves are never counted as independent statistical samples.

`baseline` exits 0 when ready, 2 when incomplete and 1 when invalid; its
`baseline_ready` field describes capture readiness, not a comparison verdict.
For default C1 `check` and capture `compare`, exits are 0 for `IMPROVED`, 2 for `REGRESSED`
or `INCONCLUSIVE`, and 1 for `INVALID`. Read the structured result rather than
treating exit 2 as a particular verdict. Complete explicit selected comparisons
instead return `DESCRIPTIVE`/exit 0, not a no-regression certificate.
The advanced raw-run/policy commands below retain their distinct semantics.

## Explicit selected captures

Recipes use the same baseline/check lifecycle with one additional baseline
option: `--selection MANIFEST`. See [packaged workload paths](INSTALL.md#optional-selections-and-failures)
and [recipe data and PR reporting](RECIPES.md). `check` has no reselection option: it verifies
and inherits the baseline's retained manifest and workload, endpoint, model
selector and credential-environment name, even outside the checkout.

Selection manifests use a separate closed schema, not the frozen four-entry
`bundle verify` manifest. Every field is required:

| Field | Contract |
|---|---|
| `version` | integer `1` |
| `id` | nonempty ASCII letters/digits/hyphen/underscore, at most 64 bytes |
| `workload` | one leaf filename beside the manifest; no traversal or symlinks |
| `source_sha256` | lowercase SHA-256 of exact workload bytes |
| `workload_sha256` | lowercase SHA-256 of compact typed Workload serialization |
| `scope` | nonblank control-free text, at most 256 bytes |
| `operation_scope` | closed enum `normal`, `stress`, or `unknown` |

The manifest is bounded to 64 KiB and the workload to the existing native
admission limits. There is no provider registry, model-name mapping or field
stripping. Any natively validated bounded workload may be selected, including
the separately versioned [conversation profile](CONVERSATIONS.md). Unsupported schema/controls,
missing deployment declarations, unsafe paths and pin drift fail before
requests. Offline validation detects declared incompatibilities, not whether a
backend actually implements a control. Backend rejection is retained without
retry or weaker fallback.

Selected captures use `performance-capture-v2`; `selection_sha256` binds the
exact retained `selection.json`, which pins raw and normalized workload digests.
Native acquisitions retain their own source, workload and collector identities.
Any manifest byte change, scope change, control change, workload membership
change or collector mismatch prevents comparison. Membership changes are
prospective: approve a new selection and acquire a new baseline rather than
removing an inconvenient cell after collection. The built-in default has no
selection manifest and continues to write capture v1 with its original
structured-C1 inference. Explicitly selecting even that C1 workload is a
different descriptive identity and never enables the default inference.

For v2, report fields `baseline_capture_sha256`/`candidate_capture_sha256` and
the candidate's `baseline_sha256` bind the capture **and** its timing receipt:
SHA-256 of `grill-perf-selected-capture-v2` followed by a NUL byte, the lowercase
raw `capture.json` SHA-256 hex, then the exact `capture-timing.json` bytes.
`CaptureTiming.capture_sha256` itself remains the raw capture-file digest.
Timing changes break an existing candidate link; impossible elapsed observations
below retained native timing bounds are rejected. Default v1 identity remains the
raw capture-file digest.

### Small concurrency ladder

The new [concurrency selection](../../crates/grill-perf/examples/concurrency-selection-v1.json)
pins C1/C2/C4 cells. Each cell has one warmup and three measured waves per
acquisition; eight acquisitions allow **56 warmup + 168 measured = 224 requests**
and **14,336 requested output tokens**. Output is exactly 64 tokens, with
temperature zero, top_p one, seed zero and the legacy thinking-off control.
The [enable-thinking variant](../../crates/grill-perf/examples/concurrency-enable-thinking-selection-v1.json)
changes only the workload name and explicit thinking-control spelling; it is
not an interchangeable workload identity.

Both freeze the prompt `Count from 1 to 32. Output only the numbers, separated
by spaces. No other text.` in every lane. No suffixes or model-dependent
prompts are introduced. Native seed scheduling remains base seed plus trial
times 64 plus lane. All warmups precede measured cells in declared cell/trial
order; each admitted wave settles before publication and the next wave.
`cache: observe` allows prefix sharing and does not establish cold state.
The declared operation scope is **unknown capacity**, not a normal-operation
guarantee. A deliberate stress selection must say `stress`; neither declaration
establishes an SLO or safe production concurrency.

Preflight prints controls, cell membership, warmups/trials/concurrency, total
request/output allowances and native time/buffer limits before collection.
With `--json`, preflight goes to stderr and the result remains JSON on stdout.
The default whole-capture allowance is still 300 seconds; the exact ladder
requires more than 47.8 output tokens/s including overhead to finish within it.
Select an affordable `--seconds` allowance prospectively; no automatic
capacity search, retries or replacement samples occur.

One deadline starts before setup and governs all native acquisitions.
`capture-timing.json` records elapsed time through publication of `capture.json`,
including setup, validation, native collection and publication. An observed
overrun makes the selected capture incomplete even if every native acquisition
completed. Its final timing receipt and report publication are outside that
observation boundary; filesystem and OS stalls have no hard completion
guarantee. These observations are not a universal overhead correction.

See [the finite offline C1 calibration](CALIBRATION.md) for measured simulation
coverage and its assumptions. It does not qualify concurrency or history inference.

### Selected reports

Selected reports use comparison v2 and retain `selected.manifest`, its digest,
controls, limits, ordered cells, baseline/candidate timing receipts and native
per-acquisition summaries. Each acquisition retains all native cell summaries
and ordered waves with complete per-lane status, usage, timing and errors,
including failed/partial lanes and sequence metadata. Missing waves remain null;
unstarted acquisition slots remain in `capture.json`, not replacement samples.

Read each cell separately: planned/observed/eligible trial counts, first generated
and first answer text, terminal/settlement latency, dispatch spread, whole-wave
makespan, aggregate achieved throughput, per-stream settlement decode and
text-window decode rates. These retain the [native boundaries](#interpret-the-result);
decode-only rates are not whole-wave throughput. Undefined metrics remain null.
Sparse trial counts do not support production p95/p99 claims. Concurrent lanes
share a wave and are not independent acquisitions; cells are never pooled.

Complete, valid selected comparisons emit `DESCRIPTIVE` (exit 0) and the terminal
banner **COMPLETE - DESCRIPTIVE ONLY**, not `INCONCLUSIVE` or a performance PASS.
`baseline` exits 0 when ready and labels selected observations descriptive-only.
Invalid evidence remains `INVALID`/exit 1; incomplete captures remain
`INCONCLUSIVE`/exit 2. This completion result belongs to selected comparison v2,
not historical C1 inference or the advanced policy engine. No pooled percentage,
C1 model interval, stronger directional
label, equivalence or noninferiority claim is produced. Sequence correctness
and strict-format checks remain distinct from native performance eligibility.
Raw evidence, endpoints and declarations are private by default.

## Captured observed-envelope policy

`run WORKLOAD --policy FILE ...` validates a bounded policy declaration before
dispatch and saves its exact bytes as `policy.json` before publishing the plan.
Every baseline, candidate and baseline repeat must capture the same policy.
Omitting the option leaves a run unbound; it cannot acquire a policy
retrospectively through `decide`. New runs use plan v3 independently of this option.
`resume` reads the captured declaration, with no replacement-policy option.
Policy-bearing plans require this reader; older readers may reject the new
optional `policy_sha256` field.

The declaration is a closed JSON object, bounded to 64 KiB:

```json
{
  "version": 1,
  "method": "observed-envelope-v1",
  "id": "approved-workload-envelope",
  "collector_sha256": "REPLACE_WITH_ACTUAL_BINARY_SHA256",
  "workload_source_sha256": "REPLACE_WITH_EXACT_WORKLOAD_FILE_SHA256",
  "min_trials": 3,
  "cells": [
    {
      "cell": "REPLACE_WITH_WORKLOAD_CELL_ID",
      "metrics": [
        {
          "metric": "wave_latency_us",
          "max_regression_bps": 0,
          "max_reference_spread_bps": 0
        }
      ]
    }
  ]
}
```

Replace the illustrative pins and cell with real values before admission; both
pins must be lowercase SHA-256 hex. Enumerate every workload cell exactly once.
Each cell declares a nonempty unique subset of `wave_latency_us`,
`achieved_completion_tokens_per_second`, `decode_tokens_per_second` and
`prefill_tokens_per_second`. Metric direction is intrinsic. `min_trials` is
3–100 and cannot exceed any cell's declared measured trials. Decisions require
at least one declared warmup per cell. Regression tolerances are integer basis
points in 0–9999; the observed reference-spread budget is in 0–1,000,000.
100 basis points equals one percent. These are schema bounds, not recommended
scientific margins; the zero values above illustrate strict equality budgets,
not a generally suitable operating policy.

Collection checks the pins against the exact workload source and actual
collector executable. Offline loading checks the captured pins against each
recorded plan, not the evaluator's current executable. The decision separately
identifies that evaluator. Policy bytes, not normalized JSON, determine binding.
Present corrupt or conflicting bindings produce `ERROR`; missing bindings or a
missing repeat produce `INCONCLUSIVE`, never a retrospective application.

```sh
target/release/grill-perf decide results/baseline results/candidate \
  --reference results/baseline-repeat --json
```

This offline command requires distinct, ordered baseline/candidate/repeat
acquisitions, currently qualified `declared_match` baseline/repeat declarations,
complete eligible uninterrupted sessions, complete warmups and measured trials,
and equal ordered trial/lane completion amounts. Copied evidence under another
path does not establish a distinct acquisition. Starts and deployment
declarations do not authenticate nonoverlap, restoration or independent
execution. Operators remain responsible for approval before collection and for
restoring the baseline without overlapping runs.

Every declared gate remains in the result, including unavailable metrics and
partial lanes. Raw verified observations form rational pairs: latency is
`(elapsed_us, 1)`, aggregate throughput is `(completion_tokens, elapsed_us)`,
decode is `(completion_tokens - 1, settle_us - first_generated_text_us)` and
prefill is `(prompt_tokens, first_generated_text_us)`. Availability and cache
rules match the descriptive measurements. Serialized rate ranges use these
tokens-per-microsecond pairs; multiplying by 1,000,000 gives tokens/second.
Zero or undefined intervals and nonpositive reference minima cannot qualify.
A defined zero candidate throughput is not silently dropped, but still must
satisfy the output-amount and eligibility requirements.

Baseline and repeat extrema are pooled as `[L,U]`, candidate extrema as `[l,u]`.
The reference variability gate requires `U/L - 1` within the declared spread
budget. Latency adverse bounds are `[l/U - 1, u/L - 1]`; rate-loss bounds are
`[1 - u/L, 1 - l/U]`. Negative adverse bounds describe observed improvement.
The decision uses checked integer cross-products, not rounded percentages or
floating-point epsilon: `PASS` requires the worst bound within tolerance;
`REGRESSION` requires the best bound strictly beyond tolerance; otherwise the
gate is `INCONCLUSIVE`. Floating adverse bounds are descriptive only.
Arithmetic overflow produces `ERROR`. Reference spread is an operator-selected
observed variability gate, not a confidence bound.

Aggregate precedence is `ERROR`, `REGRESSION`, `INCONCLUSIVE`, then `PASS`.
A qualified regression is not erased by another gate's uncertainty. `PASS`
requires every gate, not a favorable filtered subset. No outcome establishes
statistical significance, causal effect, universal no-regression or live
qualification.

The versioned decision JSON separates eligibility from policy outcome and
includes policy identity, evaluator identity, role-labelled verified evidence
hashes, scope, expected/observed coverage, rational ranges, tolerances and bounded
reason codes. Fingerprints bind exact plan/workload, session states and ordered
wave/raw-response/companion evidence, including missing states. They are not
execution attestations. Identical verified inputs and evaluator yield identical
JSON without a new timestamp, filesystem paths or raw private diagnostics.
Raw evidence and existing descriptive comparison output remain private by
default; a decision summary is not replay evidence.

Only a successfully printed versioned decision envelope constitutes a decision.
For `decide`, exits are `PASS` 0, `ERROR` 1, `INCONCLUSIVE` 2 and `REGRESSION` 3.
Parsing or output failures can share exit values without producing a decision.
Raw-run `compare` exit 0 remains an eligibility-only result, never `PASS`.
Capture comparison uses the result vocabulary above, not this policy envelope.

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

[`sparkdash-decode-v1.json`](../../crates/grill-perf/examples/sparkdash-decode-v1.json) follows [MiaAI-Lab's sparkDash decode protocol](https://github.com/MiaAI-Lab/sparkDash) with its verbatim prompts and 72 requests per run.
It needs a vLLM-compatible server whose chat template honors `chat_template_kwargs.thinking`; check `first_generated_channel` is `answer` (and `reasoning_tokens` where the server reports it) in the receipts, since the tool does not reject reasoning output.
sparkDash appends a per-stream suffix to prompts above concurrency 1; this workload sends identical prompts, so cache mode `observe` permits prefix sharing across lanes. Use `reported-prefix-zero` when provider-reported zero-prefix evidence is required.

[`sparkdash-prefill-v1.json`](../../crates/grill-perf/examples/sparkdash-prefill-v1.json) follows [MiaAI-Lab's sparkDash PrefillBench protocol](https://github.com/MiaAI-Lab/sparkDash/blob/main/server/collectors/PrefillBench.js):
salted header, repeated `" the"` filler and `Reply OK.` footer, thinking off,
temperature zero, top_p one, concurrency 1, and the default 4k/8k/16k/32k sizes.
Filler repeats are the target size minus sparkDash's 26-token header/footer
estimate for our 20–21 character salt. Deviations: 16 requests (one warmup and
three measured trials per size) instead of one request per size; output is
exactly 8 tokens rather than a cap of 8 so runs stay length matched; the salt is
per attempt; thinking is disabled through `chat_template_kwargs.thinking` only,
so check `first_generated_channel` is `answer` in the receipts.
Both selected deadlines remain ten minutes because prefill may send no bytes
before the first token; they are not the admission ceiling. The supported
`total_ms` ceiling is one hour, with positive `idle_ms <= total_ms`. Raise both
explicit declarations when a longer quiet prefill is intended; raising total
alone does not fix idle expiry. This is a bounded policy, not a server-runtime
guarantee, and longer budgets can lengthen cooperative pause/wave drain.

[`prefill-ladder-v1.json`](../../crates/grill-perf/examples/prefill-ladder-v1.json)
is a separate strict-cold ladder with approximate prompt targets of
2k/8k/32k/128k, not a revision of the sparkDash workload. It uses the existing
`" the"` fill generator with an early per-attempt `{salt}`, concurrency 1,
one warmup and three measured trials per size. Repeat counts are size proxies,
not measured tokenizer counts: the template, salt, header/footer and tokenizer
determine actual prompt tokens. Check reported usage and the server's context
limit before interpreting a size label as a token count.

The ladder streams with an output **cap** of 1, not exact output, temperature
zero and top_p one. It explicitly requests
`thinking_control: {"kind":"vllm-enable-thinking-v1","enabled":false}`.
The template must support `chat_template_kwargs.enable_thinking`; this example
does not establish live provider compatibility or prove the setting was honored.
Reported reasoning and observed answer channels remain separate evidence.
Both selected deadlines are 2,700,000 ms, within the 3,600,000 ms ceiling.

`reported-prefix-zero` requires explicit provider-reported zero cached prompt
tokens. Missing or nonzero cache evidence remains ineligible; neither the salt
nor a prefill rate proves a cold cache. Switching to `observe` changes the
workload and its claim. Real tokenizer counts, long-context endpoint support,
thinking-control compliance and live prefill rates remain unverified.

### Offline run preflight

Use `preflight` to admit the workload together with the inputs intended for
`run`, without contacting model or metrics endpoints or creating a capture:

```sh
target/release/grill-perf preflight crates/grill-perf/examples/quick.json \
  --endpoint https://your-server.example/v1/chat/completions \
  --model your-model --auth-env MODEL_API_KEY --deployment deployment.json
```

`--deployment`, `--policy`, `--metrics-url`, `--auth-env` and `--local-http` have
the same local validation as `run`; endpoint and model are required. The
credential named by `--auth-env` must be valid locally, but its value is not
reported. Output is always JSON: raw and typed workload pins, admitted controls,
request and output-token ceilings, planned waves and declaration byte counts.
There is no `--out` or `--json` flag. No execution timestamp or cache identity is
generated. This is declared-run admission, **not backend qualification**:
authorization, model availability and server support remain untested.

`bundle inspect` remains workload-only. Re-run preflight when run inputs change;
a successful check does not bypass `run` admission.

### Larger-prompt buffer sizing

The ladder's response cap is 65,536 bytes, independent of request size.
For streaming, the admitted per-wave allowance is
`concurrency * (2*response_bytes + 6*256KiB + 512KiB + fill_bytes)`,
where `fill_bytes = unit.len()*repeat`. At concurrency 1 the non-fill
allowance is 2,228,224 bytes:

| Approximate target | `" the"` repeats | Fill bytes | Required wave allowance |
|---|---:|---:|---:|
| 128k (shipped) | 131,072 | 524,288 | 2,752,512 bytes |
| 256k (sizing example only) | 262,144 | 1,048,576 | 3,276,800 bytes |

The declared 4,194,304-byte wave budget covers these allowances. The hypothetical
256k row is not a shipped or live-qualified case. Fill bytes exclude the rendered
header/footer, template controls and JSON envelope. Each complete serialized
request must independently fit 2,097,152 bytes; increasing `response_bytes`
does not increase that request cap.

The space/letter fill needs no JSON escaping, but arbitrary fill does: a control
byte can expand to a six-byte `\uXXXX` escape. A 524,288-byte control-character
fill can therefore require 3,145,728 bytes before the envelope and fail the
request cap. Reservation receipts embed request bodies as JSON strings, escaping
quotes and backslashes again. Admission also bounds those escaped strings times
concurrency within the reservation allowance described in the
[contract](CONTRACT.md#workload-admission).
Rendered messages, encoded requests and escaped reservation copies coexist
during preparation; the wave formula counts fill only once and is not a peak
RSS guarantee. Serialized reservation buffers are released before dispatch.
Neither the response cap nor the wave allowance proves server context capacity.

For a smaller compatibility probe, use
[`recipe-smoke.json`](../../crates/grill-perf/examples/recipe-smoke.json):
concurrency 1 and 2, a 64-token cap, and nine requests per run.
The [two-recipe qualification and GLM A/A receipt](2026-09-09-recipe-smoke.md)
shows successful collection alongside substantial timing variation without a
deployment change. It is a smoke example, not an optimization result.

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
- **Settlement decode rate** (`decode_tokens_per_second`) keeps its original
  `(completion_tokens - 1) / (settle - first generated text)` definition.
  It includes terminal usage/`[DONE]` processing; old policies retain this meaning.
- **Text-window decode rate** (`text_decode_tokens_per_second`) uses
  `(completion_tokens - 1) / (last generated text - first generated text)`.
  New v3 evidence retains the last-text timestamp; older evidence has no
  invented value. Missing/coincident endpoints give null, not zero or infinity.
  The arithmetic matches sparkDash only for identical usage and supplied
  text-event timestamps. Native parser clocks, framing and aggregate definitions
  are not thereby interchangeable.
- **Per-stream prefill rate** is defined to match sparkDash `prompt_tokens / TTFT`,
  from dispatch to first generated text, including queueing and first-token
  generation. It is null when the provider reports nonzero cached prompt tokens;
  a provider that omits `cached_tokens` is treated as uncached, so under `observe`
  with unsalted prompts the rate can include prefix-cache hits. Use `fill` with
  `{salt}` or `reported-prefix-zero`.
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
- Min/max ranges accompany complete-cell medians. A matched percentage is shown
  only when the runs' ranges do not overlap; `withheld` explains overlap without
  making an otherwise comparable cell ineligible. Observed ranges are descriptive,
  not a significance or equivalence test; a withheld change is not evidence of
  equality. Repeat setup A and pass `--reference results/setup-a-repeat` only
  with complete matching model, endpoint and deployment declarations. A qualified
  repeat widens the baseline range with both A runs before the overlap test.
  `drift` reports the qualified A-to-repeat median change, not a validated noise
  bound. Matching declarations do not verify server restoration, cache state,
  run timing order or causal effect.
- A supplied reference never silently falls back to A/B alone. Invalid reference
  files are errors; incomplete evidence, unequal ordered output counts or missing
  or mismatched declarations make the reference-aware cell ineligible, with
  reasons. Summaries remain inspectable. Missing decode/prefill lane observations
  on any side withhold those reference-aware metrics and drift, even when other
  lanes have measurements. Omit `--reference` explicitly for ordinary A/B.
- A compatible comparison requires the same normalized workload, collector
  binary fingerprint and transport controls. Model/endpoint deployments may
  differ; this is a descriptive deployment comparison, not causal attribution.

For raw-run commands, exit `0` means a complete eligible run/comparison; `2` means retained but
ineligible/incomplete evidence, and `1` means CLI usage, admission, integrity or local I/O
failure. No automatic retry, redirect, proxy, parameter fallback or winner
selection occurs. SIGINT and SIGTERM stop further admission and settle active
attempts. In new v3 runs, a response or measurement-eligibility failure stops
subsequent waves after the admitted peers settle. Legacy evidence retains its
original interpretation; neither path retries failed work.

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

Warmup is necessary for `reported-prefix-hit`, but does not guarantee a hit.
Servers may reuse only complete cache blocks, with engine-specific alignment
requirements. A short prompt can therefore report zero cached tokens after priming.
Consult the server's cache mechanism and inspect `cached_prompt_tokens`; use a
longer appropriately aligned prompt when testing warm decode, without assuming
length alone guarantees reuse.

If cache hits are not required for your measurement, use a separate `observe`
workload. That changes the workload and the claim; it does not repair an
ineligible `reported-prefix-hit` run or make the two workloads comparable.

Required prefix modes need `vllm-fixed-v1`. Plans name the declared mechanism
and the provider observation source; the response record retains the actual
reported count. A flag plus a reported zero is **not universal proof of cold
cache state**. Model weights, operating-system/JIT/GPU caches and engine state
are not inspected or flushed. Missing usage remains unknown, never zero.

## Phase-specific output budgets

Flat workload **version 3** optionally declares `request.warmup_output`; measured
requests retain `request.output`. For a separate capped warmup32/measured400
workload, create a new file rather than editing a frozen example:

```sh
jq '.version = 3 | .name = "phase-output-v3" |
    .request.warmup_output = {"tokens":32,"mode":"cap"} |
    .request.output = {"tokens":400,"mode":"cap"}' \
  crates/grill-perf/examples/quick.json > phase-output-v3.json
target/release/grill-perf bundle inspect phase-output-v3.json
```

Run [offline preflight](#offline-run-preflight) with the intended run inputs.
For baseline/check, use the returned raw and typed pins in a new
[explicit selection manifest](#explicit-selected-captures). Keep the same pinned
workload throughout the native baseline/control/candidate relationship.

Absent `warmup_output` uses `output` for both phases. Explicit null is invalid;
versions 1 and 2 reject the field. Each budget is 1..32,768 tokens, and exact
output in either phase requires the fixed-output profile. Conversation v2 is a
separate contract, not extended by v3.

Requests, usage eligibility and retained evidence use the actual wave's budget.
Warmup remains retained but excluded from measured results. Ceilings sum warmup
requests times the warmup budget plus measured requests times the measured
budget. A full-capture minimum completion rate is inferred only when every
planned phase is exact; an unused warmup declaration does not affect it.
Changing either phase's controls changes workload identity and prevents a
matched comparison. Existing exact400 recipes and hashes remain unchanged.
Matching token counts alone is [not sparkDash protocol equivalence](SHARED-RECIPES.md#phase-counts-are-not-protocol-equivalence).

## Optional provider snapshots

Add `--metrics-url https://your-server.example/metrics` to `run` to retain bounded
vLLM Prometheus diagnostics. The endpoint follows the model transport's URL/TLS
policy, but uses a separate client and **never receives model authorization**.
There is no discovery, redirect, retry, idle gate or additional model request.
An endpoint requiring authentication will report a scrape failure; this option
does not add metrics credentials.

The fixed allowlist contains `vllm:spec_decode_num_draft_tokens_total`,
`vllm:spec_decode_num_accepted_tokens_total`, `vllm:num_requests_running` and
`vllm:num_requests_waiting`. Counters are matched by name and the complete
canonical label set, never summed across engines/models. Before/after samples,
counter deltas, null reset/missing results and compatible acceptance ratios
appear in `compare RUN RUN --json` under `baseline_metrics`; gauges remain
snapshots. A nondecreasing counter does not prove that no restart occurred.
These are **server-wide diagnostics, not this workload's attributed tokens**.
Other clients can contribute to the same series.

The before scrape follows durable wave reservation and finishes before the
measured origin. The after scrape starts only after all lanes settle, including
failed/interrupted lanes. Scrape timestamps, durations and total telemetry
overhead are separate from measured wave latency. **Outside the timer is not
measurement-neutral**: scraping and evidence publication can change server load,
cache warmth and between-wave cadence. Comparison requires identical telemetry
configuration, including endpoint identity; opted-in and opted-out runs do not
silently qualify as matching conditions.
Signals still cancel model lanes, but telemetry is deadline-bounded rather than
signal-cancelled: an active snapshot and the required after snapshot can delay
exit. Cooperative pause continues to drain and publish the whole admitted wave.

The frozen protocol allows a deadline of at most 2 seconds per scrape, 1 MiB
body, 64 KiB line, 256 selected sample series, 16 labels and 4096 encoded label-set
bytes per selected series. Comments and unselected samples consume body/line
bounds but are skipped before label/value parsing and selected-series accounting. The whole-run
telemetry allowance is 30 seconds and retained raw data is at most 16 MiB,
with at most twice the planned wave count in requests. Each new scrape gets
the smaller of its deadline and remaining time. Elapsed capture/parse and raw
publication time are charged, not a full deadline for fast scrapes. Companion
publication overhead is charged before the next wave. Raw admission reserves a full
body allowance. Exhaustion retains `skipped_budget`, without another call.
Filesystem synchronization and OS scheduling cannot be given a hard wall-clock
guarantee; actual overhead is retained, and scheduling overrun consumes the
remaining scrape allowance rather than renewing it.

Resume reconstructs consumption from verified retained snapshots, not a new
budget. Unsettled, interrupted or partially published work is not resumable.
Scrape failures preserve bounded raw bytes and errors without changing otherwise
valid performance eligibility. Missing promised evidence, changed hashes and
local publication failures remain integrity/I/O errors.

Metrics endpoints, labels and raw bodies can expose private model names, request
identifiers or other server content, including unsupported metrics and comments.
Review all retained bytes before sharing; the allowlist is not a privacy filter.
See the [snapshot schema and bounds](CONTRACT.md#provider-snapshot-protocol).

## Deployment declarations and privacy

`--deployment FILE` optionally records a closed JSON object with nullable
`model_revision`, `runtime`, `hardware` and `settings` strings. These are operator
declarations, not verification that a server loaded those bytes. Keep secrets
out of declarations. Use exact public model/runtime references where available.
Each supplied string must be nonempty, control-free and at most 4,096 UTF-8
bytes. Rejections name the field and observed byte count without echoing its
contents. Keep declaration summaries within the bound rather than embedding
full configuration dumps.
Reference comparison requires every field on both A runs: omitted deployment or
nullable fields yield identity `unavailable`, not a match. Known differences
yield `declared_mismatch`; complete equal declarations yield `declared_match`.
Comparison JSON exposes these statuses and reasons under `reference_identity`
alongside `reference_model` and `reference_deployment`. Its output version
advances for this reference qualification contract; saved receipt formats do
not change.

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
