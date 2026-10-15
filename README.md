<p align="center">
  <img src="assets/the-grill.webp" width="180" height="180" alt="The Grill: an open charcoal barbecue">
</p>

# The Grill

**Did your change make your LLM server faster? The Grill measures it and tells you.**

Run a short test against the model server you already have. Change something:
the engine, a flag, the model build, the machine. Run it again and get a plain answer:
**measured faster**, **measured slower**, or **inconclusive**. Everything runs and stays
on your machine.

It works with any OpenAI-compatible server that reports token usage while streaming,
and has been used with vLLM, TensorFold and oMLX on Linux and Apple Silicon Macs.
**It is early, experimental software.**

## Try it

Download the [latest release](https://github.com/plotarmordev/thegrill/releases/tag/v0.6.0)
for your machine (no Rust needed):

```sh
NAME=grill-perf-0.6.0-aarch64-unknown-linux-gnu  # or ...-x86_64-unknown-linux-gnu, ...-aarch64-apple-darwin
URL=https://github.com/plotarmordev/thegrill/releases/download/v0.6.0
curl -fL -O "$URL/$NAME.tar.gz" -O "$URL/$NAME.tar.gz.sha256"
sha256sum -c "$NAME.tar.gz.sha256" && tar -xzf "$NAME.tar.gz"  # on a Mac: shasum -a 256 -c
GRILL="$PWD/$NAME/bin/grill-perf"
```

Describe your setup in a few words, then measure it:

```sh
echo '{"model_revision":"my-model","runtime":"vLLM 0.11","hardware":"1x H100","settings":"defaults"}' > serving.json
"$GRILL" baseline --workload portable-v1 --client-placement same-host \
  --endpoint http://127.0.0.1:8000/v1/chat/completions --local-http \
  --model my-model --deployment serving.json --out before
```

Change your server yourself, update `settings` in `serving.json`, and measure again:

```sh
"$GRILL" check before --change settings --deployment serving.json --out after
```

Each run takes a few minutes at most and prints the answer. Raw results stay in
`before/` and `after/`. They contain your prompts and responses, so review them
before sharing.

| Answer | Meaning |
|---|---|
| **MEASURED FASTER** / **MEASURED SLOWER** | A clear difference between the two runs |
| **INCONCLUSIVE** | Too close to call, or not enough evidence. Not the same as "equal" |
| **INVALID** | Something in the run or the evidence failed a check; the report says what |

A difference between two runs doesn't prove your change caused it: time, load and
caches can move results too. An unchanged control run helps show how much.
[Install and first capture](docs/performance/INSTALL.md) walks through control runs,
remote servers and API keys.

## Example: the same model on a DGX Spark and a Mac Studio

The same 4-bit Qwen3.8-Flash-Next checkpoint, served by TensorFold 0.6.0 on each machine:

| One request at a time | DGX Spark (GB10) | Mac Studio (M5 Ultra) |
|---|---|---|
| Output speed | 118 tok/s | 246 tok/s |
| Answer | | **MEASURED FASTER**, +108.7% |
| Repeat of the Spark run | -0.2% (stable) | |

This compares two whole setups, not two chips: the Mac build couldn't use two of the
Spark's settings, and the test's counting prompt suits speculative decoding.
[How to compare two machines](docs/performance/INSTALL.md#compare-two-deployments-aba2).

## More tests

Besides the quick test above, `grill-perf` includes the
[sparkDash](https://github.com/MiaAI-Lab/sparkDash) decode and prefill tests,
long prompts, coding prompts, multi-request tests and conversation history checks.
See the [list of tests](docs/performance/INSTALL.md#optional-selections-and-failures)
and the [full speed guide](docs/performance/README.md).

## Quality evaluation (work in progress)

A second tool, `grill`, checks model answers against task rules. It is not ready
yet: the included questions are synthetic examples, not a validated intelligence test.

<details>
<summary>Try it anyway (Linux, built from source)</summary>

Needs Rust/Cargo 1.98, a C/C++ toolchain and CMake.

```sh
git clone https://github.com/plotarmordev/thegrill.git && cd thegrill
cargo build -p grill --release --locked
target/release/grill run examples/synthetic-pack.json \
  --endpoint http://127.0.0.1:8000/v1/chat/completions \
  --local-http --model your-model --token-cap 4096 --stream --out answers
target/release/grill inspect answers --json
```

[How quality evaluation works](docs/PROJECT.md)

</details>

[Contributing](CONTRIBUTING.md) · [Code organization](docs/REPOSITORY.md) · [Security](SECURITY.md) · [Apache 2.0 license](LICENSE) (releases up to v0.5.0 were MIT)
