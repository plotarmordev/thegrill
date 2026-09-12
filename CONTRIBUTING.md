# Contributing

Keep changes focused on observable behavior. For a substantial change, open an issue describing the problem, proposed approach, and affected file formats or protocols before implementation.

## Development

For installed performance use, begin with [Install and first capture](docs/performance/INSTALL.md).
There are no published releases yet; reviewed staged archives and the documented
source fallback work before publication. The quality CLI remains a separate
source-built work in progress.

The Grill is a Rust CLI for Linux. Build and check it with:

```sh
cargo build --workspace --locked
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --locked --all-targets -- -D warnings
```

Integration tests run the CLI against synthetic inputs and loopback fixtures. Do not point tests at a model service. Use `cargo test --locked --test offline` or `cargo test --locked --test runner` for a focused run.

GitHub CI runs these four checks on Ubuntu 24.04 with Rust 1.98.0 for pushes and pull requests. It uses synthetic and loopback fixtures only; hosted CI does not qualify a live model deployment.

Performance CLI fixtures live in `crates/grill-perf/tests/`; a focused capture
run is `cargo test -p grill-perf --locked --test study`. For recipe-facing
changes, use the [same baseline/check interface and reviewed PR summary](docs/performance/RECIPES.md#contributor-pr-report).
Recipe facts are explicit data, not a reason to add a provider wrapper.

## Changes and review

- Use a feature branch and submit a pull request. Describe the behavior changed, assumptions, and checks performed.
- Keep compatibility changes explicit. Preserve existing task, protocol, and grading identities unless the contract deliberately changes.
- Preserve the fixed examples and `examples/identity-vectors.json`. Do not update expected hashes merely to make a failing check pass.
- Bump the grader implementation revision when grading behavior changes.
- Test plausible failures, boundaries, and invariants rather than implementation details or wording.
- Include valid alternate answers as well as incorrect and malformed answers when changing a grader.
- Identify external code or data incorporated into a change and preserve any required notices.
- Review staged files for credentials, sensitive inputs, and generated results before publishing.

## Evaluation claims

Distinguish task self-checks from independent validation, and submitted outputs from controlled model execution. Preserve failed and incomplete attempts in reports. State the task set, protocol, sampling assumptions, and uncertainty behind a comparison; a small pilot is not evidence of general capability.
