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

**There are no published releases yet.** The guide works with a reviewed staged
archive and includes a source fallback; it does not assume a public download.
The performance archive contains `bin/grill-perf` and adjacent pinned files under
`workloads/`. Installed use needs Linux, not Rust or a source checkout.

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
wrapper or registry.

| Display label | Existing JSON `result` code | Meaning |
|---|---|---|
| **MEASURED FASTER** | `IMPROVED` | The comparison model supports higher measured throughput between these capture periods |
| **MEASURED SLOWER** | `REGRESSED` | It supports lower measured throughput between these periods |
| **COMPLETE - DESCRIPTIVE ONLY** | `DESCRIPTIVE` | An explicitly selected comparison completed successfully; no faster/slower, equivalence or no-regression verdict |
| **INCONCLUSIVE** | `INCONCLUSIVE` | No direction is established, or evidence is insufficient; this does not mean equivalent performance |
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

[Full performance guide and measurement limits](docs/performance/README.md).
Advanced `run`, `pause`, `resume`, raw-run `compare`, and captured-policy `decide`
remain separate workflows. The [shared recipes](docs/performance/SHARED-RECIPES.md)
retain their [sparkDash](https://github.com/MiaAI-Lab/sparkDash) attribution and
original observed-envelope semantics; they are not the new default assessment,
and historical results are not reinterpreted.

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

[Contributing](CONTRIBUTING.md) · [Code organization](docs/REPOSITORY.md) · [Security](SECURITY.md) · [MIT license](LICENSE)

MIT covers this project's code and documentation. Benchmark data and model weights keep their own licenses.
