# The Grill

A Rust CLI for running language-model evaluations and comparing saved results.

Define tasks in JSON, collect answers from a Chat Completions endpoint, and inspect or regrade the results without calling the model again. The Grill keeps the inputs, declared settings, responses, and grading evidence together so comparisons can be checked rather than taken on trust.

**Status: early-stage, actively developed software.** The quality runner collects and checks evidence; its included synthetic tasks are examples, not a calibrated intelligence benchmark. The performance companion reports bounded, client-observed measurements—not server capacity. Local CLI and fixture checks do not establish compatibility with every deployment.

## What it does

- **Check and plan:** validate task packs and preview requests before making model calls.
- **Run:** collect one attempt per case, with streaming support, deadlines, and response limits.
- **Pause and resume:** drain an active request before pausing, then continue only never-started cases from validated evidence.
- **Inspect and regrade:** distinguish incorrect answers, formatting problems, output-limit stops, timeouts and service errors; inspect and regrade saved evidence offline.
- **Compare:** check compatible results while keeping incorrect, malformed, refused, and missing answers distinguishable.
- **Study:** bind a declared comparison to exact inputs and inspect paired outcomes by task family and problem unit.

No project account or hosted service is required. You choose the model endpoint and provide any credentials it needs.

## Quick start

Requirements: Linux, Rust/Cargo 1.98, and a C/C++ build toolchain with CMake.

```sh
git clone https://github.com/plotarmordev/thegrill.git
cd thegrill
cargo build --locked

target/debug/grill check examples/synthetic-pack.json --json
mkdir -p results
target/debug/grill grade examples/synthetic-pack.json examples/submission-a.json --out results/example-a
target/debug/grill grade examples/synthetic-pack.json examples/submission-b.json --out results/example-b
target/debug/grill compare results/example-a results/example-b --json
```

This example needs no model or API key. Submission A has one correct answer, two failures, and one unknown; B has three correct answers and one failure. The comparison retains all four cases.

New run and grade output directories must not already exist. Choose different names when repeating the example; `resume` continues an existing run instead of creating a replacement.

## Evaluate a model

Set `MODEL_API_KEY` in your environment if your endpoint requires authentication, then replace the endpoint and model below:

```sh
target/debug/grill run examples/synthetic-pack.json \
  --endpoint https://api.example.com/v1/chat/completions \
  --model your-model \
  --auth-env MODEL_API_KEY \
  --token-cap 128 \
  --stream \
  --out results/model-run

target/debug/grill inspect results/model-run --json
target/debug/grill regrade results/model-run --out results/model-run-regraded
```

Use `plan` instead of `run` with the same arguments to preview the request sizes and declared settings without making a model call or creating output. Omit `--auth-env` for an endpoint that does not require authentication. Run `target/debug/grill run --help` for the available controls.

To pause an active quality run, use another terminal:

```sh
target/debug/grill pause results/model-run
target/debug/grill inspect results/model-run --json
```

A pause request is not yet a completed pause. Wait for the collector to finish its active request and exit before continuing later with `target/debug/grill resume results/model-run`. Resume checks the frozen settings and evidence, refuses concurrent ownership or unsettled crash state, and does not repeat completed attempts. Earlier grade views remain historical; create a fresh `regrade` output after continuation. Performance pause/resume uses whole-wave boundaries and has separate comparison limits, documented in its [usage guide](docs/performance/README.md).

Results include prompts and model responses. Review them before sharing.

Inspection explains unfinished answers without changing their grades. For complete responses, new receipts keep the reported stop reason and usage even when no final answer was delivered. Where older runs retained the complete raw response, inspection can recover those facts from verified bytes without rewriting the run. Missing usage remains unknown—not zero—and reported usage is not a verified bill.

## Offline studies and pilot packs

After running the quick-start example, analyze its saved views with the supplied study manifest:

```sh
target/debug/grill study check examples/synthetic-study.json examples/synthetic-pack.json
target/debug/grill study compare examples/synthetic-study.json examples/synthetic-pack.json \
  results/example-a results/example-b --json
```

The manifest declares the ordered system pair, protocol, task-family provenance, sampling and per-case exposure. Analysis verifies those bindings against saved evidence, retains unknowns, and reports gains/losses by family and problem unit. Declarations do not authenticate provenance, freshness or equal effective compute. Reports are descriptive, not confidence intervals or causal verdicts.

Generate a deterministic diagnostic pack without a model:

```sh
target/debug/grill pilot --seed 42 --units 8 > results/pilot-42.json
target/debug/grill check results/pilot-42.json --json
```

This produces 32 cases: eight ledger instances and eight directed-graph instances, each with two presentation variants. **Pack files include answer keys and grading fixtures.** They are not validated capability benchmarks or automatically fresh holdouts. See [study manifests and pilot design](docs/PROJECT.md#study-manifests-and-pilot-design) before using them in a study.

## Scope

The quality runner, `grill`, supports text-only, direct-answer tasks on Linux. It does not execute generated code or operate an agent. The included synthetic examples demonstrate the format; they are not a validated capability benchmark. Saved receipts support inspection and regrading, but do not independently prove which model produced an answer.

The separate `grill-perf` companion measures bounded request waves against an already-running Chat Completions server and compares saved evidence offline. It does not grade answers or manage servers. See [performance setup and usage](docs/performance/README.md) for its build, installation, and measurement limits.

## Documentation

- [Task formats, protocols, and measurement design](docs/PROJECT.md)
- [Serving-performance setup and usage](docs/performance/README.md)
- [Code organization](docs/REPOSITORY.md)
- [Contributing](CONTRIBUTING.md)
- [Security](SECURITY.md)

## License

TheGrill's code and documentation are [MIT licensed](LICENSE). Third-party benchmark data and model weights retain their own licenses and access restrictions; this license does not grant rights to those materials.
