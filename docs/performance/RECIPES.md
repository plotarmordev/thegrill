# Recipe-facing baseline and check

Use the existing `grill-perf` binary against a server the operator already runs.
Recipe code supplies explicit inputs and delegates collection, evidence and
arithmetic to TheGrill. It does not detect models, change serving state, create
forwards, flush caches, retry controls or upload results.

## Pin source, binary and workload separately

Use a reviewed immutable source revision containing the selected-capture CLI.
The historical source pin in [SHARED-RECIPES.md](SHARED-RECIPES.md) qualifies its
older bundle workflow, not this extension. Obtain the new revision externally;
this guide does not invent a published revision or claim a live qualification.
Retain the checkout and invoke the built executable by absolute path:

```sh
: "${SOURCE:?absolute checkout path required}"
: "${SOURCE_GIT_SHA:?reviewed immutable source revision required}"
: "${GRILL_PERF:?absolute path to the chosen executable required}"
test "$(git -C "$SOURCE" rev-parse HEAD)" = "$SOURCE_GIT_SHA"
sha256sum "$GRILL_PERF" "$SOURCE/Cargo.lock"
"$GRILL_PERF" --version
```

Record the actual build command/profile, toolchain, native OS/architecture and
intentional build flags. Do not dump the environment. The Git source revision,
invoked executable SHA-256, workload `source_sha256` and normalized
`workload_sha256` are distinct identities. Deployment declarations are a further
operator claim, not something the collector authenticates from the server.

The new small selected workload is separate from the frozen four-entry bundle:

```sh
SELECTION="$SOURCE/crates/grill-perf/examples/concurrency-selection-v1.json"
```

This requests the legacy `chat_template_kwargs.thinking: false` control. For a
template explicitly qualified for `enable_thinking` instead, choose the separate
selection **before baseline**:

```sh
SELECTION="$SOURCE/crates/grill-perf/examples/concurrency-enable-thinking-selection-v1.json"
```

Do not infer this choice from a model name. The two selections differ in identity
and cannot be compared as equivalent workloads. DeepSeek/GLM recipes may retain
their historical explicitly different controls; a neutral third recipe supplies
its own model selector and chooses an independently qualified control using
exactly these commands, without adding a model-name branch.

