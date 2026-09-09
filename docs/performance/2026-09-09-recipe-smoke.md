# Two-recipe compatibility and GLM A/A smoke — 2026-09-09

**Collection and comparison passed; timing stability was not established.** This is an early qualification receipt, not an optimization result or a model ranking.

The same published `grill-perf` binary and synthetic workload collected:

| Serving setup | Requests | Completed waves | Eligible measured waves |
|---|---:|---:|---:|
| DeepSeek V4 Flash, DSpark, `deepseek-v4-flash-abliterated` | 9 | 6/6 | 4/4 |
| GLM-5.3 Flash EXL3, DFlash2, run A | 9 | 6/6 | 4/4 |
| Same GLM deployment, run B | 9 | 6/6 | 4/4 |

Both recipes are vLLM-derived. This does **not** qualify other Chat Completions implementations, every request profile, pause/resume under production load, or long-running operation.

## What the unchanged GLM comparison showed

Run B followed run A without a deployment change or restart. Both used one execution session. The offline comparator exited 0: workload, collector and transport identities were compatible; every declared warmup and measured trial was eligible; every paired request reported 64 completion tokens.

| Concurrency | A median wave latency | B median wave latency | A achieved completion tok/s | B achieved completion tok/s | Throughput change |
|---|---:|---:|---:|---:|---:|
| 1 | 1.899630 s | 2.519836 s | 33.799 | 25.903 | −23.36% |
| 2 | 1.831304 s | 2.959748 s | 69.950 | 45.717 | −34.64% |

Wave latency increased by 32.65% and 61.62%, respectively. **Nothing was optimized between these runs.** The variation is a warning against promoting a two-trial timing difference as an improvement. Its cause was not isolated.

All 18 GLM benchmark requests emitted reasoning and reached the 64-token cap without an observed answer-text event. These are **provider-reported completion-token rates, not answer-only decode rates**. Missing answer-text timestamps remain null in the [privacy-reviewed summary](2026-09-09-recipe-smoke.json). A separate, non-benchmark, thinking-disabled arithmetic smoke returned the expected `4`; it is not a quality evaluation.

Achieved throughput is each wave's reported completion count divided by its entire elapsed interval. The table takes the median of those per-wave rates; it is not calculated by dividing a token total by median latency. At concurrency 2 it is aggregate wave throughput, not a per-stream rate.

## Hardware, software and public recipe parameters

