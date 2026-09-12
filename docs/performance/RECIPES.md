# Recipe-facing baseline and check

Use the existing `grill-perf` binary against a server the operator already runs.
Recipe code supplies explicit inputs and delegates collection, evidence and
arithmetic to TheGrill. It does not detect models, change serving state, create
forwards, flush caches, retry controls or upload results.

## Use the installed workflow, not a wrapper

Follow **[Install and first capture](INSTALL.md)** for the authoritative pinned
artifact/source fallback, declaration construction, baseline, optional unchanged
control, separate operator change, candidate check and offline report commands.
There are no published releases yet; a reviewed staged artifact works outside a
checkout. The archive's executable is `$GRILL_HOME/bin/grill-perf`, exposed in
the examples as the absolute `GRILL_PERF` path.

A recipe supplies known values to that workflow; it does not generate its own
benchmark code, statistical policy or compatibility retry loop.

| Input | Recipe/operator responsibility | What `check` inherits |
|---|---|---|
| Endpoint and model selector | Explicit full resource URL and actual server selector; no model detection | Verified baseline endpoint, selector and loopback permission |
| Request controls and membership | Qualify backend support; choose default C1 or an explicit pinned selection before baseline | Exact retained workload and selection, not a recipe lookup |
| Authentication | Supply credential independently; pass only its environment-variable name to baseline, or omit for unauthenticated use | Variable name only; the value must remain available in the process environment |
| Deployment | Reuse actual `model_revision`, `runtime`, `hardware`, `settings` facts; obtain missing facts from the operator | Baseline declaration for comparison; pass an explicit candidate declaration with only the selected field changed, or the same declaration for `--change none` |
| Output and allowance | Choose a fresh private output outside the baseline with an existing nonsymlink parent; approve finite traffic prospectively | No output path or custom `--seconds` allowance is inherited |

The first-run declaration example uses actual recipe variables, not fictional
identities. Its candidate example updates just the declared field, retaining
only facts confirmed unchanged. Do not copy stale facts to hide a multi-field
migration. Declarations are operator claims, not execution attestation.

## Choose explicit packaged workloads

Add `--selection "$SELECTION"` to the existing baseline command when a recipe
needs descriptive selected scope. Choose the relevant path, not all alternatives:

| Selection path | Explicit capability |
|---|---|
| `$GRILL_HOME/workloads/concurrency-selection-v1.json` | C1/C2/C4 ladder, legacy `chat_template_kwargs.thinking: false` |
| `$GRILL_HOME/workloads/concurrency-enable-thinking-selection-v1.json` | Separate ladder identity with `chat_template_kwargs.enable_thinking: false` |
| `$GRILL_HOME/workloads/conversation-selection-v2.json` | [Bounded conversation profile](CONVERSATIONS.md), including its independently required tool/cache/usage controls |

Do not infer controls from a model name. The thinking variants cannot be paired
as equivalent workloads, and rejection never triggers a switch between them.
A third ordinary configuration supplies its own endpoint, selector, auth name
and declaration through the same commands without collection/assessment branches.

Keep each selection beside its workload leaf file and preserve exact bytes.
`$GRILL_HOME/workloads/recipes-v1.json` remains the historical bundle manifest,
not a selection manifest. [SHARED-RECIPES.md](SHARED-RECIPES.md) retains that
workflow's sparkDash attribution and observed-envelope meanings; its historical
source pin does not qualify the newer selected-capture interface.

For source fallback only, these files are under
`$SOURCE/crates/grill-perf/examples/`. Once the baseline is captured, candidate
collection does not need the original recipe files or their paths; it verifies
the baseline's retained bytes and requires the same collector executable.

## Inspect and pin offline

A recipe may supply another bounded native workload with a manifest following
the [closed selection schema](README.md#explicit-selected-captures). Obtain raw
and typed workload digests from the existing offline command:

```sh
"$GRILL_PERF" bundle inspect "${WORKLOAD:?explicit approved workload file required}"
```

Use its returned `source_sha256` and `workload_sha256`, not `jq -S` or arbitrary
canonical-JSON hashes. It also reports declared controls and one native run's
budget; capture preflight prints the complete acquisition allowance before
traffic. Scope and operation class describe prospective membership; `unknown`
does not mean normal or safe, and `stress` is not normal operation.

Keep source revision, executable digest, raw/typed workload digests and deployment
declarations distinct. The staged receipt records build and package identities;
source users record their actual toolchain, build command/profile and flags.
Do not dump the environment or infer execution attestation from a digest.

Pin drift, unsupported locally declared controls and missing declarations fail
before requests. Backend support and authorization can still fail at the server;
native status, request, response and partial evidence remain intact, with no
field stripping or resend. See [first-run failure actions](INSTALL.md#optional-selections-and-failures).

## Read the selected scope

Every explicit selection is descriptive, including selected C1. A complete
comparison reports **COMPLETE - DESCRIPTIVE ONLY** / `DESCRIPTIVE`, not a
faster/slower, PASS, equivalence or noninferiority claim. The
[first-run result table](INSTALL.md#baseline-optional-control-change-check)
separates success, incomplete evidence and invalid input from default C1 measured
directions. Exit success is not a universal no-regression certificate.

Keep acquisitions/cells separate; concurrent lanes are not independent samples.
Native summaries retain missing usage, partial peers, request failures, timing,
aggregate throughput and per-stream rates. Do not replace unavailable values with
zero or pool unlike cells into an improvement percentage. Retain unchanged
controls and budget stops rather than rerunning until a preferred result appears.

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
