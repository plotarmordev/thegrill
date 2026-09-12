# Performance release notes

## Prepared version 0.1.0 — not published

This version matches the root and `grill-perf` packages. Preparation does not
create a tag or a downloadable release. The exact reviewed source is recorded
in each staged build receipt; a future approved tag must identify that commit.

### User-visible scope

- Stage a standalone `grill-perf` binary with the existing workloads, selection
  manifests, recipe bundle, installation guide and license notices.
- Use the existing CLI outside a source checkout. `--version`, `--help`, offline
  selection/bundle verification and baseline/control/offline comparison use the
  same implementation as a source build.
- Keep stored evidence and workload identities unchanged. The release number is
  not a workload/schema version or a measurement-contract revision.
- The quality CLI remains work in progress and is not packaged.

### Target and compatibility boundary

Prepared targets are `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu`, built natively on Ubuntu 24.04 with glibc 2.39
and Rust 1.98.0. The supported installed-runtime baseline is native Ubuntu
24.04 with glibc 2.39; older libc, musl, other operating systems and emulation
are not qualified by this release mechanism. Other Linux distributions need
separate validation or a source build.

Existing evidence compatibility checks remain authoritative. A newer binary
must not reinterpret or rewrite old receipts in place. Retain the exact binary,
workload bytes and source pin needed to inspect historical evidence. Selected
concurrency, conversation and historical recipe-bundle workflows remain
separate contracts; packaging does not make them mutually comparable.

### Verification actually performed versus pending

Preparation inspected package versions, asset paths, CI definitions, pinned
upstream license/action metadata and the approved Gitleaks release checksums.
The release workflow now gates the committed source and extracted public
payload/sidecars for scope and secrets before artifact upload. The complete
redistribution ledger and required notice payload are present; staging verifies
their exact input set and hashes rather than carrying an unresolved notice
placeholder.

Native archive qualification remains pending for both targets. Before approval,
retain successful exact-source native staging runs and the public-safe
`grill-perf-VERSION-TARGET.smoke.json` summaries produced from `summary.json`.
Each summary must bind the reviewed source, version, target, archive and binary
digests and record the exact pinned runtime image, native architecture and libc.
It must demonstrate the CLI fixture flows and negative cases without Rust or a
source checkout. Commands and image identity are in the release procedure.
No native/archive success is claimed by these notes before that evidence exists.
Update this section with the exact reviewed source and actual evidence before
requesting publication; raw fixture receipts are not public release assets.
Unavailable native execution is a release blocker, not a verified-target claim.

### Remaining limits

Prepared Actions artifacts require access to their retained workflow run; they
are not approved release downloads. No arbitrary public archive availability is
promised before explicit maintainer-approved publication.

Installation and loopback fixtures are not live-backend performance qualification,
model/template qualification, GPU measurements or causal evidence. No serving,
model or GPU calls are part of release preparation. No bit-for-bit reproducible
build claim, signature or independent execution attestation is made. Checksums
bind downloaded bytes; obtain the expected digest from the reviewed release,
not an untrusted mirror alone.

See [release procedure](../RELEASES.md) and [installation](INSTALL.md).
