# Shared recipe workflow

For recipe-facing baseline/change/check integration, use the
[canonical model-agnostic recipe](RECIPES.md). Its explicit selection manifest
is separate from the historical bundle described below; no existing bundle
entry, workload byte or advanced-policy result is reinterpreted.

The small selected concurrency examples preserve the recipe control distinction:
`concurrency-selection-v1.json` requests `chat_template_kwargs.thinking: false`;
`concurrency-enable-thinking-selection-v1.json` requests
`chat_template_kwargs.enable_thinking: false`. Select according to an explicitly
qualified template, never from the model name. A neutral third recipe uses the
same path without adding a mapping. These are descriptive selected captures,
not the original sparkDash workload or its observed-envelope decision policy.

The checked-in [recipe manifest](../../crates/grill-perf/examples/recipes-v1.json)
selects the frozen sparkDash decode/prefill workloads for DeepSeek and materialized
GLM variants. The GLM variants change only the workload name and the explicit
thinking-key declaration. `bundle verify` checks that relation offline; it does
not resolve, rewrite or collect workloads. Different thinking declarations have
different normalized workload identities. Do not compare across recipes as if
the workloads were equivalent.

These command blocks are the recipe entrypoints. There is no second collector,
shell orchestration tool, model discovery, forward creation, health check,
deployment change, cache flush, weight download, retry or upload step. Operators
manage serving state separately and obtain approval before model collection.
Unsupported request controls must fail qualification; never strip a field to
make a server accept the workload.

## Install from an externally pinned source revision

Use a clean **native Linux ARM64** machine with Git, the workspace-required
Rust/Cargo toolchain, a C/C++ compiler and CMake for rustls/AWS-LC. Check the
workspace `rust-version` and the public build instructions at the chosen revision.
Do not substitute an emulated or cross-compiled installation for the native
installation smoke. Installation and bundle verification require no model service.

The initial CPU-qualified source pin is
`a6729f9ab5410587bb9da1adb7b34944a9cfc436`.
It identifies the collector and bundle, not a live recipe qualification.
Set `SOURCE` to an absolute, new checkout path. For a later release, obtain
another reviewed full immutable SHA; a branch or moving tag is not a pin.
Stop if a command fails, and verify the checkout SHA before building.

```sh
SOURCE_GIT_SHA=a6729f9ab5410587bb9da1adb7b34944a9cfc436
git clone https://github.com/plotarmordev/thegrill.git "${SOURCE:?absolute new checkout path required}"
git -C "$SOURCE" checkout --detach "${SOURCE_GIT_SHA:?reviewed full published commit SHA required}"
test "$(git -C "$SOURCE" rev-parse HEAD)" = "$SOURCE_GIT_SHA"
cargo build --manifest-path "$SOURCE/Cargo.toml" -p grill-perf --release --locked
GRILL_PERF="$SOURCE/target/release/grill-perf"
"$GRILL_PERF" --version
"$GRILL_PERF" --help
"$GRILL_PERF" bundle verify "$SOURCE/crates/grill-perf/examples/recipes-v1.json" --json
sha256sum "$GRILL_PERF" "$SOURCE/Cargo.lock"
rustc -vV
cargo --version
uname -srm
```

Check the reported checkout commit against the external pin before building.
Retain the checkout as the authoritative workload location. Select the executable
by this absolute path in every command; do not trust an unrelated `grill-perf`
on `PATH`. Record the actual binary SHA, package version, Cargo.lock SHA, build
command/profile, Rust/Cargo versions, target and native OS/architecture context.
Record deliberate build flags if used; do not collect a whole-environment dump.
The Git source revision is not `source_sha256`: that field hashes the exact
workload file bytes. The normalized `workload_sha256` is the existing typed
Workload serialization digest, not a generic JSON canonicalization.

At this pin, a clean-source native Linux ARM64 build, offline bundle verification
and the full four-entry CLI workflow were exercised with synthetic loopback
responses. This is CPU installation/protocol evidence, not model qualification.
Repeat the installation smoke on the target host before reporting it as verified.

## Verify and select the exact workload

```sh
MANIFEST="$SOURCE/crates/grill-perf/examples/recipes-v1.json"
"$GRILL_PERF" bundle verify "$MANIFEST" --json
```

The verifier admits a closed, bounded manifest with exactly four entries and the
explicit `deepseek`/`glm` decode/prefill mappings. Each entry declares its fixed
leaf filename, raw and normalized digests, and the GLM entries declare their
corresponding `base`. The manifest has no self-hash or containing Git revision.
Verification rejects unsupported versions, unknown fields, incorrect mappings,
unsafe paths, symlink files or roots, hash mismatches and semantic variant drift.
Its report contains the manifest digest, entry identities, declared controls and
request/token ceilings. Keep the source directory immutable during verification
and collection; verification is not a filesystem snapshot or signature.

