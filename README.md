<p align="center">
  <img src="assets/the-grill.webp" width="180" height="180" alt="The Grill: an open charcoal barbecue">
</p>

# The Grill

**Benchmark serving recipes. Evaluate model answers. Two tools, separate results.**

The Grill runs against your existing Chat Completions server and saves evidence locally. Use either tool independently—no project account, hosted service, or mandatory upload.

## Choose your benchmark

| Your question | Mode & command | What you get |
|---|---|---|
| **“Is this serving recipe faster?”** | **[Recipe performance](#recipe-performance)**<br>`grill-perf` | Timing, token throughput, recipe comparisons |
| **“How well does this model answer?”** | **[Intelligence evaluation](#intelligence-evaluation)**<br>`grill` · **WIP** | Answer grades, failure accounting, comparisons |

**Early-stage software:** performance collection is implemented; qualify it on your deployment. Intelligence evaluation is still a work in progress—not a calibrated general-intelligence score.

## Build once, choose a tool

Requires **Linux, Rust/Cargo 1.98, a C/C++ toolchain, and CMake**.

```sh
git clone https://github.com/plotarmordev/thegrill.git
cd thegrill
cargo build --workspace --release --locked
mkdir -p results
```

The examples below use a local server on port `8000`. Replace the port and `your-model` with your recipe's settings. Start the server yourself; The Grill does not launch or configure it.

For authenticated endpoints, set `MODEL_API_KEY` and add `--auth-env MODEL_API_KEY`. Remote servers need HTTPS or a separately managed local forward; `--local-http` permits only literal loopback addresses.

## Recipe performance

**For comparing engines, quantizations, runtime settings, or other serving recipes.** Measures speed—not answer quality.

```sh
target/release/grill-perf run crates/grill-perf/examples/quick.json \
  --endpoint http://127.0.0.1:8000/v1/chat/completions \
  --local-http --model your-model --out results/recipe-a
```

The supplied workload runs at concurrency **1 and 6**, with warmup and three measured trials per cell, capped at **1,024 output tokens** per request.

Change your recipe, then repeat that command with `--out results/recipe-b`. Keep the **same workload and collector binary**, then compare:

```sh
target/release/grill-perf compare results/recipe-a results/recipe-b --json
```

| Measure | Meaning |
|---|---|
| **Wave latency** | First request dispatch to last request settlement in a fixed group |
| **Completion throughput** | Provider-reported completion tokens over that group’s elapsed time; may include reasoning |
| **Matched change** | Withheld when the runs lack compatible evidence or paired output amounts differ |

These are client observations, **not maximum server capacity**. A token cap does not force equal output lengths. Resumed runs do not qualify as uninterrupted timing comparisons.

**[Full performance guide →](docs/performance/README.md)** Workload controls, exact-output profiles, cache observations, pause/resume, and interpretation.

## Intelligence evaluation

> **Work in progress.** The collection and grading workflow works, but the bundled synthetic tasks demonstrate the format—not a validated intelligence benchmark. Current support is text-only, direct-answer evaluation; no generated-code execution or agents.

```sh
target/release/grill run examples/synthetic-pack.json \
  --endpoint http://127.0.0.1:8000/v1/chat/completions \
  --local-http --model your-model --token-cap 4096 --stream \
  --out results/answers

target/release/grill inspect results/answers --json
target/release/grill regrade results/answers --out results/answers-regraded
```

Use your own task pack and a suitable token budget for meaningful evaluation. Incorrect answers, refusals, truncation, and missing evidence remain distinguishable. Inspection and regrading are **offline**; use `plan` instead of `run` to preview requests without sending them.

**[Quality formats & usage →](docs/PROJECT.md)** Task packs, grading, study manifests, comparisons, and pause/resume.

## Before sharing results

Use fresh output directories. Runs retain prompts and responses: **review content and rights before sharing**. Declared model names and provider usage are evidence, not independent verification of model identity or billing.

[Code organization](docs/REPOSITORY.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) · [MIT license](LICENSE)

The MIT license covers TheGrill’s code and documentation, not third-party benchmark data or model weights.
