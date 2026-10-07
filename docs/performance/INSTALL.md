# Install and first capture

`grill-perf` measures a server you already run; it never starts it, changes its
settings, downloads weights or uploads evidence. The quality CLI `grill` remains
WIP and is not in these archives.

## Expanded-coverage release

The download commands below select the published **v0.5.0** experimental release,
which adds sparkDash 1.8.7 decode prompts and a varied-text prefill workload on top
of v0.4.0's cross-platform deployment comparison and portable workloads. Use its
exact version/target, checksum and build receipt; never substitute a moving branch
or local staging hashes. Retain earlier published releases and their original pins
for historical studies. Installing v0.5.0 does not reinterpret old receipts or
qualify new backend claims.

Since 0.2.0, packages also carry the claim map and report template under
`docs/performance/`, public domain examples under `workloads/`, and the explicit
startup, retention and microbench Python modules under `tools/`. Keep all of
those bytes immutable and pin the actual executable and selected producer before
collection. The native CLI still needs no Rust/TheGrill checkout. Python producers
require a separate compatible Python/backend environment; CPU protocol checks
use Python 3.12, and real runtime/source compatibility must be reviewed separately.
No model weights, backend runtime or driver are installed by the archive.

Installing files is not permission to initialize devices, submit model traffic,
install hooks or restart a service. Source/CPU verification, native installed
verification and real-backend qualification remain separate reported statuses.

## Obtain, verify, unpack

