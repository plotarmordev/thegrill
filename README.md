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

Use **`grill-perf`** to measure a serving setup.

**1. Test your first setup.** Two workloads follow [MiaAI-Lab's sparkDash](https://github.com/MiaAI-Lab/sparkDash) protocols, with credit: `sparkdash-decode-v1.json` (prose, code, structured and JSON cases; thinking off; exact output length) and `sparkdash-prefill-v1.json` (salted 4k to 32k prompts, prompt tokens per second to first token). Both need a vLLM-compatible server.

```sh
target/release/grill-perf run crates/grill-perf/examples/sparkdash-decode-v1.json \
  --endpoint http://127.0.0.1:8000/v1/chat/completions \
  --local-http --model your-model --out results/setup-a
```

The decode workload sends **72 requests**, the prefill workload **16**. For a server without vLLM controls, `quick.json` sends **28 requests** with a **1,024 token** cap and no thinking control.

**2. Change the setup and test again.** For example, switch the quantization or context length. Repeat the command with `--out results/setup-b`. Use the same test file and tool build for both runs.

**3. Compare the saved results.** No server connection is needed for this step.

```sh
target/release/grill-perf compare results/setup-a results/setup-b --json
```

| Result | What it tells you |
|---|---|
| **Time per group** | How long a group of requests took from first send to last finish |
| **Combined tokens/sec** | Tokens produced per second across the whole group, as reported by the server |
| **Decode tokens/sec** | Per-stream rate after the first token, defined to match sparkDash |
| **Prefill tokens/sec** | Prompt tokens per second to the first token, defined to match sparkDash |
| **Change** | The difference between the two saved runs, shown only when the runs are comparable and only when the two runs' ranges do not overlap |

<details>
<summary><strong>How to avoid misleading speed comparisons</strong></summary>

- Token counts come from the server and may include thinking tokens. The sparkDash workloads declare thinking off through `chat_template_kwargs.thinking`; check `first_generated_channel` is `answer` in the receipts, because a template that ignores it is not detected. Use the same thinking settings for both runs.
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
