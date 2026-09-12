# Staging and publishing grill-perf

Release preparation is not publication. No tag or release is created by the
staging script, pull-request workflow, manual staging dispatch or installed
smoke helper. Maintainer approval is required before the first real release.
Version `0.1.0` is prepared, not a claim that a public download exists.

Staging artifacts are retained on the Actions run for reviewers with access,
subject to repository authentication and artifact retention. They are not
approved GitHub Release downloads. Use a reviewer-provided staged archive only
with its reviewed checksum and receipt; public release download instructions
apply only after an approved release exists.

## Version and evidence policy

A release version identifies a reviewed source commit and immutable named
artifacts. Before publication, the root package, `grill-perf` package, locked
workspace package versions, binary `--version`, receipt, workflow input and
`vVERSION` tag must agree. Use pre-release GitHub releases while the tool remains
experimental. Pre-1.0 patch versions describe compatible fixes; minor versions
describe deliberate compatibility changes. Document exceptions explicitly
before approval, not after users have pinned an artifact.

Keep these identities separate:

- Release version and full source commit/tree identify the implementation source.
- The archive and binary SHA-256 identify actual installed bytes.
- Raw workload hashes, normalized workload identities, selection/bundle versions,
  evidence schemas and measurement-contract revisions identify their own contracts.

A newer release does not revise old receipt meaning. Incompatible comparisons
must retain the existing rejection; do not migrate evidence in place. Upgrade
or roll back by installing an explicit version into a separate directory and
selecting its absolute binary path. Retain the old binary, workload bytes and
receipts for historical evidence. Do not pin reproducible runs to `latest`.

## Native staging from a reviewed commit

Use a clean native Ubuntu 24.04 host with glibc 2.39, Python supporting `tomllib`
and safe tar extraction, Git, Rust/Cargo 1.98.0, a C/C++ toolchain and CMake.
Install the exact Rust toolchain before staging. Native x86 uses
`x86_64-unknown-linux-gnu`; native ARM uses `aarch64-unknown-linux-gnu`.
No cross-build or emulator run qualifies a native platform. The script checks
host OS/libc/architecture and Rust host identity, but these checks are not an
independent attestation of the machine.

From the reviewed checkout, set `SOURCE` to its full immutable commit SHA,
`TARGET` to the native triple and `OUT` to a new directory outside the checkout:

```sh
python3 tools/stage-release.py --source "$SOURCE" --target "$TARGET" --out "$OUT"
```

The source must be exact clean HEAD with no untracked files, hidden index
changes, symlinks or submodules. The build uses a temporary committed-source
snapshot and a fresh target directory; ignored build leftovers are never inputs.
It refuses Cargo configuration and compiler/build override environment variables,
including `RUSTFLAGS`, target/linker/profile overrides and `target-cpu=native`.
Home/toolchain locations and terminal color are not build flags; incremental
compilation is fixed off. No whole environment or private path dump is recorded.
A trusted build host and normal dependency-download TLS remain prerequisites.

The build command is fixed:

```sh
cargo +1.98.0 build --locked --release --target "$TARGET" -p grill-perf --bin grill-perf
```

The stager executes only the freshly built binary's `--version`; this is not
the installed-runtime smoke. Errors do not overwrite an existing output. A late
packaging failure can leave an incomplete owned directory: inspect it and use
a new output path, rather than treating partial sidecars as a finished release.

## Archive and receipt contract

The archive is `grill-perf-VERSION-TARGET.tar.gz` with exactly one root named
`grill-perf-VERSION-TARGET/`. Its payload contains:

```text
bin/grill-perf
workloads/<explicit supported JSON leaf files>
LICENSE
NOTICE
licenses/sparkDash-LICENSE
licenses/NOTICE-INPUTS.json
licenses/THIRD-PARTY-NOTICES.txt
licenses/rust-standard-library/<pinned distribution notices>
INSTALL.md
```

`tools/stage-release.py` is the explicit file allowlist. Every supported
performance JSON example is individually listed; no directory glob collects
future files automatically. Selections, bundle manifest and their referenced
workloads remain adjacent in `workloads/`, with unchanged bytes and pins.
`INSTALL.md` is copied verbatim from `docs/performance/INSTALL.md`. The package
excludes the quality binary, source checkout, model weights, credentials,
private declarations, calibration outputs and captured benchmark evidence.

