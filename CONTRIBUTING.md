# Contributing

Keep changes focused on observable behavior. For a substantial change, open an issue describing the problem, proposed approach, and affected file formats or protocols before implementation.

## Development

For installed performance use, begin with [Install and first capture](docs/performance/INSTALL.md).
For upcoming releases, reviewed staged archives and the documented source fallback
work before publication. The quality CLI remains a separate
source-built work in progress.

The Grill is a Rust CLI for Linux. Build and check the workspace with:

```sh
cargo build --workspace --locked
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --locked --all-targets -- -D warnings
```

Apple Silicon macOS supports the `grill-perf` portable serving slice:

```sh
cargo build -p grill-perf --locked
cargo test -p grill-perf --locked -- --test-threads=1
cargo clippy -p grill-perf --locked --all-targets -- -D warnings
```

Native `/proc`/cgroup/NVML resource capture and external-program microbench
capture remain Linux-only. macOS changes must not weaken those Linux contracts.

Integration tests run the CLI against synthetic inputs and loopback fixtures. Do not point tests at a model service. Use `cargo test --locked --test offline` or `cargo test --locked --test runner` for a focused run.

GitHub CI runs the workspace checks on Ubuntu 24.04 and the `grill-perf` macOS checks on `macos-15`, both with Rust 1.98.0, for pushes and pull requests. It uses synthetic and loopback fixtures only; hosted CI does not qualify a live model deployment.

Performance CLI fixtures live in `crates/grill-perf/tests/`; a focused capture
run is `cargo test -p grill-perf --locked --test study`. For recipe-facing
changes, use the [same baseline/check interface and reviewed PR summary](docs/performance/RECIPES.md#contributor-pr-report).
Recipe facts are explicit data, not a reason to add a provider wrapper.

The System One comparison fixture derives its expected reference and candidate
envelopes from retained measured waves. Its reference delays follow the preceding
captures' observed makespans so reference pooling is checked without a fixed
timing margin.

## Changes and review

- Use a feature branch and submit a pull request. Describe the behavior changed, assumptions, and checks performed.
- Keep compatibility changes explicit. Preserve existing task, protocol, and grading identities unless the contract deliberately changes.
- Preserve the fixed examples and `examples/identity-vectors.json`. Do not update expected hashes merely to make a failing check pass.
- Bump the grader implementation revision when grading behavior changes.
- Test plausible failures, boundaries, and invariants rather than implementation details or wording.
- Include valid alternate answers as well as incorrect and malformed answers when changing a grader.
- Identify external code or data incorporated into a change and preserve any required notices.
- Review staged files for credentials, sensitive inputs, and generated results before publishing.

## Code standards

Each change is the smallest complete one that solves its stated problem.

- One purpose per pull request. Do not refactor, rename or reformat code the change does not need.
- Search before adding a helper. Keep one implementation per concept; a second copy is a review defect. Deliberately versioned contract modules (for example `startup_kv_v2`) are the exception.
- Add a trait, generic, module, flag or configuration field only when two real callers need it now.
- Delete what a change replaces in the same pull request: no dead code, commented-out code, compatibility shims or TODOs.
- Comments explain why or state an invariant, never what the next line does. Every `unsafe` block states its safety invariant in a `// SAFETY:` comment.
- Keep functions under 150 lines. Existing longer functions carry `#[expect(clippy::too_many_lines)]`; remove it when shortening one and never add one.
- Human-readable output prints values, not Rust `Debug` of options or strings; absent values print `null`, as in the JSON (`model::OrNull`).
- Do no avoidable work, allocation or copying on capture and timing paths. A speed claim needs before/after measurements under stated conditions.
- Documentation states each fact once and links to it rather than repeating it.

CI enforces the mechanical part: clippy denies `dbg!`, `todo!`, `unimplemented!` and undocumented `unsafe` blocks and rejects new functions over 150 lines; `ruff` rejects unused or undefined names in `tools/`, excluding files whose bytes are pinned identities. The rest is review.

## Evaluation claims

Distinguish task self-checks from independent validation, and submitted outputs from controlled model execution. Preserve failed and incomplete attempts in reports. State the task set, protocol, sampling assumptions, and uncertainty behind a comparison; a small pilot is not evidence of general capability.
