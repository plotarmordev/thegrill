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
A readable system CA trust store (Ubuntu `ca-certificates`) is required for client
initialization, including loopback HTTP. The clean-runtime smoke records the
read-only native CA bundle digest; no CA contents are packaged and TLS checks
are never disabled.

Existing evidence compatibility checks remain authoritative. A newer binary
must not reinterpret or rewrite old receipts in place. Retain the exact binary,
workload bytes and source pin needed to inspect historical evidence. Selected
concurrency, conversation and historical recipe-bundle workflows remain
separate contracts; packaging does not make them mutually comparable.

### Executed staged qualification

Preparation inspected package versions, asset paths, CI definitions, pinned
upstream license/action metadata and the approved Gitleaks release checksums.
The release workflow now gates the committed source and extracted public
payload/sidecars for scope and secrets before artifact upload. The complete
redistribution ledger and required notice payload are present; staging verifies
their exact input set and hashes rather than carrying an unresolved notice
placeholder.

Both targets completed native build, existing workspace checks, clean-runtime
installed CLI smoke, and source/output scope and secret gates at source
`ef4f9e29bb081f997e795b3dded80c6965f729da` in
[staging run 34677697749](https://github.com/plotarmordev/thegrill/actions/runs/34677697749).
The independent push and pull-request CI runs at that source also passed.
The complete local workspace gate passed 206 tests (two opt-in ignores); the
15 CPU publication-boundary tests passed without making real publication writes.

| Native target | Staged archive SHA-256 |
|---|---|
| `aarch64-unknown-linux-gnu` | `fd4beba564a940dd18a5a4005514cf47937925fdc96503f6ac265193f90b93d1` |
| `x86_64-unknown-linux-gnu` | `94078e602a3528e94be0aa406f10d02d62846bb9554c03206acc3674d1953d90` |

Each public build receipt and smoke summary binds that exact source, version,
target, archive, executable and runtime/CA-store identity. Both fixtures used the
same installed binary without Rust or a source checkout: default C1 plus an
explicitly selected/authenticated control mapping; baseline, unchanged control,
candidate and network-disabled offline replay; representative input, pin,
authentication, backend-rejection and budget failures. Raw receipts stay private.
The C1 control was inconclusive on ARM and measured slower on x86; both results
are retained, not rerun until favorable or misrepresented as a server-change
effect. Selected comparisons completed with `DESCRIPTIVE`, not a performance PASS.

These hashes identify the named staged artifacts, not every later build of
version 0.1.0. A later source revision requires new exact-source staging and
receipts before approval; these historical qualification records are not rewritten.

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