The archive has `.tar.gz.sha256` and `.receipt.json` sidecars. The receipt schema
is `grill-perf-build-receipt-v1`; its stable keys include `release_version`,
`source_commit`, `source_tree`, `target`, `cargo_lock_sha256`, `archive.sha256`,
`binary.sha256` and `files_sha256`. File-hash keys are paths relative to the
archive root. Payload entries also record source paths, byte lengths and modes.
Compiler/Cargo versions and executable hashes, native tool versions, OS/libc
assumptions and the build command are recorded. The checksum sidecar is hashed
in the receipt. The receipt is outside the archive and has no circular self-hash.

Archive entry order, ownership, modes and timestamps are normalized; gzip omits
the local filename. Compiler, linker and dependency details can still change
binary bytes. Repeatable installation of a pinned artifact is not a proven
bit-for-bit reproducible build, a signature or execution attestation.

## Verification and public-safe output

The `Stage release` workflow uses only `contents: read` on pull requests, manual
dispatch and calls from the publication workflow. Native hosted `ubuntu-24.04`
and `ubuntu-24.04-arm` runners execute these existing checks before staging:

```sh
cargo +1.98.0 build --workspace --locked
cargo +1.98.0 fmt --all --check
cargo +1.98.0 test --workspace --locked -- --test-threads=1
cargo +1.98.0 clippy --workspace --locked --all-targets -- -D warnings
```

Then stage the exact clean source with the command above. Set `VERSION` to the
checked-out `grill-perf` package version, not a moving tag or a hardcoded default.
The workflow checks any requested release version against that package version.
Invoke the installed-artifact helper with explicit archive and sidecars:

```sh
python3 tools/smoke-installed.py \
  --archive "$OUT/grill-perf-$VERSION-$TARGET.tar.gz" \
  --checksum "$OUT/grill-perf-$VERSION-$TARGET.tar.gz.sha256" \
  --receipt "$OUT/grill-perf-$VERSION-$TARGET.receipt.json" \
  --target "$TARGET" --version "$VERSION" --out "$NEW_PRIVATE_DIR"
```

The helper verifies the checksum before unpacking or executing the archive.
It uses the native Ubuntu 24.04/glibc 2.39 runtime image
`ubuntu@sha256:224a1869083a311ef3f13648a154ba79832fbef6364d31493642ca03082da254`
without Rust or a source checkout. A native system CA bundle is mounted read-only
at the standard trust-store location; its digest and source are recorded in the
smoke summary. The HTTP client requires that OS prerequisite even for loopback
initialization. Docker must be available on the staging host. Fixture collection
uses host networking; help/inspection and offline replay use network isolation.
All commands run outside the extraction directory. Negative cases cover corrupt archives,
wrong architecture/runtime assumptions and invalid evidence. No serving, model,
GPU or billing operation is part of this workflow.

Upload only the archive, checksum, build receipt and public-safe smoke summary.
The workflow renames `summary.json` to `grill-perf-VERSION-TARGET.smoke.json`;
raw fixture evidence remains private and is neither packaged nor uploaded.
A successful summary must match source, version, target, archive and binary
hashes. A failed or unavailable native run blocks advertising that target.
Action references are immutable commit SHAs inspected against their upstream
repositories; future action updates require the same review.

Before building, a committed-source snapshot is checked against the approved
repository roots and scanned with Gitleaks. It retains package source, manifests,
lockfile, workload inputs and notices; `.git`, `target`, symlinks, submodules and
unexpected roots are rejected rather than scanned as build noise. Untracked and
ignored local files never enter that snapshot; the stager separately requires
the exact clean reviewed checkout.

The workflow downloads ordinary native Gitleaks `8.30.1` binaries over HTTPS,
mapping x86_64 to upstream `linux_x64` and aarch64 to `linux_arm64`. The archive
SHA-256 pins were checked against both the upstream release asset metadata and
its checksum manifest:

```text
551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb  gitleaks_8.30.1_linux_x64.tar.gz
e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080  gitleaks_8.30.1_linux_arm64.tar.gz
```

The digest is verified before extracting or executing the scanner. Its explicit
configuration extends the built-in rules, with no repository allowlist, ignore
file, baseline or inline `gitleaks:allow` suppression. Findings are redacted.

Before upload, a separate output scope gate requires exactly the named archive,
checksum, build receipt and smoke summary. Archive members must exactly match
the stager's payload allowlist, with no links, duplicate entries or extras.
Extracted hashes must match the receipt and scanned source assets; the summary
must bind the same source, version, target, archive and binary. Unexpected
receipt/summary fields and unresolved publication blockers are rejected.
Gitleaks then scans the extracted payload and only those intended public
sidecars. Generated raw benchmark evidence is outside both the upload allowlist
and this public-output scan. A scope or secret failure prevents upload and
therefore blocks the dependent publication job. This is not proof that a scanner
can recognize every secret; manual review of publishable inputs remains required.

See [INSTALL.md](performance/INSTALL.md) for checksum-before-execution installation,
wrong-architecture errors, libc mismatch and a source-build fallback. Never
turn off TLS verification to work around a download failure. A corrupt download
must be discarded before execution; obtain the checksum from a trusted reviewed
channel. Checksums alone do not authenticate a compromised distribution channel.

## Publication gate and immutable failure handling

Before requesting approval, review the source diff, public inputs, payload
allowlist, licenses, receipt and both native summaries for scope and secrets.
Update [release notes](performance/RELEASE-NOTES.md) with the exact reviewed
source and verification actually performed. Hosted fixture success does not
qualify a live backend or authorize collecting model evidence.

The complete redistribution ledger and notice payload are fixed reviewed inputs,
not an unresolved placeholder or an inference from a root MIT label.
`licenses/THIRD-PARTY-NOTICES.txt` preserves the locked Cargo/native license
texts, including AWS-LC, deduplicating only byte-identical text with all source
attributions retained. The complete pinned Rust standard-library distribution
notices are included conservatively. `licenses/NOTICE-INPUTS.json` binds the
lockfile, dependency/feature manifests, Rust source/toolchain and every required
notice payload digest. Staging checks the exact required notice set and refuses
stale, missing or changed inputs; dependency or toolchain updates require renewed
review. This is source-backed redistribution evidence, not a legal attestation.

After all blockers are resolved, a repository administrator or maintainer with
the `admin` or `maintain` role explicitly dispatches `Publish reviewed release`:

- Select a workflow ref whose exact commit equals the reviewed source SHA.
- Enter that full SHA and the matching package version without the `v` prefix.
- Enter the literal confirmation `publish vVERSION from FULL_SHA` with the actual
  reviewed values. This dispatch is the publication approval, not a dry run.

The workflow verifies both the original and rerun actors' repository roles via
GitHub's API and fails closed if that permission cannot be established. It does
not rely on an auto-created GitHub environment or change repository settings.
The workflow code SHA must itself match the reviewed source. Read-only native
staging and installed checks run again; only the final publication job has
`contents: write`. Do not dispatch publication merely to inspect staging.
Permission approval applies only to publication: a fork pull request does not
need an `admin` or `maintain` actor to run read-only staging. A read-only check of
the collaborator-permission endpoint is not publication approval.

The publication job independently verifies the staged sidecars and summary
identities, refuses unresolved receipt blockers, checks that neither tag nor
release exists, creates a new exact-source tag, creates a draft prerelease,
uploads only the named public-safe artifacts and publishes after the complete
asset set is present. No automatic main-push release, mutable latest pin,
`--clobber`, tag move or asset replacement exists.

A failure after tag creation can leave an immutable tag, draft release or partial
asset set. Stop and record the workflow run and visible state; retries intentionally
refuse existing state. A maintainer must investigate and explicitly decide any
recovery outside this workflow. Do not silently delete/recreate the tag, overwrite
assets or claim a partial draft is released. If a new version is required, review
and prepare that source/version anew. Cancellation and network failures can leave
server-side writes complete even when the client reports failure.
