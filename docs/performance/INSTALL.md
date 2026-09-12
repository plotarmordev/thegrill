# Install and first capture

`grill-perf` measures a server you already run; it never starts it, changes its
settings, downloads weights or uploads evidence. The quality CLI `grill` remains
WIP and is not in these archives.

## Obtain, verify, unpack

The prepared version is **0.1.0**. Preparation does not establish that a public
release exists. Until a maintainer approves publication, use the staged archive
and sidecars they provide, or the source fallback below. Never guess a tag or use
a moving `latest` pin.

Choose `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu` to match `uname -m`.
The supported baseline is native Ubuntu 24.04 / glibc 2.39; older libc and other
runtimes are not qualified by that statement. Installed use needs no Rust or
checkout. A readable system CA store (Ubuntu `ca-certificates`) is required for
client initialization, including loopback HTTP; do not disable TLS checks.
The shell examples use `curl`, `sha256sum`, `tar`, and `jq`.
An `Exec format error` means the archive/CPU choice is wrong: obtain the native
target rather than treating emulation as qualification. A missing `GLIBC_*`
version means the runtime is below the supported baseline: use a qualified OS
or the source-build fallback on the intended host, not copied libc files or a
TLS-verification workaround.

For downloads, set the exact version, target and trusted HTTPS artifact directory
supplied by the maintainer. In a fresh download directory:

```sh
: "${VERSION:?exact approved or staged version}"
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
  requires vLLM-compatible exact-output controls and `chat_template_kwargs.thinking: false`.
- The credential environment-variable **name**, if authentication is required.
  Add `--auth-env NAME` to baseline; supply its value independently. No environment
  file is discovered and the value is not stored by the collector.
- Actual weights revision, runtime build, hardware/layout identity and settings
  fingerprint. These are operator declarations, not server attestation.

Use HTTPS. A separately managed literal-loopback HTTP endpoint additionally needs
`--local-http` on baseline; `localhost` is not a literal address.

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

<a id="baseline-optional-control-change-check"></a>

## Baseline → optional control → change → check

No statistical policy file or manually calculated threshold is needed. Default
C1 allows **32 requests / 12,800 output tokens / 300 seconds**: eight acquisitions,
each with one warmup and three measured requests of exactly 400 reported tokens.
Preflight shows scope, controls and the full allowance on stderr before traffic;
it is not a prompt or proof that the backend supports those controls.

```sh
"$GRILL_PERF" baseline --endpoint "${ENDPOINT:?full resource URL}" \
  --model "${MODEL:?explicit model selector}" \
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
| INVALID | `INVALID` / 1 | Invalid input, response or incompatible evidence |

Baseline readiness is separate: ready exits 0, incomplete 2, invalid 1.
Neither exit 0 nor a direction establishes causality, useful effect or guaranteed
regression detection. `compare` replays offline without credentials/network and
never rewrites saved evidence. Raw requests, responses and declarations stay
private; share only a separately reviewed sanitized summary.

## Optional selections and failures

Before baseline, add `--selection "$GRILL_HOME/workloads/FILE"` to the same command.
Choose only the intended path; there is no resolver or automatic compatibility retry.

| FILE | Explicit scope/control |
|---|---|
| `concurrency-selection-v1.json` | C1/C2/C4 with legacy `thinking: false` |
| `concurrency-enable-thinking-selection-v1.json` | Same ladder, distinct `enable_thinking: false` control |
| `conversation-selection-v2.json` | Bounded factual/history and fixed-tool continuity |

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
Until publication is approved, a clean reviewed source checkout remains usable:

```sh
: "${SOURCE:?absolute reviewed checkout path}"
: "${SOURCE_GIT_SHA:?reviewed full immutable revision}"
test "$(git -C "$SOURCE" rev-parse HEAD)" = "$SOURCE_GIT_SHA" &&
cargo +1.98.0 build --manifest-path "$SOURCE/Cargo.toml" -p grill-perf --release --locked &&
export GRILL_PERF="$SOURCE/target/release/grill-perf" &&
"$GRILL_PERF" --version
```

Source builds need Linux, Rust/Cargo 1.98.0, C/C++ tools and CMake. Use explicit
selection paths under `$SOURCE/crates/grill-perf/examples/` instead of packaged
`workloads/`. Record build flags and actual hashes; a local build is not proof of
native installed-artifact verification or live backend qualification.