| Item | Recorded value |
|---|---|
| Serving hardware | 2 × NVIDIA GB10, tensor parallel 2 |
| GPU driver | 580.173.02 |
| Collector platform | Linux aarch64; Rust 1.98.0 release build |
| Collector source | [`9150fdeb9980a641bdabdf34473305d260a97483`](https://github.com/plotarmordev/thegrill/tree/9150fdeb9980a641bdabdf34473305d260a97483) |
| GLM recipe source | [`9c0794b68d7fc124f79104409ab434769503fb31`](https://github.com/MiaAI-Lab/GLM-5.3-Flash-EXL3-2x-DGX-Sparks/tree/9c0794b68d7fc124f79104409ab434769503fb31) |
| vLLM package | `0.1.dev20051+g487ecf187` |
| Target checkpoint | `Mia-AiLab/GLM-5.3-Flash-EXL3-TR3-4bpw`, revision `024db9f7e9871e8efdf21538ba55af7442be3cd5` |
| Draft checkpoint | `incoai/GLM-5.3-Flash-DFlash2`, revision `dc77ff1c99eeb2df044ee3d4f0094eb033fee410` |
| Target / draft KV | `fp8` → packed `fp8_ds_mla` / `auto` |
| Transport | Literal-loopback HTTP, streaming Chat Completions |

The image was built from the pinned recipe Dockerfile, whose base digest is `sha256:905c02933be6021301db2dc284e24e3727467aa3a0f63b41d609885778a07bce`. Runtime source files and installed image layers were checked on both serving nodes. Model revisions remain operator declarations, not a public independent weight-byte attestation.

To reproduce the **procedure**, configure the pinned [upstream recipe](https://github.com/MiaAI-Lab/GLM-5.3-Flash-EXL3-2x-DGX-Sparks/blob/9c0794b68d7fc124f79104409ab434769503fb31/README.md) for your own machines and use these public recipe controls:

```sh
MAX_MODEL_LEN=850000
GPU_MEM_UTIL=0.85
MAX_NUM_SEQS=4
MAX_NUM_BATCHED_TOKENS=7168
EXL3_FAT_KERNEL=1
EXL3_FAT_GROUPED=1
EXL3_TEMP_ROWS_FUSED=32
GLM53_INDEXER_WORKSPACE=rightsize
GLM53_SPINWAIT_MS=16
SPEC_METHOD=dflash
DFLASH_TOKENS=7
DFLASH_DRAFT_TP=2
GLM53_ADAPTIVE_K=off
GLM53_DENSE_FP8=off
ABLIT=1
ABLIT_METHOD=transplant
ABLIT_LAYERS=15-45
ABLIT_INCLUDE_MTP=1
EXTRA_ARGS=--enable-prompt-tokens-details
```

These are benchmark parameters, not a deployment-file export. Configure connectivity and authentication separately. Follow upstream's transplant artifact and licensing instructions; do not silently substitute a different ablation method. The target weights were retained at the recorded revision. Reasoning stayed at the recipe's enabled default for the benchmark; the workload did not send a thinking override. The upstream launcher completed its post-ready shape warmup before run A.

## Workload and commands

The committed [small workload](../../crates/grill-perf/examples/recipe-smoke.json) is byte-identical to the one collected:

- One synthetic integer-counting prompt; concurrency 1 and 2.
- One warmup and two measured trials per cell: 9 requests per run, of which 6 are measured.
- `portable-chat-v1`, streaming, a **cap** of 64 tokens, temperature 0, top-p 1, seed 42.
- Cache mode `observe`; 60 s total and 30 s idle request deadlines.
- 1 MiB response limit and 64 MiB wave-buffer limit.

Use a single build for the entire pair. In a checkout at the recorded collector source, place the linked workload at `workload.json`:

```sh
cargo build -p grill-perf --release --locked
mkdir -p results

target/release/grill-perf run workload.json \
  --endpoint http://127.0.0.1:8888/v1/chat/completions --local-http \
  --model GLM-5.3-Flash-EXL3 --out results/run-a --json

# No serving change, restart, or pause between this A/A pair.
target/release/grill-perf run workload.json \
  --endpoint http://127.0.0.1:8888/v1/chat/completions --local-http \
  --model GLM-5.3-Flash-EXL3 --out results/run-b --json

target/release/grill-perf compare results/run-a results/run-b --json
```

Use your own existing endpoint and `--auth-env` when required. A remote endpoint requires HTTPS or independently managed loopback forwarding. Each output directory must be new. Record your own deployment with `--deployment` if desired; do not copy private declarations into a public report.

## Evidence and limits

- Collector binary SHA-256: `a7886072d75bf648e404b3a3d8a68671f33a4ddf65a1b29af7f84b216e321ca2`.
- Workload file SHA-256: `5b144d5fae754006612b6d7ae754c84458f73119dedb650bb91448583ead219f`.
- The linked JSON includes individual measured wave latencies/rates, accounting, null answer observations, changes, and hashes of private evidence manifests. It deliberately omits deployment declarations and raw responses. It is **not** a self-contained evidence directory that `compare` can replay.
- There are only two measured trials per cell. No confidence interval, steady-state capacity, statistically reliable regression, or E3 speed-up is established.
- Cache mode did not control or flush engine, prefix, OS, JIT or GPU caches. Reported request activity was idle before collection; exclusive use, clocks, thermals and background activity were not continuously controlled.
- This short prompt does not qualify long-context prefill, maximum context capacity, model accuracy, transplant quality, or multimodal behavior. The configured 850k limit is not an 850k workload result.
- No failed benchmark attempt was retried or removed. An earlier image-build attempt failed at a registry lookup; the same pinned source/base was then built through a working route before either GLM run.
- DeepSeek is included as an API compatibility smoke only, not a timing comparison against GLM. Both tested recipes passing is narrower than universal backend support.
