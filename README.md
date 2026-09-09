<p align="center">
  <img src="assets/the-grill.webp" width="180" height="180" alt="The Grill: an open charcoal barbecue">
</p>

# The Grill

**Compare the speed of your LLM serving recipes.**

Run the same test on two setups and compare the results. A recipe is the engine and settings you use to run a model.

The Grill also has a separate tool for checking model answers. That part is still being developed.

[![Speed benchmarks](https://img.shields.io/badge/Speed-benchmarks-0969da?style=flat-square)](#speed-benchmarks) [![Quality evaluation: WIP](https://img.shields.io/badge/Quality-WIP-a16207?style=flat-square)](#quality-evaluation-wip)

The tools run on your machine and connect to your chosen server. No project account or results upload is required. **This is experimental software.**

## Get started

You need **Linux, Rust/Cargo 1.98, a C/C++ toolchain, and CMake**.

```sh
git clone https://github.com/plotarmordev/thegrill.git
cd thegrill
cargo build --workspace --release --locked
mkdir -p results
```

Start your model server first. In the examples below, replace port `8000` and `your-model` with your server's settings. The Grill does not start or change your server.

<details>
<summary><strong>Using a remote server or an API key?</strong></summary>

- For a remote server, use its HTTPS URL or a separately managed local forward. `--local-http` permits HTTP only on literal loopback addresses such as `127.0.0.1`.
- If a key is needed, set `MODEL_API_KEY` in your environment and add `--auth-env MODEL_API_KEY` to the run command. Do not put the key in a workload file.

</details>

## Speed benchmarks

Use **`grill-perf`** to measure your serving recipe.

**1. Test your first setup.**

```sh
target/release/grill-perf run crates/grill-perf/examples/quick.json \
  --endpoint http://127.0.0.1:8000/v1/chat/completions \
  --local-http --model your-model --out results/recipe-a
```

The quick test sends **28 requests** in groups of **1 and 6**. Each request asks for a limit of **1,024 output tokens**.

**2. Change your recipe and test again.** Repeat the command with `--out results/recipe-b`. Use the same test and tool build for both runs.

**3. Compare the saved results.** No server connection is needed for this step.

```sh
target/release/grill-perf compare results/recipe-a results/recipe-b --json
```

| Result | What it tells you |
|---|---|
| **Time per group** | How long the whole group of requests took to finish |
| **Combined tokens/sec** | How many tokens your server produced per second across the group |

<details>
<summary><strong>How to avoid misleading speed comparisons</strong></summary>

- Token counts come from the server and may include thinking tokens. The Grill does not turn thinking off for you. Use the same thinking settings for both runs.
- A token cap does not force equal answer lengths. If one run produces shorter answers, the tool will not call it a matched speed improvement.
- The quick test includes warmup and three measured trials per group size. The timings include client and network effects. They are not maximum server capacity.
- Pausing and resuming changes the measurement session. Resumed performance runs do not qualify as uninterrupted timing comparisons.

</details>

[Full speed benchmark guide](docs/performance/README.md)

## Quality evaluation (WIP)

Use **`grill`** to collect model answers and check them against task rules.

**This part is a work in progress.** The included questions are synthetic examples, not a validated intelligence test. It currently supports text answers, not agents or code execution.

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