The published prerelease is [**v0.5.0**](https://github.com/plotarmordev/thegrill/releases/tag/v0.5.0),
from reviewed source `b00e6cff01650f54e1b76f56bb886af34d0cf640`.
Use the checksum sidecar and build receipt attached to that release for your
native target. Never use a moving `latest` pin.
The archive's bundled guide is the reviewed pre-publication snapshot; this page
records the published download location without changing those released bytes.

Choose `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu` to match `uname -m`.
The supported baseline is native Ubuntu 24.04 / glibc 2.39; older libc and other
runtimes are not qualified by that statement. On Apple Silicon macOS, choose
`aarch64-apple-darwin` (published from v0.6.0): the binary pins
`MACOSX_DEPLOYMENT_TARGET=11.0` (the Rust toolchain's own floor for this target),
while the installed smoke ran on the hosted macOS 15 staging runner only. For
earlier versions on a Mac, use the [source fallback](#upgrade-rollback-and-source-fallback).
Installed use needs no Rust or checkout. On Linux a readable system CA store
(Ubuntu `ca-certificates`) is required for client initialization, including
loopback HTTP; do not disable TLS checks.
On macOS the client reads the native system trust store (Security.framework);
no file bundle is configured. The shell examples use `curl`, `sha256sum`, `tar`, and `jq`
(`shasum -a 256 --check` reads the same sidecar on macOS).
An `Exec format error` means the archive/CPU choice is wrong: obtain the native
target rather than treating emulation as qualification. A missing `GLIBC_*`
version means the runtime is below the supported baseline: use a qualified OS
or the source-build fallback on the intended host, not copied libc files or a
TLS-verification workaround. The unsigned macOS binary triggers Gatekeeper only
when the download carries a quarantine attribute; see the
[release procedure](../RELEASES.md#verification-and-public-safe-output) for the
`xattr -d com.apple.quarantine` note.

For this release, set the exact version and trusted HTTPS artifact directory,
then set `TARGET` to your native triple. Use a fresh download directory:

```sh
VERSION=0.5.0
ARTIFACT_BASE_URL=https://github.com/plotarmordev/thegrill/releases/download/v0.5.0
```

```sh
: "${VERSION:?exact published or reviewed version}"
: "${TARGET:?native Linux target triple}"
: "${ARTIFACT_BASE_URL:?actual trusted HTTPS artifact directory}"
STEM="grill-perf-${VERSION}-${TARGET}"
curl --fail --location --proto '=https' --proto-redir '=https' \
  -o "$STEM.tar.gz" "${ARTIFACT_BASE_URL%/}/$STEM.tar.gz" &&
curl --fail --location --proto '=https' --proto-redir '=https' \
  -o "$STEM.tar.gz.sha256" "${ARTIFACT_BASE_URL%/}/$STEM.tar.gz.sha256" &&
curl --fail --location --proto '=https' --proto-redir '=https' \
  -o "$STEM.receipt.json" "${ARTIFACT_BASE_URL%/}/$STEM.receipt.json"
```

For files supplied locally, set `VERSION`, `TARGET` and `STEM` the same way and
skip downloading. **Verify before unpacking or executing**, and stop on failure:

```sh
test ! -e "$STEM" &&
sha256sum --check --strict "$STEM.tar.gz.sha256" &&
tar --extract --gzip --file "$STEM.tar.gz" &&
export GRILL_HOME="$PWD/$STEM" &&
export GRILL_PERF="$GRILL_HOME/bin/grill-perf" &&
"$GRILL_PERF" --version
```

The directory contains `bin/grill-perf`, adjacent pinned JSON assets in
`workloads/`, `LICENSE`, `NOTICE`, dependency notices under `licenses/`, and this
`INSTALL.md`. Keep it intact. Retain the receipt: source revision, binary hash,
archive hash and workload/schema identities are different things. A checksum
checks bytes, not whether a compromised publisher is trustworthy.

## Supply the unavoidable facts once

A recipe may already know these inputs; reuse them rather than re-entering or
guessing them:

- Full Chat Completions resource URL and explicit server model selector.
- A request-control profile the backend actually supports. The default C1 workload
  requires vLLM-compatible exact-output controls and `chat_template_kwargs` with
  both `thinking: false` and `enable_thinking: false`.
- The credential environment-variable **name**, if authentication is required.
  Add `--auth-env NAME` to baseline; supply its value independently. No environment
  file is discovered and the value is not stored by the collector.
- Actual weights revision, runtime build, hardware/layout identity and settings
  fingerprint. These are operator declarations, not server attestation.

Use HTTPS. A separately managed literal-loopback HTTP endpoint additionally needs
`--local-http` on baseline; `localhost` is not a literal address. Declare where
the collector runs with `--client-placement`: `same-host` on the serving host,
`network` when it reaches the server from another host.

In a private working directory, create the output parent and existing declaration
format from actual inputs. A changed fingerprint does not prove one internal knob changed.

```sh
umask 077
mkdir -p results
jq -n \
  --arg model_revision "${MODEL_REVISION:?actual weights revision}" \
  --arg runtime "${RUNTIME_REVISION:?actual server build revision}" \
  --arg hardware "${HARDWARE_ID:?actual device/layout identity}" \
  --arg settings "${SETTINGS_FINGERPRINT:?actual serving configuration fingerprint}" \
  '{model_revision:$model_revision,runtime:$runtime,hardware:$hardware,settings:$settings}' \
  > serving-before.json
```

### Remote loopback servers through SSH

If an authorized remote server only listens on loopback, a separately managed SSH
forward can keep that listener private. In one terminal on the collector host,
keep the forward in the foreground (replace the SSH destination and remote port):

```sh
ssh -N -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:18081:127.0.0.1:8000 user@model-host
```

In the capture terminal, use
`--endpoint http://127.0.0.1:18081/v1/chat/completions --local-http`
and **`--client-placement network`**, not `same-host`. The loopback URL names the
local end of a network tunnel, not the serving host. Latency and throughput still
include the network/SSH path and both hosts' scheduling effects; do not treat
them as same-host observations. Supply the serving deployment declaration and,
if required, `--auth-env NAME` as described above; SSH authentication does not
replace the model server's authentication.

Keep the tunnel endpoint and its remote destination unchanged for baseline and
`check --change none`. Check inherits the saved endpoint, so reopening a forward
later must use the same local port and the same verified remote deployment. If
that port is occupied, fail rather than attach the capture to an unknown listener
or edit saved evidence. A later control also includes elapsed-time drift; an
unchanged declaration is not proof that serving state stayed unchanged. Stop the
owned foreground SSH process with Ctrl-C after capture. The Grill does not create
or manage the tunnel.

<a id="baseline-optional-control-change-check"></a>

## Baseline → optional control → change → check

No statistical policy file or manually calculated threshold is needed. Default
C1 allows **32 requests / 12,800 output tokens / 300 seconds**: eight acquisitions,
each with one warmup and three measured requests of exactly 400 reported tokens.
Preflight shows scope, controls and the full allowance on stderr before traffic;
it is not a prompt or proof that the backend supports those controls.

```sh
"$GRILL_PERF" baseline --endpoint "${ENDPOINT:?full resource URL}" \
  --model "${MODEL:?explicit model selector}" --client-placement same-host \
  --deployment serving-before.json --out results/before
```

Proceed only when baseline is ready. Optionally keep serving state unchanged:

```sh
"$GRILL_PERF" check results/before --deployment serving-before.json \
  --change none --out results/control
"$GRILL_PERF" compare results/before results/control --json
```

Retain shifted and incomplete controls too; an unchanged declaration does not
prove equality or repeatability. **Make a serving change separately.** For a
settings-only change, confirm the other facts still hold and reuse them:

```sh
jq --arg settings "${CANDIDATE_SETTINGS_FINGERPRINT:?actual changed fingerprint}" \
  '.settings = $settings' serving-before.json > serving-after.json
"$GRILL_PERF" check results/before --deployment serving-after.json \
  --change settings --out results/after
"$GRILL_PERF" compare results/before results/after --json
```

Use the corresponding `--change model_revision`, `runtime` or `hardware` for that
single declared change. `check` inherits endpoint, model selector, loopback
permission, workload/selection and credential-variable name from baseline.
Keep the same executable. A custom `--seconds` is **not inherited**: each capture
has its own prospectively approved allowance (`1..3600`). Slower servers may need
more time, but the tool never probes speed, expands budgets or retries until PASS.
A budget stop is not automatically a broken server; retain the partial evidence.

| Display | JSON result / exit | Meaning |
|---|---|---|
| MEASURED FASTER | `IMPROVED` / 0 | Higher measured default-C1 throughput in these periods |
| MEASURED SLOWER | `REGRESSED` / 2 | Lower measured default-C1 throughput in these periods |
| COMPLETE - DESCRIPTIVE ONLY | `DESCRIPTIVE` / 0 | Selected observations complete; no performance verdict |
| INCONCLUSIVE | `INCONCLUSIVE` / 2 | No direction established or coverage incomplete; not equivalence |
| VERDICT PENDING | `PENDING` / 0 | Deployment candidate captured; run the A/B/A2 comparison below |
| INVALID | `INVALID` / 1 | Invalid input, response or incompatible evidence |

Baseline readiness is separate: ready exits 0, incomplete 2, invalid 1.
Neither exit 0 nor a direction establishes causality, useful effect or guaranteed
regression detection. `compare` replays offline without credentials/network and
never rewrites saved evidence. Raw requests, responses and declarations stay
private; share only a separately reviewed sanitized summary.

## Compare two deployments (A/B/A2)

To compare whole deployments, for example the same model on an Apple Silicon
Mac and on an NVIDIA host, use the portable workload, which needs only a Chat
Completions server that reports streaming usage. The result compares hardware,
runtime, model build and settings together; see the
[deployment comparison contract](CONTRACT.md#capture-v3-and-deployment-comparison).

Both collectors must be built from the same clean git commit, so
`"$GRILL_PERF" --version` prints the same `(source <commit>)` on each host;
`unrecorded` is refused. Release archives from v0.6.0 record their source commit
for both Linux and Apple Silicon; a source build must come from a `git clone`
checked out at that commit, as in [source fallback](#upgrade-rollback-and-source-fallback). Use the same
`--client-placement` on both sides and at least 60 seconds between captures.

```sh
# A, on the Mac host (collector next to the server).
"$GRILL_PERF" baseline --workload portable-v1 --client-placement same-host \
  --endpoint "$MAC_ENDPOINT" --model "$MAC_MODEL" --local-http \
  --deployment mac.json --out results/a
# B: copy results/a to the NVIDIA host, then check it against that deployment.
"$GRILL_PERF" check results/a --change deployment --client-placement same-host \
  --endpoint "$NVIDIA_ENDPOINT" --model "$NVIDIA_MODEL" --local-http \
  --deployment nvidia.json --out results/b
# A2, back on the Mac, with the Mac deployment unchanged.
"$GRILL_PERF" check results/a --change none --deployment mac.json --out results/a2
# With all three directories on one host:
"$GRILL_PERF" compare results/a results/b --reference results/a2
```

`check --change deployment` exits 0 with `VERDICT PENDING`; the verdict comes only
from the three-way `compare`. Drop `--local-http` for an HTTPS endpoint. The
declarations must differ in at least one field.

## Optional selections and failures

Before baseline, add `--selection "$GRILL_HOME/workloads/FILE"` to the same command.
Choose only the intended path; there is no resolver or automatic compatibility retry.

| FILE | Explicit scope/control |
|---|---|
| `concurrency-selection-v1.json` | C1/C2/C4 with legacy `thinking: false` |
| `concurrency-enable-thinking-selection-v1.json` | Same ladder, distinct `enable_thinking: false` control |
| `concurrency-ladder-selection-v1.json` | [C1/C2/C4/C8 coding question](README.md#concurrency-ladder), 400-token portable replies |
| `conversation-selection-v2.json` | Bounded factual/history and fixed-tool continuity |
| `long-context-decode-selection-v1.json` | [Decode after ~32K/48K/96K prompts](README.md#long-context-decode-and-prefill), portable controls |
| `portable-chat-selection-v1.json` | Portable C1 cap observation; no backend-specific controls |
| `prefill-ladder-96k-selection-v1.json` | [Prefill at ~2K/8K/32K/96K](README.md#long-context-decode-and-prefill), portable controls |
| `prefill-prose-portable-selection-v1.json` | [Generated-prose prefill](README.md#generated-prose-prefill), nominal 4k/8k/16k/32k, portable controls |
| `realistic-decode-selection-v1.json` | [Coding questions and file edits](README.md#realistic-decode), C1, portable controls |
| `sparkdash-decode-portable-selection-v1.json` | [sparkDash decode prompts](README.md#portable-sparkdash-copies), portable controls |
| `sparkdash-decode-portable-selection-v2.json` | [sparkDash 1.8.7+ decode prompts](README.md#portable-sparkdash-copies) (real code task), portable controls |
| `sparkdash-prefill-portable-selection-v1.json` | [sparkDash prefill sizes](README.md#portable-sparkdash-copies), portable controls |

For a current source build, pass
`--selection "$SOURCE/crates/grill-perf/examples/portable-chat-selection-v1.json"`.
From v0.4.0 the release path is
`$GRILL_HOME/workloads/portable-chat-selection-v1.json`. Published v0.3.0 and
earlier artifacts remain immutable and do not gain these files retroactively.

All explicit selections are descriptive, including a selected C1 workload.
`workloads/recipes-v1.json` is the historical bundle, not a selection manifest.
Use `bundle inspect "$WORKLOAD"` and `bundle verify "$GRILL_HOME/workloads/recipes-v1.json"`
for offline inspection. Consult the detailed recipe/measurement guides at the
immutable `source_commit` in the receipt, not a moving documentation revision.

- **Missing parent/existing destination:** create the parent or choose an existing
  nonsymlink parent and fresh output directory. Never overwrite evidence.
- **Missing declaration:** supply the named real identity. Do not invent a sample value.
- **Pin/collector mismatch:** use the reviewed matching files and original binary;
  a changed selection requires a new approved baseline, not edited old evidence.
- **Authentication/backend rejection:** inspect retained status privately and fix
  credentials or qualify controls separately. No field stripping or resending occurs.
- **Budget/incomplete:** inspect accounting and stop reason before planning a new
  capture. Missing usage is unknown, not zero. Failures before output creation are
  on stderr; later failures retain `report.json`/native evidence where available.

## Upgrade, rollback and source fallback

Install an explicitly selected version into a separate directory, retaining old
binaries/workloads/receipts. Do not migrate incompatible evidence in place.
A clean reviewed source checkout remains an alternative to a published archive:

```sh
: "${SOURCE:?absolute reviewed checkout path}"
: "${SOURCE_GIT_SHA:?reviewed full immutable revision}"
test "$(git -C "$SOURCE" rev-parse HEAD)" = "$SOURCE_GIT_SHA" &&
cargo +1.98.0 build --manifest-path "$SOURCE/Cargo.toml" -p grill-perf --release --locked &&
export GRILL_PERF="$SOURCE/target/release/grill-perf" &&
"$GRILL_PERF" --version
```

It prints `grill-perf <version> (source <commit>)` only when the build inputs
in the git checkout match that commit, and otherwise `(source unrecorded)`.
On macOS, release archives exist from v0.6.0; for a source build, use a git
clone at a clean commit to record the source; a build from a source tarball
reports `unrecorded`.
Source builds of the portable serving workflow need Linux or Apple Silicon
macOS, Rust/Cargo 1.98.0, C/C++ tools and CMake. Linux `/proc`/cgroup/NVML
resource sources and external-program microbench capture remain Linux-only;
macOS uses [its own resource sources](RESOURCES.md#macos-sources).
Use explicit selection paths under `$SOURCE/crates/grill-perf/examples/` instead
of packaged `workloads/`. Record build flags and actual hashes; a local build is not
proof of native installed-artifact verification or live backend qualification.