| Setting | Decode, either recipe | Prefill, either recipe |
|---|---|---|
| Request profile | `vllm-fixed-v1`, streaming | `vllm-fixed-v1`, streaming |
| Output | exact 400 tokens | exact 8 tokens |
| Sampling | temperature zero, top_p one, no seed | temperature zero, top_p one, no seed |
| Cache | `observe` | `observe`; salted filled prompts |
| Requests including warmup | 72: 18 warmup, 54 measured | 16: 4 warmup, 12 measured |
| Output token ceiling | 28,800 | 128 |
| Total / idle deadline, milliseconds | 360000 / 60000 | 600000 / 600000 |
| Response / wave allowance, bytes | 1048576 / 67108864 | 65536 / 4194304 |

The workloads retain their exact case/cell schedules and declared warmups and
trials. Prompt size names are not tokenizer measurements. Exact output requests
use `min_tokens`, `max_tokens` and `ignore_eos`; successful length stops are not
automatically censored failures. Eligibility and provider usage remain decisive.
`observe` does not establish a cold cache. Review reported generated channels and
reasoning-token evidence: a declared thinking key does not prove the template
honored it.

DeepSeek sends `chat_template_kwargs: {"thinking": false}`; GLM sends
`chat_template_kwargs: {"enable_thinking": false}`. Neither mapping automatically
qualifies the other. Preserve the frozen sparkDash files and attribution to
[MiaAI-Lab's sparkDash](https://github.com/MiaAI-Lab/sparkDash).

## Create and approve the policy before collection

Choose the recipe and one workload first. A decode study and a prefill study are
separate acquisitions with separate source pins and policy scopes. Use the
actual binary and exact selected workload to obtain the pins:

```sh
sha256sum "$GRILL_PERF" "$WORKLOAD"
```

Create `POLICY` as an absolute filename outside the checkout, using a JSON editor.
Copy the first digest into `collector_sha256` and the second into
`workload_source_sha256`. Do not use the Git SHA, Cargo.lock digest, manifest digest
or normalized workload digest for those fields. Choose an identifier, required
metrics and practical regression/reference-spread tolerances with the study
reviewer, independently of the collected outcomes. Approve the complete policy
before the first run; retain exactly the same policy bytes for A, B and A2.
`run --policy FILE` validates and persists those bytes before dispatch. It does
not prove independent preregistration or physical server restoration.

The following policies are **deliberately invalid illustrations, not runnable
policies**. Every `REPLACE_...` string must be replaced. Hashes must become actual
lowercase SHA256 strings; each tolerance must become an operator-approved JSON
integer, not a string. No tolerance below is a recommendation or implicit
default. These examples choose one metric per cell; the operator may explicitly
add other required metrics to every relevant cell before approval. Do not remove
cells. Decode requires the complete decode scope shown here for either recipe:

```json
{
  "version": 1,
  "method": "observed-envelope-v1",
  "id": "REPLACE_APPROVED_POLICY_ID",
  "collector_sha256": "REPLACE_ACTUAL_BINARY_SHA256",
  "workload_source_sha256": "REPLACE_SELECTED_DECODE_FILE_SHA256",
  "min_trials": 3,
  "cells": [
    {"cell":"structured-1","metrics":[{"metric":"decode_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"structured-2","metrics":[{"metric":"decode_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"structured-4","metrics":[{"metric":"decode_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"structured-8","metrics":[{"metric":"decode_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"prose-1","metrics":[{"metric":"decode_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"code-1","metrics":[{"metric":"decode_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"json-1","metrics":[{"metric":"decode_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]}
  ]
}
```

For a prefill study, use this separate complete policy instead:

```json
{
  "version": 1,
  "method": "observed-envelope-v1",
  "id": "REPLACE_APPROVED_POLICY_ID",
  "collector_sha256": "REPLACE_ACTUAL_BINARY_SHA256",
  "workload_source_sha256": "REPLACE_SELECTED_PREFILL_FILE_SHA256",
  "min_trials": 3,
  "cells": [
    {"cell":"prefill-4k-1","metrics":[{"metric":"prefill_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"prefill-8k-1","metrics":[{"metric":"prefill_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"prefill-16k-1","metrics":[{"metric":"prefill_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]},
    {"cell":"prefill-32k-1","metrics":[{"metric":"prefill_tokens_per_second","max_regression_bps":"REPLACE_APPROVED_INTEGER","max_reference_spread_bps":"REPLACE_APPROVED_INTEGER"}]}
  ]
}
```

The metric enum also permits `wave_latency_us` and
`achieved_completion_tokens_per_second`; direction is intrinsic to the metric.
Every workload cell must appear exactly once with nonempty unique metrics.
The declared trial minimum must fit the workload, and every cell must declare
warmup. Basis-point schema bounds are engineering limits, not scientific advice.
The reference-spread bound is an observed variability gate, not a confidence bound.
See the [performance contract](CONTRACT.md) for policy admission and decision
arithmetic. No policy generator subcommand is provided.

Prepare complete private deployment JSON files for A and B, containing nonempty
`model_revision`, `runtime`, `hardware` and `settings` declarations. Describe the
actual context limit, quantization, tensor parallelism, speculative configuration
and other relevant serving settings there; do not paste invented example values.
Reuse A's exact deployment declaration for the restored repeat. Keep the same
model selector and stable endpoint for A/A2; changing a forward port prevents the
declared reference identity from matching. Record B's actual declaration rather
than copying A's declaration when the configuration changed.

## DeepSeek entrypoint: qualify first

Set `WORKLOAD` to exactly one of these selections before creating its policy:

```sh
WORKLOAD="$SOURCE/crates/grill-perf/examples/sparkdash-decode-v1.json"
```

or, for a separate prefill study:

```sh
WORKLOAD="$SOURCE/crates/grill-perf/examples/sparkdash-prefill-v1.json"
```

Set `POLICY`, `DEPLOYMENT_A`, `DEPLOYMENT_B`, `A`, `B` and `A2` to absolute paths.
Each output directory must be new with an existing parent. Set `ENDPOINT_A`,
`ENDPOINT_B` and `MODEL` explicitly to approved serving selections. The following
blocks assume HTTPS; literal-loopback HTTP additionally needs `--local-http` on
each run. Add `--auth-env MODEL_API_KEY` only when an independently supplied
credential is required. Do not put credentials into the policy or workload.

Collect A with the approved policy:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_A" \
  --model "$MODEL" --deployment "$DEPLOYMENT_A" --out "$A" --json
```

After A has completed, obtain approval and change the serving setup separately.
Ensure collection does not overlap, then collect B:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_B" \
  --model "$MODEL" --deployment "$DEPLOYMENT_B" --out "$B" --json
```

After B has completed, restore A separately and verify the restoration through
the operator's approved procedure. Collect a new, independent repeat; copying a
run directory is not a repeat:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_A" \
  --model "$MODEL" --deployment "$DEPLOYMENT_A" --out "$A2" --json
"$GRILL_PERF" compare "$A" "$B" --reference "$A2" --json
"$GRILL_PERF" decide "$A" "$B" --reference "$A2" --json
```

## GLM entrypoint: qualify independently later

Do not transfer DeepSeek's qualification to GLM. Obtain separate authorization
and review the actual GLM template/control support first. Select exactly one
GLM workload and create its own policy with its actual source digest:

```sh
WORKLOAD="$SOURCE/crates/grill-perf/examples/glm-decode-v1.json"
```

or, for a separate prefill study:

```sh
WORKLOAD="$SOURCE/crates/grill-perf/examples/glm-prefill-v1.json"
```

Use fresh GLM-specific paths, approved model/endpoint selections and complete
actual deployment declarations, under the same prerequisites as DeepSeek.
Collect the baseline:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_A" \
  --model "$MODEL" --deployment "$DEPLOYMENT_A" --out "$A" --json
```

After completion and a separately authorized configuration change, collect B:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_B" \
  --model "$MODEL" --deployment "$DEPLOYMENT_B" --out "$B" --json
```

After completion and separately verified restoration of A, collect A2:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_A" \
  --model "$MODEL" --deployment "$DEPLOYMENT_A" --out "$A2" --json
"$GRILL_PERF" compare "$A" "$B" --reference "$A2" --json
"$GRILL_PERF" decide "$A" "$B" --reference "$A2" --json
```

## Read decisions without broadening the claim

`compare` remains descriptive: successful comparison is eligibility, never PASS.
Only a successfully printed versioned `decide` envelope constitutes a decision.
Do not infer a verdict from an exit code or missing output; parsing/output errors
can share exit values with decision outcomes. Inspect `decision`, `eligibility`,
all scoped gates, coverage and bounded reason codes. The policy evaluates the
observed envelope, not statistical significance, equivalence, intelligence,
causality or universal no-regression.

A, B and A2 must all capture the same exact policy before dispatch. Missing
bindings are INCONCLUSIVE; conflicting present bindings or corrupt evidence are
ERROR. Insufficient observations, unqualified references, copied acquisitions,
out-of-order declared starts or missing metrics cannot silently become PASS.
Declared start order and matching reference declarations do not authenticate
nonoverlap or physical restoration. Paused/resumed sessions do not qualify as
uninterrupted performance acquisitions. Preserve every cell and metric; do not
select only favorable gates or treat a withheld comparison percentage as zero.

Maintain separate source-reviewed, loopback-tested and live-qualified statuses
for each recipe and workload. Both explicit control mappings and all four
workload entries were exercised through the CLI against synthetic responses.
Live qualification remains pending: DeepSeek first under coordination, GLM
separately later. Earlier model smoke receipts do not qualify this new
bundle/policy workflow.

Use the [manual reviewed report template](SHARED-REPORT-TEMPLATE.md) only after
privacy review. It is not an exporter or replayable evidence package. No upstream
recipe files are changed by these commands; maintainers may separately adopt
these public command blocks without copying private deployment code or evidence.
