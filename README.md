<p align="center">
  <img src="assets/the-grill.webp" width="180" height="180" alt="The Grill: an open charcoal barbecue">
</p>

# The Grill

**Measure how fast a serving setup runs an LLM, then compare setups.**

A serving setup is the engine plus its settings, such as vLLM with a quantized model at a given context length. Run the same test on each setup, then compare the saved results.

| I want to... | Use | What you get |
|---|---|---|
| Check whether a serving setup is faster | [Speed benchmarks](#speed-benchmarks)<br>`grill-perf` ![Ready to try](https://img.shields.io/badge/ready_to_try-0969da?style=flat-square) | Time per request group, combined tokens per second, comparison of saved runs |
| Check whether a model answers tasks correctly | [Quality evaluation](#quality-evaluation-wip)<br>`grill` ![Work in progress](https://img.shields.io/badge/work_in_progress-a16207?style=flat-square) | Graded answers, with wrong, refused, cut-off, and missing results kept separate |

Both tools run on your machine and connect to a server you already run. No project account or results upload is required. **This is experimental software.**

## Get started

For serving-speed checks, follow **[Install and first capture](docs/performance/INSTALL.md)**:
verify a pinned artifact before unpacking, supply the actual server inputs once,
capture a baseline, optionally capture an unchanged control, change serving state
yourself, check, and replay the report offline.

The latest published pre-release is [v0.4.0](https://github.com/plotarmordev/thegrill/releases/tag/v0.4.0),
with cross-platform deployment comparison and portable workloads. Earlier
releases stay available for historical studies; do not assume an older binary
contains newer source features.
The published performance archive contains `bin/grill-perf`, pinned `workloads/`
and offline performance guides. Published binaries require Linux and no Rust or
TheGrill checkout. A Rust 1.98 source build additionally supports portable
serving captures on Apple Silicon macOS; Linux-native resource and external
program collectors remain unavailable there. Opt-in Python producers require
their explicitly reviewed Python/backend runtime; the archive does not install
those dependencies.

## Speed benchmarks

Use **`grill-perf`** against a server you already run. The
[authoritative first-run workflow](docs/performance/INSTALL.md#baseline-optional-control-change-check)
needs no statistical policy file. `check` inherits the verified baseline's
endpoint, model selector, workload and credential-environment name; deployment
identities remain explicit operator declarations, not attested server facts.

The default is the short structured **C1** comparison. For explicitly selected,
descriptive-only concurrent or conversation observations, use the same CLI with
[packaged selection paths](docs/performance/INSTALL.md#optional-selections-and-failures).
[Recipe embedding](docs/performance/RECIPES.md) supplies data, not a model-specific
wrapper or registry. To compare two whole deployments, such as one model on a
Mac and on an NVIDIA host, use the
[A/B/A2 deployment comparison](docs/performance/INSTALL.md#compare-two-deployments-aba2).

| Display label | Existing JSON `result` code | Meaning |
|---|---|---|
| **MEASURED FASTER** | `IMPROVED` | The comparison model supports higher measured throughput between these capture periods |
| **MEASURED SLOWER** | `REGRESSED` | It supports lower measured throughput between these periods |
| **COMPLETE - DESCRIPTIVE ONLY** | `DESCRIPTIVE` | An explicitly selected comparison completed successfully; no faster/slower, equivalence or no-regression verdict |
| **INCONCLUSIVE** | `INCONCLUSIVE` | No direction is established, or evidence is insufficient; this does not mean equivalent performance |
| **VERDICT PENDING** | `PENDING` | A deployment candidate is captured; its verdict needs the unchanged reference |
| **INVALID** | `INVALID` | Response, identity or evidence checks failed; the report explains why and retains the available evidence |

Baseline readiness is not a comparison verdict. Exit success is not a universal
no-regression certificate: read the result and its displayed scope. Historical
reports, stored result codes and comparison meanings are not reinterpreted.

The observed percentage is separate from its model-based uncertainty range.
Sequential captures cannot isolate the serving change from time, load or cache
effects. A measured direction establishes neither causality nor practical
significance, and there is no guaranteed precision or detection of a 5% change.

The [scope and budget reference](docs/performance/README.md#default-scope-and-budget)
defines the default workload. Preflight prints scope and the complete allowance
before traffic. For slower servers, choose a finite `--seconds` allowance
prospectively; exhaustion is not automatically server failure. There are no
automatic retries or replacement samples. Reports and raw evidence stay local;
`compare` verifies saved captures without network calls.

### Contributor claims beyond the default C1 check

Use the **[shared claim map and recipe profiles](docs/performance/SHARED-RECIPES.md)**
and **[single report template](docs/performance/SHARED-REPORT-TEMPLATE.md)** as the
entrypoint for contributor performance evidence. Select the relevant routine,
stress or domain scope before collection; do not run the entire matrix
automatically. The map separates first-output/completion, fairness, mixed
interference, accounting, tools/history, resources, retention/capacity,
startup/reload and kernel/fabric evidence.

The default CLI remains the bounded C1 assessment above. Advanced `run`, `pause`,
`resume`, raw-run `compare` and captured-policy `decide` keep their separate
meanings; domain collectors do not substitute for serving or quality checks.
The recipes retain their [sparkDash](https://github.com/MiaAI-Lab/sparkDash)
attribution, and historical evidence is not reinterpreted.

Report source implementation, CPU-protocol verification, real-adapter exercise
and live-backend qualification separately for the exact artifact and selected
scope. Fixture success is not model/cache/tokenizer proof; missing evidence and
INCONCLUSIVE results stay visible. Broader mandatory PR adoption remains subject
to completed coverage review and explicit maintainer scope/exception agreement.

[Full performance guide and measurement limits](docs/performance/README.md).

## Quality evaluation (WIP)

Use **`grill`** to collect model answers and check them against task rules.

**This part is a work in progress.** The included questions are synthetic examples, not a validated intelligence test. It currently supports text answers, not agents or code execution.

Build this separate source-only CLI on Linux with Rust/Cargo 1.98, a C/C++
toolchain and CMake; it is not shipped in the performance archive:

```sh
git clone https://github.com/plotarmordev/thegrill.git
cd thegrill
cargo build -p grill --release --locked
mkdir -p results
```

Start your server separately. Replace the endpoint and model selector below.
For remote use, select HTTPS; local HTTP requires a literal loopback address.
If authentication is required, supply the credential independently in your
environment and add `--auth-env NAME` to `run`, never the key itself.

```sh
target/release/grill run examples/synthetic-pack.json \
  --endpoint http://127.0.0.1:8000/v1/chat/completions \
  --local-http --model your-model --token-cap 4096 --stream \
  --out results/answers

target/release/grill inspect results/answers --json
target/release/grill regrade results/answers --out results/answers-regraded
```

Inspection and regrading use saved files. They do not call the model again. Wrong answers, refusals, cut-off responses, and missing results stay separate.

[Task formats and quality evaluation guide](docs/PROJECT.md)

## Keep your results safe

Use a new output directory for each run. Results contain prompts and model responses, so review them before sharing.

[Contributing](CONTRIBUTING.md) · [Code organization](docs/REPOSITORY.md) · [Security](SECURITY.md) · [Apache 2.0 license](LICENSE)

Apache License 2.0 covers this project's code and documentation; releases up to v0.5.0 were published under MIT. Benchmark data and model weights keep their own licenses.