A recipe may instead provide any other natively validated bounded workload and
a manifest following the [closed selection schema](README.md#explicit-selected-captures).
Place both files together and freeze their bytes before collection. Obtain the
raw and typed workload digests offline—without traffic or a Rust helper:

```sh
"$GRILL_PERF" bundle inspect "$WORKLOAD"
```

Use the returned `source_sha256` and `workload_sha256` in the closed selection
manifest. These are not `jq -S` or arbitrary canonical-JSON hashes. Inspect reports
declared controls and one native run's budget; capture preflight multiplies by
eight acquisitions. Scope and operation class must describe prospective membership;
`unknown` does not mean normal or safe, and `stress` is not normal operation.
No plugin or provider lookup is performed.

## Reuse the deployment declaration directly

Supply actual values already known by the recipe. Missing values are explicit
operator prerequisites, not sample defaults. Construct the existing declaration
with `jq`; there is no new deployment configuration hierarchy:

```sh
: "${DEPLOYMENT_A:?absolute private declaration filename required}"
jq -n \
  --arg model_revision "${MODEL_REVISION_A:?actual weights revision required}" \
  --arg runtime "${RUNTIME_REVISION_A:?actual engine build revision required}" \
  --arg hardware "${HARDWARE_ID_A:?actual device/layout identifier required}" \
  --arg settings "${SETTINGS_FINGERPRINT_A:?actual serving settings fingerprint required}" \
  '{model_revision:$model_revision,runtime:$runtime,hardware:$hardware,settings:$settings}' \
  > "$DEPLOYMENT_A"
```

Make the file private using the recipe's ordinary private-output policy. Keep
credentials out of every declaration. A settings fingerprint should identify the
reviewed configuration, including relevant context limit, quantization, parallel
layout and speculative settings. A changed fingerprint alone does not prove
that only one internal setting changed.

## Baseline, unchanged control, change, check

Set absolute paths for the executable, selection, declarations and new output
directories; their parents must exist. The following path works from outside the
checkout. Approve a finite collection window separately before contacting a
model endpoint. `ENDPOINT` and `MODEL` are explicit operator inputs. These
commands assume HTTPS; literal-loopback HTTP additionally requires
`--local-http` on baseline.

```sh
"$GRILL_PERF" baseline \
  --selection "${SELECTION:?explicit approved selection required}" \
  --endpoint "${ENDPOINT:?approved Chat Completions resource URL required}" \
  --model "${MODEL:?explicit server model selector required}" \
  --deployment "$DEPLOYMENT_A" \
  --auth-env "${AUTH_ENV:?name of independently supplied credential variable required}" \
  --seconds "${CAPTURE_SECONDS:?approved whole-capture allowance required}" \
  --out "${BASELINE:?new absolute private output directory required}" --json
```

Omit `--auth-env` for an unauthenticated endpoint; do not supply a dummy key.
The collector stores only the environment-variable name, not the credential.
Preflight is on stderr under `--json`; stdout contains the structured result.
It enumerates controls, every cell, warmup/trial/concurrency counts, request and
output-token ceilings, time limits and buffer allowances before dispatch. Pin
checks and unsupported declared profiles fail offline; backend control support
still requires independent qualification. A backend rejection remains an error
with retained evidence, never a signal to strip fields and resend.

For an unchanged control, leave the server and every declaration unchanged:

```sh
"$GRILL_PERF" check "$BASELINE" --deployment "$DEPLOYMENT_A" --change none \
  --seconds "$CAPTURE_SECONDS" --out "${CONTROL:?new private control directory required}" --json
"$GRILL_PERF" compare "$BASELINE" "$CONTROL" --json
```

Retain the control even when it shifts or is incomplete. It observes another
capture period, not equality, repeatability or proof of unchanged internals.
Do not keep rerunning until a preferred result appears.

For an actual settings change, change serving state separately and construct the
candidate declaration from explicit actual values. Reuse only values the
operator confirms unchanged:

```sh
: "${DEPLOYMENT_B:?absolute private candidate declaration filename required}"
jq -n \
  --arg model_revision "${MODEL_REVISION_B:?actual candidate weights revision required}" \
  --arg runtime "${RUNTIME_REVISION_B:?actual candidate engine revision required}" \
  --arg hardware "${HARDWARE_ID_B:?actual candidate hardware identifier required}" \
  --arg settings "${SETTINGS_FINGERPRINT_B:?actual candidate settings fingerprint required}" \
  '{model_revision:$model_revision,runtime:$runtime,hardware:$hardware,settings:$settings}' \
  > "$DEPLOYMENT_B"
"$GRILL_PERF" check "$BASELINE" --deployment "$DEPLOYMENT_B" --change settings \
  --seconds "$CAPTURE_SECONDS" --out "${CANDIDATE:?new private candidate directory required}" --json
"$GRILL_PERF" compare "$BASELINE" "$CANDIDATE" --json
```

Select `model_revision`, `runtime` or `hardware` instead when that is the declared
change. Exactly the selected declaration must differ. Multiple changed fields
are rejected; do not conceal them by copying a stale declaration. Candidate
collection inherits the retained selection and workload, endpoint, model selector
and credential-variable name. There is no check-side reselection knob. Original
source files need not remain at their original path, but retained baseline bytes
and the collector executable must match. Offline `compare` needs no credentials,
performs no network calls and does not rewrite reports.

Every explicit selection is descriptive, including selected C1 workloads.
Complete selected checks emit `DESCRIPTIVE` (exit 0) with a **COMPLETE -
DESCRIPTIVE ONLY** terminal banner. This means successful observation/comparison,
not a faster/slower, PASS, equivalence or noninferiority claim.
Incomplete captures remain `INCONCLUSIVE`/exit 2; invalid evidence is `INVALID`/exit 1.
Keep each acquisition/cell separate;
concurrent lanes are not independent acquisition samples. Native summaries and
wave attempts retain missing usage, partial peers, request failures, timing,
aggregate throughput and per-stream rates. Do not replace an unavailable value
with zero or pool unlike cells into an improvement percentage.

## Contributor PR report

For a performance-affecting change, identify baseline and candidate revisions,
choose a relevant workload and explicit controls before collection, and retain
unchanged controls plus all incomplete/invalid outcomes. CPU protocol fixtures
can establish CLI and arithmetic behavior, not model/backend performance.
Documentation-only PRs do not require GPU collection. Authors must not invent
measurements or manufacture a favorable verdict to obtain review.

Raw requests, responses, endpoint URLs, deployment declarations and report paths
remain private by default. A local report is not an automatic public-safe export.
Prepare a separate manual summary, review every field for identifiers, secrets,
private prompts and rights, and share only the approved summary. Retain private
native evidence for offline review; sanitizing it in place would break its hashes.

A manual PR summary can use this structure. Replace each bracketed description
with reviewed facts or an explicit unavailable/not-run statement; this is not a
fabricated measurement example:

```text
Change: [reviewed baseline/candidate source revisions and serving-change category]
Collector: [source revision, actual binary digest, build profile/toolchain]
Workload: [selection id and digest, raw and normalized workload digests]
Declared deployment: [reviewed public-safe model/runtime/hardware/settings IDs]
Scope: [prospective complete cell membership; normal/stress/unknown declaration]
Allowance: [preflight acquisitions, warmups, measured trials, requests, tokens, time]
Protocol verification: [actual local fixture commands/results, or not run]
Backend qualification: [approved deployment/window and evidence, or not run]
Coverage: [per acquisition/cell planned, observed, eligible and failed counts]
Observations: [per-cell makespan/dispatch/latency, aggregate and per-stream metrics]
Unavailable or incomplete: [all missing usage, failed/partial lanes and budget stops]
Control: [unchanged capture-period observations, or not collected]
Result: [exact stored result code; descriptive-only scope where selected]
Limits: [dependence, cache state, period drift; no causal/capacity/tail guarantee]
Private evidence: [reviewed access procedure, not an endpoint or raw upload]
Sanitization review: [reviewer and exact public fields approved]
```

Source review, local protocol verification and live backend qualification are
separate statuses. No uploader, public export command, CI benchmark mandate or
serving lifecycle manager is introduced by this recipe.
