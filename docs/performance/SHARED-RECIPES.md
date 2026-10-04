# Shared recipe workflow

For recipe-facing baseline/change/check integration, use the
[canonical model-agnostic recipe](RECIPES.md). Its explicit selection manifest
is separate from the historical bundle described below; no existing bundle
entry, workload byte or advanced-policy result is reinterpreted.

## Contributor claims beyond the default C1 check

Use this shared claim map and recipe profiles and the
**[single report template](SHARED-REPORT-TEMPLATE.md)** as the
entrypoint for contributor performance evidence. Select the relevant routine,
stress or domain scope before collection; do not run the entire matrix
automatically. The map separates first-output/completion, fairness, mixed
interference, accounting, tools/history, resources, retention/capacity,
startup/reload and kernel/fabric evidence.

The default CLI remains the bounded C1 assessment. Advanced `run`, `pause`,
`resume`, raw-run `compare` and captured-policy `decide` keep their separate
meanings; domain collectors do not substitute for serving or quality checks.
The recipes retain their [sparkDash](https://github.com/MiaAI-Lab/sparkDash)
attribution, and historical evidence is not reinterpreted.

Report source implementation, CPU-protocol verification, real-adapter exercise
and live-backend qualification separately for the exact artifact and selected
scope. Fixture success is not model/cache/tokenizer proof; missing evidence and
INCONCLUSIVE results stay visible. Broader mandatory PR adoption remains subject
to completed coverage review and explicit maintainer scope/exception agreement.

## Recipe bundle and selections

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

## Routine and mixed claim profiles

The `glm-routine-decode-v3.json`, `glm-routine-prefill-v3.json` and
`glm-mixed-prefill-decode-v4.json` files, and their `deepseek-` counterparts,
reuse the existing bundle prompts without changing the frozen bundle files.
They are explicit workload choices, not model detection. GLM-labelled files
retain `chat_template_kwargs.enable_thinking: false`; DeepSeek-labelled files
retain `chat_template_kwargs.thinking: false`. A label does not establish that a
particular backend honors that key. Verify the actual template and retained
request/response evidence before qualification; never retry with a stripped key.

“Routine” describes the bounded workload scope below, not an agreed maintainer
requirement or a recommended deployment configuration. Select the rows relevant
to the change. Do not run every row automatically.

| Profile suffix, for either recipe | Declared scope | Warmup / measured requests per run | Output-token ceiling per run | A/B/A2 requests / output-token ceiling |
|---|---|---:|---:|---:|
| `routine-decode-v3` | Structured C1/C2; prose, code and JSON C1; capped warmup32/measured400; observe cache | 6 / 18 | 7,392 | 72 / 22,176 |
| `routine-prefill-v3` | Approximately 4K/16K input, C1; capped8; required reported-zero prefix hits | 2 / 6 | 64 | 24 / 192 |
| `mixed-prefill-decode-v4` | Solo decode, solo approximately 4K prefill, then a two-lane mixed scenario; capped decode warmup32/measured400 and prefill8 | 4 / 12 | 2,528 | 48 / 7,584 |

Every cell/scenario has one retained warmup and three measured trials. Decode
retains the source workload's 360-second wave deadline, 60-second idle deadline,
1 MiB response limit and 64 MiB wave-buffer limit. Routine prefill retains
600-second total/idle deadlines, a 64 KiB response limit and a 4 MiB wave buffer.
Mixed schedules use the decode limits and admit at most two lanes. They contain
12 waves, at most 16 model requests, and a sum of scenario deadlines of 4,320
seconds per run. This is not a wall-clock bound on filesystem publication or
operator work. Output ceilings include warmup and are ceilings, not promises that
the server generates that many tokens.

Mixed `prefill` dispatch is triggered by the first generated text from `decode`;
it is not triggered by headers or a role-only delta. `solo-decode/decode` and
`solo-prefill/prefill` are the named controls. A declared trigger is not proof of
overlap: require retained decode/prefill overlap for every required repetition.
Missing triggers, early settlement, cancellation and absent overlap remain in the
evidence; none is a replacement opportunity.

Source-byte pins for these exact files:

| File | SHA256 |
|---|---|
| `glm-routine-decode-v3.json` | `6d075fb8617280ad062393e169f4b162d12840338bbb2749e6871be5672b5846` |
| `glm-routine-prefill-v3.json` | `f0ddfda5f5bb35b6c1637fd27e02c5ae3ef9aeb37ed5e61fc0b953b52a61edd6` |
| `glm-mixed-prefill-decode-v4.json` | `886c96c9efeaa4f6bac85e3d19af3408469edd55eb1fdc3a82aedaa3dacce06f` |
| `deepseek-routine-decode-v3.json` | `2e92488152a007774a1f2614b6e18e83a70b2f4ab8d9d5c052b551a2316f0074` |
| `deepseek-routine-prefill-v3.json` | `37a121c31bcabb5df8b508edbbf3e216880a514f81d29c3401ff51c6f4d987b9` |
| `deepseek-mixed-prefill-decode-v4.json` | `0a84ab9151e262af2e38e64c3f62601f385bcf44aa2841b9a4eb9ad12b588559` |

Before an authorized acquisition, select the actual installed collector, exact
workload and complete prospective policy. Obtain the binary/source pins and
approve thresholds through the policy workflow below, then run native
`preflight` with the actual model, endpoint, deployment and policy arguments.
Preflight dispatches no requests; it validates declarations, not backend support.
Retain its output and recheck it whenever any input changes. Optional telemetry
adds its own bounded requests and overhead; it is not included in the model
request counts above. These six profiles were CPU-fixture captured and replayed,
including both thinking-key spellings and actual mixed overlap. That does not
verify real tokenizer lengths, cache behavior or serving performance.

The new source pins are not interchangeable with #45's completed studies.
Reuse the frozen #45 evidence for its original qualified scope; do not rerun that
campaign simply to demonstrate these new filenames. A new first-output, mixed,
cache-accounting or other claim needs the corresponding evidence and gates.
`compare` is descriptive eligibility, never PASS. A serving decision does not
establish semantic/JSON/code quality; retain the applicable recipe quality checks
and use the [single shared report](SHARED-REPORT-TEMPLATE.md).

### Repeated conversation and explicit stress profiles

The following version-6 files have separate `glm-` and `deepseek-` variants with
the same single, explicit thinking-key mappings described above. They are
prospective workload declarations, not evidence of backend qualification.
Do not automatically add stress rows to routine PR traffic.

| Profile suffix | Scope and output | Warmup / measured requests per run | Output-token ceiling per run | A/B/A2 requests / output-token ceiling |
|---|---|---:|---:|---:|
| `tools-v6` | Actual streamed tool call, locally supplied result and actual follow-up; capped128 | 2 / 6 | 1,024 | 24 / 3,072 |
| `history-branches-v6` | Prime/reuse, alternate histories, edit/restore and tool follow-up; capped128 | 11 / 33 | 5,632 | 132 / 16,896 |
| `history-branches-shared-tools-v6` | Same history branches with the tool declaration shared across the tool history; capped128 | 11 / 33 | 5,632 | 132 / 16,896 |
| `long-context-v6` — stress | Four strict-cold C1 sizes, including a 520,000-byte filler; capped1 | 4 / 12 | 16 | 48 / 48 |
| `p95-stress-v6` — stress | C2, 200 measured waves, exact8 per lane; completion tail and first-output/fairness means | 2 / 400 | 3,216 | 1,206 / 9,648 |

Conversation profiles retain one complete warmup acquisition and three complete
measured acquisitions. Every required step remains part of completeness and
whole-conversation wall time. The history profile excludes its two priming steps
from per-step performance means, not from execution or eligibility. It reuses
the eleven long-prefix branch cases from `conversation-v2.json`; the separate
`short-prime`/`short-reuse` probe remains in that unchanged legacy file and is
**not qualified by this profile**. Required reported-zero/hit checks on the
selected history cases remain required. The tool-only profile observes cache
state without claiming reuse.

The GLM `history-branches-shared-tools-v6` variant declares the fixed tool fixture
on every step of the tool-bearing history (`expect.shared: true`). This removes
the tools-head prefix mismatch only when the backend retains those declarations
for `tool_choice: "none"`; the authorized native retest confirmed that retention
(168 prompt tokens with the declaration versus 17 without on the same probe),
while the strict workload itself still stopped on its first factual response,
so a passing GLM campaign is not claimed. The original `history-branches-v6`
files are unchanged and retain their pinned hashes and prior outcomes, including
the observed GLM first-tool-turn cache miss.

Tools have a 1 MiB encoded-input allowance and 1 MiB retained-history allowance;
history branches have 1 MiB and 16 MiB respectively. Their wave-buffer allowances
are 16 MiB and 32 MiB. Long context has a 2 MiB encoded-input allowance and
16 MiB wave buffer; p95 has 64 KiB and 16 MiB. All responses are bounded to
64 KiB. All four profiles use a 360-second wave deadline, with a 60-second idle
deadline except long context, whose idle deadline is also 360 seconds.
These are declared serialization/collection bounds, not process-RSS guarantees.

The long-context profile reuses `prefill-ladder-v1.json` prompt data. Its final
case is explicitly renamed `prefill-large` and declares 130,000 repetitions of
`" the"` rather than the legacy 131,072, reserving prospective context headroom.
That is 520,000 filler bytes, **not a measured token count**. Count the actual
encoded body and use the actual tokenizer/context allowance before dispatch;
retain provider prompt-token usage before making any context-length claim.
Neither this case name nor a 128 KiB limit establishes 128K tokens. The frozen
legacy ladder is unchanged. Never shorten an already declared acquisition after
a context rejection.

The p95 population is 200 complete measured **waves**, each containing two lanes:
it is not 400 independent tail observations. The tail sample is the maximum
completion latency across both eligible lanes. One missing lane or acquisition
withholds the required tail gate. This profile does not qualify p99; p99 requires
its separately declared population of at least 1,000 complete observations.
First-output and fairness means remain distinct from completion-tail statistics.

| File | SHA256 |
|---|---|
| `glm-tools-v6.json` | `a2cbebeb24f0ef6cc74dada8ab17c662d6dd54d0f064456be327d1ed6a7d55ce` |
| `glm-history-branches-v6.json` | `dd461c1c89c0fca5b003d40b205abab35576f83f02547ff924bf142c6e11134c` |
| `glm-history-branches-shared-tools-v6.json` | `066c5f6c15ff92af600b21594b23ac2a62357cbd4fe20b2e798c728895a2e55f` |
| `glm-long-context-v6.json` | `ef0a6f381ad5158f73b7756f6e637789cd8c948a81827cdddf4c57176a33402b` |
| `glm-p95-stress-v6.json` | `b18bccca830672d5dba62c50e252d9730d50d436d7bb362b3c18c8862ed41315` |
| `deepseek-tools-v6.json` | `609e8d45f9b42b6f990172a2426e2bdc9399803945e38efcc71253f0e453a126` |
| `deepseek-history-branches-v6.json` | `4849036ee09ecf2cee85745e12be3b6a79da8d64ba8afb77013982dd9d8d9d40` |
| `deepseek-long-context-v6.json` | `75a15591dadd2a672d6fe37f98c9d12035e36a8efaf48a5c0b14d222620eb844` |
| `deepseek-p95-stress-v6.json` | `7705bfb554fe39f40e50ea0f7cbbb555f37aa90222e57400a29393de352c946b` |

### Prospective policy3 for the version-6 profiles

After installation, set `GRILL_PERF`, `WORKLOAD` and a fresh `POLICY` path as in
the workflow below. Explicitly select `CLAIM=conversation`, `long-context` or
`p95`; set the two approved threshold environment variables
`MAX_REGRESSION_BPS` and `MAX_REFERENCE_SPREAD_BPS`. The following local authoring
step hashes actual bytes, dispatches no requests and refuses to replace a policy.
Thresholds must be approved before collection, not selected from candidate data.

```python
import hashlib
import json
import os
from pathlib import Path

binary = Path(os.environ["GRILL_PERF"])
workload_path = Path(os.environ["WORKLOAD"])
source = workload_path.read_bytes()
workload = json.loads(source)
claim = os.environ["CLAIM"]
threshold = {
    "max_regression_bps": int(os.environ["MAX_REGRESSION_BPS"]),
    "max_reference_spread_bps": int(os.environ["MAX_REFERENCE_SPREAD_BPS"]),
}
assert workload["version"] == 6
assert claim in {"conversation", "long-context", "p95"}
conversation = workload["acquisition"]["kind"] == "conversation"
assert conversation == (claim == "conversation")
measured = set(workload["acquisition"].get("measured_steps", []))
cases = {case["id"]: case for case in workload["cases"]}
cells = []
tails = []
for cell in workload["cells"]:
    if conversation and cell["case"] not in measured:
        continue
    expected = cases[cell["case"]].get("step", {}).get("expect", {}).get("kind")
    if expected == "tool":
        metrics = ["first_tool_delta_us", "first_validated_tool_call_us",
                   "completion_latency_us"]
    elif claim == "long-context":
        assert workload["request"]["cache"] == "reported-prefix-zero"
        metrics = ["prefill_tokens_per_second", "first_generated_text_us",
                   "completion_latency_us"]
    else:
        metrics = ["first_generated_text_us", "first_answer_text_us",
                   "completion_latency_us"]
    if claim == "p95":
        assert cell["trials"] >= 200 and cell["concurrency"] >= 2
        metrics += ["worst_lane_first_answer_us", "first_answer_max_min_ratio"]
        tails.append({"target": {"kind": "completion", "cell": cell["id"]},
                      "percentile": "p95", **threshold})
    cells.append({"cell": cell["id"],
                  "metrics": [{"metric": metric, **threshold} for metric in metrics]})
policy = {
    "version": 3,
    "method": "observed-envelope-v3",
    "id": workload["name"] + "-policy",
    "collector_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
    "workload_source_sha256": hashlib.sha256(source).hexdigest(),
    "min_trials": 3,
    "cells": cells,
}
if conversation:
    policy["whole_conversation"] = threshold
if tails:
    policy["tail"] = tails
with Path(os.environ["POLICY"]).open("x") as output:
    json.dump(policy, output, indent=2)
    output.write("\n")
```

Retain native `preflight` output with the actual endpoint/model and this policy
before using the existing A/B/A2 run/decide workflow. Preflight is offline schema
and budget admission, not tokenizer, cache, thinking-control or backend proof.
For tools, the two tool-time selectors require actual streamed deltas and a
validated complete call; text timing cannot substitute for either.

### Remaining claim entrypoints and non-substitution rules

Both recipe families use the same explicit domain workflows; no backend-name
registry or automatic tuning/collection is introduced.

| Claim | Required evidence path |
|---|---|
| First generated/answer output, completion, C2 fairness | Relevant routine decode cells with prospective policy2 latency/fairness selectors, or the explicit policy3 scope above |
| Mixed interference | Declared solo controls and actual required overlap in `mixed-prefill-decode-v4`; [schedule and policy contract](README.md) |
| Cache/speculative/preemption accounting | [Bounded provider accounting](PROVIDER-ACCOUNTING.md), selected metrics2 source and exclusive acquisition declaration where required |
| Whole conversation, branches, actual tools | Version-6 profiles above and [conversation evidence](CONVERSATIONS.md) |
| Host/device memory and power | [Native resource capture/inspect/compare](RESOURCES.md); `resource-acquisitions-v6.json` for review-only serving attachment |
| Finite capacity and actual retention eviction/recovery | [Capacity preflight/run/inspect](CAPACITY.md); explicitly selected real source set, native counters and journal, never inference from the small history fixture |
| Startup and restart-cache lifecycle | [Startup observe/store/reload/inspect/compare](STARTUP.md); actual lifecycle/source/cache identity evidence |
| E3 kernel and collective/fabric | [Native microbench capture/inspect/compare](KERNEL-FABRIC.md); explicit producer, runtime/source pins and device window |

C4/C8 remains an explicit scope in the existing full decode workloads, not hidden
routine traffic. Resource/capacity/lifecycle/kernel plans need private actual
source, process/device and collector identities frozen before collection.
Examples are not permission to discover processes, reset caches, start services,
allocate to OOM or initialize devices. Separate authorization is mandatory.
Telemetry adds its own bounded requests and overhead beyond the model-request
counts above. Required-hit and deliberately cache-disabled operation are
incompatible. Inspect suitable existing receipts before changing serving state;
missing historical fields do not prove a current backend lacks them, zero hits
do not prove warm reuse, and missing counters are not zero.

Record implemented, CPU-protocol verified, real-adapter exercised and
live-backend qualified separately for every selected claim in the
[single report template](SHARED-REPORT-TEMPLATE.md). Preserve every failed,
unsupported and inconclusive result. Reuse #45 only for its frozen qualified
scope. Serving throughput cannot substitute for resources, retention, lifecycle,
kernel, semantic quality or tokenizer evidence. Broader mandatory adoption still
requires complete coverage reconciliation and explicit maintainer agreement.

## Install and pin the actual artifact

Follow [download, checksum verification and installation](INSTALL.md) before
executing an archive. Use an explicitly approved release, or a separately reviewed
staged archive while publication is pending; staged artifacts are not promised
public release downloads. The supported native Linux targets and runtime
requirements are stated there. No Rust checkout is required for this workflow.

Set `GRILL_ROOT` to the absolute extracted package directory:

```sh
GRILL_PERF="$GRILL_ROOT/bin/grill-perf"
WORKLOADS="$GRILL_ROOT/workloads"
"$GRILL_PERF" --version
"$GRILL_PERF" bundle verify "$WORKLOADS/recipes-v1.json" --json
sha256sum "$GRILL_PERF"
```

Retain the reviewed archive checksum/build receipt and actual binary/workload
identities. Use this absolute executable path throughout, not an unrelated binary
on `PATH`. Keep packaged workloads immutable during inspection and collection.
The Git revision is not the workload `source_sha256`; the latter hashes exact
file bytes. `workload_sha256` is the typed normalized workload identity.

The source-build fallback remains documented in [INSTALL.md](INSTALL.md).
Historical source `a6729f9ab5410587bb9da1adb7b34944a9cfc436` was CPU-qualified
on native ARM64 for the original four-entry workflow. That historical record is
not a current binary pin, live qualification or qualification of another target.

## Verify and select the exact workload

```sh
MANIFEST="$WORKLOADS/recipes-v1.json"
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

### Phase counts are not protocol equivalence

The separate [v3 phase-budget option](README.md#phase-specific-output-budgets)
can request capped warmup32/measured400 without changing these frozen exact400
recipes. It is a new workload identity, not a repair or relabeling of old evidence.
The historical installation pin above predates v3; use an approved source revision
containing the feature, not a historical qualification receipt as a feature claim.

At pinned upstream sparkDash revision
`d0c7f71296a1071d0d75f95b14c21413d4d06321`,
[`DecodeBench.js`](https://github.com/MiaAI-Lab/sparkDash/blob/d0c7f71296a1071d0d75f95b14c21413d4d06321/server/collectors/DecodeBench.js)
uses capped 32-token best-effort warmup. Measured output defaults to 400 with
`min_tokens`, `ignore_eos` and `stop: []`; an HTTP 400 path can strip fill-force
fields and resend. A capped400 TheGrill request therefore differs from that
initial measured request, even when its maximum token count matches.
Upstream [`LlmStreaming.js`](https://github.com/MiaAI-Lab/sparkDash/blob/d0c7f71296a1071d0d75f95b14c21413d4d06321/server/collectors/LlmStreaming.js)
also sends `enable_thinking`, `thinking` and `thinking_mode` together and has
thinking-control fallback behavior. TheGrill sends one explicitly declared
thinking-key mapping, never infers it from a model name and never strips controls
or retries. It retains warmup evidence and stops on ineligible responses rather
than treating warmup as best-effort.

Neither matching 32/400 counts nor selecting exact measured output establishes
full protocol equivalence: control sets, failure handling, schedules and timing
semantics remain separate. Qualify the declared workload and template rather
than claiming source fidelity from token counts alone.

### Measurement compatibility matrix

This records how the existing selected workloads relate to the pinned upstream
protocol. "Match" means equal under the stated prerequisites; "different" is an
intentional, declared difference; "unsupported" is deliberately absent. It adds
no workload variant, declared field or runtime behavior, and matching prompt text
alone never establishes protocol equivalence.

| Aspect | Existing TheGrill behavior | Pinned upstream `d0c7f712` behavior | Classification |
|---|---|---|---|
| Prompt bytes and concurrent lane variation | Frozen decode cases contain no `{salt}`, so concurrent lanes send the same case text. Where explicitly present, `{salt}` renders as `namespace[..16]-wave.index-lane`; the frozen prefill workload places it before its repeated fill. | `pickDecodeBenchPrompts` returns the base prompt for one lane and appends ` (stream i/n)` per lane at higher concurrency; `buildPrefillPrompt` leads every size with `[prefill-bench <uuid>]`. | Decode base prompts match; concurrent prompt variation differs. Prefill salt placement is analogous, not identical prompt bytes or proof of cache isolation. |
| Warmup schedule, output and failure policy | Per-cell `warmup_trials`; frozen decode declares seven warmup waves (18 warmup requests) and prefill four, each at the declared exact output (400 and 8). Prefill warmups use each cell's full prompt size. Warmup is retained evidence: policy requires every declared warmup eligible, and a new v3 run stops later admission on failure without retry. | DecodeBench runs one capped 32-token warmup per job before the concurrency loop and ignores its failure; PrefillBench runs one best-effort warmup with an estimated 512-token prompt and an eight-token output cap. | Different: count, prompt size, output allowance and failure handling. The generic v3 `warmup_output` declaration can explicitly separate output allowance without rewriting these frozen workloads. |
| Measured `min_tokens`, `ignore_eos`, `stop` | Exact output sets `min_tokens` and `ignore_eos: true`; the request schema has no `stop` field, so no `stop` is serialized, and a failed response is never re-sent with stripped controls. | DecodeBench adds `min_tokens`, `ignore_eos: true` and `stop: []`; an HTTP 400 carrying fill-force fields re-sends once through `stripFillForceFields`, which deletes all three. PrefillBench instead caps output at eight tokens without those fill-force fields. | Decode's initial `min_tokens`/`ignore_eos` match; prefill's output contract differs. `stop` is unsupported and its omission is not proven equivalent to `[]`; control-stripping retries are intentionally unsupported. |
| Explicit thinking mapping | Exactly one declared mapping per workload: legacy `thinking` sends `chat_template_kwargs.thinking`; `thinking_control` `vllm-enable-thinking-v1` sends `chat_template_kwargs.enable_thinking`. Both cannot be non-null and neither is inferred from a model name. | `applyThinkingFlags` sends `enable_thinking`, `thinking` and `thinking_mode` together (off for these benches), and an HTTP 400 retry re-adds all three plus top-level `thinking`/`enable_thinking`. | Different: one declared key, no fallback. |
| Reported versus estimated usage | Required metric counts come from provider-reported usage. Missing counts remain unavailable; the policy's matched-output gate requires completion counts for every metric. Counts are never estimated from text or SSE events. | Prefers `usage.completion_tokens` but falls back to `estimateTokenCount` (about four characters per token) for decode and prompt counts when usage is absent. | Different: estimates are unsupported. |
| Text-event versus settle timing window | Policy `decode_tokens_per_second` uses `(n - 1) * 1e6 / (settle_us - first_generated_text_us)`, including terminal delay and parsing. The text-event rate uses `(n - 1) * 1e6 / (last_generated_text_us - first_generated_text_us)` and is an observation, not a selectable policy metric. | `decodeTps` uses the first-visible-token to last-visible-token window, excluding stream teardown. | Different for the policy gate; the text-event formula is not silently aliased to settle-window decode. |
| Prefill cache exclusion | The derived prefill rate, and the policy gate, require provider-reported cached prompt tokens absent or zero; otherwise the sample is null. Absent cache telemetry is unknown, not evidence of a cold cache. A salt appended after a shared prefix does not isolate that prefix. | `buildPrefillPrompt` puts a fresh random salt at the start to reduce cross-run prefix reuse, runs one request per size and derives `prompt_tokens / TTFT` with a text-estimate fallback. | Analogous leading-salt placement; cache-field exclusion and usage fallback differ. |
| Aggregation and failed-cell behavior | A fixed wave needs every lane eligible; an incomplete response nulls the wave completion total, and a failure or ineligibility stops later admission without retries. Policy requires complete eligible waves and observations per cell and metric; a missing or failed cell cannot pass and every gate reason is retained. | Aggregates only successful streams (`streamsOk`) into mean/median/min/max, reports `streamsFailed` and an error string, and continues to the next concurrency; the aggregate decode rate uses the earliest first-token to latest last-token window. | Different: fail-stop completeness versus a success-only partial aggregate. |

This matrix introduces no new workload variant. Any necessary change identified
by a pilot requires an explicit new declaration and identity, not an edit to a
frozen workload or a silently weakened protocol.

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
Admission enforces the declared trial minimum against the workload. The decision
additionally requires warmup in every cell; admission alone does not enforce that
condition. Basis-point schema bounds are engineering limits, not scientific advice.
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
Native `run` admission permits absent deployment fields; it does not certify
reference completeness. Missing reference declarations prevent a qualified
`decide` result even when collection was admitted.

## One baseline/change/check decision path

Choose exactly one workload file before creating its policy. These frozen bundle
entries are explicit data choices, not model detection or interchangeable
qualifications:

| Declared mapping | Decode file | Prefill file |
|---|---|---|
| `enable_thinking: false` (historical GLM entry) | `glm-decode-v1.json` | `glm-prefill-v1.json` |
| `thinking: false` (historical DeepSeek entry) | `sparkdash-decode-v1.json` | `sparkdash-prefill-v1.json` |

Set `WORKLOAD` to the chosen absolute file under `WORKLOADS`, or another explicitly
reviewed workload. A new backend uses declared data through these same commands,
not a new bundle mapping. Qualify its controls separately.

Set `POLICY`, `DEPLOYMENT_A`, `DEPLOYMENT_B`, `A`, `B` and `A2` to absolute paths.
Each output directory must be new with an existing parent. Set `ENDPOINT_A`,
`ENDPOINT_B` and `MODEL` explicitly. These commands assume HTTPS; literal-loopback
HTTP additionally needs `--local-http` on each preflight/run. Add
`--auth-env MODEL_API_KEY` only when the independently supplied credential is
required; its value never belongs in the policy, workload or command arguments.

First inspect and admit both declared configurations offline:

```sh
"$GRILL_PERF" bundle inspect "$WORKLOAD"
"$GRILL_PERF" preflight "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_A" \
  --model "$MODEL" --deployment "$DEPLOYMENT_A"
"$GRILL_PERF" preflight "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_B" \
  --model "$MODEL" --deployment "$DEPLOYMENT_B"
```

Preflight prints each run's complete warmup/measured request and output ceilings.
Approve the whole three-role window, not just one run's allowance: the unchanged
workload is executed once in each of A, B and A2. A separate unchanged-control
exercise adds its own explicitly planned traffic. Per-request deadlines remain
finite; no automatic probes, extra repetitions or budget expansion occur.

Capture the baseline with the policy already bound:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_A" \
  --model "$MODEL" --deployment "$DEPLOYMENT_A" --out "$A" --json
```

After A completes, make the separately authorized serving change, then capture B:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_B" \
  --model "$MODEL" --deployment "$DEPLOYMENT_B" --out "$B" --json
```

After B completes, restore and independently verify A through the operator's
approved procedure. Capture a fresh A2, then check the retained evidence offline:

```sh
"$GRILL_PERF" run "$WORKLOAD" --policy "$POLICY" --endpoint "$ENDPOINT_A" \
  --model "$MODEL" --deployment "$DEPLOYMENT_A" --out "$A2" --json
"$GRILL_PERF" decide "$A" "$B" --reference "$A2" --json
```

This is the native policy-bound `run`/`decide` path, not a new mode of the
eight-acquisition `baseline`/`check` capture commands. Explicit selections there
remain descriptive; the default C1 assessment retains its own semantics.
Do not pass a capture root as a
native policy run or choose one of its acquisitions after seeing the results.
A final A/B/A2 verdict cannot precede the post-candidate reference.

For an unchanged-control exercise, declare the same deployment in all three roles
and make no serving change. Its result tests the stated policy against those
observed periods; PASS does not establish causality or future repeatability.

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

### Supported policy eligibility conditions

These are the existing conditions under which an explicitly chosen workload can
support a `decide` verdict. Failing a required condition prevents PASS; unresolved
and invalid gates remain visible alongside any separately resolved regression.

- The policy must bind prospective exact pins before dispatch: `collector_sha256`
  is the actual executable and `workload_source_sha256` is the exact selected
  workload file bytes. Baseline, candidate and reference must all carry the same
  captured policy bytes.
- A separately collected A2 reference is required and must declare a start after
  the candidate. A missing repeat, or a baseline/reference identity that is not a
  declared match, is INCONCLUSIVE; `compare` alone is eligibility-only.
- Every declared warmup must exist and be eligible. A cell that declares no
  warmup cannot pass.
- Every declared cell and metric must show complete eligible coverage: the
  expected measured waves and observations for the declared trials and
  concurrency. Missing observations are reported, never treated as zero.
- The ordered measured lanes must report equal completion counts across baseline,
  candidate and reference, and this applies to every policy metric including
  latency-only gates. A completed capture without matched reported counts is not
  eligible.
- The pooled baseline/reference range must sit around a positive minimum and fit
  the declared reference-spread budget.
- Decode and prefill ranges pool lane observations from every measured wave, so
  concurrent lanes contribute dispersion rather than independent repetitions; the
  declared minimum trial count is a floor, not a precision or power guarantee.
- Every gate keeps its own coverage and bounded reason codes. A resolved
  REGRESSION outranks an unresolved INCONCLUSIVE gate; the aggregate outcome is
  the maximum over gates, and PASS requires every gate rather than a favorable
  filtered subset.

Explicit-selection baseline/check captures and raw `compare` output remain
descriptive; neither produces this policy PASS/REGRESSION vocabulary.
See the [policy guide](README.md#captured-observed-envelope-policy) for the closed schema
and decision arithmetic.

Maintain separate source-reviewed, loopback-tested and live-qualified statuses
for each recipe and workload. Both explicit control mappings and all four
workload entries were exercised through the CLI against synthetic responses.
The current campaign includes GLM EXL3 and DeepSeek DSpark2 only. Qwen is
explicitly excluded because its deployment is not set up; its live qualification
is not a campaign completion gate. The operator owns every serving change,
restoration, credential and traffic budget, and TheGrill adds no launcher.
Earlier model smoke receipts do not qualify this new bundle/policy workflow.

### Demonstrated capped-prefill variant

The GLM live exercise exposed a difference that source review alone did not
qualify: under exact eight-token output, a prefill response emitted `OK` and then
an unsolicited tool-call frame. Native collection stopped as unsupported, with
missing usage and incomplete coverage. That failed acquisition remains failed.
Do not weaken the text-only parser, estimate usage, or retry the same request
after stripping exact-output controls.

An explicitly new capped-eight-token workload and prospective policy completed
the prefill exercise. For the same 4K/16K scope, author a new file from either
declared prefill mapping before creating any policy:

```sh
# PREFILL_SOURCE is the explicitly chosen original workload, not a selection.
# Choose a new PREFILL_CAPPED path; noclobber prevents overwriting prior evidence.
(
  set -C
  jq '.version = 3
      | .name = "campaign-prefill-capped-4k-16k-v1"
      | .request.output.mode = "cap"
      | .cases |= map(select(.id == "prefill-4k" or .id == "prefill-16k"))
      | .cells |= map(select(.id == "prefill-4k-1" or .id == "prefill-16k-1"))' \
    "$PREFILL_SOURCE" > "$PREFILL_CAPPED"
)
"$GRILL_PERF" bundle inspect "$PREFILL_CAPPED"
```

This is explicit workload authoring with `jq`, not a backend fallback or a new
installed-runtime dependency. It preserves the chosen thinking mapping. Bind the
new file's exact source hash in a new policy and collect fresh A/B/A2 evidence;
never reuse an exact-output acquisition in the capped study. With three measured
trials and one warmup per cell, the variant admits eight requests and at most
64 output tokens per acquisition. It is not historical exact-eight-token
equivalence and is not added to the frozen `recipes-v1.json` mapping.

Cap mode permits differing completion counts. The existing matched-output gate
still requires equal reported counts across A/B/A2 for every measured lane;
variation remains INCONCLUSIVE, not a reason to relax the gate after collection.

### Completed GLM live scope

The installed ARM64 collector at source
`e26b2e61d1ba2d2802059d53bf7985aa0ed1c567`, binary SHA-256
`e19a5794c12e72534e46a06b1ca3bf55d394421305395a5a09788be57ffed5a5`,
completed five separately collected roles for decode C1/C2 and capped prefill
4K/16K. Policies were frozen before acquisition: three measured trials per cell,
1,000 basis points adverse tolerance and 2,500 basis points reference spread.
Those are this campaign's engineering limits, not recommended statistical margins.

| Study | Unchanged A/control/control-reference | A/candidate/restored-A2 |
|---|---|---|
| Decode: latency, achieved throughput, settle-window decode at C1/C2 | PASS, all six gates | REGRESSION, all six gates |
| Capped prefill: reported prompt tokens / first-text time at 4K/16K | PASS, both gates | PASS, both gates |

All ten accepted acquisitions were complete and eligible. All four offline
decisions replayed with zero network syscalls. The campaign issued 107 generation
requests, including the four-request protocol probe and three requests in the
failed exact-prefill acquisition; requested output ceiling was 20,056 tokens.
Reported completion counts totalled 19,808 for requests with usage; the failed
request had no usage and its actual completion count remains unavailable.
The capped-prefill responses reported two completion tokens and answered `OK`;
decode's ordered counting output was truncated by its exact-token budget.
No semantic full-task, model-quality, cold-device, causal or future-repeatability
claim follows. Deployment identities, serving changes and raw receipts stay
private; the table does not qualify another model, workload, or artifact.

### Completed DeepSeek live scope

The same installed collector and prospective engineering limits completed decode
C1/C2 and capped prefill 4K/16K on the declared DeepSeek deployment. Each study
retains separate A, control, control-reference, candidate and restored-A2 roles.

| Study | Unchanged A/control/control-reference | A/candidate/restored-A2 |
|---|---|---|
| Decode: latency, achieved throughput, settle-window decode at C1/C2 | PASS, all six gates | REGRESSION, all six gates |
| Capped prefill: reported prompt tokens / first-text time at 4K/16K | PASS, both gates | PASS, both gates |

All ten accepted acquisitions were complete and eligible, and all four decisions
replayed with zero network syscalls. The candidate included a coupled serving
change, not an isolated causal variable. An earlier candidate acquisition for
each workload had an incorrect deployment declaration: effective startup
configuration disagreed with the declared derived setting. Both captures remain
retained but excluded from qualification. Before inspecting their timing or
computing a candidate decision, the operator pinned a corrected declaration and
an additional finite allowance, then collected separately identified replacement
captures without changing the workloads or policy thresholds.

Those replacements had extra prior-acquisition warmup history relative to the
fresh-launch baseline and restored reference. The direction of any warmup or
thermal effect is unknown; neither PASS nor REGRESSION establishes symmetric
cold-start performance or gains a stronger causal interpretation from this
history.

The DeepSeek exercise issued 127 generation requests with a requested output
ceiling of 23,888 tokens. This includes 124 benchmark requests (the protocol
probe, ten accepted acquisitions and two declaration-invalid acquisitions) plus
three bounded launcher smoke requests. The benchmark responses reported 23,504
completion tokens; the launcher smoke responses' actual token counts were not
retained, and their combined requested ceiling was 96. An earlier unsuccessful
startup served no requests. Optional unbounded startup API warmup was disabled;
internal engine initialization is not a benchmark request.

Every retained benchmark response reported zero cached prompt tokens and no
reasoning text. Capped-prefill responses answered `OK` with two reported
completion tokens; decode output was truncated by the exact-token budget.
These are observed control/usage facts for the captured requests, not cache
attestation, semantic full-task completion, model-quality validation or future
repeatability. Raw receipts and deployment-local details remain private.

### Source-reviewed Qwen diagnostic, excluded from this campaign

The Qwen diagnostic remains source-reviewed from the authoritative pinned
[`ndec.py`](https://github.com/MiaAI-Lab/Qwen3.8-27B-SGLang-DGX-Spark/blob/9fb18edf8cfb3364e8aa89258e6d5ab1fe1fd11a/bench/ndec.py):
one 16-token warmup, then two nonstreaming calls per prompt at caps 60 and 600
across two prompts, reporting `(c600 - c60) / (t600 - t60)` from provider
completion counts and wall times. That differential estimate is a separate
diagnostic, not streaming decode equivalence, and TheGrill does not implement it.

Use the [manual reviewed report template](SHARED-REPORT-TEMPLATE.md) only after
privacy review. It is not an exporter or replayable evidence package. No upstream
recipe files are changed by these commands; maintainers may separately adopt
these public command blocks without copying private deployment code or evidence.
