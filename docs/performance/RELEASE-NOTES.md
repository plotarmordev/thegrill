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
- Invalid capture loads no longer substitute the built-in C1/exact400 scope for
  an unverified selected capture. Scope and human acquisition counts are explicitly
  unavailable; timing-window errors name the acquisition and both duration values.
  Accounting is not promoted, and timing checks, report schemas and exit codes
  remain unchanged. Offline replay does not rewrite historical reports.
- Add offline `preflight` using the same local admission as `run`, including
  optional deployment, policy, metrics and credential inputs. It returns workload
  pins and bounded budgets without network traffic or capture creation. Deployment
  validation now names the offending field and observed UTF-8 byte count; the
  4,096-byte, nonempty and control-free limits remain unchanged.
- Add flat workload v3 with an optional typed warmup output budget, separate from
  measured output and conversation v2. Phase controls bind request/evidence
  identity, weighted ceilings, usage checks and failure reporting; incompatible
  phase controls cannot be compared as matched workloads. Old examples, recipe
  hashes and absent-override behavior remain unchanged. A 32/400 declaration
  does not establish sparkDash protocol or live-backend equivalence.

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

#### Qualification after preflight and phase-output integration

Source `dd771ac36963290cf8b1079b4c2a38be40cec949` completed both native
installed-runtime workflows and source/output gates in
[staging run 34740266773](https://github.com/plotarmordev/thegrill/actions/runs/34740266773).
The local workspace gate passed 221 Rust tests (two existing opt-in ignores)
and 15 publication-boundary tests, plus build, formatting and Clippy. The merged
source tree is identical; [post-merge CI passed](https://github.com/plotarmordev/thegrill/actions/runs/34740590731).

| Native target | Staged archive SHA-256 |
|---|---|
| `aarch64-unknown-linux-gnu` | `a9d73cfbe7e3b49e29c63112c0b5d98c450fca01e8695a972a1ace056d93cfe5` |
| `x86_64-unknown-linux-gnu` | `b5a724bcd851a8fc78389a04b1b1642f63e2825cc696f690f4abc90d56d06078` |

Both summaries report the same two neutral scenarios: C1 baseline ready,
unchanged control `INCONCLUSIVE`, candidate `IMPROVED`; selected baseline ready,
control and candidate `DESCRIPTIVE`. These are controlled CPU fixtures, not
live performance claims. Both runtimes had neither Rust nor a source checkout.
Checks covered inherited inputs, network-disabled replay, corrupt downloads,
asset hashes, input/pin/authentication/backend failures and budget exhaustion.
The downloaded archives and every payload digest were independently checked
against their checksum sidecars and build receipts.

These are additional exact-source staging records, not published releases or
replacements for the historical hashes above. First publication still requires
explicit maintainer approval.

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
