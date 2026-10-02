# Serving-performance companion

`grill-perf` measures requests to an already-running Chat Completions server.
It does not start servers, download models or grade answer quality. Performance
capture and assessment are separate from `grill` quality evaluation.

Explicit startup/bootstrap and operator-restart cache studies use the separate
[`startup` domain commands](STARTUP.md), not serving Waves or the default
baseline/check workflow. Their local-process producer and CPU fixtures do not
qualify real recipe startup, hidden-warmup suppression or persistent model KV.

## Baseline, change, check

Start with **[Install and first capture](INSTALL.md)** for the authoritative
published-prerelease or reviewed staged-archive workflow, source fallback,
declarations, optional unchanged control, candidate check and offline replay.
Pin the exact version and target. The default path needs no policy file or
statistical settings.
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
binary and measurement contract are required, except for a
[deployment comparison](#deployment-comparison). Declarations are not
independently read from or attested by the server. `baseline` records the
declared `--client-placement` (`same-host` or `network`).

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

### Deployment comparison

To compare two deployments as a whole, for example one model on an Apple Silicon
Mac and on an NVIDIA host, follow the [A/B/A2 steps](INSTALL.md#compare-two-deployments-aba2):

1. A: `baseline` on the first deployment, usually `--workload portable-v1`.
2. B: `check A --change deployment` against the second deployment, with its own
   `--endpoint`, `--model`, declaration and the same `--client-placement`. It
   reports `VERDICT PENDING` (JSON `PENDING`, exit 0).
3. A2: `check A --change none` on the unchanged first deployment, after B.
4. `compare A B --reference A2` gives `MEASURED FASTER`/`SLOWER` only when B
   differs in the same direction from both A and A2.

Both collectors must be the same version built from the same recorded source
commit; their binaries may differ. The [deployment comparison contract](CONTRACT.md#capture-v3-and-deployment-comparison)
defines admission, the 60-second ordering, the verdict rule and what the
whole-deployment scope does not support.

### Output identity

A speedup such as speculative decoding is lossless only if the text is
unchanged. `grill-perf outputs A B [--json]` checks that offline for two run
directories, or two captures paired acquisition by acquisition, of the same
greedy workload, for example drafts off (A) and on (B):

```sh
grill-perf outputs before after
```

Each measured lane is `identical`, `differs` (first differing character and
byte, with excerpts) or `unavailable`. Exit 0 means every lane is identical; 2
means a lane differs or could not be compared; 1 means the inputs are invalid or
not comparable. Endpoints, models and collectors may differ. The
[output identity contract](CONTRACT.md#output-identity) defines admission and
what is compared.

### Default scope and budget

The versioned `baseline-v2.json` workload, the default for `baseline --workload`,
selects the existing structured count prompt at C1. It requests streaming,
thinking off through `chat_template_kwargs` with both the `thinking` and
`enable_thinking` keys (thinking control `chat-template-thinking-v1`), temperature
zero, top_p one and **exactly 400** output tokens using vLLM controls. It differs
from the earlier default `baseline-v1` only in that control, which sent the legacy
`thinking` key alone. A backend rejecting those controls is
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
| `VERDICT PENDING` | `PENDING` | A complete deployment candidate; the verdict needs the unchanged reference ([deployment comparison](#deployment-comparison)) |
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
instead return `DESCRIPTIVE`/exit 0, not a no-regression certificate. A complete
`check --change deployment` returns `PENDING`/exit 0.
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

To pin a custom workload, run `grill-perf preflight WORKLOAD --endpoint URL --model NAME`.
It validates the workload offline, sends no request and prints both digests
(`source_sha256` and `workload_sha256`); put them in a manifest beside the
workload file:

```json
{
  "version": 1,
  "id": "my-decode-v1",
  "workload": "my-decode-v1.json",
  "source_sha256": "<source_sha256 from preflight>",
  "workload_sha256": "<workload_sha256 from preflight>",
  "scope": "Descriptive C1 decode on my coding prompts.",
  "operation_scope": "normal"
}
```

The manifest is bounded to 64 KiB and the workload to the existing native
admission limits. There is no provider registry, model-name mapping or field
stripping. Any natively validated bounded workload may be selected, including
the separately versioned [conversation profile](CONVERSATIONS.md). Unsupported schema/controls,
missing deployment declarations, unsafe paths and pin drift fail before
requests. Offline validation detects declared incompatibilities, not whether a
backend actually implements a control. Backend rejection is retained without
retry or weaker fallback.

Selected captures (`performance-capture-v2`, and v3 with a selection) bind the
exact retained `selection.json` through `selection_sha256`; it pins raw and
normalized workload digests.
Native acquisitions retain their own source, workload and collector identities.
Any manifest byte change, scope change, control change, workload membership
change or collector mismatch prevents comparison. Membership changes are
prospective: approve a new selection and acquire a new baseline rather than
removing an inconvenient cell after collection. Built-in captures (v1, and v3
naming a built-in) have no selection manifest and keep their structured-C1
inference. Explicitly selecting even that C1 workload is a
different descriptive identity and never enables the default inference.

For selected captures, report fields `baseline_capture_sha256`/`candidate_capture_sha256` and
the candidate's `baseline_sha256` bind the capture **and** its timing receipt:
SHA-256 of `grill-perf-selected-capture-v2` followed by a NUL byte, the lowercase
raw `capture.json` SHA-256 hex, then the exact `capture-timing.json` bytes.
`CaptureTiming.capture_sha256` itself remains the raw capture-file digest.
Timing changes break an existing candidate link; impossible elapsed observations
below retained native timing bounds are rejected. Built-in capture identity
remains the raw capture-file digest.

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

### Portable local Chat Completions C1

The [portable selection](../../crates/grill-perf/examples/portable-chat-selection-v1.json)
pins the adjacent [`portable-chat-v1` workload](../../crates/grill-perf/examples/portable-chat-v1.json).
It sends one streaming C1 cell with one warmup and three measured trials per
acquisition, a 64-token output cap and `cache: observe`, using the existing
conservative native limits. Across eight acquisitions that permits 32 requests
and 2,048 requested output tokens.

The request uses `max_tokens` and streaming usage collection. It deliberately
omits exact-output `min_tokens`/`ignore_eos`, thinking controls, cache-prefix
controls, `cache_salt` and a seed requirement. There is no backend detector,
field-stripping retry or provider-specific fallback. The synthetic repetition
prompt is intended to reach the cap, but cap delivery and reported usage remain
observed backend evidence, not attested facts.

This selection declares unknown capacity and is descriptive-only: completion
does not qualify the backend or produce a faster/slower, equivalence or
no-regression verdict. See the [installation guide](INSTALL.md#optional-selections-and-failures)
for current-source versus next-reviewed-release paths.

### Realistic decode

The [realistic decode selection](../../crates/grill-perf/examples/realistic-decode-selection-v1.json)
pins [`realistic-decode-v1`](../../crates/grill-perf/examples/realistic-decode-v1.json):
four ordinary coding questions and two whole-file edits of one fixed 5.8 KB
Python module (change every `logger.debug(` to `logger.info(`; rename `qty` to
`quantity`), each its own C1 cell with one warmup and three measured trials. It
uses `portable-chat-v1` with a 400-token [`cap-reached`](#exact-output-and-cache-observations)
output, greedy sampling and thinking off. Eight acquisitions permit 192 requests
and 76,800 requested output tokens.

Speculative decoding accepts far more drafts on the count prompts of
`portable-v1` and `baseline-v2` than on these questions, and a reply that
copies its prompt, as a file edit does, rewards drafting from the context. Read
the chat and edit cells separately and never pool them with count prompts.
The selection is descriptive only.

### Concurrency ladder

The [concurrency ladder selection](../../crates/grill-perf/examples/concurrency-ladder-selection-v1.json)
pins [`concurrency-ladder-v1`](../../crates/grill-perf/examples/concurrency-ladder-v1.json):
one ordinary coding question in every lane at C1, C2, C4 and C8, each cell with
one warmup and three measured waves. It uses `portable-chat-v1` with a 400-token
[`cap-reached`](#exact-output-and-cache-observations) output, greedy sampling
and thinking off. Eight acquisitions permit 480 requests and 192,000 requested
output tokens. Unlike the [small ladder](#small-concurrency-ladder)'s 64-token
count replies, each lane decodes long enough to measure sustained multi-stream
decode as well as admission. The selection is descriptive only.

### Long-context decode and prefill

Two selections cover long prompts with portable controls only.
[`long-context-decode-v1`](../../crates/grill-perf/examples/long-context-decode-selection-v1.json)
decodes an ordinary coding answer after about 32K, 48K and 96K filler tokens,
with a 256-token [`cap-reached`](#exact-output-and-cache-observations) output.
[`prefill-ladder-96k-v1`](../../crates/grill-perf/examples/prefill-ladder-96k-selection-v1.json)
reads about 2K, 8K, 32K and 96K filler tokens for one `cap-reached` token. It
keeps the prompt shape of
[`prefill-ladder-v1`](../../crates/grill-perf/examples/prefill-ladder-v1.json)
but ends at 96K, so it fits a 131,072-token window, and uses `portable-chat-v1`
instead of vLLM cache controls. Each C1 cell has one warmup and three measured
trials, with greedy sampling and thinking off. Eight acquisitions permit 96
requests and 24,576 output tokens for decode, 128 requests and 128 output
tokens for prefill.

Filler is one `" the"` per token on common tokenizers; the counts are
approximate. `cache: observe` sends no cache control: a per-request `{salt}` at
the start of the prompt keeps it from matching an earlier prefix, while any
reported cached tokens remain visible in the evidence. Both are descriptive only.

### Long-context recall

[`long-context-recall-v1`](../../crates/grill-perf/examples/long-context-recall-v1.json)
checks that a server still reads a long prompt correctly. Each of six ordered
C1 steps places one fixed access code before or after about 16K, 48K or 96K
filler tokens and asks for it back; the answer is graded as the exact
[JSON fact](CONVERSATIONS.md), and the first wrong answer stops the run. Steps
run from the shortest and nearest record to the 96K record at the start of the
prompt, so a stop names the length and depth where recall failed. Three
repetitions give 18 requests with a 32-token cap, greedy sampling and thinking
off.

Recall needs more than a selected capture's 128 KiB conversation request, so it
is a workload-6 conversation acquisition: run it with
`grill-perf run long-context-recall-v1.json` and compare runs descriptively.
The profile sends `cache_salt`, which servers without it may ignore; a cached
prefix does not change the graded answer.

### Selected reports

Selected reports use comparison v2 and retain `selected.manifest`, its digest,
controls, limits, ordered cells, baseline/candidate timing receipts and native
per-acquisition summaries. Each acquisition retains all native cell summaries
and ordered waves with complete per-lane status, usage, timing and errors,
including failed/partial lanes and sequence metadata. Missing waves remain null;
unstarted acquisition slots remain in `capture.json`, not replacement samples.

Read each cell separately: planned/observed/eligible trial counts, first generated
and first answer text, terminal/settlement latency, dispatch spread, whole-wave
makespan, aggregate achieved throughput, per-stream settlement decode,
text-window decode and prefill rates. These retain the [native boundaries](#interpret-the-result);
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

For installed-artifact commands, follow the
[prospective A/B/A2 workflow](SHARED-RECIPES.md#one-baselinechangecheck-decision-path).
It accepts an explicitly chosen workload; selected `baseline`/`check` captures
remain a separate descriptive path.

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

### Policy2 first-output and finite fairness gates

Policy `"version": 2` with `"method": "observed-envelope-v2"` uses the same
prospective pins, A/B/A2 qualification, complete warmups and measured repetitions,
exact ordered completion-token matching, tolerance arithmetic and exit meanings.
It emits decision JSON version 2. Policy1 remains version 1 with its original
four metrics and output shape; the default C1 and selected descriptive workflows
are unchanged.

Policy2 supports homogeneous flat workloads1/3 and named scheduled workload4.
In addition to the four old metrics, each cell may declare these lower-is-better
gates:

| Metric | Exact sample and population |
|---|---|
| `first_generated_text_us` | `(first_generated_text_us, 1)` per eligible measured lane, including generated reasoning text |
| `first_answer_text_us` | `(first_answer_text_us, 1)` per eligible measured lane, distinct from first generated reasoning |
| `completion_latency_us` | `(settle_us, 1)` per eligible measured lane: client completion, not terminal, last text or whole-wave elapsed time |
| `worst_lane_first_answer_us` | `(max(first_answer_text_us), 1)` once per complete measured wave |
| `first_answer_max_min_ratio` | `(max(first_answer_text_us), min(first_answer_text_us))` once per complete measured wave |

All times are relative to that lane's dispatch. Dispatch offsets are never added
to service latencies. Missing or zero observations are unavailable; headers,
first body, terminal and settlement cannot substitute for missing first text.
Worst-lane and max/min require at least two admitted lanes, every lane complete
and eligible, and every first-answer latency positive. Admission rejects these
two gates on a single-lane cell. No survivor-only maximum or ratio is evaluated.

Coverage distinguishes wave repetitions from lane observations. The three-trial
minimum counts acquired waves, never three peers inside one wave. Each fairness
gate produces one sample per repetition; per-lane extrema remain descriptive
observations within those repetitions, not an independent-lane confidence model.
Policy2 gate `sample_unit` states this distinction. Max/min is dimensionless:
its numerator is the slowest first-answer service latency and denominator the
fastest within the same wave; a smaller ratio does not alone establish a better
worst latency, so declare both gates when both properties matter.

Actual reported-hit eligibility permits first-generated/answer latency gates.
It does not create a cold-prefill sample: positive reported cached tokens still
make `prefill_tokens_per_second` unavailable. Equal latency or passing a narrowly
declared gate does not prove cache attribution, semantic correctness or general
fairness. Failed output/cache eligibility and unequal ordered output amounts
remain blocking reasons rather than exclusions.

For workload4, `cells[].cell` selects the exact scenario ID. Per-lane latency
gates require `metrics[].lane` with an exact lane ID; fairness aggregates forbid
`lane` and require an overlap scenario. Every scenario has a nonempty policy
entry, including solos. Empty legacy cells cannot produce PASS. The four
historical throughput/wave metrics are forbidden on heterogeneous schedules:
unlike lane token rates are never pooled into a decision.

The matched solo must have identical case, full effective settings, warmup and
trial counts, and equal observed output amounts in the same phase/repetition.
Every required solo and overlap acquisition must qualify. Each generated-output
dependency must show the source's generated-text interval positively intersecting
the target's prefill interval. A fixed-offset overlap lane must participate in a
positive mixed decode/prefill interval. In-flight overlap or a delayed terminal
alone is insufficient. Missing controls or false required overlap withhold gates.
The report retains the `lane` selector and `sample_unit` for each gate.
Homogeneous workloads reject `lane`; all present-null selectors are invalid.
Policy1 rejects all new metric names and lane fields.

Policy2 also accepts a prospective root `required_telemetry` array. It requires
metrics version 2 and retains a separate assessment for every A/B/A2 role.
Missing promised observations yield INCONCLUSIVE; a candidate's refuted
zero-counter requirement yields REGRESSION; a refuted baseline or repeat
withholds reference qualification. Invalid required evidence yields ERROR.
Optional diagnostics still do not alter eligibility. See
[provider accounting](PROVIDER-ACCOUNTING.md#required-telemetry-in-policy2) for
the exact selectors, isolation declaration and source limits.

### Workload6 acquisitions and policy3

Native `preflight`, `run`, raw-run `compare` and `decide` admit workload6 under
plan5/reservation2/wave2 with `generated-text-arrival-v2`. Workloads1–5 and
policies1/2 retain their historical limits and meanings; neither policy1 nor
policy2 accepts workload6. Legacy selected capture/baseline/check reject
workloads4–6 before dispatch. Workload6 cannot resume a partial acquisition.

The required workload root `acquisition` is a closed tagged object:

```json
{"kind":"flat","input_bytes":2097152}
```

or:

```json
{
  "kind":"conversation",
  "repetitions":3,
  "warmup_repetitions":1,
  "measured_steps":["lookup","followup"],
  "input_bytes":1048576,
  "retained_history_bytes":1048576
}
```

There is no schedule/acquisition combination. Flat mode allows only
`portable-chat-v1` or `vllm-fixed-v1`, cell trials1–1000 and warmups0–20.
Conversation mode requires `vllm-conversation-v3`, repetitions1–1000,
warmup_repetitions0–20, at most128 ordered steps, and cells C1/trials1/warmup0.
`measured_steps` is a nonempty unique list of **case IDs**, not cell IDs.
All other required primes, controls, tools and followups still qualify and
consume traffic; they are excluded only from step performance gates.

Input caps are positive and at most2MiB, retained shared history at most256MiB,
source at most4MiB, capture attempts and waves each at most10000. Old workloads
keep their former1024-wave and128KiB content/history limits. Input bytes mean
actual encoded bytes, not tokenizer counts or128K tokens. Preflight checks all
expanded declared inputs. Unknown actual parent outputs consume the declared
input/history allowance at runtime, never an expected-answer substitute.
Exceeding input allowance retains `acquisition-failure.json` with version1,
exact `wave` and replay-checked `detail`; exceeding retained output/history
allowance retains the response and failed sequence check. No truncated prompt,
replacement acquisition or hidden retry is used.

Preflight's `acquisition_budget` reports `warmup_requests`, `control_requests`,
`measured_requests`, `output_token_ceiling`, `encoded_input_byte_ceiling`,
`response_byte_ceiling`, `tool_trace_byte_ceiling`, `wall_time_ceiling_us`,
`retained_history_bytes` and `serialized_bounds_not_rss_guarantees`.
The last flag is always true: allocator/process RSS is not certified by these
serialization/buffer bounds. Active input, response/parser and tool trace buffers
plus shared retained history must fit `limits.wave_buffer_bytes`. Shared message
payloads use immutable Rc ownership; branches do not duplicate every prefix.
The wall allowance is the sum of declared wave total deadlines, enforced across
the capture including between-step work; blocking filesystem publication remains
cooperative rather than a hard real-time guarantee.

Each workload6 WaveSpec contains `acquisition: {"phase":"warmup"|"measured",
"index":N}`. Flat identity is scoped by cell ID. Conversation steps all share
their acquisition phase and index; prime/control status never changes Phase.
All steps settle in order before another acquisition begins. Conversation cache
namespace is SHA256 of UTF-8
`grill-acquisition-v1:{original_namespace}:{phase}:{index}`; phases are exactly
`warmup`/`measured`, index is decimal, and the existing history suffix follows
the digest. State resets between acquisitions; actual acquired parent outputs
and fixed-tool call IDs/results remain linked inside each acquisition.

Every new Wave records `acquisition_clock: {clock_id, kind, units,
started_offset_us, settled_offset_us}`. `clock_id` is the exact parent plan
SHA256, `kind` is exactly `std_instant_monotonic`, and `units` is exactly
`microseconds`; the clock origin is capture-local, not Unix time. Recorded
microsecond quantization is not a claim of hardware clock resolution. Existing
Timing fields remain lane/wave-relative. Replay checks ordered offsets and exact final lane
settlement. Whole-conversation wall time is last required settlement minus first
required step start, including controls and inter-step work, excluding observer
setup before that first step. It is not summed step latency or a token-rate mean.
Raw comparison JSON version5 adds `acquisition` baseline/candidate/reference
reports, each with `protocol`, `scope` and all declared `records`. Records retain
identity, flat cell, required/measured/missing/ineligible steps, start/settlement,
complete eligibility and nullable `whole_conversation_wall_us`. Incomplete
acquisitions have no favorable whole sample. Cell summaries separately label
`performance_measured` and retain every measured-repetition `sequence_checks`.

Policy `"version":3`, `"method":"observed-envelope-v3"` requires workload6,
min_trials3–1000, and at least one warmup per gated population. Conversation
population counts come from protocol repetitions, not cell.trials. Policy cells
cover exactly the cells whose case IDs are in measured_steps (all cells in flat
mode). Existing per-cell metrics remain explicit; `first_tool_delta_us` and
`first_validated_tool_call_us` are additional lower-is-better metrics admitted
only on measured fixed-tool steps. They never borrow first text, headers or
terminal timestamps. Failed required warmups/controls invalidate qualification.

Optional root `whole_conversation` is
`{"max_regression_bps":500,"max_reference_spread_bps":1000}` and is illegal in
flat mode. Optional nonempty `tail` has at most256 entries:

```json
{
  "target":{"kind":"completion","cell":"c1"},
  "percentile":"p95",
  "max_regression_bps":500,
  "max_reference_spread_bps":1000
}
```

`completion` is flat-only. Conversation targets are exactly
`{"kind":"whole_conversation"}`. Percentile is exactly `p95` or `p99`; duplicates
of target/percentile and first-output tail selectors are rejected. Completion
population is one maximum settle_us across **all** complete eligible lanes per
measured wave. Conversation population is one complete required acquisition wall
duration. Warmups are never samples. p95 requires at least200 samples and p99
at least1000 in each A/B/A2 role and target; insufficient populations yield
INCONCLUSIVE with no statistic. Missing any declared acquisition also withholds
the point estimate instead of computing a survivor-only percentile.

Statistic is the sorted value at 1-based `ceil(p*n)`, no interpolation. The floor
supplies at least10 nominal upper-tail observations; it is **not** a confidence,
precision, IID, causal or production-percentile guarantee. Policy3 decision JSON
adds optional `whole_conversation` and `tail` gates, retaining target, percentile,
population counts, separate A/A2 statistic ranges, candidate range and the exact
reference-envelope decision. Gate fields retain existing threshold/reason/exit
meanings and ERROR > REGRESSION > INCONCLUSIVE > PASS precedence. All ordered
required outputs must match amounts across roles, including conversation
controls and warmups. Root `required_telemetry` carries forward unchanged from
policy2; optional diagnostics never turn unavailable required evidence into PASS.

The explicit source examples are `crates/grill-perf/examples/flat-acquisitions-v6.json`
and `crates/grill-perf/examples/conversation-acquisitions-v6.json`. This command
is offline admission only; it does not connect to the example port:

```sh
grill-perf preflight crates/grill-perf/examples/conversation-acquisitions-v6.json \
  --endpoint http://127.0.0.1:9/v1/chat/completions \
  --model synthetic-fixture --local-http --json
```

After an operator separately provides the intended endpoint and allowance,
`run` takes the same workload/options plus `--out NEW_RUN` and an optional
prospectively pinned `--policy POLICY.json`. No command launches a model service.
Use `compare A B --reference A2 --json` for descriptive replay and
`decide A B --reference A2 --json` for the captured policy verdict.

Source implementation and authored CPU fixtures are not execution qualification.
New real-provider, empirical-tail, larger-history and adoption claims still need
their separately authorized evidence and independent review.

## Build and use

Portable serving collection supports Linux and Apple Silicon macOS source builds
with Rust/Cargo 1.98 and the native build tools required by rustls/AWS-LC.
Published archives, the Linux `/proc`/cgroup/NVML resource sources and the
external-program collector remain Linux-only. On macOS, `baseline`, `check`,
ordinary `run`/lifecycle, offline comparison, and bundle inspection are
supported; resource capture and attachments observe only the
[macOS resource sources](RESOURCES.md#macos-sources), and Linux sources fail closed.

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

[`sparkdash-decode-v2.json`](../../crates/grill-perf/examples/sparkdash-decode-v2.json) follows sparkDash's prompts since its release 1.8.7 (pinned at [`b4228a33`](https://github.com/MiaAI-Lab/sparkDash/blob/b4228a330a7877dcb5a30516500d57e26affa45a/src/shared/llmPrompts.js)).
Only the code prompt changes: sparkDash replaced the 50 identical `clamp_NN` helpers, which drafts predict almost perfectly, with a real Python task (`binary_search`).
Everything else, including the deviations above, matches v1. sparkDash also gives each concurrent code stream a different task and warms the code type up on a separate prompt; v2 runs code at concurrency 1 and warms up on the measured prompt. Results are not comparable to v1's code cell.

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

### Portable sparkDash copies

The [`sparkdash-decode-portable-v1`](../../crates/grill-perf/examples/sparkdash-decode-portable-selection-v1.json),
[`sparkdash-decode-portable-v2`](../../crates/grill-perf/examples/sparkdash-decode-portable-selection-v2.json)
and [`sparkdash-prefill-portable-v1`](../../crates/grill-perf/examples/sparkdash-prefill-portable-selection-v1.json)
selections run the sparkDash workloads above on any Chat Completions server,
including MLX servers on Apple Silicon. Prompts, sizes, cells, trials and limits
are byte-identical to `sparkdash-decode-v1`, `sparkdash-decode-v2` and
`sparkdash-prefill-v1`; only the request controls change, to the `portable-chat-v1`
profile with the `chat-template-thinking-v1` control off. Use the decode v2 copy to
match current sparkDash. Decode uses a 400-token
[`cap-reached`](#exact-output-and-cache-observations) output instead of forcing
exactly 400 tokens, so a reply that ends early makes its lane ineligible rather
than unequal; the count-to-200 prompt can end before 400 tokens with tokenizers
that merge whole numbers. Prefill caps output at 8 tokens: its rate depends only
on the prompt and the time to first token.

### Generated-prose prefill

The [`prefill-prose-portable-v1` selection](../../crates/grill-perf/examples/prefill-prose-portable-selection-v1.json)
is a separate companion to the portable sparkDash prefill copy, not a revision.
It keeps the same cells, trials, limits and portable controls: output cap 8,
thinking off and `cache: observe`. Its nominal 4k/8k/16k/32k cells use exactly
16,280/32,664/65,432/130,968 fill characters, matching the copy's filler byte
lengths, not promising equal token counts. Use reported prompt-token usage.

`fill: {"kind":"generated-prose-v1","characters":16280}` draws random words
from our compiled, fixed English list, with varying sentence lengths and
punctuation. It reuses the recorded per-request text salt to generate a distinct
body as well as a salted header; saved evidence regenerates the exact bytes.
The [fill contract](CONTRACT.md) defines its versioned generator and bounds.
This is synthetic random-word prose, not a natural-language corpus or proof of
typical routing, table coverage, zero cached tokens or cold storage.

Run each selection separately against the same unchanged server, then compare
corresponding cells and their actual prompt-token counts. Never pool prose and
repeated-unit results; their workload identities are incompatible. Selected
preflight and reports show `generated-prose-v1`, `repeated-unit` or `none` fill
kinds, including mixed selections. Existing sparkDash workload bytes and pins
remain unchanged.

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
where `fill_bytes = unit.len()*repeat` for repeated units or `characters` for
generated prose. At concurrency 1 the non-fill
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

For any Chat Completions server, `output.mode: "cap-reached"` sends only
`max_tokens` and accepts a lane only when the server reports exactly that many
completion tokens with finish reason `length`. The
`chat-template-thinking-v1` control sends both `thinking` and `enable_thinking`
template keys; with thinking disabled, reported reasoning makes a lane
ineligible ([contract](CONTRACT.md#workload-admission)).
[`portable-v1`](../../crates/grill-perf/examples/portable-v1.json) combines both
at C1 with greedy sampling on `portable-chat-v1`;
[`baseline-v2`](../../crates/grill-perf/examples/baseline-v2.json) is
`baseline-v1` with the new thinking control. Both count numbers, a synthetic
and highly predictable prompt: they measure request throughput at C1 including
prefill, and speculative decoding on either side dominates the result. Select
either with `baseline --workload`; the capture records the name.

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

## Finite heterogeneous schedules

Workload **version 4** is the schedule-only contract for raw `preflight`, `run`,
`pause`/`resume`, and offline raw-run `compare`. It does not change existing
workload1/2/3 inputs or historical receipts. Capture-v2 `baseline`/`check`
selections and policy1 do not admit schedule4; no empty `cells` result is treated
as a successful legacy comparison. Native plan4 pins workload4 and uses
reservation2/wave2, while preserving `generated-text-arrival-v2`.

Keep `cases` as the existing flat cases and set `cells: []`. Add a nonempty
`schedule` array of at most 64 scenarios:

```json
{
  "version": 4,
  "name": "synthetic-mixed",
  "request": {
    "profile": "portable-chat-v1", "stream": true,
    "output": {"tokens": 8, "mode": "cap"}, "cache": "observe"
  },
  "limits": {
    "total_ms": 3000, "idle_ms": 1000,
    "response_bytes": 65536, "wave_buffer_bytes": 33554432
  },
  "cases": [
    {"id": "a", "messages": [{"role": "user", "content": "Write a short sentence."}]},
    {"id": "b", "messages": [{"role": "user", "content": "Read this synthetic repetition: {fill} Then say done."}],
     "fill": {"unit": "word ", "repeat": 10000}}
  ],
  "cells": [],
  "schedule": [
    {"id": "solo-a", "kind": "solo", "warmup_trials": 1, "trials": 3,
     "lanes": [{"id": "a", "case": "a", "arrival": {"kind": "fixed_offset", "offset_us": 0}}]},
    {"id": "solo-b", "kind": "solo", "warmup_trials": 1, "trials": 3,
     "lanes": [{"id": "b", "case": "b",
       "request": {"profile": "portable-chat-v1", "stream": true, "output": {"tokens": 3, "mode": "cap"}, "cache": "observe"},
       "arrival": {"kind": "fixed_offset", "offset_us": 0}}]},
    {"id": "mixed", "kind": "overlap", "warmup_trials": 1, "trials": 3,
     "lanes": [
       {"id": "decode", "case": "a",
        "arrival": {"kind": "fixed_offset", "offset_us": 0},
        "control": {"scenario": "solo-a", "lane": "a"}},
       {"id": "prefill", "case": "b",
        "request": {"profile": "portable-chat-v1", "stream": true, "output": {"tokens": 3, "mode": "cap"}, "cache": "observe"},
        "arrival": {"kind": "after_first_generated", "lane": "decode", "offset_us": 40000},
        "control": {"scenario": "solo-b", "lane": "b"}}
     ]}
  ]
}
```

This is synthetic schedule data, not a tokenizer-qualified long-prefill study.
Each scenario has 1..64 unique named lanes, 0..20 warmup repetitions and 1..100
measured repetitions. A solo has exactly one fixed-offset-zero lane and no
control. Every overlap lane explicitly names a solo lane in the same workload
with the identical case, complete resolved request settings, and repetition
counts. Missing solo observations cannot be borrowed from another acquisition.
Scenario/lane/repetition identities, not completion order, align comparison arms.

An absent lane `request` uses the complete workload request. A present request
**replaces it entirely**; fields are never merged. Both default and replacement
must use `portable-chat-v1` or `vllm-fixed-v1`, never conversation profiles or
steps. The existing phase-output and cache-eligibility rules apply per lane.
Scheduled seeds use `seed + trial * 64`, without positional lane increments, so
the same declared lane settings resolve to the same seed in its solo control.

Arrivals are either `fixed_offset {offset_us}` from the admitted scenario's
single monotonic origin, or `after_first_generated {lane, offset_us}` from an
earlier-declared lane's first accepted generated text. Any scenario with a text
trigger requires all lanes to stream. Headers, role-only events, body arrival,
tool fragments and terminal frames are not generated-text triggers. A missing
trigger or a source that settles before its dependent dispatch is retained as
failure, not replaced by fixed-delay traffic.

Admission checks all named lane bodies, settings, seeds, memory, trials and
warmups prospectively. Existing hard limits remain: 10,000 total attempts, 1,024
scenario repetitions, 2 MiB encoded requests, 8 MiB response caps and 512 MiB
maximum wave buffer. `limits.total_ms` additionally bounds the **whole
scenario**, including every arrival wait; it is not restarted for each new lane.
There are no retries, replacement lanes, or subsequent scenario admission after
fatal failure. All admitted peers settle before response/receipt publication.
Cooperative pause takes effect only after this whole-scenario barrier; explicit
resume admits only never-started scenarios and remains a continued acquisition,
not uninterrupted comparison evidence.

For outside-checkout use, point at the exact staged binary and new workload:

```sh
PERF=/absolute/path/to/staged/bin/grill-perf
WORK=/absolute/path/to/synthetic-mixed.json
"$PERF" preflight "$WORK" --endpoint https://your-server.example/v1/chat/completions \
  --model YOUR_MODEL
# Only in a separately approved, prospectively budgeted endpoint window:
"$PERF" run "$WORK" --endpoint https://your-server.example/v1/chat/completions \
  --model YOUR_MODEL --out /absolute/path/to/new-schedule-run --json
"$PERF" compare /absolute/path/to/baseline-run /absolute/path/to/candidate-run --json
```

The example allows **16 requests / 88 output tokens / 1,048,576 response bytes**
including warmups, at most two admitted lanes, and twelve scenario deadlines
(36 seconds of network/arrival allowance, not a bound on filesystem stalls).
Preflight also prints the actual summed encoded request-body bytes and maximum
admitted lanes. No preflight command performs endpoint discovery or requests.
Prompt bytes are not tokenizer token counts.

Wave2 retains every lane position, including undispatched failures. In schedule
observations, `settled_offset_us` and `wave.elapsed_us` both retain actual
origin-to-barrier duration, including initial waits and cancellation settlement
tail; the tail is never truncated to manufacture a deadline success.
Completed lanes do not exempt the whole-scenario barrier from that deadline.
Timeout classification uses the retained barrier clock, including a scheduler
delay after the final admission-loop check.
Undispatched attempts have zero/default response timings, not fictional
dispatch or service intervals. Dispatch spread uses dispatched lanes only.

Offline replay verifies the exact deterministic lane spec, request/body hashes,
per-lane controls/usage, trigger offsets, deadlines and all overlap booleans:

- `request_inflight`: positive intersection of dispatch-to-settlement intervals.
- `generated_text`: positive intersection of first-to-last generated-text intervals.
- `left_decode_right_prefill` / `right_decode_left_prefill`: positive intersection
  of one lane's generated-text interval with the other's dispatch-to-first-text
  interval. Terminal/settlement tails never substitute for last generated text.

Missing or zero-span text intervals do not establish overlap. False actual
overlap remains visible even when every request completed. The report keeps
named per-lane first-generated/first-answer/completion observations, matched-solo
availability, all missing/failing repetitions and descriptive complete-scenario
throughput. It never pools unlike lane decode/prefill rates into a verdict.
Exit 0 means complete eligible **descriptive** evidence; incomplete or continued
evidence gives exit 2, and corrupt/incompatible evidence gives exit 1. These are
not fairness, mixed-load nonregression or backend-qualification verdicts.
Prospective named-lane gates use the policy2 contract above through `decide`.
No GLM/DeepSeek schedule qualification is implied by CPU fixture coverage.

## Kernel and collective microbenchmarks

`grill-perf microbench` keeps kernel and fabric observations in their own
closed artifact family, because HTTP output speed cannot prove that an isolated
kernel or collective changed. The subcommands are `capture` (run the
in-process CPU reference, or one explicitly declared adapter program), `import`
(validate retained producer bytes and record them with imported provenance),
`inspect` (validate and print identity, samples, byte definitions and retained
failures) and `compare` (a declared study with the shared exact envelope
arithmetic).

Provenance is `declared`, `imported` or `native_observed`, and importing never
upgrades it: a payload that claims native execution keeps that claim only in
`submitted_provenance`. A kernel or collective result never sets a
serving-speed claim; that requires a separately linked serving acquisition.
Device and collective capture is behind `--authorize-device-window`, and under
the current CPU-only authorization only the in-process `sum_u64` reference runs:
it proves native operation execution, sample recording, inspection and exact
comparison, not GPU support.

A comparison needs a prospectively declared study of **at least three complete
measured acquisitions in each of the A, B and A2 roles**. Slots are declared
before capture, membership is exact (an undeclared directory is an error, a
declared but missing or failed acquisition withholds the verdict rather than
being replaced or filtered), every pin except the declared revision axis must
match, and the A/A2 statistics are pooled into the reference envelope. A
revision is the SHA-256 of the implementation descriptor the producing process
actually observed — never a free-form operator label and never copied from the
plan, so a mismatching expectation is retained and rejected at comparison
rather than synthesized away. Identical revisions in all roles are an admitted
A/A control, and the observed runtime descriptor is the only permitted change
axis. Each sample keeps the timer's own decimal representation and its exact
rational nanosecond duration, so a fractional nanosecond and a
scientific-notation device sample survive unrounded, and the collective adapter
is executed directly with an explicit process-group environment instead of
being wrapped in a launcher.

Frozen cells, clock/unit/synchronization contracts with declared timer
resolution, the study and group-comparison contract, observed-source and
runtime-observation pins, the pinned public E3 and nccl-tests sources with
their hashes, the collective rank-scope sample unit, and the corrected byte
definitions (`payload_bytes = numel * sizeof(dtype)`; `2*(world-1)/world` is the
labelled nccl-tests bus normalization, never measured link traffic) are
specified in [the kernel and collective protocol](KERNEL-FABRIC.md), together
with the CPU smoke plan and the explicitly unexercised device gaps.

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

### Explicit accounting protocol (metrics version 2)

`--metrics-version 2` selects the versioned accounting protocol described in
[PROVIDER-ACCOUNTING.md](PROVIDER-ACCOUNTING.md): a closed allowlist of
server-wide prefix-cache, prompt-source, preemption, draft-round and
per-position counters with typed units, full label identities, whole-capture
continuity with exporter-epoch attestation, bounded scrape/series/overhead
budgets with bounded partial cancellation, and per-position acceptance against
the provider's own draft-round exposure. Optional version 2 diagnostics never
change performance eligibility; required evidence is an explicit policy
declaration that fails closed, so missing or contradicted telemetry is
unavailable rather than PASS. `--metrics-auth-env NAME` names a metrics-only
credential: the name is retained, the value is sent only to the metrics endpoint
and never inherits the model credential. Version 1 runs, plans and receipts keep
their existing schema and bytes.

## Independent host resource observations

[`resource capture/import/inspect/compare`](RESOURCES.md) provides a bounded
ordinary Linux process/cgroup/explicit host or macOS process/host-memory
observer and source-specific offline A/B/A2 comparisons. It does not change the
default serving collector, execute GPU sources, or implement capacity/retention
studies. Imported device/provider bytes remain imported evidence; source
delivery is not live qualification.

## Finite capacity and acquisition resources

[`capacity preflight/run/inspect`](CAPACITY.md) adds explicit workload6
complete-acquisition resource attachments and finite context/load/retention studies.
The existing native serving/sequence collector is reused; every failed cell and
undispatched remainder is retained. Attached resources are review-only outside
prospective capacity limits, and throughput policies cannot silently PASS them.
The source-pinned backend eviction hook is distinct from provider-reported misses
or preemptions. No automatic stress search, reset, retry or operator recovery exists;
largest successful tested cell is not safe universal capacity or live qualification.

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
