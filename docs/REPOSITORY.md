# Code organization

The workspace builds the quality binary `grill`, the serving-performance companion `grill-perf`, and the dependency-free `grill-sse` library. The quality modules separate task admission, collection, evidence storage, and grading. The performance companion has its own workload admission, fixed-wave collection, evidence verification, and offline comparison. Their `run` and `resume` commands can make model requests; checking, planning, pausing, inspection, grading, regrading, and comparison work offline.

| Path | Responsibility |
|---|---|
| `src/main.rs` | CLI commands and arguments |
| `src/pack.rs`, `src/contract.rs`, `src/record.rs` | Task packs, protocol contracts, and serialized records |
| `src/identity.rs` | Deterministic identities for tasks, protocols, and grading |
| `src/study.rs` | Closed study manifests, declared exposure, and exact evidence bindings |
| `src/pilot.rs` | Deterministic ledger/reachability task generation and self-qualification |
| `src/run.rs` | Offline preparation and serial run lifecycle |
| `src/lifecycle.rs` | Cooperative pause, exclusive continuation ownership, immutable execution sessions, and historical grade-view validation |
| `src/transport.rs` | Quality HTTP requests, response semantics, and streaming collection |
| `crates/grill-sse/` | Bounded SSE byte framing with explicit caller-owned limits; no HTTP, timing, completion-marker or grading policy |
| `crates/grill-perf/` | Serving-performance workload admission, bounded fixed-wave collection, retained evidence, offline comparison, and real-CLI regressions |
| `src/store.rs` | Evidence files, receipts, and safe filesystem operations |
| `src/grade.rs`, `src/report.rs` | Answer grading, inspection, shared paired arithmetic and family/unit analysis |
| `src/grade/atlas.rs` | Explicit v4 text, numeric, exact JSON and mechanical-constraint graders |
| `tests/offline.rs` | Offline CLI and evidence-integrity regressions |
| `tests/runner.rs` | Runner integration tests with counted loopback fixtures |
| `examples/` | Synthetic task packs, submissions, study manifest and identity vectors |
| `Cargo.toml`, `Cargo.lock` | Package configuration and pinned dependencies |

The [quality protocol and measurement guide](PROJECT.md) and [performance contract](performance/CONTRACT.md) describe their file formats, compatibility rules, and interpretation of results. See [Contributing](../CONTRIBUTING.md) for development commands and review expectations.

## Shared SSE framing

Sibling workspace packages use `grill-sse = { path = "../grill-sse" }`.
The root workspace discovers packages under `crates/*`; its default members
remain the quality binary and shared framer. Use `cargo test --workspace --locked`
to include every workspace member.

The public surface is `SseLimits { line_bytes, event_bytes }`,
`Parser::new(limits) -> Result<Parser, LimitError>` and
`Parser::feed(chunk, handler) -> Result<Option<usize>, Error<E>>`.
The handler receives borrowed event data and returns `Flow::Continue` or
`Flow::Stop`. A stop reports the consumed offset in the current chunk; handlers
own response semantics, completion markers and observations.

Limits must be nonzero and at most `isize::MAX`; construction does not preallocate
their full sizes. The event cap counts the buffered LF appended for each data
field; the final LF is removed before the handler. Framing errors distinguish
line cap, event cap and invalid UTF-8. Handler errors pass through unchanged.
Discard the parser after either error. EOF never implicitly dispatches an event.
Empty feeds are no-ops, including between a split CR/LF pair; the quality
network collector already discards empty body chunks.

The quality caller retains its existing 256 KiB line/event caps and its own
`[DONE]`, JSON, collection and receipt semantics. HTTP/authentication helpers have
not been extracted; a consumer must not depend on quality-only internals.
